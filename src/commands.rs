use {
    crate::{
        LttwResult, autocmd::clear_filetype_autocommand, debug_clear, debug_disable, debug_enable,
        disable_plugin, enable_plugin, instruction, is_enabled, toggle_auto_fim,
    },
    nvim_oxi::api::create_user_command,
};

/// Register nvim-oxi commands for the plugin
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
        "LttwEnableDebug",
        |_| -> LttwResult<()> {
            debug_enable()?;
            nvim_oxi::api::command("echo 'debug enabled'")?;
            Ok(())
        },
        &Default::default(),
    );

    let _ = create_user_command(
        "LttwDisableDebug",
        |_| -> LttwResult<()> {
            debug_disable()?;
            nvim_oxi::api::command("echo 'debug disabled'")?;
            Ok(())
        },
        &Default::default(),
    );

    let _ = create_user_command(
        "LttwDebugClear",
        |_| -> LttwResult<()> {
            debug_clear()?;
            Ok(())
        },
        &Default::default(),
    );

    Ok(())
}
