use {
    crate::{disable_plugin, enable_plugin, get_state, on_buf_enter},
    nvim_oxi::{
        api::{self, Buffer},
        Result as NvimResult,
    },
    std::sync::atomic::Ordering,
};

/// Get filetype function
pub fn get_filetype() -> NvimResult<String> {
    let buf = Buffer::current();
    let path = buf.get_name().map_err(|_| {
        nvim_oxi::Error::Api(api::Error::Other("Failed to get buffer name".to_string()))
    })?;

    // TODO update this
    let filetype = if path.ends_with(".rs") {
        "rust"
    } else if path.ends_with(".py") {
        "python"
    } else if path.ends_with(".js") || path.ends_with(".ts") {
        "javascript"
    } else {
        "unknown"
    };

    Ok(filetype.to_string())
}

/// Check if filetype is enabled
fn is_filetype_enabled() -> NvimResult<bool> {
    let state = get_state();
    let filetype = get_filetype()?;
    let config = state.config.read();
    Ok(config.is_filetype_enabled(&filetype))
}

/// Filetype check autocmd handler - enables/disables plugin based on filetype
pub fn on_buf_enter_and_check_filetype() -> NvimResult<()> {
    let state = get_state();
    let is_enabled = state.enabled.load(Ordering::SeqCst);
    drop(state);

    // Check if current filetype should enable/disable the plugin
    let should_be_enabled = {
        let state = get_state();
        let filetype = get_filetype().unwrap_or_default();
        let config = state.config.read();
        config.is_filetype_enabled(&filetype)
    };

    if should_be_enabled && !is_enabled {
        enable_plugin()?;
    } else if !should_be_enabled && is_enabled {
        disable_plugin()?;
    }

    // Also gather ring buffer chunks (original BufEnter behavior)
    on_buf_enter()
}
