use {
    crate::{
        LttwResult, autocmd::clear_filetype_autocommand, debug_word_statistics,
        diagnostics::debug_output_diagnostics, disable_info, disable_plugin, enable_info,
        enable_plugin, instruction, is_enabled, toggle_auto_fim,
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
