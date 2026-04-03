// src/lib.rs - Library interface for lttw Neovim plugin
//
// This module provides the entry point for the Neovim plugin using nvim-oxi.
// All core logic is implemented in Rust modules and exposed to Neovim via FFI.
pub mod autocommands;
pub mod cache;
pub mod commands;
pub mod config;
pub mod context;
pub mod debug;
pub mod error;
pub mod filetype;
pub mod fim;
pub mod instruction;
pub mod keymap;
pub mod plugin_state;
pub mod ring_buffer;
pub mod utils;

pub use error::{Error, LttwResult};

use {
    fim::{fim_try_hint, FimAcceptType},
    nvim_oxi::{
        api::{
            del_autocmd, {self, Buffer, Window},
        },
        Dictionary, Function,
    },
    plugin_state::{get_state, init_state},
    std::{
        convert::TryInto,
        sync::atomic::Ordering,
        time::{Duration, Instant},
    },
    tokio::sync::mpsc,
    utils::{get_buf_filename, get_buf_lines, get_current_buffer_id, get_pos, in_insert_mode},
};

// FIM completion channel types for async communication between worker and main thread
/// Timing information from FIM completion
#[derive(Debug, Clone, Default)]
pub struct FimTimingsData {
    pub n_prompt: i64,
    pub t_prompt_ms: f64,
    pub s_prompt: f64,
    pub n_predict: i64,
    pub t_predict_ms: f64,
    pub s_predict: f64,
    pub tokens_cached: u64,
    pub truncated: bool,
}

/// Message sent from async worker to main thread when completion is ready
#[derive(Debug, Clone)]
pub struct FimCompletionMessage {
    buffer_handle: u64,        // Buffer handle to ensure we're still in same buffer
    buffer_lines: Vec<String>, // All buffer lines captured at start
    cursor_x: usize,           // Cursor position X
    cursor_y: usize,           // Cursor position Y
    content: String,           // FIM response content
    timings: Option<FimTimingsData>, // Timing information from server response
}

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
    let mut functions = Dictionary::new();

    functions.insert::<&str, Function<(), ()>>("lttw_setup", Function::from(|_| lttw_setup()));

    Ok(functions)
}

// TODO process errors in this function
fn lttw_setup() {
    // Initialize plugin state
    init_state();

    // Initialize persistent tokio runtime and completion channel
    init_completion_processing_thread();

    // Setup timer-based ring buffer updates (every ring_update_ms)
    let _ = ring_buffer::setup_ring_buffer_timer();

    // Register nvim-oxi commands
    let _ = commands::register_commands();

    // Setup keymaps
    let _ = keymap::setup_keymaps();

    // Setup autocmds
    let _ = autocommands::setup_filetype_autocmd();
    let _ = autocommands::setup_non_filetype_autocmds();
}

// ---------------------------

/// Check if FIM hint is shown - internal helper for commands
fn fim_is_hint_shown() -> LttwResult<bool> {
    let state = get_state();
    let fim_state_lock = state.fim_state.read();
    Ok(fim_state_lock.hint_shown)
}

/// State for FIM (Fill-in-Middle) completion
#[derive(Debug, Clone, Default)]
pub struct FimState {
    hint_shown: bool,
    pos_x: usize,
    pos_y: usize,
    line_cur: String,
    can_accept: bool,
    content: Vec<String>,
    /// Last cursor Y position where ring buffer chunks were picked
    last_pick_pos_y: Option<usize>,
    /// Timing data from the last completion for display in info string
    timings: Option<FimTimingsData>,
}

impl FimState {
    #[allow(clippy::too_many_arguments)]
    fn update(
        &mut self,
        hint_shown: bool,
        pos_x: usize,
        pos_y: usize,
        line_cur: String,
        can_accept: bool,
        content: Vec<String>,
        timings: Option<FimTimingsData>,
    ) {
        self.hint_shown = hint_shown;
        self.pos_x = pos_x;
        self.pos_y = pos_y;
        self.line_cur = line_cur;
        self.can_accept = can_accept;
        self.content.clear();
        self.content = content;
        self.timings = timings;
    }

    fn clear(&mut self) {
        self.hint_shown = false;
        self.pos_x = 0;
        self.pos_y = 0;
        self.line_cur.clear();
        self.can_accept = false;
        self.content.clear();
        self.last_pick_pos_y = None;
        self.timings = None;
    }

    /// Update the last pick position
    fn set_last_pick_pos_y(&mut self, pos_y: usize) {
        self.last_pick_pos_y = Some(pos_y);
    }
    /// Get the last pick position
    fn get_last_pick_pos_y(&self) -> Option<usize> {
        self.last_pick_pos_y
    }
}

/// Initialize persistent tokio runtime and completion channel
fn init_completion_processing_thread() {
    let state = get_state();

    // Create channel for completion messages
    let (tx, mut rx) = mpsc::channel::<FimCompletionMessage>(16);
    *state.fim_completion_tx.write() = Some(tx);

    // Spawn a task that receives completion messages and adds them to the pending display queue
    // This runs on its own dedicated current-thread runtime separate from the main multi-threaded one
    let state_ = state.clone();
    let rt = state.tokio_runtime.clone();
    rt.read().spawn(async move {
        while let Some(msg) = rx.recv().await {
            state_.debug_manager.read().log("pending_queue msg", "");
            state_.pending_display.write().push(msg);
        }
    });

    // Set up a Neovim timer to periodically process the pending display queue This ensures display
    // updates happen on the main thread
    //
    // NOTE This won't work with a tokio thread, it needs to execute on the neovim main thread
    // to actually render extmarks
    let _ = nvim_oxi::libuv::TimerHandle::start(
        Duration::from_millis(500),
        Duration::from_millis(50), // repeat
        |_| {
            // Need this so that it executes on the main thread (or else extmarks won't display)
            nvim_oxi::schedule(|_| process_pending_display());
        },
    );
}

/// Process pending FIM display queue - drains and displays messages on the main thread
fn process_pending_display() -> LttwResult<()> {
    let state = get_state();

    // Only display if we are in insert mode
    if !in_insert_mode()? {
        fim_hide(); // failsafe if somehow a hint weezled its way in there
        return Ok(());
    }

    // Take all pending messages (clear the queue)
    let messages: Vec<FimCompletionMessage> = {
        let mut pending_queue = state.pending_display.write();
        std::mem::take(&mut *pending_queue)
    };
    if messages.is_empty() {
        return Ok(());
    }

    state.debug_manager.read().log(
        "process_pending_display",
        format!("Processing {} pending display messages", messages.len()),
    );
    // process the most recent message which has content and isn't only whitespace
    for msg in messages.into_iter().rev() {
        if msg.content.is_empty() || msg.content.trim().is_empty() {
            continue;
        }
        handle_fim_completion_message(msg)?;
        break;
    }

    // XXX NOTE the following code would be relatively consistent with llama.vim
    // however it would lead to recursive execution loop... maybe uncomment after
    // trying?
    //
    // if either the hint isn't shown OR it's only whitespace then trigger another fim
    if !state.fim_state.read().hint_shown || !state.fim_state.read().can_accept {
        fim_try_hint()?;
    }

    Ok(())
}

/// Handle FIM completion message received from async worker
fn handle_fim_completion_message(msg: FimCompletionMessage) -> LttwResult<()> {
    let state = get_state();

    // Check if we're still in the same buffer
    let current_buf: u64 = Buffer::current().handle().try_into().unwrap_or(0);
    if current_buf != msg.buffer_handle {
        state.debug_manager.read().log(
            "handle_fim_completion_message",
            format!(
                "Buffer changed, ignoring completion (expected {}, got {})",
                msg.buffer_handle, current_buf
            ),
        );
        return Ok(());
    }

    state.debug_manager.read().log(
        "handle_fim_completion_message",
        format!(
            "Received completion for buffer {} at ({}, {})",
            msg.buffer_handle, msg.cursor_x, msg.cursor_y
        ),
    );

    // Parse response and render
    let ctx = context::get_local_context(
        &msg.buffer_lines,
        msg.cursor_x,
        msg.cursor_y,
        None,
        &state.config.read(),
    );

    //state.debug_manager.read().log(
    //    "handle_fim_completion_message",
    //    format!("msg.content: \n{}", msg.content),
    //);
    fim::render_fim_suggestion(
        state,
        msg.cursor_x,
        msg.cursor_y,
        &msg.content,
        ctx.line_cur,
        msg.timings,
    )
}

/// FIM accept function - accepts the FIM suggestion
fn fim_accept(accept_type: FimAcceptType) -> LttwResult<Option<String>> {
    // Log before releasing the lock
    let state = get_state();
    {
        let debug_manager = state.debug_manager.read().clone();
        debug_manager.log("fim_accept_triggered", "");
    }

    let (hint_shown, can_accept, pos_x, pos_y, line_cur, content) = {
        let fim_state_lock = state.fim_state.read();
        (
            fim_state_lock.hint_shown,
            fim_state_lock.can_accept,
            fim_state_lock.pos_x,
            fim_state_lock.pos_y,
            fim_state_lock.line_cur.clone(),
            fim_state_lock.content.clone(),
        )
    };

    if !hint_shown || !can_accept {
        return Ok(None);
    }

    state.debug_manager.read().log(
        "fim_accept",
        format!("Accepting {} suggestion", accept_type),
    );

    // Use the accept_fim_suggestion function from fim module
    let (new_line, rest, inline_loc) =
        fim::accept_fim_suggestion(accept_type, pos_x, &line_cur, &content);

    state.debug_manager.read().log(
        "fim_accept",
        format!("new_line {new_line}\n\t rest {rest:?}"),
    );

    // Set the buffer lines with the accepted content
    let buf = Buffer::current();

    // Get current lines and convert to owned strings
    let all_lines: Vec<String> = match buf.get_lines(.., false) {
        Ok(iter) => iter.map(|s| s.to_string()).collect(),
        Err(_) => Vec::new(),
    };

    // Update the current line with the new content
    let mut all_lines_modified = all_lines.clone();
    if pos_y < all_lines_modified.len() {
        all_lines_modified[pos_y] = new_line.clone();
    }

    // If there are rest lines (from 'full' or 'line' accept), insert them
    if let Some(rest_lines) = &rest {
        for (i, line) in rest_lines.iter().enumerate() {
            all_lines_modified.insert(pos_y + 1 + i, line.clone());
        }
    }

    // Set the lines back to the buffer (replace from pos_y to end)
    let end_line = if let Some(rest_lines) = &rest {
        pos_y + rest_lines.len() + 1
    } else {
        pos_y + 1
    };

    let mut buf = Buffer::current();
    buf.set_lines(
        pos_y..=pos_y, // replace the one line with all the new content (can be multiple lines)
        true,
        all_lines_modified[pos_y..end_line].to_vec(),
    )?;

    // Move the cursor to the end of the accepted text
    let mut window = Window::current();
    if let Some(rest_lines) = &rest {
        let new_pos_y = pos_y + rest_lines.len();
        let new_pos_x = rest_lines.last().map_or(0, |line| line.len());
        let _ = window.set_cursor(new_pos_y + 1, new_pos_x);
    } else if let Some(inline) = inline_loc {
        let _ = window.set_cursor(pos_y + 1, inline);
    } else {
        let new_col = new_line.len();
        let _ = window.set_cursor(pos_y + 1, new_col);
    }

    // Clear the FIM hint - use write lock
    state.fim_state.write().clear();

    // Clear virtual text from display
    if let Some(ns_id) = state.extmark_ns {
        let mut buf = Buffer::current();
        let _ = buf.clear_namespace(ns_id, ..);
    }

    Ok(Some(new_line))
}

/// FIM hide function - clears the FIM hint from display
fn fim_hide() {
    let state = get_state();
    //state
    //    .debug_manager
    //    .read()
    //    .log("fim_hide", "Hiding FIM hint");

    // Clear virtual text using nvim_buf_clear_namespace()
    if let Some(ns_id_val) = state.extmark_ns {
        let mut buf = Buffer::current();
        let _ = buf.clear_namespace(ns_id_val, ..);
    }

    state.fim_state.write().clear();
}

/// Debug toggle function - toggles logging
fn debug_toggle() -> LttwResult<bool> {
    let state = get_state();
    let enabled = state.debug_manager.read().is_enabled();

    // Toggle logging
    let mut debug_manager_lock = state.debug_manager.write();
    debug_manager_lock.set_enabled(!enabled);

    Ok(!enabled)
}

/// Debug clear function
fn debug_clear() -> LttwResult<()> {
    let state = get_state();
    state.debug_manager.write().clear();
    Ok(())
}

fn is_enabled() -> bool {
    let state = get_state();
    if state.enabled.load(Ordering::SeqCst) {
        return true;
    }
    false
}

/// Enable the plugin - sets up keymaps, autocmds, and state
fn enable_plugin() -> LttwResult<()> {
    let state = get_state();

    // Check if already enabled
    if state.enabled.load(Ordering::SeqCst) {
        return Ok(());
    }

    // Check filetype
    let filetype = filetype::get_filetype()?;
    if !state.config.read().is_filetype_enabled(&filetype) {
        state.debug_manager.read().log(
            "enable_plugin",
            format!("Plugin not enabled for filetype: {}", filetype),
        );
        return Ok(());
    }

    state
        .debug_manager
        .read()
        .log("enable_plugin", "Enabling plugin");

    // Setup keymaps
    keymap::setup_keymaps()?;

    // Setup autocmds
    autocommands::setup_non_filetype_autocmds()?;

    // Hide any existing FIM hints
    fim_hide();

    // Mark as enabled
    state.enabled.store(true, Ordering::SeqCst);

    Ok(())
}

/// Disable the plugin - removes keymaps, clears autocmds, and hides hints
fn disable_plugin() -> LttwResult<()> {
    let state = get_state();

    // Check if already disabled
    if !state.enabled.load(Ordering::SeqCst) {
        return Ok(());
    }

    state
        .debug_manager
        .read()
        .log("disable_plugin", "Disabling plugin");

    // Hide FIM hints
    fim_hide();

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
fn toggle_auto_fim() -> LttwResult<bool> {
    let state = get_state();

    // Toggle auto_fim in config
    let new_value = !state.config.read().auto_fim;
    {
        let mut config_lock = state.config.write();
        config_lock.auto_fim = new_value;
    }

    // Re-setup autocmds with new config
    autocommands::setup_non_filetype_autocmds()?;

    Ok(new_value)
}

fn on_move() -> LttwResult<()> {
    let state = get_state();
    *state.last_move_time.write() = Instant::now();
    state.debug_manager.read().log("on_move", "Cursor moved");
    fim_hide();
    fim_try_hint()?;
    Ok(())
}

/// Handle TextYankPost event - gather chunks from yanked text
fn on_text_yank_post() -> LttwResult<()> {
    let state = get_state();

    // Get yanked text using vim.fn.getreg()
    // NOTE " is the default register for yanked text
    let reg_content: String =
        api::call_function("getreg", ("\"",)).unwrap_or_else(|_| String::new());

    // Split by newlines to get individual lines
    let yanked: Vec<String> = reg_content.split('\n').map(|s| s.to_string()).collect();

    if !yanked.is_empty() {
        let filename = get_buf_filename().unwrap_or_default();

        state.debug_manager.read().log(
            "on_text_yank_post",
            format!("Yanked {} lines from {}", yanked.len(), filename),
        );

        // Pick chunk from yanked text
        let mut ring_buffer_lock = state.ring_buffer.write();
        ring_buffer_lock.pick_chunk(yanked, filename, false, true)?;
    }

    Ok(())
}

/// Handle BufEnter event - gather chunks from entered buffer
fn on_buf_enter_gather_chunks() -> LttwResult<()> {
    let state = get_state();

    let buf = Buffer::current();
    let all_lines = buf.get_lines(.., false);
    let lines: Vec<String> = match all_lines {
        Ok(iter) => iter.map(|s| s.to_string()).collect(),
        Err(_) => Vec::new(),
    };

    if lines.len() > 3 {
        let filename = get_buf_filename().unwrap_or_default();

        state.debug_manager.read().log(
            "on_buf_enter",
            format!("Entered buffer with {} lines: {}", lines.len(), filename),
        );

        // Pick chunk from buffer
        let mut ring_buffer_lock = state.ring_buffer.write();
        ring_buffer_lock.pick_chunk(lines, filename, false, true)?;
    }

    Ok(())
}

/// Handle BufLeave event - gather chunks from buffer before leaving
fn on_buf_leave() -> LttwResult<()> {
    let state = get_state();

    let buf = Buffer::current();
    let all_lines = buf.get_lines(.., false);
    let lines: Vec<String> = match all_lines {
        Ok(iter) => iter.map(|s| s.to_string()).collect(),
        Err(_) => Vec::new(),
    };

    if lines.len() > 3 {
        let filename = get_buf_filename().unwrap_or_default();

        state.debug_manager.read().log(
            "on_buf_leave",
            format!("Leaving buffer with {} lines: {}", lines.len(), filename),
        );

        // Pick chunk from buffer
        let mut ring_buffer_lock = state.ring_buffer.write();
        ring_buffer_lock.pick_chunk(lines, filename, false, true)?;
    }

    Ok(())
}

/// Handle BufWritePost event - gather chunks after saving buffer
fn on_buf_write_post() -> LttwResult<()> {
    let state = get_state();

    let buf = Buffer::current();
    let all_lines = buf.get_lines(.., false);
    let lines: Vec<String> = match all_lines {
        Ok(iter) => iter.map(|s| s.to_string()).collect(),
        Err(_) => Vec::new(),
    };

    if lines.len() > 3 {
        let filename = get_buf_filename().unwrap_or_default();

        state.debug_manager.read().log(
            "on_buf_write_post",
            format!("Buffer saved with {} lines: {filename}", lines.len()),
        );

        // Pick chunk from buffer
        let mut ring_buffer_lock = state.ring_buffer.write();
        ring_buffer_lock.pick_chunk(lines, filename, false, true)?;
    }

    Ok(())
}
