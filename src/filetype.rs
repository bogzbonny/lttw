use {
    crate::{disable_plugin, enable_plugin, get_state},
    nvim_oxi::{
        api::{get_option_value, opts::OptionOpts},
        Result as NvimResult,
    },
    std::sync::atomic::Ordering,
};

/// Get filetype function
pub fn get_filetype() -> NvimResult<String> {
    let ft = get_option_value::<String>("filetype", &OptionOpts::default())?;
    Ok(ft)
}

/// Filetype check autocmd handler - enables/disables plugin based on filetype
pub fn on_buf_enter_check_filetype() -> NvimResult<()> {
    let is_enabled = {
        let state = get_state();
        state.enabled.load(Ordering::SeqCst)
    };

    // Check if current filetype should enable/disable the plugin
    let should_be_enabled = {
        let state = get_state();
        let filetype = get_filetype().unwrap_or_default();
        let config = state.config.read();
        let out = config.is_filetype_enabled(&filetype);

        state.debug_manager.read().log(
            "on_buf_enter_check_filetype",
            format!("filetype {filetype}, should_be_enabled {out}",),
        );
        out
    };

    if should_be_enabled && !is_enabled {
        enable_plugin()?;
    } else if !should_be_enabled && is_enabled {
        disable_plugin()?;
    }
    Ok(())
}
