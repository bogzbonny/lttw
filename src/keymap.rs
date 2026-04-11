use {
    crate::{
        fim::{
            accept::{fim_accept, FimAcceptType},
            cycle::{fim_cycle_next, fim_cycle_prev},
            fim_try_hint_regenerate,
        },
        get_state, LttwResult,
    },
    nvim_oxi::api::{del_keymap, opts::SetKeymapOptsBuilder, set_keymap, types::Mode},
};

// Expression mapping helper functions removed - using command-based callbacks instead
/// Setup keymaps function - maps keys to call nvim-oxi commands directly
#[tracing::instrument]
pub fn setup_keymaps() -> LttwResult<()> {
    // Instruction trigger
    let _ = set_keymap(
        Mode::Normal,
        "<leader>lli",
        ":LttwInst<CR>",
        &Default::default(),
    );

    // Instruction rerun
    let _ = set_keymap(
        Mode::Normal,
        "<leader>llr",
        ":LttwInstRerun<CR>",
        &Default::default(),
    );

    // Instruction continue
    let _ = set_keymap(
        Mode::Normal,
        "<leader>llc",
        ":LttwInstContinue<CR>",
        &Default::default(),
    );

    // FIM keymaps - use command-based callbacks for proper TAB handling
    // These commands check if FIM hint is shown and act accordingly

    // FIM accept full (Shift TAB) - check if FIM shown, accept if yes, insert tab if no
    let _ = set_keymap(
        Mode::Insert,
        "<S-Tab>",
        "",
        &SetKeymapOptsBuilder::default()
            .callback(|_| {
                if let Err(e) = fim_accept(FimAcceptType::Full) {
                    // Log error but don't crash
                    info!("Error accepting FIM: {:?}", e);
                }
                // TODO insert tab if not in hint mode
            })
            .build(),
    );

    // FIM accept line (TAB) - check if FIM shown, accept line if yes, re-inject S-Tab if no
    let _ = set_keymap(
        Mode::Insert,
        "<Tab>",
        "",
        &SetKeymapOptsBuilder::default()
            .callback(|_| {
                if let Err(e) = fim_accept(FimAcceptType::Line) {
                    // Log error but don't crash
                    info!("Error accepting FIM: {:?}", e);
                }
                // TODO insert tab if not in hint mode
            })
            .build(),
    );

    // FIM cycle next (CTRL-j) - cycle through completion options
    let _ = set_keymap(
        Mode::Insert,
        "<C-j>",
        "",
        &SetKeymapOptsBuilder::default()
            .callback(|_| {
                if let Err(e) = fim_cycle_next() {
                    // Log error but don't crash
                    info!("Error cycling to next FIM: {:?}", e,);
                }
            })
            .build(),
    );

    // FIM cycle previous (CTRL-k) - cycle through completion options
    let _ = set_keymap(
        Mode::Insert,
        "<C-k>",
        "",
        &SetKeymapOptsBuilder::default()
            .callback(|_| {
                if let Err(e) = fim_cycle_prev() {
                    // Log error but don't crash
                    info!("Error cycling to previous FIM: {:?}", e,);
                }
            })
            .build(),
    );

    // FIM regenerate (CTRL-l) - trigger new completion at current position
    let _ = set_keymap(
        Mode::Insert,
        "<C-l>",
        "",
        &SetKeymapOptsBuilder::default()
            .callback(|_| {
                if let Err(e) = fim_try_hint_regenerate() {
                    // Log error but don't crash
                    info!("Error regenerating FIM: {:?}", e,);
                }
            })
            .build(),
    );

    Ok(())
}

/// Remove keymaps function - unmaps all plugin keymaps
#[tracing::instrument]
pub fn remove_keymaps() -> LttwResult<()> {
    let state = get_state();
    let config = state.config.read();

    // Unmap FIM keymaps
    if !config.keymap_fim_trigger.is_empty() {
        let _ = del_keymap(Mode::Normal, &config.keymap_fim_trigger);
    }
    if !config.keymap_fim_accept_full.is_empty() {
        let _ = del_keymap(Mode::Normal, &config.keymap_fim_accept_full);
    }
    if !config.keymap_fim_accept_line.is_empty() {
        let _ = del_keymap(Mode::Normal, &config.keymap_fim_accept_line);
    }
    if !config.keymap_fim_accept_word.is_empty() {
        let _ = del_keymap(Mode::Normal, &config.keymap_fim_accept_word);
    }

    // Unmap instruction keymaps
    if !config.keymap_inst_trigger.is_empty() {
        let _ = del_keymap(Mode::Normal, &config.keymap_inst_trigger);
    }
    if !config.keymap_inst_rerun.is_empty() {
        let _ = del_keymap(Mode::Normal, &config.keymap_inst_rerun);
    }
    if !config.keymap_inst_continue.is_empty() {
        let _ = del_keymap(Mode::Normal, &config.keymap_inst_continue);
    }
    if !config.keymap_inst_accept.is_empty() {
        let _ = del_keymap(Mode::Normal, &config.keymap_inst_accept);
    }
    if !config.keymap_inst_cancel.is_empty() {
        let _ = del_keymap(Mode::Normal, &config.keymap_inst_cancel);
    }

    // Unmap debug keymaps
    if !config.keymap_debug_toggle.is_empty() {
        let _ = del_keymap(Mode::Normal, &config.keymap_debug_toggle);
    }

    // Unmap FIM insert-mode keymaps for accept/cancel (these are always set up)
    let _ = del_keymap(Mode::Insert, "<Tab>");
    let _ = del_keymap(Mode::Insert, "<Esc>");
    let _ = del_keymap(Mode::Insert, "<S-Tab>");
    let _ = del_keymap(Mode::Insert, "<C-j>");
    let _ = del_keymap(Mode::Insert, "<C-k>");
    let _ = del_keymap(Mode::Insert, "<C-l>");

    Ok(())
}
