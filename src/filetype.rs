use crate::{get_state, utils::get_current_filetype};

// Check if current filetype should enable/disable the plugin
#[tracing::instrument]
pub fn should_be_enabled() -> bool {
    let state = get_state();
    let filetype = get_current_filetype().unwrap_or_default();
    let config = state.config.read();
    let out = config.is_filetype_enabled(&filetype);

    info!("filetype {filetype}, should_be_enabled {out}");
    out
}
