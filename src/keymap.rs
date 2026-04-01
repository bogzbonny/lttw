use {
    crate::{fim::FimAcceptType, fim_accept, fim_is_hint_shown, get_state},
    nvim_oxi::{
        api::{
            opts::SetKeymapOptsBuilder,
            types::Mode,
            {self},
        },
        Result as NvimResult,
    },
};

// Expression mapping helper functions removed - using command-based callbacks instead
/// Setup keymaps function - maps keys to call nvim-oxi commands directly
pub fn setup_keymaps() -> NvimResult<()> {
    // Instruction trigger
    let _ = api::set_keymap(
        Mode::Normal,
        "<leader>lli",
        ":LttwInst<CR>",
        &Default::default(),
    );

    // Instruction rerun
    let _ = api::set_keymap(
        Mode::Normal,
        "<leader>llr",
        ":LttwInstRerun<CR>",
        &Default::default(),
    );

    // Instruction continue
    let _ = api::set_keymap(
        Mode::Normal,
        "<leader>llc",
        ":LttwInstContinue<CR>",
        &Default::default(),
    );

    // FIM keymaps - use command-based callbacks for proper TAB handling
    // These commands check if FIM hint is shown and act accordingly

    // FIM accept full (TAB) - check if FIM shown, accept if yes, insert tab if no
    let _ = api::set_keymap(
        Mode::Insert,
        "<Tab>",
        "",
        &SetKeymapOptsBuilder::default()
            .callback(|_| {
                if let Ok(true) = fim_is_hint_shown() {
                    if let Err(e) = fim_accept(FimAcceptType::Full) {
                        // Log error but don't crash
                        let state = get_state();
                        state
                            .debug_manager
                            .read()
                            .log("Tab accept", &[&format!("Error accepting FIM: {:?}", e)]);
                    }
                }
                // TODO insert tab if not in hint mode
            })
            .build(),
    );

    // FIM accept line (S-Tab) - check if FIM shown, accept line if yes, re-inject S-Tab if no
    let _ = api::set_keymap(
        Mode::Insert,
        "<S-Tab>",
        "",
        &SetKeymapOptsBuilder::default()
            .callback(|_| {
                if let Ok(true) = fim_is_hint_shown() {
                    if let Err(e) = fim_accept(FimAcceptType::Line) {
                        // Log error but don't crash
                        let state = get_state();
                        state.debug_manager.read().log(
                            "LttwFimAcceptFullOrTab",
                            &[&format!("Error accepting FIM: {:?}", e)],
                        );
                    }
                }
                // TODO insert tab if not in hint mode
            })
            .build(),
    );

    Ok(())
}

/// Remove keymaps function - unmaps all plugin keymaps
pub fn remove_keymaps() -> NvimResult<()> {
    let state = get_state();
    let config = state.config.read();

    // Unmap FIM keymaps
    if !config.keymap_fim_trigger.is_empty() {
        let _ = api::del_keymap(Mode::Normal, &config.keymap_fim_trigger);
    }
    if !config.keymap_fim_accept_full.is_empty() {
        let _ = api::del_keymap(Mode::Normal, &config.keymap_fim_accept_full);
    }
    if !config.keymap_fim_accept_line.is_empty() {
        let _ = api::del_keymap(Mode::Normal, &config.keymap_fim_accept_line);
    }
    if !config.keymap_fim_accept_word.is_empty() {
        let _ = api::del_keymap(Mode::Normal, &config.keymap_fim_accept_word);
    }

    // Unmap instruction keymaps
    if !config.keymap_inst_trigger.is_empty() {
        let _ = api::del_keymap(Mode::Normal, &config.keymap_inst_trigger);
    }
    if !config.keymap_inst_rerun.is_empty() {
        let _ = api::del_keymap(Mode::Normal, &config.keymap_inst_rerun);
    }
    if !config.keymap_inst_continue.is_empty() {
        let _ = api::del_keymap(Mode::Normal, &config.keymap_inst_continue);
    }
    if !config.keymap_inst_accept.is_empty() {
        let _ = api::del_keymap(Mode::Normal, &config.keymap_inst_accept);
    }
    if !config.keymap_inst_cancel.is_empty() {
        let _ = api::del_keymap(Mode::Normal, &config.keymap_inst_cancel);
    }

    // Unmap debug keymaps
    if !config.keymap_debug_toggle.is_empty() {
        let _ = api::del_keymap(Mode::Normal, &config.keymap_debug_toggle);
    }

    // Unmap FIM insert-mode keymaps for accept/cancel (these are always set up)
    let _ = api::del_keymap(Mode::Insert, "<Tab>");
    let _ = api::del_keymap(Mode::Insert, "<Esc>");
    let _ = api::del_keymap(Mode::Insert, "<S-Tab>");

    Ok(())
}
