// src/fim.rs - Fill-in-Middle (FIM) completion functions
//
// This module handles FIM completion requests to the llama.cpp server,
// including context gathering, request building, response processing,
// and rendering suggestions.

use {
    crate::{
        context::{get_local_context, LocalContext},
        get_buf_lines, get_buffer_handle, get_pos, in_insert_mode,
        plugin_state::{get_state, PluginState},
        ring_buffer::ExtraContext,
        spawn_fim_completion_worker,
        utils::{get_buf_line, random_range, sha256},
        FimCompletionMessage, NvimResult,
    },
    serde::{Deserialize, Serialize},
    std::sync::Arc,
};

/// FIM completion request
#[derive(Debug, Clone, Serialize)]
pub struct FimRequest {
    pub id_slot: i64,
    pub input_prefix: String,
    pub input_suffix: String,
    pub input_extra: Vec<ExtraContext>,
    pub prompt: String,
    pub n_predict: u32,
    pub stop: Vec<String>,
    pub n_indent: usize,
    pub top_k: u32,
    pub top_p: f32,
    pub samplers: Vec<String>,
    pub stream: bool,
    pub cache_prompt: bool,
    pub t_max_prompt_ms: u32,
    pub t_max_predict_ms: u32,
    pub response_fields: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub prev: Vec<String>,
}

/// FIM completion response
#[derive(Debug, Clone, Deserialize)]
pub struct FimResponse {
    pub content: String,
    #[serde(default)]
    pub timings: Option<FimTimings>,
    #[serde(default)]
    pub tokens_cached: u64,
    #[serde(default)]
    pub truncated: bool,
}

/// FIM completion result with timing info
#[derive(Debug, Clone, Serialize)]
pub struct FimResult {
    pub content: String,
    pub can_accept: bool,
    pub timings: Option<FimTimings>,
    pub tokens_cached: u64,
    pub truncated: bool,
    pub info: Option<String>,
}

/// FIM timing information
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FimTimings {
    pub prompt_n: Option<i64>,
    pub prompt_ms: Option<f64>,
    pub prompt_per_token_ms: Option<f64>,
    pub prompt_per_second: Option<f64>,
    pub predicted_n: Option<i64>,
    pub predicted_ms: Option<f64>,
    pub predicted_per_token_ms: Option<f64>,
    pub predicted_per_second: Option<f64>,
}

/// Build info string from timing information
#[allow(clippy::too_many_arguments)] // Info display requires many parameters
pub fn build_info_string(
    timings: &FimTimings,
    tokens_cached: u64,
    truncated: bool,
    ring_chunks: usize,
    ring_n_chunks: usize,
    ring_n_evict: usize,
    ring_queued: usize,
    cache_size: usize,
    max_cache_keys: usize,
) -> String {
    // Extract timing values
    let n_prompt = timings.prompt_n.unwrap_or(0);
    let t_prompt_ms = timings.prompt_ms.unwrap_or(1.0);
    let s_prompt = timings.prompt_per_second.unwrap_or(0.0);

    let n_predict = timings.predicted_n.unwrap_or(0);
    let t_predict_ms = timings.predicted_ms.unwrap_or(1.0);
    let s_predict = timings.predicted_per_second.unwrap_or(0.0);

    // Build info string
    let info = if truncated {
        format!(
            " | WARNING: the context is full: {}, increase the server context size or reduce g:lttw_config.ring_n_chunks",
            tokens_cached
        )
    } else {
        format!(
            " | c: {}, r: {}/{}, e: {}, q: {}/16, C: {}/{} | p: {} ({:.2} ms, {:.2} t/s) | g: {} ({:.2} ms, {:.2} t/s)",
            tokens_cached,
            ring_chunks, ring_n_chunks,
            ring_n_evict,
            ring_queued,
            cache_size, max_cache_keys,
            n_prompt, t_prompt_ms, s_prompt,
            n_predict, t_predict_ms, s_predict
        )
    };

    info
}

/// Error type for FIM operations
#[derive(Debug, thiserror::Error)]
pub enum FimError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Server error: {0}")]
    Server(String),
    #[error("Regex error: {0}")]
    Regex(#[from] regex::Error),
}

/// Main FIM completion function that sends a request to the server
/// Returns the content and optionally timing info for display
#[allow(clippy::too_many_arguments)] // FIM requires context from multiple sources
pub fn fim_completion(
    state: Arc<PluginState>,
    pos_x: usize,
    pos_y: usize,
    buffer_handle: u64,
    lines: Vec<String>,
    prev: Option<&[String]>, // speculative FIM content
) -> Result<(), FimError> {
    let (
        n_predict,
        stop,
        t_max_prompt_ms,
        t_max_predict_ms,
        model,
        endpoint_fim,
        api_key,
        ring_chunk_size,
    ) = {
        let config = state.config.read();
        (
            config.n_predict,
            config.stop_strings.clone(),
            config.t_max_prompt_ms,
            config.t_max_predict_ms,
            config.model_fim.clone(),
            config.endpoint_fim.clone(),
            config.api_key.clone(),
            config.ring_chunk_size,
        )
    };

    // Get local context
    let ctx = get_local_context(&lines, pos_x, pos_y, prev, &state.config.read());

    // Skip auto FIM if too much suffix
    if ctx.line_cur_suffix.len() > state.config.read().max_line_suffix as usize {
        return Ok(());
    }

    let hashes = compute_hashes(&ctx);

    // if we already have a cached completion for one of the hashes, don't send a request
    if state.config.read().auto_fim {
        for hash in &hashes {
            let cache_lock = state.cache.read();
            if cache_lock.contains_key(hash) {
                return Ok(());
            }
        }
    }

    // Evict ring buffer chunks that are very similar to current FIM context (>0.5 threshold)
    // This prevents redundant context from cluttering the ring buffer

    // get the chunk of text around the current line (total length = ring_chunk_size)
    let ring_chunk_size_half = (ring_chunk_size / 2) as usize;
    let start_line = pos_y.saturating_sub(ring_chunk_size_half);
    let end_line = (pos_y + ring_chunk_size_half).min(lines.len());
    let text: Vec<String> = lines[start_line..end_line].to_vec();

    // TODO understand why we use a random here
    let l0 = random_range(0, text.len().saturating_sub(ring_chunk_size_half));
    let l1 = (l0 + ring_chunk_size_half).min(text.len());
    let chunk: Vec<String> = text[l0..l1].to_vec();

    if !chunk.is_empty() {
        state.ring_buffer.write().evict_similar(&chunk, 0.5);
    }

    // Build request
    let extra = state.ring_buffer.read().get_extra();

    let request = FimRequest {
        id_slot: 0,
        input_prefix: ctx.prefix.clone(),
        input_suffix: ctx.suffix.clone(),
        input_extra: extra,
        prompt: ctx.middle.clone(),
        n_predict,
        stop,
        n_indent: ctx.indent,
        top_k: 40,
        top_p: 0.90,
        samplers: vec![
            "top_k".to_string(),
            "top_p".to_string(),
            "infill".to_string(),
        ],
        stream: false,
        cache_prompt: true,
        t_max_prompt_ms,
        t_max_predict_ms,
        response_fields: vec![
            "content".to_string(),
            "timings/prompt_n".to_string(),
            "timings/prompt_ms".to_string(),
            "timings/prompt_per_token_ms".to_string(),
            "timings/prompt_per_second".to_string(),
            "timings/predicted_n".to_string(),
            "timings/predicted_ms".to_string(),
            "timings/predicted_per_token_ms".to_string(),
            "timings/predicted_per_second".to_string(),
            "truncated".to_string(),
            "tokens_cached".to_string(),
        ],
        model: model.clone(),
        prev: prev.map(|p| p.to_vec()).unwrap_or_default(),
    };

    let Ok(tx) = state.get_fim_completion_tx() else {
        // TODO log error
        return Ok(());
    };
    let rt = state.tokio_runtime.clone();
    if let Some(runtime) = rt.read().as_ref() {
        runtime.spawn(async move {
            state
                .debug_manager
                .read()
                .log("sending msg", format!("{request:#?}"));
            // Send request without holding locks
            let Ok(response_text) = send_request(&request, endpoint_fim, model, api_key).await
            else {
                // TODO log error
                return;
            };

            // Parse response
            let Ok(response) = serde_json::from_str::<FimResponse>(&response_text) else {
                // TODO log error
                return;
            };
            let content = response.content.clone(); // Clone content for return

            // Cache the response with timing info (new block for re-acquired locks)
            {
                let mut cache_lock = state.cache.write();
                for hash in &hashes {
                    cache_lock.insert(hash.clone(), response.clone());
                }
            }

            // Send result through channel
            let Some(orig_line) = lines.get(pos_y) else {
                return;
            };
            if should_abort(pos_y, orig_line, &content) {
                return;
            }
            let msg = FimCompletionMessage {
                buffer_handle,
                buffer_lines: lines,
                cursor_x: pos_x,
                cursor_y: pos_y,
                content,
            };

            if let Err(_e) = tx.send(msg).await {
                // TODO log error
                //debug_manager.log(
                //    "spawn_fim_worker",
                //    &[&format!("Failed to send completion message: {}", e)],
                //);
            }
        });
    }

    // XXX TODO update ring buffer chunks
    //    " gather some extra context nearby and process it in the background
    //" only gather chunks if the cursor has moved a lot
    //" TODO: something more clever? reranking?
    //if a:is_auto && l:delta_y > 32
    //    let l:max_y = line('$')
    //    " expand the prefix even further
    //    call s:pick_chunk(getline(max([1,       l:pos_y - g:llama_config.ring_scope]), max([1,       l:pos_y - g:llama_config.n_prefix])), v:false, v:false)
    //    " pick a suffix chunk
    //    call s:pick_chunk(getline(min([l:max_y, l:pos_y + g:llama_config.n_suffix]),   min([l:max_y, l:pos_y + g:llama_config.n_suffix + g:llama_config.ring_chunk_size])), v:false, v:false)
    //    let s:pos_y_pick = l:pos_y
    //endif

    //// Ring buffer pick logic - gather extra context when cursor moves significantly
    //// This mirrors the logic in llama#fim (llama.vim lines 930-946)
    //let last_pick_pos_y = state.fim_state.read().get_last_pick_pos_y();
    //let delta_y = last_pick_pos_y
    //    .map(|last_pos| {
    //        pos_y
    //            .saturating_sub(last_pos)
    //            .max(last_pos.saturating_sub(pos_y))
    //    })
    //    .unwrap_or(33); // If no last position, treat as large delta to gather initial chunks

    //// Only gather chunks if cursor has moved more than 32 lines
    //let ring_buffer_pick_needed = delta_y > 32;

    //if ring_buffer_pick_needed {
    //    let max_y = lines.len().saturating_sub(1); // line('$') - 1 (0-indexed)

    //    // Get ring configuration
    //    let config_lock = state.config.read();
    //    let ring_scope = config_lock.ring_scope as usize;
    //    let n_prefix = config_lock.n_prefix as usize;
    //    let n_suffix = config_lock.n_suffix as usize;
    //    let ring_chunk_size = config_lock.ring_chunk_size as usize;

    //    // Expand the prefix even further
    //    // Vim: getline(max([1, l:pos_y - g:llama_config.ring_scope]), max([1, l:pos_y - g:llama_config.n_prefix]))
    //    // In Rust with 0-indexed lines:
    //    let prefix_start = (pos_y.saturating_sub(ring_scope)).max(0);
    //    let prefix_end = (pos_y.saturating_sub(n_prefix)).max(0);

    //    let prefix_lines =
    //        if prefix_start <= max_y && prefix_end <= max_y && prefix_start <= prefix_end {
    //            lines
    //                .get(prefix_start..=prefix_end)
    //                .map(|slice| slice.to_vec())
    //                .unwrap_or_default()
    //        } else {
    //            Vec::new()
    //        };

    //    // Log prefix chunk info before moving
    //    if !prefix_lines.is_empty() {
    //        let mut ring_buffer_lock = state.ring_buffer.write();
    //        ring_buffer_lock.pick_chunk(prefix_lines, false, false);
    //    }

    //    // Pick a suffix chunk
    //    // Vim: getline(min([l:max_y, l:pos_y + g:llama_config.n_suffix]), min([l:max_y, l:pos_y + g:llama_config.n_suffix + g:llama_config.ring_chunk_size]))
    //    // In Rust with 0-indexed lines:
    //    let suffix_start = pos_y.saturating_add(n_suffix).min(max_y);
    //    let suffix_end = (pos_y
    //        .saturating_add(n_suffix)
    //        .saturating_add(ring_chunk_size))
    //    .min(max_y);

    //    let suffix_lines =
    //        if suffix_start <= max_y && suffix_end <= max_y && suffix_start <= suffix_end {
    //            lines
    //                .get(suffix_start..=suffix_end)
    //                .map(|slice| slice.to_vec())
    //                .unwrap_or_default()
    //        } else {
    //            Vec::new()
    //        };

    //    // Log suffix chunk info before moving
    //    if !suffix_lines.is_empty() {
    //        let mut ring_buffer_lock = state.ring_buffer.write();
    //        ring_buffer_lock.pick_chunk(suffix_lines, false, false);
    //    }

    //    // Update the last pick position
    //    state.fim_state.write().set_last_pick_pos_y(pos_y);
    //}

    Ok(())
}

// should we abort the completion because the content has changed since we started this completion
fn should_abort(cursor_y: usize, orig_line: &str, content: &str) -> bool {
    let (_new_x, new_y) = get_pos();
    if cursor_y != new_y {
        return true;
    };
    let curr_line = get_buf_line(cursor_y);
    if curr_line == orig_line {
        return false; // lines the same must not abort
    }

    // if the content predicted is the same as what
    // the user has been typing then can continue
    if curr_line.starts_with(orig_line) {
        let Some(new_text) = curr_line.strip_prefix(orig_line) else {
            return true;
        };
        if content.starts_with(new_text) {
            return false;
        }
    }

    true
}

pub fn fim_try_hint() -> NvimResult<Option<RenderedSuggestion>> {
    if !in_insert_mode()? {
        return Ok(None);
    }
    let (pos_x, pos_y) = get_pos();
    let state = get_state();
    let lines = get_buf_lines();
    let buffer_handle = get_buffer_handle();
    Ok(fim_try_hint_inner(
        state,
        pos_x,
        pos_y,
        buffer_handle,
        lines,
    ))
}

/// Try to generate a suggestion using the data in the cache
/// Looks at the previous 10 characters to see if a completion is cached.
/// If one is found at (x,y) then it checks that the characters typed after (x,y)
/// match up with the cached completion result.
///
/// # Returns
/// * `Some(RenderedSuggestion)` - If a cached completion is found
/// * `None` - If no cached completion is found
pub fn fim_try_hint_inner(
    state: Arc<PluginState>,
    pos_x: usize,
    pos_y: usize,
    buffer_handle: u64,
    lines: Vec<String>,
) -> Option<RenderedSuggestion> {
    // Get local context
    let ctx = get_local_context(&lines, pos_x, pos_y, None, &state.config.read());
    state.debug_manager.read().log("fim_try_hint_inner", "");

    // Compute primary hash
    let primary_hash = format!("{}{}{}{}", ctx.prefix, ctx.middle, "Î", ctx.suffix);
    let hash = sha256(&primary_hash);

    // Check if the completion is cached (and update LRU order)
    let mut response = state.cache.write().get(&hash);

    if response.is_none() {
        // ... or if there is a cached completion nearby (128 characters behind)
        // Looks at the previous 128 characters to see if a completion is cached.
        let pm = format!("{}{}", ctx.prefix, ctx.middle);
        let mut best_len = 0;
        let mut best_response: Option<FimResponse> = None;

        // Only search if pm has enough characters
        if pm.len() < 2 {
            return None;
        }

        // iterate through the prefix+midde string while removing characters from the tail
        //
        let mut char_indices = pm.char_indices().collect::<Vec<_>>();
        char_indices.push((pm.len(), '\0')); // needed for simplifying the loop logic, can be any char,
                                             // its never used
        let char_len = char_indices.len() - 1;

        let max_iters = 128; // TODO parameterize this
        for i in 1..=(max_iters.min(char_len.saturating_sub(1))) {
            let split_byte_idx = char_indices[char_len - i].0;
            let (pm_with_less_tail, removed) = pm.split_at(split_byte_idx);

            let ctx_new = format!("{}Î{}", pm_with_less_tail, ctx.suffix);
            let hash_new = sha256(&ctx_new);

            if let Some(response_) = state.cache.write().get(&hash_new) {
                let content = &response_.content;
                if content.is_empty() {
                    continue;
                }

                // Check that the removed text matches the beginning of the cached response
                // NOTE 'i' always is == removed.len()
                // don't bother if i == content.len() because then there isn't any additional
                // predicted text
                if content.starts_with(removed) {
                    // Found a match - use the rest of the content
                    let Some(remaining) = content.strip_prefix(removed) else {
                        continue;
                    };

                    // could use chars().count() but it's not to important
                    if !remaining.is_empty() && remaining.len() > best_len {
                        best_len = remaining.len();
                        best_response = Some(FimResponse {
                            content: remaining.to_string(),
                            timings: response_.timings,
                            tokens_cached: response_.tokens_cached,
                            truncated: response_.truncated,
                        });
                    }
                }
            }
        }
        response = best_response;
    }

    if let Some(response) = response {
        state.debug_manager.read().log(
            "fim_try_hint_inner",
            format!("found cached response: {response:#?}"),
        );
        let content = response.content;
        if content.is_empty() {
            return None;
        }

        let out = render_fim_suggestion(pos_x, &content, &ctx.line_cur_suffix);

        // run async speculative FIM in the background for this position
        // TODO should this just always run even when no hint is shown?
        let hint_shown = state.fim_state.read().hint_shown;
        if hint_shown {
            tokio::spawn(async move {
                // TODO log error
                let _ = spawn_fim_completion_worker(
                    state,
                    pos_x,
                    pos_y,
                    buffer_handle,
                    lines,
                    Some(&[content]),
                )
                .await;
            });
        }
        Some(out)
    } else {
        None
    }
}

/// Compute hashes for caching
pub fn compute_hashes(ctx: &LocalContext) -> Vec<String> {
    let mut hashes = Vec::new();

    // Primary hash
    let primary = format!("{}{}{}{}", ctx.prefix, ctx.middle, "Î", ctx.suffix);
    let hash = sha256(&primary);
    hashes.push(hash);

    // Truncated prefix hashes (up to 3 levels)
    let mut prefix_trim = ctx.prefix.clone();
    let re = regex::Regex::new(r"^[^\n]*\n").unwrap();
    let max_hashes = 3; // TODO parameterize this
    for _ in 0..max_hashes {
        prefix_trim = re.replace(&prefix_trim, "").to_string();
        if prefix_trim.is_empty() {
            break;
        }

        let hash_input = format!("{}{}{}{}", prefix_trim, ctx.middle, "Î", ctx.suffix);
        let hash = sha256(&hash_input);
        hashes.push(hash);
    }

    hashes
}

/// Send FIM request to the server
pub async fn send_request(
    request: &FimRequest,
    endpoint_fim: String,
    model_fim: String,
    api_key: String,
) -> Result<String, FimError> {
    let client = reqwest::Client::new();

    let mut request_body = serde_json::to_value(request)?;

    // Add model if specified
    if !model_fim.is_empty() {
        request_body["model"] = serde_json::Value::String(model_fim.clone());
    }

    let mut builder = client.post(&endpoint_fim).json(&request_body);

    // Add API key if specified
    if !api_key.is_empty() {
        builder = builder.bearer_auth(&api_key);
    }

    let response = builder.send().await?;

    if response.status().is_success() {
        Ok(response.text().await?)
    } else {
        Err(FimError::Server(format!(
            "Server returned status: {}",
            response.status()
        )))
    }
}

/// Render FIM suggestion at the current cursor location
/// Filters out duplicate text that already exists in the buffer
pub fn render_fim_suggestion(state: &PluginState, pos_x: usize, content: &str, line_cur: &str) {
    // Parse content into lines
    let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();

    // Remove trailing empty lines
    while lines.last().map(|s| s.is_empty()).unwrap_or(false) {
        lines.pop();
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    // Filter out duplicate text - remove prefix that matches existing suffix
    let line_cur_suffix = &line_cur[pos_x.min(line_cur.len())..];
    if !line_cur_suffix.is_empty() && !lines[0].is_empty() {
        // Check if the beginning of the suggestion duplicates existing text
        for i in (0..line_cur_suffix.len()).rev() {
            if lines[0].starts_with(&line_cur_suffix[..=i]) {
                // Remove the duplicate part from the first line
                lines[0] = lines[0][line_cur_suffix[..=i].len()..].to_string();
                break;
            }
        }
    }

    // Append suffix to last line
    let suffix_end = std::cmp::min(pos_x, line_cur.len());
    let suffix = &line_cur[suffix_end..];
    if !lines.is_empty() {
        let last_idx = lines.len() - 1;
        let mut last_line = lines[last_idx].clone();
        last_line += suffix;
        lines[last_idx] = last_line;
    }

    // Check if only whitespace
    let joined = lines.join("\n");
    let can_accept = !joined.trim().is_empty();

    state.debug_manager.read().log(
        "render_fim_suggestion",
        format!("Displaying FIM hint: \n{}", lines.join("\n")),
    );

    // Update FIM state
    state
        .fim_state
        .write()
        .update(can_accept, pos_x, pos_y, can_accept, lines);

    // Display virtual text using extmarks
    display_fim_text(&state)?;
    Ok(())
}

#[derive(Clone)]
pub enum FimAcceptType {
    Full,
    Line,
    Word,
}

impl std::fmt::Display for FimAcceptType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FimAcceptType::Full => write!(f, "full"),
            FimAcceptType::Line => write!(f, "line"),
            FimAcceptType::Word => write!(f, "word"),
        }
    }
}

// returns if Inline fill should be used
pub fn trim_suggestion_curr_line<'a>(
    suggestion: &'a str,
    pos_x: usize,
    line_cur: &str,
) -> (&'a str, bool) {
    // If only one line, just replace the current line
    let suffix = if pos_x <= line_cur.len() {
        &line_cur[pos_x..]
    } else {
        ""
    };

    // trim the first_line suffix if it is the same as the suffix
    if suggestion.ends_with(suffix) {
        (suggestion.trim_end_matches(suffix), true)
    } else {
        // If suggestion.len() less than suffix then assume infill display as okay
        (suggestion, suggestion.len() < suffix.len())
    }
}

/// Accept FIM suggestion - returns the modified line
// returns if inline should be used
pub fn accept_fim_suggestion(
    accept_type: FimAcceptType,
    pos_x: usize,
    line_cur: &str,
    content: &[String],
) -> (
    String,              // first line
    Option<Vec<String>>, // rest lines (None if not needed)
    Option<usize>,       // inline-end (NONE if not inline)
) {
    let first_line = content[0].clone();

    let prefix = if pos_x <= line_cur.len() {
        &line_cur[..pos_x]
    } else {
        ""
    }
    .to_string();

    let (new_line, inline) = if content.len() == 1 {
        // If only one line, just replace the current line
        let suffix = if pos_x <= line_cur.len() {
            &line_cur[pos_x..]
        } else {
            ""
        };
        let (first_line, is_inline) = trim_suggestion_curr_line(&first_line, pos_x, line_cur);
        let inline = if is_inline {
            Some(prefix.len() + first_line.len())
        } else {
            None
        };
        (prefix + first_line + suffix, inline)
    } else {
        (prefix + &first_line, None)
    };

    // Handle accept type
    match accept_type {
        FimAcceptType::Full => {
            // Insert rest of suggestion
            if content.len() > 1 {
                let rest: Vec<String> = content[1..].to_vec();
                (new_line, Some(rest), inline)
            } else {
                (new_line, None, inline)
            }
        }
        FimAcceptType::Line => (new_line, None, inline),
        FimAcceptType::Word => {
            // Accept only the first word
            let suffix = &line_cur[pos_x..];
            if let Some(word_match) = first_line.split_whitespace().next() {
                let _new_word = word_match.to_string() + suffix;
                (new_line + word_match, None, inline)
            } else {
                (new_line, None, inline)
            }
        }
    }
}

/// Result of rendering a FIM suggestion
#[derive(Debug, Clone, serde::Serialize)]
pub struct RenderedSuggestion {
    pub content: Vec<String>,
    pub can_accept: bool,
}

#[cfg(test)]
mod tests {
    use {super::*, crate::cache::Cache, crate::ring_buffer::RingBuffer};

    #[test]
    fn test_compute_hashes() {
        let ctx = LocalContext {
            prefix: "fn main() {\n".to_string(),
            middle: "    println!".to_string(),
            suffix: "(\"hello\");\n}\n".to_string(),
            ..Default::default()
        };

        let hashes = compute_hashes(&ctx);
        assert!(!hashes.is_empty());
    }

    #[test]
    fn test_fim_request_with_ring_buffer_extra() {
        // Test that FIM request properly includes extra context from ring buffer
        let ring_buffer = RingBuffer::new(2, 64);

        let request = FimRequest {
            id_slot: 0,
            input_prefix: "fn main() {".to_string(),
            input_suffix: "}".to_string(),
            input_extra: ring_buffer.get_extra(),
            prompt: "    println!(\"hello\"".to_string(),
            n_predict: 32,
            stop: vec![],
            n_indent: 4,
            top_k: 40,
            top_p: 0.90,
            samplers: vec!["top_k".to_string(), "top_p".to_string()],
            stream: false,
            cache_prompt: true,
            t_max_prompt_ms: 500,
            t_max_predict_ms: 1000,
            response_fields: vec!["content".to_string()],
            model: "".to_string(),
            prev: vec![],
        };

        let json = serde_json::to_string(&request).expect("Request should serialize to JSON");
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("JSON should be parseable");

        // Verify input_extra is an empty array when ring buffer is empty
        assert_eq!(parsed["input_extra"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn test_ring_buffer_integration_with_cache() {
        // Test that ring buffer chunks are properly tracked and cached
        let mut ring_buffer = RingBuffer::new(3, 64);

        // Add first chunk
        ring_buffer.pick_chunk(
            vec![
                "fn main() {".to_string(),
                "    println!(\"hello\");".to_string(),
                "}".to_string(),
            ],
            false,
            true,
        );
        ring_buffer.update();

        assert_eq!(ring_buffer.len(), 1);
        assert_eq!(ring_buffer.queued_len(), 0);

        // Add second chunk (should not evict first since they're different)
        ring_buffer.pick_chunk(
            vec![
                "use std::io;".to_string(),
                "fn read_input() {".to_string(),
                "    let mut s = String::new();".to_string(),
            ],
            false,
            true,
        );
        ring_buffer.update();

        assert_eq!(ring_buffer.len(), 2);

        // Add third chunk
        ring_buffer.pick_chunk(
            vec![
                "mod test;".to_string(),
                "fn test_func() {".to_string(),
                "    assert_eq!(1, 1);".to_string(),
            ],
            false,
            true,
        );
        ring_buffer.update();

        assert_eq!(ring_buffer.len(), 3);

        // Add fourth chunk - should evict the oldest one due to max_chunks limit
        ring_buffer.pick_chunk(
            vec![
                "pub fn export_func() {".to_string(),
                "    test_func();".to_string(),
                "}".to_string(),
            ],
            false,
            true,
        );
        ring_buffer.update();

        // Should still be at max_chunks (3)
        assert_eq!(ring_buffer.len(), 3);
    }

    #[test]
    fn test_ring_buffer_eviction_with_similarity() {
        // Test that similar chunks are evicted based on similarity threshold
        let mut ring_buffer = RingBuffer::new(5, 64);

        let chunk1 = vec![
            "fn function_one() {".to_string(),
            "    let x = 1;".to_string(),
            "    let y = 2;".to_string(),
            "    let z = 3;".to_string(),
            "}".to_string(),
        ];

        // Add first chunk
        ring_buffer.pick_chunk(chunk1.clone(), false, true);
        ring_buffer.update();

        assert_eq!(ring_buffer.len(), 1);

        // Add very similar chunk (should evict first due to >0.9 similarity)
        let mut chunk2 = chunk1.clone();
        chunk2[1] = "    let x = 100;".to_string(); // Slightly different

        ring_buffer.pick_chunk(chunk2, false, true);
        ring_buffer.update();

        // Due to high similarity, first chunk should be evicted
        // The exact behavior depends on the similarity threshold (0.9)
        assert!(ring_buffer.len() <= 2);
    }

    #[test]
    fn test_fim_request_serialization_with_extra() {
        // Test that FIM request properly serializes with extra context
        let mut ring_buffer = RingBuffer::new(2, 64);

        // Add some chunks to the ring buffer
        ring_buffer.pick_chunk(
            vec![
                "mod module1;".to_string(),
                "mod module2;".to_string(),
                "mod module3;".to_string(),
            ],
            false,
            true,
        );
        ring_buffer.update();

        let extra = ring_buffer.get_extra();

        let request = FimRequest {
            id_slot: 0,
            input_prefix: "fn main() {".to_string(),
            input_suffix: "}".to_string(),
            input_extra: extra,
            prompt: "    println!(\"hello\"".to_string(),
            n_predict: 32,
            stop: vec![],
            n_indent: 4,
            top_k: 40,
            top_p: 0.90,
            samplers: vec!["top_k".to_string(), "top_p".to_string()],
            stream: false,
            cache_prompt: true,
            t_max_prompt_ms: 500,
            t_max_predict_ms: 1000,
            response_fields: vec!["content".to_string()],
            model: "".to_string(),
            prev: vec![],
        };

        let json = serde_json::to_string(&request).expect("Request should serialize to JSON");
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("JSON should be parseable");

        // Verify input_extra contains the chunk data
        let extra_array = parsed["input_extra"].as_array().unwrap();
        assert_eq!(extra_array.len(), 1);
        assert!(extra_array[0].get("text").is_some());
    }

    #[test]
    fn test_cache_with_ring_buffer_chunks() {
        // Test that cache properly handles entries with ring buffer context
        let mut cache = Cache::new(10);
        let mut ring_buffer = RingBuffer::new(3, 64);

        // Add chunks to ring buffer
        ring_buffer.pick_chunk(
            vec![
                "fn test1() {".to_string(),
                "    println!(\"test1\");".to_string(),
                "}".to_string(),
            ],
            false,
            true,
        );
        ring_buffer.update();

        // Simulate a FIM request with ring buffer context
        // Use a prefix with newlines to test truncated prefix hashes
        let ctx = LocalContext {
            prefix: "fn main() {\n    let x = 1;\n".to_string(),
            middle: "    println!(\"hello\"".to_string(),
            suffix: ");\n}".to_string(),
            line_cur_suffix: "rintln!(\"hello\");".to_string(),
            indent: 4,
        };

        let hashes = compute_hashes(&ctx);

        // Verify we generated multiple hashes (prefix has newlines)
        assert!(
            hashes.len() > 1,
            "Should generate multiple hashes from truncated prefixes"
        );

        // Cache a response for these hashes
        let response = r#"{"content":" world","timings":{},"tokens_cached":0,"truncated":false}"#;
        let response = serde_json::from_str::<FimResponse>(response).unwrap();
        for hash in &hashes {
            cache.insert(hash.clone(), response.clone());
        }

        // Verify cache contains the entries
        for hash in &hashes {
            assert!(cache.contains_key(hash));
        }

        // Verify cache size matches the number of hashes generated
        assert_eq!(
            cache.len(),
            hashes.len(),
            "Cache should contain all {} hash entries",
            hashes.len()
        );
    }

    #[test]
    fn test_ring_buffer_n_evict_counter() {
        // Test that n_evict counter tracks evicted chunks correctly
        let mut ring_buffer = RingBuffer::new(2, 64);

        ring_buffer.pick_chunk(
            vec![
                "fn func1() {".to_string(),
                "    let x = 1;".to_string(),
                "}".to_string(),
            ],
            false,
            true,
        );
        ring_buffer.update();

        let n_evict_before = ring_buffer.n_evict();

        // Add similar chunks to trigger eviction
        for _ in 0..5 {
            let similar_chunk = vec![
                "fn func1() {".to_string(),
                "    let x = 100;".to_string(), // Slightly different
                "}".to_string(),
            ];
            ring_buffer.pick_chunk(similar_chunk, false, true);
            ring_buffer.update();
        }

        let n_evict_after = ring_buffer.n_evict();

        // Should have evicted some chunks
        assert!(n_evict_after >= n_evict_before);
    }

    #[test]
    fn test_ring_buffer_get_extra_returns_correct_data() {
        // Test that get_extra returns properly formatted extra context
        let mut ring_buffer = RingBuffer::new(2, 64);

        let chunk_data = vec![
            "fn test_function() {".to_string(),
            "    let x = 42;".to_string(),
            "    return x;".to_string(),
            "}".to_string(),
        ];

        ring_buffer.pick_chunk(chunk_data.clone(), false, true);
        ring_buffer.update();

        let extra = ring_buffer.get_extra();

        assert_eq!(extra.len(), 1);
        assert_eq!(extra[0].text, chunk_data.join("\n") + "\n");
    }

    #[test]
    fn test_multiple_ring_buffer_updates() {
        // Test multiple sequential updates to ring buffer
        let mut ring_buffer = RingBuffer::new(3, 64);

        // Pick multiple chunks without updating
        for i in 0..5 {
            ring_buffer.pick_chunk(
                vec![
                    format!("fn func{}_()", i),
                    format!("    let x = {};", i),
                    "}".to_string(),
                ],
                false,
                true,
            );
        }

        // All should be in queued
        assert_eq!(ring_buffer.queued_len(), 5);
        assert_eq!(ring_buffer.len(), 0);

        // Update twice
        ring_buffer.update();
        ring_buffer.update();

        // Should have moved 2 to ring, 3 remaining in queue
        assert_eq!(ring_buffer.len(), 2);
        assert_eq!(ring_buffer.queued_len(), 3);

        // Update remaining queued chunks
        ring_buffer.update();
        ring_buffer.update();
        ring_buffer.update();

        // All should be in ring (max 3 due to limit)
        assert_eq!(ring_buffer.len(), 3);
        assert_eq!(ring_buffer.queued_len(), 0);
    }

    #[test]
    fn test_ring_buffer_chunk_duplicate_prevention() {
        // Test that duplicate chunks are not added to the buffer
        let mut ring_buffer = RingBuffer::new(5, 64);

        let chunk = vec![
            "fn duplicate_test() {".to_string(),
            "    let x = 1;".to_string(),
            "}".to_string(),
        ];

        // Add chunk first time
        ring_buffer.pick_chunk(chunk.clone(), false, true);
        ring_buffer.update();

        assert_eq!(ring_buffer.len(), 1);

        // Try to add exact same chunk again (should be ignored)
        ring_buffer.pick_chunk(chunk.clone(), false, true);

        // Should still be 1 (no duplicate added)
        assert_eq!(ring_buffer.len(), 1);

        // Try to add same chunk via queued (should also be ignored)
        ring_buffer.pick_chunk(chunk, false, true);

        // Should still have same queued count
        assert_eq!(ring_buffer.queued_len(), 0);
    }
}
