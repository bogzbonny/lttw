use {
    crate::{
        autocmd, autocmd::clear_filetype_autocommand, fim_hide, get_state, instruction, keymap,
        utils::get_current_filetype, LttwResult,
    },
    nvim_oxi::api::{create_user_command, del_autocmd},
    std::sync::atomic::Ordering,
};

/// Register nvim-oxi commands for the plugin
#[tracing::instrument]
pub fn register_commands() -> LttwResult<()> {
    let _ = create_user_command(
        "LttwToggleAutoFim",
        |_| -> LttwResult<()> {
            if let Err(e) = toggle_auto_fim() {
                error!(e);
            }
            Ok(())
        },
        &Default::default(),
    );

    let _ = create_user_command(
        "LttwDisable",
        |_| -> LttwResult<()> {
            // manual disabling also removes the filetype check autocommand
            clear_filetype_autocommand()?;
            if let Err(e) = disable_plugin() {
                error!(e);
            }
            Ok(())
        },
        &Default::default(),
    );

    let _ = create_user_command(
        "LttwDebugWordStats",
        |_| -> LttwResult<()> {
            if let Err(e) = debug_word_statistics() {
                error!(e);
            }
            Ok(())
        },
        &Default::default(),
    );

    let _ = create_user_command(
        "LttwEnable",
        |_| -> LttwResult<()> {
            if let Err(e) = enable_plugin() {
                error!(e);
            }
            Ok(())
        },
        &Default::default(),
    );

    let _ = create_user_command(
        "LttwIsEnabled",
        |_| -> LttwResult<()> {
            let en = is_enabled();
            let msg = format!("LttwIsEnabled: {}", en);
            // Use nvim_command to execute an echo command
            nvim_oxi::api::command(&format!("echo '{}'", msg))?;
            Ok(())
        },
        &Default::default(),
    );

    // Instruction commands
    let _ = create_user_command(
        "LttwInst",
        |_| -> LttwResult<()> {
            // TODO: Get visual range and start instruction
            //debug_log("Starting instruction editing", vec![])?;
            Ok(())
        },
        &Default::default(),
    );

    let _ = create_user_command(
        "LttwInstRerun",
        |_| -> LttwResult<()> {
            instruction::inst_rerun()?;
            Ok(())
        },
        &Default::default(),
    );

    let _ = create_user_command(
        "LttwInstContinue",
        |_| -> LttwResult<()> {
            instruction::inst_continue()?;
            Ok(())
        },
        &Default::default(),
    );

    // Debug commands

    let _ = create_user_command(
        "LttwDebugClear",
        |_| -> LttwResult<()> {
            //debug_clear()?; // TODO update
            Ok(())
        },
        &Default::default(),
    );

    //// TODO delete
    //let _ = create_user_command(
    //    "LttwDia",
    //    |_: nvim_oxi::api::types::CommandArgs| -> LttwResult<()> {
    //        diagnostics::debug_output_diagnostics(nvim_oxi::Object::nil())?;
    //        Ok(())
    //    },
    //    &Default::default(),
    //);

    let _ = create_user_command(
        "LttwEnableInfo",
        |_| -> LttwResult<()> {
            enable_info()?;
            nvim_oxi::api::command("echo 'info display enabled'")?;
            Ok(())
        },
        &Default::default(),
    );

    let _ = create_user_command(
        "LttwDisableInfo",
        |_| -> LttwResult<()> {
            disable_info()?;
            nvim_oxi::api::command("echo 'info display disabled'")?;
            Ok(())
        },
        &Default::default(),
    );

    // Completion source toggles
    let _ = create_user_command(
        "LttwToggleLspCompletions",
        |_| -> LttwResult<()> {
            let new_value = toggle_lsp_completions();
            nvim_oxi::api::command(&format!(
                "echo 'LSP completions {}'",
                if new_value { "enabled" } else { "disabled" }
            ))?;
            Ok(())
        },
        &Default::default(),
    );

    let _ = create_user_command(
        "LttwToggleLlmCompletions",
        |_| -> LttwResult<()> {
            let new_value = toggle_llm_completions();
            nvim_oxi::api::command(&format!(
                "echo 'LLM completions {}'",
                if new_value { "enabled" } else { "disabled" }
            ))?;
            Ok(())
        },
        &Default::default(),
    );

    let _ = create_user_command(
        "LttwToggleDuelMode",
        |_| -> LttwResult<()> {
            let new_value = toggle_duel_mode();
            nvim_oxi::api::command(&format!(
                "echo 'Duel mode {}'",
                if new_value { "enabled" } else { "disabled" }
            ))?;
            Ok(())
        },
        &Default::default(),
    );

    Ok(())
}

/// Enable info display (set show_info = 2)
#[tracing::instrument]
fn enable_info() -> LttwResult<()> {
    let state = get_state();
    state.config.write().show_info = 2;
    Ok(())
}

/// Disable info display (set show_info = 0)
#[tracing::instrument]
fn disable_info() -> LttwResult<()> {
    let state = get_state();
    state.config.write().show_info = 0;
    Ok(())
}

#[tracing::instrument]
fn is_enabled() -> bool {
    let state = get_state();
    if state.enabled.load(Ordering::SeqCst) {
        return true;
    }
    false
}

/// Enable the plugin - sets up keymaps, autocmds, and state
#[tracing::instrument]
pub fn enable_plugin() -> LttwResult<()> {
    let state = get_state();

    // Check if already enabled
    if state.enabled.load(Ordering::SeqCst) {
        return Ok(());
    }

    // Check filetype
    let filetype = get_current_filetype()?;
    if !state.config.read().is_filetype_enabled(&filetype) {
        info!("Plugin not enabled for filetype: {}", filetype);
        return Ok(());
    }

    info!("Enabling plugin");

    // Setup keymaps
    keymap::setup_keymaps()?;

    // Setup autocmds
    autocmd::setup_non_filetype_autocmds()?;

    // Hide any existing FIM hints
    fim_hide()?;

    // Mark as enabled
    state.enabled.store(true, Ordering::SeqCst);

    Ok(())
}

/// Disable the plugin - removes keymaps, clears autocmds, and hides hints
#[tracing::instrument]
pub fn disable_plugin() -> LttwResult<()> {
    let state = get_state();

    // Check if already disabled
    if !state.enabled.load(Ordering::SeqCst) {
        return Ok(());
    }

    info!("Disabling plugin");

    // Hide FIM hints
    fim_hide()?;

    // Remove keymaps
    keymap::remove_keymaps()?;

    {
        let mut autocmd_ids_lock = state.autocmd_ids.write();
        for id in autocmd_ids_lock.drain(..) {
            del_autocmd(id)?
        }
    }

    // Mark as disabled
    state.enabled.store(false, Ordering::SeqCst);

    Ok(())
}

/// Toggle auto_fim configuration
#[tracing::instrument]
fn toggle_auto_fim() -> LttwResult<bool> {
    let state = get_state();

    // Toggle auto_fim in config
    let new_value = !state.config.read().auto_fim;
    {
        let mut config_lock = state.config.write();
        config_lock.auto_fim = new_value;
    }

    // Re-setup autocmds with new config
    autocmd::setup_non_filetype_autocmds()?;

    Ok(new_value)
}

pub fn debug_word_statistics() -> LttwResult<()> {
    let state = get_state();
    let output = state.debug_word_statistics();
    if output.is_empty() {
        nvim_oxi::api::command("echo 'No word statistics available'")?;
    } else {
        nvim_oxi::api::command(&format!("echo '{}'", output))?;
    }
    Ok(())
}

/// Toggle LSP completions, returns the new state
#[tracing::instrument]
fn toggle_lsp_completions() -> bool {
    let state = get_state();
    let new_value = !state.config.read().lsp_completions;
    {
        let mut config_lock = state.config.write();
        config_lock.lsp_completions = new_value;
    }
    info!(
        "LSP completions {}",
        if new_value { "enabled" } else { "disabled" }
    );
    new_value
}

/// Toggle LLM completions, returns the new state
#[tracing::instrument]
fn toggle_llm_completions() -> bool {
    let state = get_state();
    let new_value = !state.config.read().llm_completions;
    {
        let mut config_lock = state.config.write();
        config_lock.llm_completions = new_value;
    }
    info!(
        "LLM completions {}",
        if new_value { "enabled" } else { "disabled" }
    );
    new_value
}

/// Toggle Duel Mode (dual-model FIM completion), returns the new state
#[tracing::instrument]
fn toggle_duel_mode() -> bool {
    let state = get_state();
    let new_value = !state.config.read().duel_model_mode;
    {
        let mut config_lock = state.config.write();
        config_lock.duel_model_mode = new_value;
    }
    info!(
        "Duel mode {}",
        if new_value { "enabled" } else { "disabled" }
    );
    new_value
}
