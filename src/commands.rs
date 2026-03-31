use {
    crate::{
        autocommands::clear_filetype_autocommand, debug_clear, debug_log, debug_toggle,
        disable_plugin, enable_plugin, instruction, is_enabled, toggle_auto_fim,
    },
    nvim_oxi::{
        api::{self},
        Result as NvimResult,
    },
};

/// Register nvim-oxi commands for the plugin
pub fn register_commands() -> NvimResult<()> {
    let _ = api::create_user_command(
        "LttwToggleAutoFim",
        |_| -> NvimResult<()> {
            let _ = toggle_auto_fim();
            Ok(())
        },
        &Default::default(),
    );

    let _ = api::create_user_command(
        "LttwDisable",
        |_| -> NvimResult<()> {
            // manual disabling also removes the filetype check autocommand
            clear_filetype_autocommand()?;
            let _ = disable_plugin();
            Ok(())
        },
        &Default::default(),
    );

    let _ = api::create_user_command(
        "LttwEnable",
        |_| -> NvimResult<()> {
            let _ = enable_plugin();
            Ok(())
        },
        &Default::default(),
    );

    let _ = api::create_user_command(
        "LttwIsEnabled",
        |_| -> NvimResult<()> {
            let en = is_enabled();
            let msg = format!("LttwIsEnabled: {}", en);
            // Use nvim_command to execute an echo command
            api::command(&format!("echo '{}'", msg))?;
            Ok(())
        },
        &Default::default(),
    );

    // Instruction commands
    let _ = api::create_user_command(
        "LttwInst",
        |_| -> NvimResult<()> {
            // TODO: Get visual range and start instruction
            debug_log("Starting instruction editing", vec![])?;
            Ok(())
        },
        &Default::default(),
    );

    let _ = api::create_user_command(
        "LttwInstRerun",
        |_| -> NvimResult<()> {
            instruction::inst_rerun()?;
            Ok(())
        },
        &Default::default(),
    );

    let _ = api::create_user_command(
        "LttwInstContinue",
        |_| -> NvimResult<()> {
            instruction::inst_continue()?;
            Ok(())
        },
        &Default::default(),
    );

    // Debug commands
    let _ = api::create_user_command(
        "LttwDebugToggle",
        |_| -> NvimResult<()> {
            debug_toggle()?;
            Ok(())
        },
        &Default::default(),
    );

    let _ = api::create_user_command(
        "LttwDebugClear",
        |_| -> NvimResult<()> {
            debug_clear()?;
            Ok(())
        },
        &Default::default(),
    );

    Ok(())
}
