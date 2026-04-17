use {
    crate::{
        LttwResult,
        fim::{
            accept::{FimAcceptType, fim_accept},
            cycle::{fim_cycle_next, fim_cycle_prev},
            fim_try_hint_regenerate,
        },
        get_state,
    },
    nvim_oxi::api::{del_keymap, opts::SetKeymapOptsBuilder, set_keymap, types::Mode},
};

// Expression mapping helper functions removed - using command-based callbacks instead
/// Setup keymaps function - maps keys to call nvim-oxi commands directly
#[tracing::instrument]
pub fn setup_keymaps() -> LttwResult<()> {
    let state = get_state();
    let config = state.config.read();

    //// Instruction trigger
    //let _ = set_keymap(
    //    Mode::Normal,
    //    "<leader>lli",
    //    ":LttwInst<CR>",
    //    &Default::default(),
    //);

    //// Instruction rerun
    //let _ = set_keymap(
    //    Mode::Normal,
    //    "<leader>llr",
    //    ":LttwInstRerun<CR>",
    //    &Default::default(),
    //);

    //// Instruction continue
    //let _ = set_keymap(
    //    Mode::Normal,
    //    "<leader>llc",
    //    ":LttwInstContinue<CR>",
    //    &Default::default(),
    //);

    // FIM keymaps - use command-based callbacks for proper TAB handling
    // These commands check if FIM hint is shown and act accordingly

    // FIM accept line (TAB) - check if FIM shown, accept line if yes, re-inject S-Tab if no
    if !config.keymap_fim_accept_line.is_empty() {
        let _ = set_keymap(
            Mode::Insert,
            &config.keymap_fim_accept_line,
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
    }

    // FIM accept full (Shift TAB) - check if FIM shown, accept if yes, insert tab if no
    if !config.keymap_fim_accept_full.is_empty() {
        let _ = set_keymap(
            Mode::Insert,
            &config.keymap_fim_accept_full,
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
    }

    // FIM regenerate (CTRL-l) - trigger new completion at current position
    if !config.keymap_fim_force_retrigger.is_empty() {
        let _ = set_keymap(
            Mode::Insert,
            &config.keymap_fim_force_retrigger,
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
    }

    // FIM cycle next (CTRL-j) - cycle through completion options
    if !config.keymap_fim_cycle_fim_next.is_empty() {
        let _ = set_keymap(
            Mode::Insert,
            &config.keymap_fim_cycle_fim_next,
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
    }

    // FIM cycle previous (CTRL-k) - cycle through completion options
    if !config.keymap_fim_cycle_fim_prev.is_empty() {
        let _ = set_keymap(
            Mode::Insert,
            &config.keymap_fim_cycle_fim_prev,
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
    }

    Ok(())
}

/// Remove keymaps function - unmaps all plugin keymaps
#[tracing::instrument]
pub fn remove_keymaps() -> LttwResult<()> {
    let state = get_state();
    let config = state.config.read();

    // Unmap FIM keymaps
    if !config.keymap_fim_accept_line.is_empty() {
        let _ = del_keymap(Mode::Insert, &config.keymap_fim_accept_line);
    }
    if !config.keymap_fim_accept_full.is_empty() {
        let _ = del_keymap(Mode::Insert, &config.keymap_fim_accept_full);
    }
    if !config.keymap_fim_force_retrigger.is_empty() {
        let _ = del_keymap(Mode::Insert, &config.keymap_fim_force_retrigger);
    }
    if !config.keymap_fim_cycle_fim_next.is_empty() {
        let _ = del_keymap(Mode::Insert, &config.keymap_fim_cycle_fim_next);
    }
    if !config.keymap_fim_cycle_fim_prev.is_empty() {
        let _ = del_keymap(Mode::Insert, &config.keymap_fim_cycle_fim_prev);
    }

    Ok(())
}
