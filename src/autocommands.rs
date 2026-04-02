use {
    crate::{
        filetype::on_buf_enter_check_filetype, fim_hide, get_state, on_buf_enter_gather_chunks,
        on_buf_leave, on_buf_write_post, on_move, on_text_yank_post, trigger_fim,
    },
    nvim_oxi::{
        api::{self, del_autocmd},
        Result as NvimResult,
    },
};

/// Setup autocmds function - creates autocmds for auto-triggering FIM and ring buffer
pub fn setup_non_filetype_autocmds() -> NvimResult<()> {
    clear_non_filetype_autocommands()?;

    let state = get_state();
    let mut ids = Vec::new();

    let id = api::create_autocmd(
        ["InsertLeavePre", "CompleteChanged"],
        &nvim_oxi::api::opts::CreateAutocmdOptsBuilder::default()
            .callback(|_| {
                fim_hide();
                false
            })
            .build(),
    )
    .unwrap_or(0);
    ids.push(id);

    let id = api::create_autocmd(
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
        state
            .debug_manager
            .read()
            .log("registering auto fim autocmds", "");
        let id = api::create_autocmd(
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

        let id = api::create_autocmd(
            //["CursorMovedI", "InsertEnter", "InsertChange"],
            //["CursorMovedI", "InsertEnter"],
            ["CursorMovedI"],
            &nvim_oxi::api::opts::CreateAutocmdOptsBuilder::default()
                .callback(|_| {
                    let _ = trigger_fim();
                    false
                })
                .build(),
        )
        .unwrap_or(0);
        ids.push(id);
    }

    // Yank text for ring buffer (TextYankPost)
    let id = api::create_autocmd(
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
    let id = api::create_autocmd(
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

    // Buffer write for ring buffer
    let id = api::create_autocmd(
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

    // InsertLeave - hide FIM hint when leaving Insert mode
    let id = api::create_autocmd(
        ["InsertLeave"],
        &nvim_oxi::api::opts::CreateAutocmdOptsBuilder::default()
            .callback(|_| {
                fim_hide();
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

pub fn setup_filetype_autocmd() -> NvimResult<()> {
    clear_filetype_autocommand()?;
    let state = get_state();
    let id = api::create_autocmd(
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

pub fn clear_non_filetype_autocommands() -> NvimResult<()> {
    let state = get_state();
    let mut autocmd_ids_lock = state.autocmd_ids.write();
    for id in autocmd_ids_lock.drain(..) {
        del_autocmd(id)?
    }
    Ok(())
}

pub fn clear_filetype_autocommand() -> NvimResult<()> {
    let state = get_state();
    let ft_ac_id = state.autocmd_id_filetype_check.write().take();
    if let Some(id) = ft_ac_id {
        del_autocmd(id)?;
    }
    Ok(())
}
