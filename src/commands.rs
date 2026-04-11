use {
    crate::{
        autocmd::clear_filetype_autocommand, debug_word_statistics,
        diagnostics::debug_output_diagnostics, disable_info, disable_plugin, enable_info,
        enable_plugin, instruction, is_enabled, toggle_auto_fim, LttwResult,
    },
    nvim_oxi::api::create_user_command,
};

/// Register nvim-oxi commands for the plugin
#[tracing::instrument]
pub fn register_commands() -> LttwResult<()> {
    let _ = create_user_command(
        "LttwToggleAutoFim",
        |_| -> LttwResult<()> {
            let _ = toggle_auto_fim();
            Ok(())
        },
        &Default::default(),
    );

    let _ = create_user_command(
        "LttwDisable",
        |_| -> LttwResult<()> {
            // manual disabling also removes the filetype check autocommand
            clear_filetype_autocommand()?;
            let _ = disable_plugin();
            Ok(())
        },
        &Default::default(),
    );

    let _ = create_user_command(
        "LttwDebugWordStats",
        |_| -> LttwResult<()> {
            debug_word_statistics();
            Ok(())
        },
        &Default::default(),
    );

    let _ = create_user_command(
        "LttwEnable",
        |_| -> LttwResult<()> {
            let _ = enable_plugin();
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
            //debug_clear()?; // XXX
            Ok(())
        },
        &Default::default(),
    );

    let _ = create_user_command(
        "LttwDia",
        |_: nvim_oxi::api::types::CommandArgs| -> LttwResult<()> {
            debug_output_diagnostics(nvim_oxi::Object::nil())?;
            Ok(())
        },
        &Default::default(),
    );

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
fn enable_plugin() -> LttwResult<()> {
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
fn disable_plugin() -> LttwResult<()> {
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

pub fn debug_word_statistics() {
    let state = get_state();
    state.debug_word_statistics();
}
