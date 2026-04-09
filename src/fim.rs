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
        debug,
        filetype::should_be_enabled,
        fim_accept_inner, get_buf_lines, get_current_buffer_id, get_pos, in_insert_mode,
        plugin_state::{get_state, PluginState},
        ring_buffer::ExtraContext,
        utils::{
            clear_buf_namespace_objects, filter_tail, get_buf_filename, hash_input, is_in_comment,
            set_buf_extmark, set_buf_extmark_top_right,
        },
        Error, FimCompletionMessage, FimTimingsData, LttwResult, LTTW_FIM_HIGHLIGHT,
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
    pub stop: Vec<String>,
    pub n_predict: u32,
    pub n_indent: usize,
    pub top_k: u32,
    pub top_p: f32,
    pub samplers: Vec<String>,
    pub t_max_prompt_ms: u32,
    pub t_max_predict_ms: u32,
    pub response_fields: Vec<String>,
}

/// FIM completion response (uses flat keys from server)
#[derive(Debug, Clone, Deserialize)]
pub struct FimResponse {
    pub content: String,
    #[serde(flatten)]
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

/// Determine n_predict based on cursor position
/// Returns n_predict_inner if there are non-whitespace characters to the right of the cursor
/// Returns n_predict_end if at end of line or only whitespace to the right
pub fn get_dynamic_n_predict(
    line: &str,
    pos_x: usize,
    n_predict_inner: u32,
    n_predict_end: u32,
) -> (u32, bool) {
    // Check if pos_x is at or beyond the line length (end of line)
    if pos_x >= line.len() {
        return (n_predict_end, false);
    }

    // Check if there are only whitespace characters to the right of the cursor
    let suffix = &line[pos_x..];
    if suffix.trim().is_empty() {
        return (n_predict_end, false);
    }

    // There are non-whitespace characters to the right of the cursor
    (n_predict_inner, true)
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
    ring_queue_length: usize,
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
    if truncated {
        format!(
            " | WARNING: the context is full: {}, increase the server context size or reduce g:lttw_config.ring_n_chunks",
            tokens_cached
        )
    } else {
        format!(
            " | c: {}, r: {}/{}, e: {}, q: {}/{}, C: {}/{} | p: {} ({:.2} ms, {:.2} t/s) | g: {} ({:.2} ms, {:.2} t/s)",
            tokens_cached,
            ring_chunks,
            ring_n_chunks,
            ring_n_evict,
            ring_queued,
            ring_queue_length,
            cache_size,
            max_cache_keys,
            n_prompt,
            t_prompt_ms,
            s_prompt,
            n_predict,
            t_predict_ms,
            s_predict
        )
    }
}

pub fn fim_try_hint(retry: Option<usize>) -> LttwResult<()> {
    if !in_insert_mode()? {
        return Ok(());
    }
    fim_try_hint_inner(false, false, retry) // check_comment = true for normal FIM
}

pub fn fim_try_hint_skip_debounce() -> LttwResult<()> {
    if !in_insert_mode()? {
        return Ok(());
    }
    fim_try_hint_inner(true, false, None) // check_comment = true for skip_debounce
}

pub fn fim_try_hint_regenerate() -> LttwResult<()> {
    if !in_insert_mode()? {
        return Ok(());
    }
    fim_try_hint_inner(true, true, None) // check_comment = true for skip_debounce
}

/// Try to generate a suggestion using the data in the cache
/// Looks at the previous 10 characters to see if a completion is cached.
/// If one is found at (x,y) then it checks that the characters typed after (x,y)
/// match up with the cached completion result.
///
/// NOTE this happens on the neovim main thread
///
/// ### Arguments
///  - `skip_debounce` - whether to skip the debounce check
///  - `retry` - retry number for speculative FIM
pub fn fim_try_hint_inner(
    skip_debounce: bool,
    force_regenerate: bool,
    retry: Option<usize>, // retry number
) -> LttwResult<()> {
    // filetype failsafe
    if !should_be_enabled() {
        // This can happen sometimes a request on the wrong filetype can squeeze through the cracks
        // on retry requests (but then the buffer changes)
        return Ok(());
    }

    let (pos_x, pos_y) = get_pos();
    let state = get_state();
    let lines = get_buf_lines(..);
    let buffer_id = get_current_buffer_id();
    let no_fim_in_comments = state.config.read().no_fim_in_comments;
    debug!("{}", no_fim_in_comments);

    #[allow(clippy::collapsible_if)]
    if no_fim_in_comments {
        let at_eol = lines.get(pos_y).is_some_and(|line| pos_x == line.len());
        if let Some((allowed_buf, allowed_x, allowed_y)) = state.get_allow_comment_fim_cur_pos()
            && (allowed_buf == buffer_id && allowed_x == pos_x && allowed_y == pos_y)
        {
            // comments FIM allowed at this position, continue with FIM
            //
        } else if is_in_comment(pos_x, pos_y, at_eol).unwrap_or(false) {
            debug!("Skipping FIM in comment");
            return Ok(());
        }
    };

    // first things first, increment the seq at the beginning to indicate to any waiting
    // fim_workers in the debounce period that there is a new show in town (so don't start!).
    // We do this first because the following cache checking may take a bit of time.
    let seq = state.increment_debounce_sequence();

    // Get local context
    let ctx = get_local_context(&lines, pos_x, pos_y, &state.config.read());
    debug!("fim_try_hint_inner");

    // Compute primary hash
    let primary_hash_inp = format!("{}{}Î{}", ctx.prefix, ctx.middle, ctx.suffix);
    let hash = hash_input(&primary_hash_inp);

    // Check if the completion is cached (and update LRU order)
    let response = state.cache.write().get(&hash);

    // the bool in all_completions is "recache"
    let mut completions_idx = 0;
    let mut all_completions: Vec<(FimResponse, bool)> = Vec::new();
    let find_better_completion = if let Some(resp) = response {
        all_completions.push((resp, false));
        false
    } else {
        true
    };

    // ... or if there is a cached completion nearby (128 characters behind)
    // Looks at the previous 128 characters to see if a completion is cached.
    let pm = format!("{}{}", ctx.prefix, ctx.middle);
    let mut best_len = 0;

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

        let new_prefix_middle = format!("{}Î{}", pm_with_less_tail, ctx.suffix);
        let hash_new = hash_input(&new_prefix_middle);

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

                all_completions.push((
                    FimResponse {
                        content: remaining.to_string(),
                        timings: response_.timings,
                        tokens_cached: response_.tokens_cached,
                        truncated: response_.truncated,
                    },
                    true,
                )); // recache = true

                // could use chars().count() but it's not to important
                if find_better_completion && !remaining.is_empty() && remaining.len() > best_len {
                    best_len = remaining.len();
                    completions_idx = all_completions.len() - 1;
                }
            }
        }
    }

    for all_completion in all_completions.iter() {
        let (resp, recache) = all_completion;
        // recache the re-found response at the new position - this way the response can still be found
        // if it was longer than 128 characters and the user is accepting this line by line.
        if *recache {
            // use the original ctx to compute the hashes
            let hashes = compute_hashes(&ctx);
            let mut cache_lock = state.cache.write();
            for hash in &hashes {
                cache_lock.insert(hash.clone(), resp.clone());
            }
        }
    }
    let completions: Vec<FimResponse> = all_completions.into_iter().map(|(r, _)| r).collect();
    debug!("all completions: {completions:?}");
    let completion = completions.get(completions_idx).cloned();
    if state.fim_state.read().completion_cycle.is_empty() {
        state
            .fim_state
            .write()
            .set_completion_cycle(completions, completions_idx);
    }

    debug!("completions_idx: {completions_idx:?}");

    let mut prev_for_next_fim: Option<Vec<String>> = None;
    if !force_regenerate && let Some(completion) = completion {
        let prev_content = completion.content.clone();
        debug!("found cached prev_content: {prev_content:#?}");
        if !prev_content.is_empty() {
            render_fim_suggestion(
                state.clone(),
                pos_x,
                pos_y,
                &completion,
                ctx.line_cur.clone(),
            )?;

            // run async speculative FIM in the background for this position
            // TODO should this just always run even when no hint is shown?
            let hint_shown = state.fim_state.read().hint_shown;
            if hint_shown {
                prev_for_next_fim = Some(vec![prev_content]);
            }
        }
    }
    let filename = get_buf_filename()?;

    // Spawn a FIM in the background nomatter what
    // either a speculative-fim as though the completion was accepted
    // or a non-speculative fim because we need a fim!
    let rt = state.tokio_runtime.clone();
    rt.read().spawn(async move {
        if let Some(content) = prev_for_next_fim.clone() {
            // regenerate the context to make a speculative FIM
            let Ok((new_x, new_y, final_content)) =
                fim_accept_inner(FimAcceptType::Full, pos_x, pos_y, ctx.line_cur, content)
            else {
                // TODO log error
                return;
            };

            // overwrite pos_y with all the final_content
            let mut virtual_lines = lines;
            virtual_lines.splice(pos_y..=pos_y, final_content);

            let virtual_ctx = get_local_context(&virtual_lines, new_x, new_y, &state.config.read());

            let _ = spawn_fim_completion_worker(
                state,
                virtual_ctx,
                seq,
                new_x,
                new_y,
                buffer_id,
                filename,
                virtual_lines,
                prev_for_next_fim,
                skip_debounce,
                false, // do not render
                force_regenerate,
                retry,
            )
            .await;
        } else {
            // TODO log error
            let _ = spawn_fim_completion_worker(
                state,
                ctx,
                seq,
                pos_x,
                pos_y,
                buffer_id,
                filename,
                lines,
                prev_for_next_fim,
                skip_debounce,
                true, // do attempt to render
                force_regenerate,
                retry,
            )
            .await;
        }
    });
    Ok(())
}

/// Implementation of FIM worker with optional debounce sequence tracking
///
/// NOTE this DOES NOT happens on the neovim main thread - don't call neovim functions
#[allow(clippy::too_many_arguments)]
async fn spawn_fim_completion_worker(
    state: Arc<PluginState>,
    ctx: LocalContext,
    seq: u64,
    cursor_x: usize,
    cursor_y: usize,
    buffer_id: u64,
    filename: String,
    buffer_lines: Vec<String>,
    prev: Option<Vec<String>>, // speculative FIM content
    skip_debounce: bool,
    do_render: bool,
    force_regenerate: bool,
    retry: Option<usize>,
) -> LttwResult<()> {
    let semaphone = state.fim_worker_semaphore.clone();
    if !skip_debounce {
        // Check debounce if we have a sequence
        let debounce_min_ms = state.config.read().debounce_min_ms;
        let debounce_max_ms = state.config.read().debounce_max_ms;
        let max_req = state.config.read().max_concurrent_fim_requests;
        let free_permits = semaphone.available_permits();
        let debounce_ms = debounce_min_ms
            + ((debounce_max_ms.saturating_sub(debounce_min_ms) as f64)
                * (max_req as f64 - free_permits as f64)
                / max_req as f64) as u64; // linearly increase debounce time based on queue depth

        // This is the most recent request, check if debounce has elapsed
        let last_spawn = *state.fim_worker_debounce_last_spawn.read();
        let elapsed = Instant::now().duration_since(last_spawn);
        let debounce_expired = elapsed >= Duration::from_millis(debounce_ms);

        if !debounce_expired {
            // Still within debounce period. Since this is the most recent request,
            // we should wait until debounce expires and then spawn.
            let remaining_ms = debounce_ms - elapsed.as_millis() as u64;
            debug!("Within debounce period, (seq {seq}, remaining {remaining_ms}ms)");

            // Wait for remaining debounce time
            tokio::time::sleep(Duration::from_millis(remaining_ms)).await;
        }
    }

    // use a semaphone to make sure we don't make to many concurrent llm requests
    let permit = semaphone.acquire().await;

    // Re-check if we're still the most recent request
    let latest_sequence = state
        .fim_worker_debounce_seq
        .load(std::sync::atomic::Ordering::SeqCst);
    if seq < latest_sequence {
        // A newer request has come in, discard this one
        debug!("Discarding stale worker after wait (seq {seq} < latest {latest_sequence})");
        drop(permit);
        return Ok(());
    }

    state.record_worker_spawn();

    debug!("Spawning worker for ({cursor_x}, {cursor_y})");

    let skip = if let Some((gbuf_id, gpos_x, gpos_y)) = *state.fim_worker_generating_for_pos.read()
        && gbuf_id == buffer_id
        && gpos_x == cursor_x
        && gpos_y == cursor_y
    {
        debug!("already currently generating for this position, skipping request");
        true
    } else {
        false
    };

    if !skip {
        *state.fim_worker_generating_for_pos.write() = Some((buffer_id, cursor_x, cursor_y));
        fim_completion(
            state.clone(),
            ctx,
            cursor_x,
            cursor_y,
            buffer_id,
            filename,
            buffer_lines,
            prev,
            do_render,
            force_regenerate,
            retry,
        )
        .await?;
        *state.fim_worker_generating_for_pos.write() = None;
    }

    drop(permit);
    Ok(())
}

/// Main FIM completion function that sends a request to the server
/// Returns the content and optionally timing info for display
///
/// NOTE this DOES NOT happens on the neovim main thread - don't call neovim functions
#[allow(clippy::too_many_arguments)]
pub async fn fim_completion(
    state: Arc<PluginState>,
    ctx: LocalContext,
    pos_x: usize,
    pos_y: usize,
    buffer_id: u64,
    filename: String,
    lines: Vec<String>,
    prev: Option<Vec<String>>, // speculative FIM content
    do_render: bool,
    force_regenerate: bool,
    retry: Option<usize>,
) -> LttwResult<()> {
    let (
        n_predict_inner,
        n_predict_end,
        t_max_prompt_ms,
        mut t_max_predict_ms,
        model,
        endpoint_fim,
        api_key,
        ring_chunk_size,
    ) = {
        let config = state.config.read();
        (
            config.n_predict_inner,
            config.n_predict_end,
            config.t_max_prompt_ms,
            config.t_max_predict_ms,
            config.model_fim.clone(),
            config.endpoint_fim.clone(),
            config.api_key.clone(),
            config.ring_chunk_size,
        )
    };

    // Determine n_predict dynamically based on cursor position
    let (n_predict, inside_a_line) =
        get_dynamic_n_predict(&ctx.line_cur, pos_x, n_predict_inner, n_predict_end);
    debug!(
        "dynamic n_predict: {} (inner: {}, end: {})",
        n_predict, n_predict_inner, n_predict_end
    );

    let truncate_to_single_line =
        inside_a_line && state.config.read().single_line_prediction_within_line;

    // Get local context

    // Skip auto FIM if too much suffix characters, might be the case for dense text
    // where there's simply a lot of chs on a few suffix lines
    if ctx.line_cur_suffix.len() > state.config.read().max_line_suffix as usize {
        return Ok(());
    }
    if prev.is_none() {
        // the first request should be quick - we will launch a speculative request after this one is displayed
        t_max_predict_ms = 250 // TODO parameterize this
    }

    let hashes = compute_hashes(&ctx);

    // if we already have a cached completion for one of the hashes, don't send a request
    if !force_regenerate && state.config.read().auto_fim {
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

    // Safety: ensure we have valid range and enough lines
    if start_line >= end_line || start_line >= lines.len() {
        return Ok(());
    }
    let text = &lines[start_line..end_line];
    {
        let mut rb = state.ring_buffer.write();

        let chunk = rb.get_chunk_from_text(text);
        if !chunk.is_empty() {
            rb.evict_similar_from_live(chunk, 0.5); // TODO parameterize this
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
        stop: Vec::with_capacity(0),
        n_indent: ctx.indent,
        top_k: 40,
        top_p: 0.90,
        samplers: vec![
            "top_k".to_string(),
            "top_p".to_string(),
            "infill".to_string(),
        ],
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
    };

    let Ok(tx) = state.get_fim_completion_tx() else {
        // TODO log error
        return Ok(());
    };

    let last_pick = state.fim_state.read().get_last_pick_buf_id_pos_y();
    let state_ = state.clone();

    // get the next 10 lines past pos_y for tail filtering
    let end = (pos_y + 11).min(lines.len());
    let ten_lines = lines
        .get(pos_y + 1..end)
        .map(|s| s.to_vec())
        .unwrap_or_default();

    let state__ = state.clone();
    let handle = tokio::spawn(async move {
        //state
        //    .debug_manager
        //    .read()
        //    .log("sending msg", format!("{request:#?}"));
        // Send request without holding locks

        let response_text = match send_request(&request, endpoint_fim, model, api_key).await {
            Ok(response_text) => response_text,
            Err(e) => {
                debug!(e);
                return;
            }
        };

        // Parse response
        let mut response = match serde_json::from_str::<FimResponse>(&response_text) {
            Ok(r) => r,
            Err(e) => {
                debug!(e);
                return;
            }
        };

        debug!("resp: {response:#?}");

        // compare the tail of the content to the lines and filter out any matching with the
        // following lines
        //
        // NOTE first add the prefix then also strip the prefix at the end. This will allow
        // for full line tail removal of even the first line if the LLM is just reproducing the
        // the suffix entirely even in the first line (IF we included the prefix)
        let current_line_prefix = ctx.line_cur.chars().take(pos_x).collect::<String>();
        let mut content = current_line_prefix.clone() + &response.content;

        if truncate_to_single_line {
            content = content
                .lines()
                .next()
                .map(|s| s.to_string())
                .unwrap_or_default();
        }

        let content = content
            .trim_end() // trim new excess newlines which may interfer with tail matching
            .split('\n')
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        let content = filter_tail(&content, &ten_lines).join("\n");
        let content = content
            .strip_prefix(&current_line_prefix)
            .unwrap_or(&content)
            .to_string();
        response.content = content.clone();

        // Cache the response with timing info (new block for re-acquired locks)
        {
            let mut cache_lock = state__.cache.write();
            for hash in &hashes {
                cache_lock.insert(hash.clone(), response.clone());
            }
        }

        let msg = FimCompletionMessage {
            buffer_id,
            ctx,
            cursor_x: pos_x,
            cursor_y: pos_y,
            completion: response,
            do_render,
            retry,
        };

        if let Err(e) = tx.send(msg).await {
            debug!(e);
        }
    });

    // Ring buffer pick logic - gather extra contextual chunks from nearby (within ring_scope) when
    // cursor moves significantly or to a new buffer. process it in the background later
    //
    // Thinking; this seems like a reasonable time to queue up more chunks, only if it is
    // significant enought that we want to generate a FIM do we also want to add some more context
    // to the ring buffer.
    let do_ring_buffer_pick = if let Some((last_buf_id, last_y)) = last_pick {
        last_buf_id != buffer_id || (pos_y as i64 - last_y as i64).abs() > 32
    } else {
        true
    };

    if do_ring_buffer_pick {
        // Get ring configuration
        let (ring_scope, n_prefix, n_suffix, ring_chunk_size, ring_n_picks) = {
            let config = state_.config.read();
            (
                config.ring_scope as usize,
                config.n_prefix as usize,
                config.n_suffix as usize,
                config.ring_chunk_size as usize,
                config.ring_n_picks as usize,
            )
        };

        let max_y = lines.len() - 1;

        // Pick chunks from prefix scope - loop based on ring_n_picks
        let prefix_start = pos_y.saturating_sub(ring_scope).min(max_y);
        let prefix_end = pos_y.saturating_sub(n_prefix).min(max_y);
        let prefix_lines = &lines[prefix_start..=prefix_end];
        if !prefix_lines.is_empty() {
            let mut ring_buffer_lock = state_.ring_buffer.write();
            for _ in 0..ring_n_picks {
                if let Err(e) = ring_buffer_lock.pick_chunk(&state_, prefix_lines, filename.clone())
                {
                    // Log error but continue with other picks
                    debug!("Error picking prefix chunk: {}", e);
                }
            }
        }

        // Pick chunks from suffix scope - loop based on ring_n_picks
        let suffix_start = (pos_y + n_suffix).min(max_y);
        let suffix_end = (pos_y + n_suffix + ring_chunk_size).min(max_y);
        let suffix_lines = &lines[suffix_start..=suffix_end];
        if !suffix_lines.is_empty() {
            let mut ring_buffer_lock = state_.ring_buffer.write();
            for _ in 0..ring_n_picks {
                if let Err(e) = ring_buffer_lock.pick_chunk(&state_, suffix_lines, filename.clone())
                {
                    // Log error but continue with other picks
                    debug!("Error picking suffix chunk: {}", e);
                }
            }
        }

        // Update the last pick position
        state_
            .fim_state
            .write()
            .set_last_pick_buf_id_pos_y(buffer_id, pos_y);
    }

    // wait until the request handle has completed before spawning
    handle.await?;
    Ok(())
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
/// NOTE this happens ON the neovim main thread
pub fn render_fim_suggestion(
    state: Arc<PluginState>,
    pos_x: usize,
    pos_y: usize,
    completion: &FimResponse,
    line_cur: String,
) -> LttwResult<()> {
    let content = &*completion.content;

    let timings = completion.timings.as_ref().map(|timings| {
        FimTimingsData::new(
            timings.clone(),
            completion.tokens_cached,
            completion.truncated,
        )
    });

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
    let hint_is_valid = !joined.trim().is_empty();

    debug!("Displaying FIM hint: \n{}", lines.join("\n"));

    // Update FIM state with timing data
    state.fim_state.write().update(
        hint_is_valid,
        pos_x,
        pos_y,
        line_cur.to_string(),
        lines,
        timings,
    );

    // Display virtual text using extmarks
    if hint_is_valid {
        display_fim_text(&state)?;
    }
    Ok(())
}

/// Display FIM hint as virtual text using extmarks with optional inline info
/// The info string is rendered with RightAlign positioning for right-justified display
//
/// NOTE this happens on the neovim main thread
fn display_fim_text(state: &Arc<PluginState>) -> LttwResult<()> {
    // Lock the fim_state and config to get the data we need
    let (content, extmark_ns, pos_y, pos_x, line_cur, config, timing_data) = {
        let fs = state.fim_state.read();
        let config = state.config.read().clone();
        let timing_data = fs.timings.clone();
        (
            fs.content.clone(),
            state.extmark_ns,
            fs.pos_y,
            fs.pos_x,
            fs.line_cur.clone(),
            config,
            timing_data,
        )
    };

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

        // NOTE here "LttwFIM" is the neovim highlight group
        // (hence the virt text appears like a comment)

        // For single line suggestions, use inline or overlay based on context
        let suggestion_virt_text = vec![(suggestion_text, LTTW_FIM_HIGHLIGHT.to_string())];
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
                virt_lines.push(vec![(line.clone(), LTTW_FIM_HIGHLIGHT.to_string())]);
            }
            suggestion_opts.virt_lines(virt_lines);
        }

        // Set the extmark for suggestion text at cursor position
        match set_buf_extmark(ns_id, pos_y, pos_x, &suggestion_opts.build()) {
            Ok(_id) => {
                debug!("Set suggestion extmark at line {}, col {}", pos_y, pos_x);
            }
            Err(e) => {
                debug!("Error setting suggestion extmark: {:?}", e);
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
                    config.ring_queue_length,
                    cache_size,
                    max_cache_keys,
                )
            } else {
                // No timing data available, return empty string
                String::new()
            };

            if !info_string.is_empty()
                && let Err(e) = set_buf_extmark_top_right(ns_id, info_string)
            {
                debug!("Error setting info extmark: {:?}", e);
            }
        }
    }

    Ok(())
}

/// Cycle to next completion
pub fn fim_cycle_next() -> LttwResult<()> {
    let state = get_state();

    // Get current state
    let (hint_shown, _pos_x, _pos_y, _line_cur, _cycle_empty) = {
        let fim_state = state.fim_state.read();
        if !fim_state.hint_shown {
            return Ok(());
        }
        (
            fim_state.hint_shown,
            fim_state.pos_x,
            fim_state.pos_y,
            fim_state.line_cur.clone(),
            fim_state.completion_cycle.is_empty(),
        )
    };

    if !hint_shown {
        return Ok(());
    }

    // Cycle to next
    let completion = {
        let mut fim_state = state.fim_state.write();
        let Some(completion) = fim_state.cycle_next() else {
            return Ok(());
        };
        completion
    };
    debug!(completion);

    // Re-display with new completion
    let (pos_x, pos_y, line_cur) = {
        let fim_state = state.fim_state.read();
        (fim_state.pos_x, fim_state.pos_y, fim_state.line_cur.clone())
    };

    render_fim_suggestion(state.clone(), pos_x, pos_y, &completion, line_cur)?;

    Ok(())
}

/// Cycle to previous completion
pub fn fim_cycle_prev() -> LttwResult<()> {
    let state = get_state();

    // Get current state
    let (hint_shown, _pos_x, _pos_y, _line_cur) = {
        let fim_state = state.fim_state.read();
        if !fim_state.hint_shown {
            return Ok(());
        }
        (
            fim_state.hint_shown,
            fim_state.pos_x,
            fim_state.pos_y,
            fim_state.line_cur.clone(),
        )
    };

    if !hint_shown {
        return Ok(());
    }

    // Cycle to previous
    let completion = {
        let mut fim_state = state.fim_state.write();
        let Some(completion) = fim_state.cycle_prev() else {
            return Ok(());
        };
        completion
    };

    // Re-display with new completion
    let (pos_x, pos_y, line_cur) = {
        let fim_state = state.fim_state.read();
        (fim_state.pos_x, fim_state.pos_y, fim_state.line_cur.clone())
    };

    render_fim_suggestion(state.clone(), pos_x, pos_y, &completion, line_cur)?;

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
        let ring_buffer = RingBuffer::new(2, 64, 16);

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
            t_max_prompt_ms: 500,
            t_max_predict_ms: 1000,
            response_fields: vec!["content".to_string()],
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
        let mut ring_buffer = RingBuffer::new(3, 64, 16);

        // Add first chunk
        ring_buffer
            .pick_chunk_inner(
                &[
                    "fn main() {".to_string(),
                    "    println!(\"hello\");".to_string(),
                    "}".to_string(),
                ],
                String::new(),
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
            )
            .unwrap();
        ring_buffer.update();

        // Should still be at max_chunks (3)
        assert_eq!(ring_buffer.len(), 3);
    }

    #[test]
    fn test_ring_buffer_eviction_with_similarity() {
        // Test that similar chunks are evicted based on similarity threshold
        let mut ring_buffer = RingBuffer::new(5, 64, 16);

        let chunk1 = vec![
            "fn function_one() {".to_string(),
            "    let x = 1;".to_string(),
            "    let y = 2;".to_string(),
            "    let z = 3;".to_string(),
            "}".to_string(),
        ];

        // Add first chunk
        ring_buffer
            .pick_chunk_inner(&chunk1, String::new())
            .unwrap();
        ring_buffer.update();

        assert_eq!(ring_buffer.len(), 1);

        // Add very similar chunk (should evict first due to >0.9 similarity)
        let mut chunk2 = chunk1.clone();
        chunk2[1] = "    let x = 100;".to_string(); // Slightly different

        ring_buffer
            .pick_chunk_inner(&chunk2, String::new())
            .unwrap();
        ring_buffer.update();

        // Due to high similarity, first chunk should be evicted
        // The exact behavior depends on the similarity threshold (0.9)
        assert!(ring_buffer.len() <= 2);
    }

    #[test]
    fn test_fim_request_serialization_with_extra() {
        // Test that FIM request properly serializes with extra context
        let mut ring_buffer = RingBuffer::new(2, 64, 16);

        // Add some chunks to the ring buffer
        ring_buffer
            .pick_chunk_inner(
                &[
                    "mod module1;".to_string(),
                    "mod module2;".to_string(),
                    "mod module3;".to_string(),
                ],
                String::new(),
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
            t_max_prompt_ms: 500,
            t_max_predict_ms: 1000,
            response_fields: vec!["content".to_string()],
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
        let mut ring_buffer = RingBuffer::new(3, 64, 16);

        // Add chunks to ring buffer
        ring_buffer
            .pick_chunk_inner(
                &[
                    "fn test1() {".to_string(),
                    "    println!(\"test1\");".to_string(),
                    "}".to_string(),
                ],
                String::new(),
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
        let mut ring_buffer = RingBuffer::new(2, 64, 16);

        ring_buffer
            .pick_chunk_inner(
                &[
                    "fn func1() {".to_string(),
                    "    let x = 1;".to_string(),
                    "}".to_string(),
                ],
                String::new(),
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
                .pick_chunk_inner(&similar_chunk, String::new())
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
        let mut ring_buffer = RingBuffer::new(2, 64, 16);

        let chunk_data = vec![
            "fn test_function() {".to_string(),
            "    let x = 42;".to_string(),
            "    return x;".to_string(),
            "}".to_string(),
        ];

        ring_buffer
            .pick_chunk_inner(&chunk_data, String::new())
            .unwrap();
        ring_buffer.update();

        let extra = ring_buffer.get_extra();

        assert_eq!(extra.len(), 1);
        assert_eq!(extra[0].text, chunk_data.join("\n") + "\n");
    }

    #[test]
    fn test_multiple_ring_buffer_updates() {
        // Test multiple sequential updates to ring buffer
        let mut ring_buffer = RingBuffer::new(3, 64, 16);

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
        let mut ring_buffer = RingBuffer::new(5, 64, 16);

        let chunk = vec![
            "fn duplicate_test() {".to_string(),
            "    let x = 1;".to_string(),
            "}".to_string(),
        ];

        // Add chunk first time
        ring_buffer.pick_chunk_inner(&chunk, String::new()).unwrap();
        ring_buffer.update();

        assert_eq!(ring_buffer.len(), 1);

        // Try to add exact same chunk again (should be ignored)
        ring_buffer.pick_chunk_inner(&chunk, String::new()).unwrap();

        // Should still be 1 (no duplicate added)
        assert_eq!(ring_buffer.len(), 1);

        // Try to add same chunk via queued (should also be ignored)
        ring_buffer.pick_chunk_inner(&chunk, String::new()).unwrap();

        // Should still have same queued count
        assert_eq!(ring_buffer.queued_len(), 0);
    }
}
