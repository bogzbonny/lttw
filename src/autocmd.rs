use crate::{
    diagnostics::handle_diagnostic_changed,
    filetype::on_buf_enter_check_filetype,
    fim_hide, get_state, on_buf_enter_gather_chunks, on_buf_enter_update_file_contents,
    on_buf_leave, on_buf_write_post, on_move, on_text_yank_post,
    ring_buffer::mode_change_maybe_start_processing_ring_updates,
    set_cur_buffer_info_in_state, set_mode_in_state,
    utils::{create_autocmd, del_autocmd},
    LttwResult,
};

/// Setup autocmds function - creates autocmds for auto-triggering FIM and ring buffer
pub fn setup_non_filetype_autocmds() -> LttwResult<()> {
    clear_non_filetype_autocommands()?;

    let state = get_state();
    let mut ids = Vec::new();

    let id = create_autocmd(
        ["InsertLeavePre", "CompleteChanged"],
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
        debug!("registering auto fim autocmds");
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
                debug!("DiagnosticChanged autocmd fired");
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
    let state = get_state();
    if state.config.read().diff_tracking_enabled {
        let id = create_autocmd(
            ["BufEnter"],
            &nvim_oxi::api::opts::CreateAutocmdOptsBuilder::default()
                .callback(|_| {
                    let _ = on_buf_enter_update_file_contents();
                    false
                })
                .build(),
        )
        .unwrap_or(0);
        ids.push(id);
    }

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
        debug!("registering bufwritepost autocmd");
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

pub fn clear_non_filetype_autocommands() -> LttwResult<()> {
    let state = get_state();
    let mut autocmd_ids_lock = state.autocmd_ids.write();
    for id in autocmd_ids_lock.drain(..) {
        del_autocmd(id)?
    }
    Ok(())
}

pub fn clear_filetype_autocommand() -> LttwResult<()> {
    let state = get_state();
    let ft_ac_id = state.autocmd_id_filetype_check.write().take();
    if let Some(id) = ft_ac_id {
        del_autocmd(id)?;
    }
    Ok(())
}
