use {
    crate::{
        filetype::on_buf_enter_and_check_filetype, fim_hide, get_state, on_buf_leave,
        on_buf_write_post, on_text_yank_post, trigger_fim,
    },
    nvim_oxi::{
        api::{self},
        Result as NvimResult,
    },
};

/// Setup autocmds function - creates autocmds for auto-triggering FIM and ring buffer
pub fn setup_autocmds() -> NvimResult<()> {
    let state = get_state();

    // Clear existing autocmd IDs first (cleanup)
    {
        let mut autocmd_ids_lock = state.autocmd_ids.write();
        autocmd_ids_lock.clear();
    }

    // Cursor movement for auto-FIM (CursorMovedI in insert mode)
    if state.config.read().auto_fim {
        let id = api::create_autocmd(
            ["CursorMovedI", "InsertEnter", "InsertChange"],
            &nvim_oxi::api::opts::CreateAutocmdOptsBuilder::default()
                .callback(|_| {
                    let _ = trigger_fim();
                    false // DO NOT DELETE this autocommand once used
                })
                .build(),
        )
        .unwrap_or(0);
        let mut autocmd_ids_lock = state.autocmd_ids.write();
        autocmd_ids_lock.push(id as u64);
    }

    // Yank text for ring buffer (TextYankPost)
    let id = api::create_autocmd(
        ["TextYankPost"],
        &nvim_oxi::api::opts::CreateAutocmdOptsBuilder::default()
            .callback(|_| {
                let _ = on_text_yank_post();
                false // DO NOT DELETE this autocommand once used
            })
            .build(),
    )
    .unwrap_or(0);
    {
        let mut autocmd_ids_lock = state.autocmd_ids.write();
        autocmd_ids_lock.push(id as u64);
    }

    // Buffer enter for ring buffer AND filetype check
    let id = api::create_autocmd(
        ["BufEnter"],
        &nvim_oxi::api::opts::CreateAutocmdOptsBuilder::default()
            .callback(|_| {
                let _ = on_buf_enter_and_check_filetype();
                false // DO NOT DELETE this autocommand once used
            })
            .build(),
    )
    .unwrap_or(0);
    {
        let mut autocmd_ids_lock = state.autocmd_ids.write();
        autocmd_ids_lock.push(id as u64);
    }

    // Buffer leave for ring buffer
    let id = api::create_autocmd(
        ["BufLeave"],
        &nvim_oxi::api::opts::CreateAutocmdOptsBuilder::default()
            .callback(|_| {
                let _ = on_buf_leave();
                false // DO NOT DELETE this autocommand once used
            })
            .build(),
    )
    .unwrap_or(0);
    {
        let mut autocmd_ids_lock = state.autocmd_ids.write();
        autocmd_ids_lock.push(id as u64);
    }

    // Buffer write for ring buffer
    let id = api::create_autocmd(
        ["BufWritePost"],
        &nvim_oxi::api::opts::CreateAutocmdOptsBuilder::default()
            .callback(|_| {
                let _ = on_buf_write_post();
                false // DO NOT DELETE this autocommand once used
            })
            .build(),
    )
    .unwrap_or(0);
    {
        let mut autocmd_ids_lock = state.autocmd_ids.write();
        autocmd_ids_lock.push(id as u64);
    }

    // InsertLeave - hide FIM hint when leaving Insert mode
    let id = api::create_autocmd(
        ["InsertLeave"],
        &nvim_oxi::api::opts::CreateAutocmdOptsBuilder::default()
            .callback(|_| {
                let _ = fim_hide();
                false // DO NOT DELETE this autocommand once used
            })
            .build(),
    )
    .unwrap_or(0);
    {
        let mut autocmd_ids_lock = state.autocmd_ids.write();
        autocmd_ids_lock.push(id as u64);
    }
    Ok(())
}
