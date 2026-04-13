pub mod accept;
pub mod cycle;
pub mod info_stats;
pub mod render;
pub mod state;

pub use {info_stats::FimTimings, state::FimState};

use {
    crate::{
        cache::compute_hashes,
        context::get_local_context,
        context::LocalContext,
        filetype::should_be_enabled,
        fim_hide, get_buf_lines, get_current_buffer_id, get_pos, in_insert_mode,
        plugin_state::{get_state, PluginState},
        utils::{self, filter_tail, get_buf_filename, is_in_comment},
        DisplayMessage, FimCompletionMessage, FimResponse, FimResponseWithInfo, LttwResult,
    },
    accept::{fim_accept_inner, FimAcceptType},
    std::sync::Arc,
    std::time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, Default)]
pub enum FimModel {
    LSP,
    #[default]
    LLMFast,
    LLMSlow,
}

#[derive(Debug, Clone, Copy)]
pub enum FimLLM {
    Fast,
    Slow,
}

impl From<FimLLM> for FimModel {
    fn from(llm: FimLLM) -> FimModel {
        match llm {
            FimLLM::Fast => FimModel::LLMFast,
            FimLLM::Slow => FimModel::LLMSlow,
        }
    }
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

#[tracing::instrument(skip(retry))]
pub fn fim_try_hint(retry: Option<usize>) -> LttwResult<()> {
    let _span = tracing::info_span!("fim_try_hint").entered();
    if !in_insert_mode()? {
        return Ok(());
    }
    fim_try_hint_inner(false, false, retry) // check_comment = true for normal FIM
}

#[tracing::instrument]
pub fn fim_try_hint_skip_debounce() -> LttwResult<()> {
    let _span = tracing::info_span!("fim_try_hint_skip_debounce").entered();
    if !in_insert_mode()? {
        return Ok(());
    }
    fim_try_hint_inner(true, false, None) // check_comment = true for skip_debounce
}

#[tracing::instrument]
pub fn fim_try_hint_regenerate() -> LttwResult<()> {
    let _span = tracing::info_span!("fim_try_hint_regenerate").entered();
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
/// NOTE this happens on the neovim main thread (but then spawns tasks)
///
/// ### Arguments
///  - `skip_debounce` - whether to skip the debounce check
///  - `retry` - retry number for speculative FIM
#[tracing::instrument(skip(retry))]
pub fn fim_try_hint_inner(
    skip_debounce: bool,
    force_regenerate: bool,
    retry: Option<usize>, // retry number
) -> LttwResult<()> {
    let _span = tracing::info_span!("fim_try_hint_inner").entered();
    // filetype failsafe
    if !should_be_enabled() {
        // This can happen sometimes a request on the wrong filetype can squeeze through the cracks
        // on retry requests (but then the buffer changes)
        info!("FIM not enabled for current filetype, returning");
        return Ok(());
    }

    let (pos_x, pos_y) = get_pos();
    let state = get_state();
    let filename = get_buf_filename()?;
    let lines = get_buf_lines(..);
    let buffer_id = get_current_buffer_id();
    let no_fim_in_comments = state.config.read().no_fim_in_comments;
    info!("no_fim_in_comments = {}", no_fim_in_comments);

    #[allow(clippy::collapsible_if)]
    if no_fim_in_comments {
        let at_eol = lines.get(pos_y).is_some_and(|line| pos_x == line.len());
        if let Some((allowed_buf, allowed_x, allowed_y)) = state.get_allow_comment_fim_cur_pos()
            && (allowed_buf == buffer_id && allowed_x == pos_x && allowed_y == pos_y)
        {
            // comments FIM allowed at this position, continue with FIM
            //
        } else if is_in_comment(pos_x, pos_y, at_eol).unwrap_or(false) {
            fim_hide()?; // remove any comments which may exist
            info!("Skipping FIM in comment at ({}, {})", pos_x, pos_y);
            return Ok(());
        }
    };

    // Spawn a FIM in the background nomatter what
    // either a speculative-fim as though the completion was accepted
    // or a non-speculative fim because we need a fim!
    let rt = state.tokio_runtime.clone();
    rt.read().spawn(async move {
        // first things first, increment the seq at the beginning to indicate to any waiting
        // fim_workers in the debounce period that there is a new show in town (so don't start!).
        // We do this first because the following cache checking may take a bit of time.
        let seq = state.increment_debounce_sequence();
        let (llm_completion_enabled, reduce_cognitive_offloading_percentage) = {
            let c = state.config.read();
            (c.llm_completions, c.reduce_cognitive_offloading_percentage)
        };

        // Get local context
        let ctx = get_local_context(&lines, pos_x, pos_y, &state.config.read());
        info!("fim_try_hint_inner for pos ({}, {})", pos_x, pos_y);

        let (mut all_completions, completions_idx) = if !force_regenerate && llm_completion_enabled
        {
            state.cache.write().get_cached_completion(&ctx)
        } else {
            (Vec::new(), 0)
        };

        let tx = match state.get_fim_completion_tx() {
            Ok(tx) => tx,
            Err(e) => {
                error!(e);
                return;
            }
        };

        let mut msgs: Vec<DisplayMessage> = vec![];

        if retry.is_none() {
            msgs.push(DisplayMessage::ClearFIM);

            // TRIGGER LSP COMPLETION
            //
            // only trigger completions when not on a whitespace line and also
            // not when there is whitespace left of the cursor (eg. after a space)
            if all_completions.is_empty() {
                let lsp_completion_enabled = state.config.read().lsp_completions;
                let left_char = ctx.line_cur.chars().nth(pos_x.saturating_sub(1));
                if lsp_completion_enabled
                    && !ctx.line_cur.trim().is_empty()
                    && let Some(lch) = left_char
                    && !lch.is_whitespace()
                    && let Err(e) = tx.send(DisplayMessage::TriggerLSPCompletion).await
                {
                    error!(e);
                }
            }
        }

        // roll some dice with our users well being
        let prevent_cognitive_decline = if reduce_cognitive_offloading_percentage > 0 {
            let roll = utils::random_range(1, 100) as u8;
            roll <= reduce_cognitive_offloading_percentage
        } else {
            false
        };

        if !llm_completion_enabled || prevent_cognitive_decline {
            // early exit, send the msg
            if let Err(e) = tx.send(msgs.into()).await {
                error!(e)
            }
            return;
        }

        info!("all completions: {} found", all_completions.len());

        let mut final_completion = None;
        if state.fim_state.read().completion_cycle.is_empty() {
            // send all the messages besides the final message to display
            for (i, c) in all_completions.drain(..).enumerate() {
                if i != completions_idx {
                    final_completion = Some(c.clone());
                    continue;
                }
                let msg = FimCompletionMessage {
                    buffer_id,
                    line_cur: ctx.line_cur.clone(),
                    cursor_x: pos_x,
                    cursor_y: pos_y,
                    completion: c,
                    // if forcing to regenerate no need to render, there is already results on the screen and
                    // rerendering would be disruptive
                    do_render: !force_regenerate,
                    retry: None,
                };
                msgs.push(msg.into());
            }
        }

        info!("completions_idx: {}", completions_idx);

        let mut prev_for_next_fim: Option<Vec<String>> = None;

        // if forcing to regenerate no need to render, there is already results on the screen and
        // rerendering would be disruptive
        if !force_regenerate && let Some(completion) = final_completion {
            let prev_content = completion.resp.content.clone();
            info!("found cached prev_content ({} chars)", prev_content.len());
            if !prev_content.is_empty() {
                let msg = FimCompletionMessage {
                    buffer_id,
                    line_cur: ctx.line_cur.clone(),
                    cursor_x: pos_x,
                    cursor_y: pos_y,
                    completion,
                    do_render: true,
                    retry: None,
                };
                msgs.push(msg.into());

                // run async speculative FIM in the background for this position
                // TODO should this just always run even when no hint is shown?
                let hint_shown = state.fim_state.read().hint_shown;
                if hint_shown {
                    prev_for_next_fim = Some(vec![prev_content]);
                }
            }
        }

        info!("cache sending message {:?}", msgs);
        if let Err(e) = tx.send(msgs.into()).await {
            error!(e)
        }

        if let Some(content) = prev_for_next_fim.clone() {
            // regenerate the context to make a speculative FIM
            let (new_x, new_y, final_content) =
                match fim_accept_inner(FimAcceptType::Full, pos_x, pos_y, ctx.line_cur, content) {
                    Ok((new_x, new_y, final_content)) => (new_x, new_y, final_content),
                    Err(e) => {
                        error!(e);
                        return;
                    }
                };

            // overwrite pos_y with all the final_content
            let mut virtual_lines = lines;
            virtual_lines.splice(pos_y..=pos_y, final_content);

            let virtual_ctx = get_local_context(&virtual_lines, new_x, new_y, &state.config.read());

            if let Err(e) = spawn_fim_completion_worker(
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
            .await
            {
                error!(e);
            }
        } else {
            // TODO log error
            if let Err(e) = spawn_fim_completion_worker(
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
            .await
            {
                error!(e);
            }
        }
    });
    Ok(())
}

/// Implementation of FIM worker with optional debounce sequence tracking
///
/// NOTE this DOES NOT happens on the neovim main thread - don't call neovim functions
#[allow(clippy::too_many_arguments)]
#[tracing::instrument(skip(ctx, buffer_lines))]
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
            info!("Within debounce period, (seq {seq}, remaining {remaining_ms}ms)");

            // Wait for remaining debounce time
            tokio::time::sleep(Duration::from_millis(remaining_ms)).await
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
        info!("Discarding stale worker after wait (seq {seq} < latest {latest_sequence})");
        drop(permit);
        return Ok(());
    }

    state.record_worker_spawn();

    info!("Spawning worker for ({cursor_x}, {cursor_y})");

    let skip = if let Some((gbuf_id, gpos_x, gpos_y)) = *state.fim_worker_generating_for_pos.read()
        && gbuf_id == buffer_id
        && gpos_x == cursor_x
        && gpos_y == cursor_y
    {
        info!("already currently generating for this position, skipping request");
        true
    } else {
        false
    };

    if !skip {
        *state.fim_worker_generating_for_pos.write() = Some((buffer_id, cursor_x, cursor_y));
        fim_completion(
            state.clone(),
            FimLLM::Fast, // XXX
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
#[tracing::instrument]
pub async fn fim_completion(
    state: Arc<PluginState>,
    m: FimLLM,
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
    let (n_predict_inner, n_predict_end, t_max_prompt_ms, mut t_max_predict_ms, ring_chunk_size) = {
        let config = state.config.read();
        (
            config.get_n_predict_inner(m),
            config.get_n_predict_end(m),
            config.get_t_max_prompt_ms(m),
            config.get_t_max_predict_ms(m),
            config.get_ring_chunk_size(m),
        )
    };

    // Determine n_predict dynamically based on cursor position
    let (n_predict, inside_a_line) =
        get_dynamic_n_predict(&ctx.line_cur, pos_x, n_predict_inner, n_predict_end);
    info!(
        "dynamic n_predict: {} (inner: {}, end: {})",
        n_predict, n_predict_inner, n_predict_end
    );

    let truncate_to_single_line =
        inside_a_line && state.config.read().single_line_prediction_within_line;

    // Get local context

    // Skip auto FIM if too many suffix lines, this might be the case for not dense text
    if ctx.line_cur_suffix.len() > state.config.read().get_max_line_suffix(m) as usize {
        info!(
            "Skipping FIM due to large suffix ({} chars)",
            ctx.line_cur_suffix.len()
        );
        return Ok(());
    }
    if prev.is_none() {
        // the first request should be quick - we will launch a speculative request after this one is displayed
        t_max_predict_ms = 250 // TODO parameterize this
    }

    let hashes = compute_hashes(&ctx.prefix, &ctx.middle, &ctx.suffix);

    // if we already have a cached completion for one of the hashes, don't send a request
    if !force_regenerate && state.config.read().auto_fim {
        for hash in &hashes {
            let cache_lock = state.cache.read();
            if cache_lock.contains_key(hash) {
                info!("FIM completion cached for hash, returning early");
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
        let rb = state.get_ring_buffer(m);
        let mut rb_ = rb.write();

        let chunk = rb_.get_chunk_from_text(text);
        if !chunk.is_empty() {
            rb_.evict_similar_from_live(chunk, 0.5); // TODO parameterize this
        }
    }

    // Build request
    let extra = state.get_ring_buffer(m).read().get_extra();

    let tx = match state.get_fim_completion_tx() {
        Ok(tx) => tx,
        Err(e) => {
            error!(e);
            return Ok(());
        }
    };

    let last_pick = state.get_last_pick_buf_id_pos_y(m);
    let state_ = state.clone();

    // get the next 10 lines past pos_y for tail filtering
    let end = (pos_y + 11).min(lines.len());
    let ten_lines = lines
        .get(pos_y + 1..end)
        .map(|s| s.to_vec())
        .unwrap_or_default();

    let state__ = state.clone();
    let handle = tokio::spawn(async move {
        let response_text = match state__
            .send_fim_request_full(m, &ctx, extra, t_max_prompt_ms, t_max_predict_ms, n_predict)
            .await
        {
            Ok(response_text) => response_text,
            Err(e) => {
                info!("send_request error: {}", e);
                return;
            }
        };

        // Parse response
        let mut response = match serde_json::from_str::<FimResponse>(&response_text) {
            Ok(r) => r,
            Err(e) => {
                info!("FimResponse parse error: {}", e);
                return;
            }
        };

        info!(
            "FIM response received, content length: {}",
            response.content.len()
        );

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

        let response = FimResponseWithInfo {
            resp: response,
            cached: false,
            model: m.into(),
        };

        // Cache the response with timing info (new block for re-acquired locks)
        {
            let mut cache_lock = state__.cache.write();
            for hash in &hashes {
                cache_lock.insert(hash.clone(), response.clone());
            }
        }

        let msg = FimCompletionMessage {
            buffer_id,
            line_cur: ctx.line_cur,
            cursor_x: pos_x,
            cursor_y: pos_y,
            completion: response,
            do_render,
            retry,
        };

        if let Err(e) = tx.send(msg.into()).await {
            info!(e);
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
                config.get_ring_scope(m) as usize,
                config.n_prefix as usize,
                config.n_suffix as usize,
                config.get_ring_chunk_size(m) as usize,
                config.get_ring_n_picks(m) as usize,
            )
        };

        let max_y = lines.len() - 1;

        {
            // Pick chunks from prefix scope - loop based on ring_n_picks
            let prefix_start = pos_y.saturating_sub(ring_scope).min(max_y);
            let prefix_end = pos_y.saturating_sub(n_prefix).min(max_y);
            let prefix_lines = &lines[prefix_start..=prefix_end];
            if !prefix_lines.is_empty() {
                for _ in 0..ring_n_picks {
                    state_.pick_chunk(prefix_lines, filename.clone())
                }
            }

            // Pick chunks from suffix scope - loop based on ring_n_picks
            let suffix_start = (pos_y + n_suffix).min(max_y);
            let suffix_end = (pos_y + n_suffix + ring_chunk_size).min(max_y);
            let suffix_lines = &lines[suffix_start..=suffix_end];
            if !suffix_lines.is_empty() {
                for _ in 0..ring_n_picks {
                    state_.pick_chunk(suffix_lines, filename.clone())
                }
            }
        }

        // Update the last pick position
        state_.set_last_pick_buf_id_pos_y(m, buffer_id, pos_y);
    }

    // wait until the request handle has completed before spawning
    handle.await?;
    Ok(())
}
