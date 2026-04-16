// src/lib.rs - Library interface for lttw Neovim plugin
//
// This module provides the entry point for the Neovim plugin using nvim-oxi.
// All core logic is implemented in Rust modules and exposed to Neovim via FFI.

#[macro_use]
pub mod log; // note, must be first for the macro to work throughout
pub mod autocmd;
pub mod cache;
pub mod commands;
pub mod config;
pub mod context;
pub mod diagnostics;
pub mod diff_chunk;
pub mod error;
pub mod filetype;
pub mod fim;
pub mod instruction;
pub mod keymap;
pub mod llama_client;
pub mod lsp_completion;
pub mod plugin_state;
pub mod ring_buffer;
pub mod router;
pub mod utils;

pub use {
    error::{Error, LttwResult},
    fim::FimTimings,
    llama_client::{FimRequest, FimResponse, FimResponseWithInfo},
    router::{DisplayMessage, FimCompletionMessage},
};

use {
    diff_chunk::calculate_diff_between_contents,
    fim::fim_try_hint,
    nvim_oxi::libuv::AsyncHandle,
    nvim_oxi::{Dictionary, Function},
    plugin_state::{get_state, init_state, PluginState},
    router::process_pending_display,
    std::time::Duration,
    tokio::sync::mpsc,
    utils::{
        clear_buf_namespace_objects, get_buf_filename, get_buf_lines, get_current_buffer_id,
        get_current_buffer_info, get_mode_bz, get_pos, get_yanked_text, in_insert_mode,
        set_buf_extmark_right, set_buf_top_right_pos_y,
    },
};

/// Initialize the plugin with configuration
///
/// # Arguments
/// * `config` - Configuration dictionary from Neovim
///
/// # Returns
/// * `Ok(Dictionary)` - Dictionary of exposed functions
/// * `Err(nvim_oxi::Error)` - Error message if initialization failed
#[nvim_oxi::plugin]
pub fn lttw() -> LttwResult<Dictionary> {
    let _span = tracing::info_span!("plugin_init").entered();
    let mut functions = Dictionary::new();

    functions.insert::<&str, Function<nvim_oxi::Object, ()>>("setup", Function::from(setup));
    functions.insert::<&str, Function<nvim_oxi::Object, ()>>(
        "process_pending_display",
        Function::from(process_pending_display_neovim),
    );
    Ok(functions)
}

/// Initialize the plugin setup with tracing
#[tracing::instrument(skip(c))]
fn setup(c: nvim_oxi::Object) {
    let _span = tracing::info_span!("plugin_setup").entered();
    // Initialize plugin state
    init_state(c);

    let state = get_state();
    let (tracing_enabled, log_file, tracing_level, disable_cleanup) = {
        let config = state.config.read();
        (
            config.tracing_enabled,
            config.tracing_log_file,
            config.tracing_level.clone(),
            config.disable_cleanup,
        )
    };

    // Initialize persistent tokio runtime and completion channel
    init_completion_processing_and_tracing_thread(
        tracing_enabled,
        log_file,
        tracing_level,
        disable_cleanup,
    );

    // Setup timer-based ring buffer updates (every ring_update_ms)
    let _ = ring_buffer::setup_ring_buffer_timer();

    // Register nvim-oxi commands
    let _ = commands::register_commands();

    // Setup keymaps
    let _ = keymap::setup_keymaps();

    // Setup autocmds
    let _ = autocmd::setup_filetype_autocmd();
    let _ = autocmd::setup_non_filetype_autocmds();

    // Initialize the LttwFIM highlight group to match Comment
    let _ = init_fim_highlight();

    tracing::info!("Lttw plugin setup complete");
}

// NOTE we do not not need to wrap this in a nvim_oxi::schedule(...) as
// the callback for the lsp completions happens on the main thread of neovim.
#[tracing::instrument]
fn process_pending_display_neovim(_obj: nvim_oxi::Object) {
    tracing::info!("Processing pending display messages... FROM_NEOVIM");
    if let Err(e) = process_pending_display() {
        info!("process_pending_display() error: {}", e);
    }
}

/// Highlight group name for FIM generated text
pub const LTTW_FIM_HIGHLIGHT: &str = "LttwFIM";

/// Initialize the LttwFIM highlight group to match the Comment highlight group
/// Reads the Comment highlight attributes using Neovim's get_hl_by_name() and applies them to LttwFIM
fn init_fim_highlight() -> LttwResult<()> {
    nvim_oxi::api::set_hl(
        0,
        LTTW_FIM_HIGHLIGHT,
        &nvim_oxi::api::opts::SetHighlightOpts::builder()
            .link("Comment")
            .build(),
    )?;
    Ok(())
}

/// Initialize persistent tokio runtime and completion channel
/// also start tracing.
#[tracing::instrument]
fn init_completion_processing_and_tracing_thread(
    tracing_enabled: bool,
    log_file: bool,
    trace_level: String,
    disable_cleanup: bool,
) {
    let state = get_state();

    // Create channel for completion messages
    let (tx, mut rx) = mpsc::channel::<DisplayMessage>(16);
    *state.fim_completion_tx.write() = Some(tx);

    let handle = match AsyncHandle::new(move || {
        // Need to use schedule here so that it executes on the main thread (or else will kill
        // neovim). Additionally we can't simply call the following code without use of the handle
        // (makes tokio panic)
        nvim_oxi::schedule(move |_| {
            if let Err(e) = process_pending_display() {
                info!("process_pending_display() error: {}", e);
            }
        });
    }) {
        Ok(handle) => handle,
        Err(e) => {
            error!("Failed to create async handle: {}", e);
            // not much more we can do
            return;
        }
    };

    // Spawn a task that receives completion messages and adds them to the pending display queue
    // This runs on its own dedicated current-thread runtime separate from the main multi-threaded one
    let state_ = state.clone();
    let rt = state.tokio_runtime.clone();
    rt.read().spawn(async move {
        let _gaurd = if tracing_enabled {
            Some(log::init_tracing_subscriber(log_file, trace_level))
        } else {
            None
        };
        while let Some(msg) = rx.recv().await {
            info!("pending_queue msg received");
            state_.pending_display.write().push(msg);
            if let Err(e) = handle.send() {
                error!("Error when sending to async handle: {}", e);
            }
        }
    });

    // failsafe - TODO the goal is to remove this chunk of code
    //
    // Set up a Neovim timer to periodically process the pending display queue This ensures display
    // updates happen on the main thread
    //
    // NOTE This won't work with a tokio thread, it needs to execute on the neovim main thread
    // to actually render extmarks
    if !disable_cleanup {
        let _ = nvim_oxi::libuv::TimerHandle::start(
            Duration::from_millis(100),
            Duration::from_millis(100), // repeat every 100ms
            |_| {
                // Need this so that it executes on the main thread (or else extmarks won't display)
                nvim_oxi::schedule(|_| {
                    if let Err(e) = fim_hide_if_not_insert() {
                        info!("fim_hide_if_not_insert() error: {}", e);
                    }
                });
            },
        );
    }
}

fn fim_hide_if_not_insert() -> LttwResult<()> {
    if !in_insert_mode()? {
        fim_hide()?; // failsafe if somehow a hint weezled its way in there
    }
    Ok(())
}

/// FIM hide function - clears the FIM hint from display
#[tracing::instrument]
fn fim_hide() -> LttwResult<()> {
    let state = get_state();
    fim_hide_inner(&state)?;
    Ok(())
}

#[tracing::instrument]
fn move_info_line_if_buf_shifted(state: &PluginState) -> LttwResult<()> {
    let Some(ns_id_val) = state.info_ns else {
        return Ok(());
    };
    let top_line = set_buf_top_right_pos_y();
    if let Some((ref ns_top_line, ref info_string)) = *state.info_ns_line.read()
        && *ns_top_line != top_line
    {
        clear_buf_namespace_objects(ns_id_val)?;
        set_buf_extmark_right(ns_id_val, info_string, top_line)?;
    }
    Ok(())
}

#[tracing::instrument]
fn fim_hide_inner(state: &PluginState) -> LttwResult<()> {
    // Clear virtual text using nvim_buf_clear_namespace()
    if let Some(ns_id_val) = state.extmark_ns {
        clear_buf_namespace_objects(ns_id_val)?
    }
    state.fim_state.write().clear();

    //clear_info_line_if_moved(state)?;

    Ok(())
}
