use {
    super::info_stats::build_info_string,
    crate::{
        llama_client::FimTimingsData,
        plugin_state::PluginState,
        utils::{self, clear_buf_namespace_objects, set_buf_extmark, set_buf_extmark_top_right},
        FimResponse, FimTimings, LttwResult, LTTW_FIM_HIGHLIGHT,
    },
    nvim_oxi::api::{opts::SetExtmarkOptsBuilder, types::ExtmarkVirtTextPosition},
    std::sync::Arc,
};

/// Render FIM suggestion at the current cursor location
/// Filters out duplicate text that already exists in the buffer
///
/// NOTE this happens ON the neovim main thread
#[tracing::instrument]
pub fn render_fim_suggestion(
    state: Arc<PluginState>,
    pos_x: usize,
    pos_y: usize,
    completion: &FimResponse,
    line_cur: String,
) -> LttwResult<()> {
    let sug_content = &*completion.content;

    let timings = completion.timings.as_ref().map(|timings| {
        FimTimingsData::new(
            timings.clone(),
            completion.tokens_cached,
            completion.truncated,
        )
    });

    // Parse content into lines
    let mut sug_lines: Vec<String> = sug_content.lines().map(|s| s.to_string()).collect();

    // Remove trailing empty lines
    while sug_lines.last().map(|s| s.is_empty()).unwrap_or(false) {
        sug_lines.pop();
    }

    if sug_lines.is_empty() {
        sug_lines.push(String::new());
    }

    // Filter out duplicate text - remove suggested prefix that matches existing suffix
    // Safety: ensure bounds before slicing
    let line_cur_len = line_cur.len();
    let safe_pos_x = pos_x.min(line_cur_len);
    let line_cur_suffix = if safe_pos_x < line_cur_len {
        &line_cur[safe_pos_x..]
    } else {
        ""
    };
    if !line_cur_suffix.is_empty() && !sug_lines.is_empty() && !sug_lines[0].is_empty() {
        // Check if the beginning of the suggestion duplicates existing text
        for i in (0..line_cur_suffix.len()).rev() {
            if sug_lines[0].starts_with(&line_cur_suffix[..=i]) {
                // Remove the duplicate part from the first line of suggestion
                let dup_len = line_cur_suffix[..=i].len();
                if dup_len < sug_lines[0].len() {
                    sug_lines[0] = sug_lines[0][dup_len..].to_string();
                } else {
                    sug_lines[0] = String::new();
                }
                break;
            }
        }
    }

    // Check if only whitespace
    let joined = sug_lines.join("\n");
    let hint_is_valid = !joined.trim().is_empty();

    info!(
        "Displaying FIM hint ({} lines, valid: {})",
        sug_lines.len(),
        hint_is_valid
    );

    // Update FIM state with timing data
    state.fim_state.write().update(
        hint_is_valid,
        pos_x,
        pos_y,
        line_cur.to_string(),
        sug_lines,
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
#[tracing::instrument]
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

        let suffix = if pos_x <= line_cur.len() {
            &line_cur[pos_x..]
        } else {
            ""
        };
        let (suggestion_text, new_suffix, use_infill) =
            trim_suggestion_and_suffix_on_curr_line(&suggestion_text, suffix);

        let suggestion_text = if let Some(new_suffix) = new_suffix {
            suggestion_text.to_owned() + &new_suffix
        } else {
            suggestion_text.to_string()
        };

        // Create extmark opts for suggestion text
        let mut suggestion_opts = SetExtmarkOptsBuilder::default();

        // NOTE here "LttwFIM" is the neovim highlight group
        // (hence the virt text appears like a comment)

        // For single line suggestions, use inline or overlay based on context
        let suggestion_virt_text = vec![(suggestion_text, LTTW_FIM_HIGHLIGHT.to_string())];
        suggestion_opts.virt_text(suggestion_virt_text);

        let suggestion_pos = if content.len() == 1 && use_infill {
            ExtmarkVirtTextPosition::Inline
        } else {
            ExtmarkVirtTextPosition::Overlay
        };
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
                info!("Set suggestion extmark at line {}, col {}", pos_y, pos_x);
            }
            Err(e) => {
                info!("Error setting suggestion extmark: {:?}", e);
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
                info!("Error setting info extmark: {:?}", e);
            }
        }
    }

    Ok(())
}

/// Trims the suggestion if there are matching characters with the beginning of the suffix of the
/// current line at the end of the suffix.
/// Trims the suffix of the current line (existing text) IFF while ignoring the final
/// character of the suggestion the suffix matches the suggestion. This is useful in situations
/// such as:
///     Eg. if suggestion is "Option<String>" and the suffix is "String {" then the suffix matches
///     with the end of the suggestion if ">" was ignored, and thus the final completion should be
///     "Option<String> {"
///  
/// Returns:
///   - trimmed suggestion
///   - trimmed suffix
///   - whether infill should be used
#[tracing::instrument]
pub fn trim_suggestion_and_suffix_on_curr_line<'a>(
    suggestion: &'a str,
    suffix: &str,
) -> (&'a str, Option<String>, bool) {
    // trim the first_line suffix if it is the same as the suffix
    if suggestion.ends_with(suffix) {
        (suggestion.trim_end_matches(suffix), None, true)
    } else {
        // check to see if the suffix should be trimmed at all if matches the suggestion if
        // the suggestions final ch was removed
        let sug_one_less = if suggestion.is_empty() {
            suggestion
        } else {
            let last_char_start = suggestion
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0);
            &suggestion[..last_char_start]
        };

        let new_suffix = utils::remove_matching_prefix(sug_one_less, suffix);
        if new_suffix.len() == suffix.len() {
            return (suggestion, None, true);
        };

        (suggestion, Some(new_suffix), false) // do not infill we must overlay
    }
}
