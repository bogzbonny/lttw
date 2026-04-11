use {
    crate::{
        calculate_diff_between_contents,
        commands::{disable_plugin, enable_plugin},
        diagnostics::handle_diagnostic_changed,
        filetype::should_be_enabled,
        fim_hide, fim_try_hint, get_buf_filename, get_buf_lines, get_current_buffer_id,
        get_current_buffer_info, get_mode_bz, get_pos, get_state, get_yanked_text,
        ring_buffer::mode_change_maybe_start_processing_ring_updates,
        utils::{create_autocmd, del_autocmd},
        LttwResult,
    },
    std::sync::atomic::Ordering,
    std::time::Instant,
};

/// Setup autocmds function - creates autocmds for auto-triggering FIM and ring buffer
#[tracing::instrument]
pub fn setup_non_filetype_autocmds() -> LttwResult<()> {
    clear_non_filetype_autocommands()?;

    let state = get_state();
    let mut ids = Vec::new();

    let id = create_autocmd(
        ["InsertLeavePre", "CompleteChanged"],
        &nvim_oxi::api::opts::CreateAutocmdOptsBuilder::default()
            .callback(|_| {
                if let Err(e) = fim_hide() {
                    error!(e)
                }
                false
            })
            .build(),
    )
    .unwrap_or(0);
    ids.push(id);

    let id = create_autocmd(
        ["ModeChanged"],
        &nvim_oxi::api::opts::CreateAutocmdOptsBuilder::default()
            .callback(|_| {
                // TODO log error
                let _ = set_mode_in_state();
                let _ = mode_change_maybe_start_processing_ring_updates();
                false
            })
            .build(),
    )
    .unwrap_or(0);
    ids.push(id);

    let id = create_autocmd(
        ["CompleteDone"],
        &nvim_oxi::api::opts::CreateAutocmdOptsBuilder::default()
            .callback(|_| {
                let _ = on_move();
                false
            })
            .build(),
    )
    .unwrap_or(0);
    ids.push(id);

    if state.config.read().auto_fim {
        info!("registering auto fim autocmds");
        let id = create_autocmd(
            ["CursorMoved", "CursorMovedI"],
            &nvim_oxi::api::opts::CreateAutocmdOptsBuilder::default()
                .callback(|_| {
                    let _ = on_move();
                    false
                })
                .build(),
        )
        .unwrap_or(0);
        ids.push(id);
    }

    // DiagnosticChanged - track diagnostics when they change
    let id = create_autocmd(
        ["DiagnosticChanged"],
        &nvim_oxi::api::opts::CreateAutocmdOptsBuilder::default()
            .callback(|_| {
                info!("DiagnosticChanged autocmd fired");
                let _ = handle_diagnostic_changed(nvim_oxi::Object::nil());
                false
            })
            .build(),
    )
    .unwrap_or(0);
    ids.push(id);

    // For keeping track of the active buffer and whether it is modified
    // for the tokio threads
    let id = create_autocmd(
        ["BufModifiedSet", "BufEnter"],
        &nvim_oxi::api::opts::CreateAutocmdOptsBuilder::default()
            .callback(|_| {
                let _ = set_cur_buffer_info_in_state();
                false
            })
            .build(),
    )
    .unwrap_or(0);
    ids.push(id);

    // Update file contents when buffer is first opened (only if not already stored)
    // This runs on BufEnter and checks if diff_tracking is enabled
    let id = create_autocmd(
        ["BufEnter", "BufWinEnter"],
        &nvim_oxi::api::opts::CreateAutocmdOptsBuilder::default()
            .callback(|_| {
                info!("BufEnter diff_tracking_enabled autocmd fired");
                let _ = on_buf_enter_update_file_contents();
                false
            })
            .build(),
    )
    .unwrap_or(0);
    ids.push(id);

    // Yank text for ring buffer (TextYankPost)
    let id = create_autocmd(
        ["TextYankPost"],
        &nvim_oxi::api::opts::CreateAutocmdOptsBuilder::default()
            .callback(|_| {
                let _ = on_text_yank_post();
                false
            })
            .build(),
    )
    .unwrap_or(0);
    ids.push(id);

    // Buffer leave for ring buffer
    let id = create_autocmd(
        ["BufLeave"],
        &nvim_oxi::api::opts::CreateAutocmdOptsBuilder::default()
            .callback(|_| {
                let _ = on_buf_leave();
                false
            })
            .build(),
    )
    .unwrap_or(0);
    ids.push(id);

    let state = get_state();

    // Buffer write for ring buffer (only if diff tracking is enabled)
    if state.config.read().diff_tracking_enabled {
        info!("registering bufwritepost autocmd");
        let id = create_autocmd(
            ["BufWritePost"],
            &nvim_oxi::api::opts::CreateAutocmdOptsBuilder::default()
                .callback(|_| {
                    let _ = on_buf_write_post();
                    false
                })
                .build(),
        )
        .unwrap_or(0);
        ids.push(id);
    }

    // InsertLeave - hide FIM hint when leaving Insert mode
    let id = create_autocmd(
        ["InsertLeave"],
        &nvim_oxi::api::opts::CreateAutocmdOptsBuilder::default()
            .callback(|_| {
                let _ = fim_hide();
                // TODO log error
                false
            })
            .build(),
    )
    .unwrap_or(0);
    ids.push(id);

    let mut autocmd_ids_lock = state.autocmd_ids.write();
    autocmd_ids_lock.clear();
    autocmd_ids_lock.extend(ids);
    Ok(())
}

#[tracing::instrument]
fn on_move() -> LttwResult<()> {
    let state = get_state();
    *state.last_move_time.write() = Instant::now();

    // Check if cursor has moved to a different position than allow_comment_fim_cur_pos
    let (pos_x, pos_y) = get_pos();
    let buf_id = get_current_buffer_id();

    if let Some((allowed_buf, allowed_x, allowed_y)) = state.get_allow_comment_fim_cur_pos()
        && (buf_id != allowed_buf || pos_x != allowed_x || pos_y != allowed_y)
    {
        info!(
            "on_move clearing allow_comment_fim_cur_pos buf_id={buf_id}, pos_x={pos_x}, \
           pos_y={pos_y}, allowed_buf={allowed_buf}, allowed_x={allowed_x}, allowed_y={allowed_y}",
        );
        state.clear_allow_comment_fim_cur_pos();
    }

    info!("Cursor moved");
    //fim_hide()?; //
    fim_try_hint(None)?;
    Ok(())
}

#[tracing::instrument]
pub fn setup_filetype_autocmd() -> LttwResult<()> {
    clear_filetype_autocommand()?;
    let state = get_state();
    let id = create_autocmd(
        ["BufEnter"],
        &nvim_oxi::api::opts::CreateAutocmdOptsBuilder::default()
            .callback(|_| {
                let _ = on_buf_enter_check_filetype(); // TODO log error

                // Also gather ring buffer chunks
                let _ = on_buf_enter_gather_chunks(); // TODO log error

                false // DO NOT DELETE this autocommand once used
            })
            .build(),
    )
    .unwrap_or(0);
    *state.autocmd_id_filetype_check.write() = Some(id);
    Ok(())
}

#[tracing::instrument]
pub fn clear_non_filetype_autocommands() -> LttwResult<()> {
    let state = get_state();
    let mut autocmd_ids_lock = state.autocmd_ids.write();
    for id in autocmd_ids_lock.drain(..) {
        del_autocmd(id)?
    }
    Ok(())
}

#[tracing::instrument]
pub fn clear_filetype_autocommand() -> LttwResult<()> {
    let state = get_state();
    let ft_ac_id = state.autocmd_id_filetype_check.write().take();
    if let Some(id) = ft_ac_id {
        del_autocmd(id)?;
    }
    Ok(())
}

/// Toggle auto_fim configuration
#[tracing::instrument]
fn set_mode_in_state() -> LttwResult<()> {
    let state = get_state();
    *state.nvim_mode.write() = get_mode_bz()?;
    Ok(())
}

/// Toggle auto_fim configuration
#[tracing::instrument]
fn set_cur_buffer_info_in_state() -> LttwResult<()> {
    let info = get_current_buffer_info()?;
    get_state().set_cur_buffer_info(info);
    Ok(())
}

/// Handle TextYankPost event - gather chunks from yanked text
#[tracing::instrument]
fn on_text_yank_post() -> LttwResult<()> {
    let state = get_state();

    // Get yanked text using vim.fn.getreg()
    // NOTE " is the default register for yanked text
    let reg_content = get_yanked_text()?;

    // Split by newlines to get individual lines
    let yanked: Vec<String> = reg_content.split('\n').map(|s| s.to_string()).collect();

    if !yanked.is_empty() {
        let filename = get_buf_filename().unwrap_or_default();

        info!("Yanked {} lines from {}", yanked.len(), filename,);

        // Pick chunk from yanked text
        let mut ring_buffer_lock = state.ring_buffer.write();
        ring_buffer_lock.pick_chunk(&state, &yanked, filename);
    }

    Ok(())
}

/// Handle BufEnter event - track file content and gather chunks from entered buffer
#[tracing::instrument]
fn on_buf_enter_gather_chunks() -> LttwResult<()> {
    let state = get_state();

    let lines = get_buf_lines(..);

    if lines.len() > 3 {
        let filename = get_buf_filename().unwrap_or_default();

        info!("Entered buffer with {} lines: {}", lines.len(), filename,);

        // Pick chunk from buffer
        let mut ring_buffer_lock = state.ring_buffer.write();
        ring_buffer_lock.pick_chunk(&state, &lines, filename.clone());
    }

    Ok(())
}

/// Handle BufLeave event - track file content and gather chunks from buffer before leaving
#[tracing::instrument]
fn on_buf_leave() -> LttwResult<()> {
    let state = get_state();

    let lines = get_buf_lines(..);

    if lines.len() > 3 {
        let filename = get_buf_filename().unwrap_or_default();

        info!("Leaving buffer with {} lines: {}", lines.len(), filename,);

        // Pick chunk from buffer
        let mut ring_buffer_lock = state.ring_buffer.write();
        ring_buffer_lock.pick_chunk(&state, &lines, filename);
    }

    Ok(())
}

/// Handle BufEnter event - update file contents if not already stored
/// This only reads from disk if there's no existing content saved for this file
#[tracing::instrument]
fn on_buf_enter_update_file_contents() -> LttwResult<()> {
    info!("on_buf_enter_update_file_contents");
    let state = get_state();

    // Only update file contents if diff tracking is enabled
    if !state.config.read().diff_tracking_enabled {
        return Ok(());
    }

    let filename = get_buf_filename()?;

    // If we already have content saved for this file, do nothing
    if state.has_file_contents(&filename) {
        return Ok(());
    }

    let new_content = std::fs::read_to_string(&filename)?;

    // Save the current file content for future diff comparison
    state.set_file_contents(filename.clone(), new_content);

    Ok(())
}

/// Filetype check autocmd handler - enables/disables plugin based on filetype
#[tracing::instrument]
pub fn on_buf_enter_check_filetype() -> LttwResult<()> {
    let is_enabled = {
        let state = get_state();
        state.enabled.load(Ordering::SeqCst)
    };

    // Check if current filetype should enable/disable the plugin
    let should_be_enabled = should_be_enabled();

    if should_be_enabled && !is_enabled {
        enable_plugin()?;
    } else if !should_be_enabled && is_enabled {
        disable_plugin()?;
    }
    Ok(())
}

/// Handle BufWritePost event - track file content and evaluate diff chunks after saving buffer
#[tracing::instrument]
fn on_buf_write_post() -> LttwResult<()> {
    let state = get_state();

    let lines = get_buf_lines(..);

    if lines.len() > 3 {
        let filename = get_buf_filename().unwrap_or_default();

        info!("Buffer saved with {} lines: {filename}", lines.len(),);

        // Pick chunk from buffer
        {
            let mut ring_buffer_lock = state.ring_buffer.write();
            ring_buffer_lock.pick_chunk(&state, &lines, filename.clone());
        }

        if state.config.read().diff_tracking_enabled {
            let has_file = state.has_file_contents(&filename);
            if !has_file {
                state.set_file_contents_empty(filename);
            }

            let mut to_write = Vec::new();
            for (filename_, old_content) in state.file_contents_read().iter() {
                // get the new file contents from the filesystem
                let Ok(new_content) = std::fs::read_to_string(filename_) else {
                    continue;
                };

                // Get saved content for this file
                let diff_chunks = {
                    // Calculate diff between saved content and current content
                    if let Some(old_content) = old_content {
                        calculate_diff_between_contents(filename_, old_content, &new_content)?
                    } else {
                        // No previous content - return empty
                        Vec::new()
                    }
                };

                // TODO should check ALL the files that we've ever looked at.

                to_write.push((filename_.clone(), new_content));

                info!("diff_chunks: {:#?}", diff_chunks);

                // Process diff chunks
                if !diff_chunks.is_empty() {
                    // Apply changes to ring buffer in a separate locked section
                    let mut ring_buffer_lock = state.ring_buffer.write();

                    // Perform additions (after removals)
                    // TODO delete old intersecting chunks too
                    for chunk in &diff_chunks {
                        //let ring_chunk = chunk.to_ring_chunk();
                        ring_buffer_lock.pick_chunk_inner(&chunk.content, chunk.filepath.clone());
                        info!("diff_chunk_added Added to queued: {}", chunk.filepath,);
                    }
                }

                // process word statistics for diff chunks
                for c in diff_chunks {
                    state.adjust_word_statistics_for_diff(c.content);
                }
            }

            for (filename_, new_content) in to_write.into_iter() {
                // Save the current file content for future diff comparison
                state.set_file_contents_bypass_word_stats(filename_.clone(), new_content);
            }
        }
    }

    Ok(())
}
