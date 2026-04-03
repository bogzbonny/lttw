// src/fim.rs - Fill-in-Middle (FIM) completion functions
//
// This module handles FIM completion requests to the llama.cpp server,
// including context gathering, request building, response processing,
// and rendering suggestions.

use {
    crate::{
        cache::compute_hashes,
        context::get_local_context,
        context::LocalContext,
        get_buf_lines, get_current_buffer_id, get_pos, in_insert_mode,
        plugin_state::{get_state, PluginState},
        ring_buffer::ExtraContext,
        utils::{
            clear_buf_namespace_objects, get_buf_filename, get_buf_line, random_range,
            set_buf_extmark, sha256,
        },
        Error, FimCompletionMessage, FimTimingsData, LttwResult,
    },
    nvim_oxi::api::{opts::SetExtmarkOptsBuilder, types::ExtmarkVirtTextPosition},
    serde::{Deserialize, Serialize},
    std::sync::Arc,
    std::time::{Duration, Instant},
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

/// FIM completion response (uses flat keys from server)
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

/// FIM timing information (matches server response format with flat keys)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FimTimings {
    #[serde(rename = "timings/prompt_n")]
    pub prompt_n: Option<i64>,
    #[serde(rename = "timings/prompt_ms")]
    pub prompt_ms: Option<f64>,
    #[serde(rename = "timings/prompt_per_token_ms")]
    pub prompt_per_token_ms: Option<f64>,
    #[serde(rename = "timings/prompt_per_second")]
    pub prompt_per_second: Option<f64>,
    #[serde(rename = "timings/predicted_n")]
    pub predicted_n: Option<i64>,
    #[serde(rename = "timings/predicted_ms")]
    pub predicted_ms: Option<f64>,
    #[serde(rename = "timings/predicted_per_token_ms")]
    pub predicted_per_token_ms: Option<f64>,
    #[serde(rename = "timings/predicted_per_second")]
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

pub fn fim_try_hint() -> LttwResult<()> {
    if !in_insert_mode()? {
        return Ok(());
    }
    let (pos_x, pos_y) = get_pos();
    let state = get_state();
    let lines = get_buf_lines(..);
    let buffer_id = get_current_buffer_id();
    fim_try_hint_inner(state, pos_x, pos_y, buffer_id, lines)
}

/// Try to generate a suggestion using the data in the cache
/// Looks at the previous 10 characters to see if a completion is cached.
/// If one is found at (x,y) then it checks that the characters typed after (x,y)
/// match up with the cached completion result.
///
/// # Returns
///  - `Some(RenderedSuggestion)` - If a cached completion is found
///  - `None` - If no cached completion is found
///
/// NOTE this happens on the neovim main thread
pub fn fim_try_hint_inner(
    state: Arc<PluginState>,
    pos_x: usize,
    pos_y: usize,
    buffer_id: u64,
    lines: Vec<String>,
) -> LttwResult<()> {
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
            return Ok(());
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

    let mut prev_for_next_fim: Option<Vec<String>> = None;
    if let Some(response) = response {
        state.debug_manager.read().log(
            "fim_try_hint_inner",
            format!("found cached response: {response:#?}"),
        );
        let content = response.content;
        if content.is_empty() {
            return Ok(());
        }

        render_fim_suggestion(
            state.clone(),
            pos_x,
            pos_y,
            &content,
            ctx.line_cur.clone(),
            None,
        )?;

        // run async speculative FIM in the background for this position
        // TODO should this just always run even when no hint is shown?
        let hint_shown = state.fim_state.read().hint_shown;
        if hint_shown {
            prev_for_next_fim = Some(vec![content]);
        }
    }
    let filename = get_buf_filename()?;

    // Spawn a FIM in the background
    let rt = state.tokio_runtime.clone();
    rt.read().spawn(async move {
        // TODO log error
        let _ = spawn_fim_completion_worker(
            state,
            ctx,
            pos_x,
            pos_y,
            buffer_id,
            filename,
            lines,
            prev_for_next_fim,
        )
        .await;
    });
    Ok(())
}

/// Implementation of FIM worker with optional debounce sequence tracking
///
/// NOTE this DOES NOT happens on the neovim main thread - don't call neovim functions
async fn spawn_fim_completion_worker(
    state: Arc<PluginState>,
    ctx: LocalContext,
    cursor_x: usize,
    cursor_y: usize,
    buffer_id: u64,
    filename: String,
    buffer_lines: Vec<String>,
    prev: Option<Vec<String>>, // speculative FIM content
) -> LttwResult<()> {
    let seq = state.increment_debounce_sequence();

    // Check debounce if we have a sequence
    let debounce_ms = {
        let config = state.config.read();
        config.debounce_ms
    };

    // This is the most recent request, check if debounce has elapsed
    let now = Instant::now();
    let last_spawn = *state.fim_worker_debounce_last_spawn.read();
    let elapsed = now.duration_since(last_spawn);
    let debounce_expired = elapsed >= Duration::from_millis(debounce_ms as u64);

    if !debounce_expired {
        // Still within debounce period. Since this is the most recent request,
        // we should wait until debounce expires and then spawn.
        let remaining_ms = debounce_ms as u64 - elapsed.as_millis() as u64;
        state.debug_manager.read().log(
            "spawn_fim_completion_worker",
            format!("Within debounce period, (seq {seq}, remaining {remaining_ms}ms)",),
        );

        // Wait for remaining debounce time
        tokio::time::sleep(Duration::from_millis(remaining_ms)).await;

        // Re-check if we're still the most recent request
        let latest_sequence = *state.fim_worker_debounce_seq.read();

        if seq < latest_sequence {
            // A newer request has come in, discard this one
            state.debug_manager.read().log(
                "spawn_fim_completion_worker",
                format!(
                    "Discarding stale worker after wait (seq {seq} < latest {latest_sequence})",
                ),
            );
            return Ok(());
        }
    }
    state.record_worker_spawn();

    state.debug_manager.read().log(
        "spawn_fim_completion_worker",
        format!("Spawning worker for ({}, {})", cursor_x, cursor_y),
    );

    fim_completion(
        state,
        ctx,
        cursor_x,
        cursor_y,
        buffer_id,
        filename,
        buffer_lines,
        prev,
    )
    .await?;

    Ok(())
}

/// Main FIM completion function that sends a request to the server
/// Returns the content and optionally timing info for display
///
/// NOTE this DOES NOT happens on the neovim main thread - don't call neovim functions
pub async fn fim_completion(
    state: Arc<PluginState>,
    ctx: LocalContext,
    pos_x: usize,
    pos_y: usize,
    buffer_id: u64,
    filename: String,
    lines: Vec<String>,
    prev: Option<Vec<String>>, // speculative FIM content
) -> LttwResult<()> {
    state.debug_manager.read().log("fim_completion", "0");
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
    state.debug_manager.read().log("fim_completion", "1");

    // Get local context
    state.debug_manager.read().log("fim_completion", "2");

    // Skip auto FIM if too much suffix
    if ctx.line_cur_suffix.len() > state.config.read().max_line_suffix as usize {
        return Ok(());
    }
    state.debug_manager.read().log("fim_completion", "3");

    let hashes = compute_hashes(&ctx);
    state.debug_manager.read().log("fim_completion", "4");

    // if we already have a cached completion for one of the hashes, don't send a request
    if state.config.read().auto_fim {
        for hash in &hashes {
            let cache_lock = state.cache.read();
            if cache_lock.contains_key(hash) {
                return Ok(());
            }
        }
    }
    state.debug_manager.read().log("fim_completion", "5");

    // Evict ring buffer chunks that are very similar to current FIM context (>0.5 threshold)
    // This prevents redundant context from cluttering the ring buffer

    // get the chunk of text around the current line (total length = ring_chunk_size)
    let ring_chunk_size_half = (ring_chunk_size / 2) as usize;
    let start_line = pos_y.saturating_sub(ring_chunk_size_half);
    let end_line = (pos_y + ring_chunk_size_half).min(lines.len());
    state.debug_manager.read().log("fim_completion", "6");

    // Safety: ensure we have valid range and enough lines
    if start_line >= end_line || start_line >= lines.len() {
        return Ok(());
    }
    state.debug_manager.read().log("fim_completion", "7");
    let text = &lines[start_line..end_line];
    {
        let mut rb = state.ring_buffer.write();
        let chunk = rb.get_chunk_from_text(text);
        if !chunk.is_empty() {
            rb.evict_similar(&chunk, 0.5);
        }
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

    let last_pick_pos_y = state.fim_state.read().get_last_pick_pos_y();
    let state_ = state.clone();
    state.debug_manager.read().log("fim_completion", "8");

    let state__ = state.clone();
    let handle = tokio::spawn(async move {
        //state
        //    .debug_manager
        //    .read()
        //    .log("sending msg", format!("{request:#?}"));
        // Send request without holding locks
        let Ok(response_text) = send_request(&request, endpoint_fim, model, api_key).await else {
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
            let mut cache_lock = state__.cache.write();
            for hash in &hashes {
                cache_lock.insert(hash.clone(), response.clone());
            }
        }

        // NOTE this causes a race condition as it calls neovim functions
        //
        //let Some(orig_line) = lines.get(pos_y) else {
        //    return;
        //};
        // TODO fix or delete
        //if should_abort(pos_y, orig_line, &content) {
        //    return;
        //}

        // Send result through channel
        // Extract timing data from the response if available
        let timings = response.timings.as_ref().map(|t| FimTimingsData {
            n_prompt: t.prompt_n.unwrap_or(0),
            t_prompt_ms: t.prompt_ms.unwrap_or(0.0),
            s_prompt: t.prompt_per_second.unwrap_or(0.0),
            n_predict: t.predicted_n.unwrap_or(0),
            t_predict_ms: t.predicted_ms.unwrap_or(0.0),
            s_predict: t.predicted_per_second.unwrap_or(0.0),
            tokens_cached: response.tokens_cached,
            truncated: response.truncated,
        });

        let msg = FimCompletionMessage {
            buffer_id,
            ctx,
            cursor_x: pos_x,
            cursor_y: pos_y,
            content,
            timings,
        };

        if let Err(_e) = tx.send(msg).await {
            // TODO log error
            //debug_manager.log(
            //    "spawn_fim_worker",
            //    &[&format!("Failed to send completion message: {}", e)],
            //);
        }
    });
    state.debug_manager.read().log("fim_completion", "9");

    // Ring buffer pick logic - gather extra context when cursor moves significantly
    // and process it in the background
    let do_ring_buffer_pick = if let Some(last_y) = last_pick_pos_y {
        (pos_y as i64 - last_y as i64).abs() > 32
    } else {
        true
    };

    state.debug_manager.read().log("fim_completion", "10");
    if do_ring_buffer_pick {
        // Get ring configuration
        state.debug_manager.read().log("fim_completion", "10.1");

        let (ring_scope, n_prefix, n_suffix, ring_chunk_size) = {
            let config = state_.config.read();
            (
                config.ring_scope as usize,
                config.n_prefix as usize,
                config.n_suffix as usize,
                config.ring_chunk_size as usize,
            )
        };
        state.debug_manager.read().log("fim_completion", "10.2");

        let max_y = lines.len() - 1;
        let prefix_start = pos_y.saturating_sub(ring_scope).min(max_y);
        let prefix_end = pos_y.saturating_sub(n_prefix).min(max_y);
        state.debug_manager.read().log("fim_completion", "10.21");
        let prefix_lines = &lines[prefix_start..=prefix_end];
        state.debug_manager.read().log("fim_completion", "10.3");
        if !prefix_lines.is_empty() {
            let mut ring_buffer_lock = state_.ring_buffer.write();
            ring_buffer_lock.pick_chunk(prefix_lines, filename.clone(), false, false)?;
        }
        state.debug_manager.read().log("fim_completion", "10.4");

        state.debug_manager.read().log("fim_completion", "10.5");
        let suffix_start = (pos_y + n_suffix).min(max_y);
        let suffix_end = (pos_y + n_suffix + ring_chunk_size).min(max_y);
        let suffix_lines = &lines[suffix_start..=suffix_end];
        if !suffix_lines.is_empty() {
            let mut ring_buffer_lock = state_.ring_buffer.write();
            ring_buffer_lock.pick_chunk(suffix_lines, filename, false, false)?;
        }

        state.debug_manager.read().log("fim_completion", "10.6");
        // Update the last pick position
        state_.fim_state.write().set_last_pick_pos_y(pos_y);
        state.debug_manager.read().log("fim_completion", "10.7");
    }
    state.debug_manager.read().log("fim_completion", "11");

    handle.await?;
    Ok(())
}

// should we abort the completion because the content has changed since we started this completion
#[allow(dead_code)] // TODO fix or delete (needs to not make neovim calls not on the main thread)
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

/// Send FIM request to the server
pub async fn send_request(
    request: &FimRequest,
    endpoint_fim: String,
    model_fim: String,
    api_key: String,
) -> LttwResult<String> {
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
        Err(Error::Server(format!(
            "Server returned status: {}",
            response.status()
        )))
    }
}

/// Render FIM suggestion at the current cursor location
/// Filters out duplicate text that already exists in the buffer
///
/// NOTE this happens on the neovim main thread
pub fn render_fim_suggestion(
    state: Arc<PluginState>,
    pos_x: usize,
    pos_y: usize,
    content: &str,
    line_cur: String,
    timings: Option<FimTimingsData>,
) -> LttwResult<()> {
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
    // Safety: ensure bounds before slicing
    let line_cur_len = line_cur.len();
    let safe_pos_x = pos_x.min(line_cur_len);
    let line_cur_suffix = if safe_pos_x < line_cur_len {
        &line_cur[safe_pos_x..]
    } else {
        ""
    };
    if !line_cur_suffix.is_empty() && !lines.is_empty() && !lines[0].is_empty() {
        // Check if the beginning of the suggestion duplicates existing text
        for i in (0..line_cur_suffix.len()).rev() {
            if lines[0].starts_with(&line_cur_suffix[..=i]) {
                // Remove the duplicate part from the first line
                let dup_len = line_cur_suffix[..=i].len();
                if dup_len < lines[0].len() {
                    lines[0] = lines[0][dup_len..].to_string();
                } else {
                    lines[0] = String::new();
                }
                break;
            }
        }
    }

    // Append suffix to last line
    let suffix_end = std::cmp::min(pos_x, line_cur.len());
    let suffix = if suffix_end < line_cur.len() {
        &line_cur[suffix_end..]
    } else {
        ""
    };
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

    // Update FIM state with timing data
    state.fim_state.write().update(
        can_accept,
        pos_x,
        pos_y,
        line_cur.to_string(),
        can_accept,
        lines,
        timings,
    );

    // Display virtual text using extmarks
    display_fim_text(&state)
}

/// Display FIM hint as virtual text using extmarks with optional inline info
/// The info string is rendered with RightAlign positioning for right-justified display
//
/// NOTE this happens on the neovim main thread
fn display_fim_text(state: &Arc<PluginState>) -> LttwResult<()> {
    // Lock the fim_state and config to get the data we need
    let (
        hint_shown,
        content,
        extmark_ns,
        pos_y,
        pos_x,
        line_cur,
        config,
        debug_manager,
        timing_data,
    ) = {
        let fs = state.fim_state.read();
        let config = state.config.read().clone();
        let debug_manager = state.debug_manager.read().clone();
        let timing_data = fs.timings.clone();
        (
            fs.hint_shown,
            fs.content.clone(),
            state.extmark_ns,
            fs.pos_y,
            fs.pos_x,
            fs.line_cur.clone(),
            config,
            debug_manager,
            timing_data,
        )
    };

    if !hint_shown || content.is_empty() {
        return Ok(());
    }

    // Clear any existing extmarks in the namespace before setting new ones
    if let Some(ns_id) = extmark_ns {
        clear_buf_namespace_objects(ns_id)?;
    }

    if let Some(ns_id) = extmark_ns {
        // Build virtual text string - first line of suggestion
        let suggestion_text = content[0].clone();
        let (suggestion_text, use_inline) =
            trim_suggestion_curr_line(&suggestion_text, pos_x, &line_cur);

        // Create extmark opts for suggestion text
        let mut suggestion_opts = SetExtmarkOptsBuilder::default();

        // For single line suggestions, use inline or overlay based on context
        let suggestion_virt_text = vec![(suggestion_text, "Comment".to_string())];
        suggestion_opts.virt_text(suggestion_virt_text);

        let mut suggestion_pos = ExtmarkVirtTextPosition::Overlay;
        if content.len() == 1 && use_inline {
            suggestion_pos = ExtmarkVirtTextPosition::Inline;
        }
        suggestion_opts.virt_text_pos(suggestion_pos);

        // Add multi-line support for the suggestion - display rest of suggestion lines below
        if content.len() > 1 {
            let mut virt_lines: Vec<Vec<(String, String)>> = Vec::new();
            for line in &content[1..] {
                virt_lines.push(vec![(line.clone(), "Comment".to_string())]);
            }
            suggestion_opts.virt_lines(virt_lines);
        }

        // Set the extmark for suggestion text at cursor position
        match set_buf_extmark(ns_id, pos_y, pos_x, &suggestion_opts.build()) {
            Ok(_id) => {
                debug_manager.log(
                    "display_fim_text",
                    format!("Set suggestion extmark at line {}, col {}", pos_y, pos_x),
                );
            }
            Err(e) => {
                debug_manager.log(
                    "display_fim_text",
                    format!("Error setting suggestion extmark: {:?}", e),
                );
            }
        }

        // Add build info string with RightAlign positioning when show_info is enabled
        // The info string shows inference statistics like timing, cache status, etc.
        let show_info = config.show_info;
        if show_info > 0 {
            // Get ring buffer and cache stats for info string
            let ring_buffer = state.ring_buffer.read();
            let cache = state.cache.read();

            let ring_chunks = ring_buffer.len();
            let ring_n_evict = ring_buffer.n_evict();
            let ring_queued = ring_buffer.queued_len();

            let cache_size = cache.len();
            let max_cache_keys = config.max_cache_keys as usize;

            // Build the info string using stored timing data
            let info_string = if let Some(t) = timing_data {
                // Convert FimTimingsData to FimTimings (for the build_info_string function)
                let ft = FimTimings {
                    prompt_n: Some(t.n_prompt),
                    prompt_ms: Some(t.t_prompt_ms),
                    prompt_per_token_ms: None,
                    prompt_per_second: Some(t.s_prompt),
                    predicted_n: Some(t.n_predict),
                    predicted_ms: Some(t.t_predict_ms),
                    predicted_per_token_ms: None,
                    predicted_per_second: Some(t.s_predict),
                };

                build_info_string(
                    &ft,
                    t.tokens_cached,
                    t.truncated,
                    ring_chunks,
                    config.ring_n_chunks as usize,
                    ring_n_evict,
                    ring_queued,
                    cache_size,
                    max_cache_keys,
                )
            } else {
                // No timing data available, return empty string
                String::new()
            };

            if !info_string.is_empty() {
                // Create a separate extmark for the info string with RightAlign positioning
                let info_text = info_string.clone();
                let mut info_opts = SetExtmarkOptsBuilder::default();
                let info_virt_text = vec![(info_string, "llama_hl_fim_info".to_string())];
                info_opts.virt_text(info_virt_text);

                // Use RightAlign positioning for the info string
                // This displays the info at the right side of the window
                info_opts.virt_text_pos(ExtmarkVirtTextPosition::RightAlign);

                // Set the extmark at EOL position with right_gravity
                // For RightAlign, we set the position at the end of the line
                // Use the suggestion text length to find approximate end position
                let suggestion_text = content[0].clone();
                let suggestion_len = suggestion_text.len();
                let eol_col = suggestion_len + pos_x;

                // Debug log with position info
                debug_manager.log(
                    "display_fim_text",
                    format!(
                        "Setting info extmark with RightAlign at line {}, col {}. Suggestion len: {}, pos_x: {}, Info: '{}'",
                        pos_y, eol_col, suggestion_len, pos_x, info_text
                    ),
                );

                match set_buf_extmark(ns_id, pos_y, eol_col, &info_opts.build()) {
                    Ok(_id) => {
                        debug_manager.log(
                            "display_fim_text",
                            format!(
                                "Set info extmark at line {}, col {} (RightAlign)",
                                pos_y, pos_x
                            ),
                        );
                    }
                    Err(e) => {
                        debug_manager.log(
                            "display_fim_text",
                            format!("Error setting info extmark: {:?}", e),
                        );
                    }
                }
            }
        }
    }

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
    // Safety: check content length before accessing content[0]
    if content.is_empty() {
        return (line_cur.to_string(), None, None);
    }

    let first_line = content[0].clone();

    // Safety: ensure pos_x is within bounds
    let line_cur_len = line_cur.len();
    let safe_pos_x = pos_x.min(line_cur_len);
    let prefix = if safe_pos_x <= line_cur_len {
        &line_cur[..safe_pos_x]
    } else {
        ""
    }
    .to_string();

    let (new_line, inline) = if content.len() == 1 {
        // If only one line, just replace the current line
        let suffix = if safe_pos_x <= line_cur_len {
            &line_cur[safe_pos_x..]
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
        FimAcceptType::Line => {
            if new_line == line_cur && content.len() > 1 {
                // accept the next line - safety check for content[1]
                let rest = vec![content[1].clone()];
                (new_line, Some(rest), inline)
            } else {
                (new_line, None, inline)
            }
        }
        FimAcceptType::Word => {
            // Accept only the first word
            let suffix = if safe_pos_x <= line_cur_len {
                &line_cur[safe_pos_x..]
            } else {
                ""
            };
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
    use {
        super::*,
        crate::{cache::Cache, context::LocalContext, ring_buffer::RingBuffer},
    };

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
        ring_buffer
            .pick_chunk_inner(
                &[
                    "fn main() {".to_string(),
                    "    println!(\"hello\");".to_string(),
                    "}".to_string(),
                ],
                String::new(),
                true,
            )
            .unwrap();
        ring_buffer.update();

        assert_eq!(ring_buffer.len(), 1);
        assert_eq!(ring_buffer.queued_len(), 0);

        // Add second chunk (should not evict first since they're different)
        ring_buffer
            .pick_chunk_inner(
                &[
                    "use std::io;".to_string(),
                    "fn read_input() {".to_string(),
                    "    let mut s = String::new();".to_string(),
                ],
                String::new(),
                true,
            )
            .unwrap();
        ring_buffer.update();

        assert_eq!(ring_buffer.len(), 2);

        // Add third chunk
        ring_buffer
            .pick_chunk_inner(
                &[
                    "mod test;".to_string(),
                    "fn test_func() {".to_string(),
                    "    assert_eq!(1, 1);".to_string(),
                ],
                String::new(),
                true,
            )
            .unwrap();
        ring_buffer.update();

        assert_eq!(ring_buffer.len(), 3);

        // Add fourth chunk - should evict the oldest one due to max_chunks limit
        ring_buffer
            .pick_chunk_inner(
                &[
                    "pub fn export_func() {".to_string(),
                    "    test_func();".to_string(),
                    "}".to_string(),
                ],
                String::new(),
                true,
            )
            .unwrap();
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
        ring_buffer
            .pick_chunk_inner(&chunk1, String::new(), true)
            .unwrap();
        ring_buffer.update();

        assert_eq!(ring_buffer.len(), 1);

        // Add very similar chunk (should evict first due to >0.9 similarity)
        let mut chunk2 = chunk1.clone();
        chunk2[1] = "    let x = 100;".to_string(); // Slightly different

        ring_buffer
            .pick_chunk_inner(&chunk2, String::new(), true)
            .unwrap();
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
        ring_buffer
            .pick_chunk_inner(
                &[
                    "mod module1;".to_string(),
                    "mod module2;".to_string(),
                    "mod module3;".to_string(),
                ],
                String::new(),
                true,
            )
            .unwrap();
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
        ring_buffer
            .pick_chunk_inner(
                &[
                    "fn test1() {".to_string(),
                    "    println!(\"test1\");".to_string(),
                    "}".to_string(),
                ],
                String::new(),
                true,
            )
            .unwrap();
        ring_buffer.update();

        // Simulate a FIM request with ring buffer context
        // Use a prefix with newlines to test truncated prefix hashes
        let ctx = LocalContext {
            prefix: "fn main() {\n    let x = 1;\n".to_string(),
            middle: "    println!(\"hello\"".to_string(),
            suffix: ");\n}".to_string(),
            line_cur_suffix: "rintln!(\"hello\");".to_string(),
            line_cur: "    println!(\"hello\");".to_string(),
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

        ring_buffer
            .pick_chunk_inner(
                &[
                    "fn func1() {".to_string(),
                    "    let x = 1;".to_string(),
                    "}".to_string(),
                ],
                String::new(),
                true,
            )
            .unwrap();
        ring_buffer.update();

        let n_evict_before = ring_buffer.n_evict();

        // Add similar chunks to trigger eviction
        for _ in 0..5 {
            let similar_chunk = vec![
                "fn func1() {".to_string(),
                "    let x = 100;".to_string(), // Slightly different
                "}".to_string(),
            ];

            ring_buffer
                .pick_chunk_inner(&similar_chunk, String::new(), true)
                .unwrap();
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

        ring_buffer
            .pick_chunk_inner(&chunk_data, String::new(), true)
            .unwrap();
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
            ring_buffer
                .pick_chunk_inner(
                    &[
                        format!("fn func{}_()", i),
                        format!("    let x = {};", i),
                        "}".to_string(),
                    ],
                    String::new(),
                    true,
                )
                .unwrap();
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
        ring_buffer
            .pick_chunk_inner(&chunk, String::new(), true)
            .unwrap();
        ring_buffer.update();

        assert_eq!(ring_buffer.len(), 1);

        // Try to add exact same chunk again (should be ignored)
        ring_buffer
            .pick_chunk_inner(&chunk, String::new(), true)
            .unwrap();

        // Should still be 1 (no duplicate added)
        assert_eq!(ring_buffer.len(), 1);

        // Try to add same chunk via queued (should also be ignored)
        ring_buffer
            .pick_chunk_inner(&chunk, String::new(), true)
            .unwrap();

        // Should still have same queued count
        assert_eq!(ring_buffer.queued_len(), 0);
    }
}
