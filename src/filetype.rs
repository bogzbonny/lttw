use {
    crate::{LttwResult, disable_plugin, enable_plugin, get_state, utils::get_current_filetype},
    std::sync::atomic::Ordering,
};

/// Filetype check autocmd handler - enables/disables plugin based on filetype
pub fn on_buf_enter_check_filetype() -> LttwResult<()> {
    let is_enabled = {
        let state = get_state();
        state.enabled.load(Ordering::SeqCst)
    };

    // Check if current filetype should enable/disable the plugin
    let should_be_enabled = should_be_enabled();

    if should_be_enabled && !is_enabled {
        enable_plugin()?;
    } else if !should_be_enabled && is_enabled {
        disable_plugin()?;
    }
    Ok(())
}

// Check if current filetype should enable/disable the plugin
pub fn should_be_enabled() -> bool {
    let state = get_state();
    let filetype = get_current_filetype().unwrap_or_default();
    let config = state.config.read();
    let out = config.is_filetype_enabled(&filetype);

    debug!("filetype {filetype}, should_be_enabled {out}");
    out
}
