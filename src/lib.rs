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
pub mod filetype;
pub mod fim;
pub mod instruction;
pub mod keymap;
pub mod plugin_state;
pub mod ring_buffer;
pub mod utils;

use {
    fim::{fim_try_hint, FimAcceptType},
    nvim_oxi::{
        api::{
            del_autocmd, {self, Buffer, Window},
        },
        Dictionary, Function, Result as NvimResult,
    },
    plugin_state::{get_state, init_state, PluginState},
    std::{
        convert::TryInto,
        sync::{atomic::Ordering, Arc},
        time::{Duration, Instant},
    },
    tokio::{runtime::Runtime, sync::mpsc},
    utils::{get_buf_lines, get_buffer_handle, get_pos, in_insert_mode},
};

// FIM completion channel types for async communication between worker and main thread
/// Message sent from async worker to main thread when completion is ready
#[derive(Debug, Clone)]
pub struct FimCompletionMessage {
    buffer_handle: u64,        // Buffer handle to ensure we're still in same buffer
    buffer_lines: Vec<String>, // All buffer lines captured at start
    cursor_x: usize,           // Cursor position X
    cursor_y: usize,           // Cursor position Y
    content: String,           // FIM response content
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
pub fn lttw() -> NvimResult<Dictionary> {
    let mut functions = Dictionary::new();

    functions.insert::<&str, Function<(), ()>>("lttw_setup", Function::from(|_| lttw_setup()));

    Ok(functions)
}

fn lttw_setup() -> NvimResult<()> {
    // Initialize plugin state
    init_state();

    // Initialize persistent tokio runtime and completion channel
    init_tokio_runtime();

    // Setup timer-based ring buffer updates (every ring_update_ms)
    ring_buffer::setup_ring_buffer_timer()?;

    // Register nvim-oxi commands
    commands::register_commands()?;

    // Setup keymaps
    keymap::setup_keymaps()?;

    // Setup autocmds
    autocommands::setup_filetype_autocmd()?;
    autocommands::setup_non_filetype_autocmds()?;

    Ok(())
}

// ---------------------------

/// Check if FIM hint is shown - internal helper for commands
fn fim_is_hint_shown() -> Result<bool, nvim_oxi::Error> {
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
}

impl FimState {
    fn update(
        &mut self,
        hint_shown: bool,
        pos_x: usize,
        pos_y: usize,
        line_cur: String,
        can_accept: bool,
        content: Vec<String>,
    ) {
        self.hint_shown = hint_shown;
        self.pos_x = pos_x;
        self.pos_y = pos_y;
        self.line_cur = line_cur;
        self.can_accept = can_accept;
        self.content.clear();
        self.content = content;
    }

    fn clear(&mut self) {
        self.hint_shown = false;
        self.pos_x = 0;
        self.pos_y = 0;
        self.line_cur.clear();
        self.can_accept = false;
        self.content.clear();
        self.last_pick_pos_y = None;
    }

    // NOTE used for ring buffer logic
    ///// Update the last pick position
    //fn set_last_pick_pos_y(&mut self, pos_y: usize) {
    //    self.last_pick_pos_y = Some(pos_y);
    //}
    ///// Get the last pick position
    //fn get_last_pick_pos_y(&self) -> Option<usize> {
    //    self.last_pick_pos_y
    //}
}

/// Initialize persistent tokio runtime and completion channel
fn init_tokio_runtime() {
    let state = get_state();

    // Create a multi-threaded tokio runtime
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4) // TODO parameterize this
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            state.debug_manager.read().log(
                "init_tokio_runtime",
                format!("Failed to create tokio runtime: {}", e),
            );
            return;
        }
    };

    // Create channel for completion messages
    let (tx, rx) = mpsc::channel::<FimCompletionMessage>(16);
    *state.fim_completion_tx.write() = Some(tx);

    completion_processing_thread(&state, rx, &runtime);

    *state.tokio_runtime.write() = Some(runtime);
}

fn completion_processing_thread(
    state: &PluginState,
    mut rx: mpsc::Receiver<FimCompletionMessage>,
    rt: &Runtime,
) {
    // Spawn a task that receives completion messages and adds them to the pending display queue
    // This runs on its own dedicated current-thread runtime separate from the main multi-threaded one
    // TODO use a tokio thread?
    let state = state.clone();
    rt.spawn(async move {
        while let Some(msg) = rx.recv().await {
            // Push to pending display queue (this is thread-safe)
            state
                .debug_manager
                .read()
                //.log("pending_queue msg", &[&format!("msg {msg:?}")]);
                .log("pending_queue msg", "");
            state.pending_display.write().push(msg);
            // Release lock automatically when pending_queue goes out of scope
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

/// Implementation of FIM worker with optional debounce sequence tracking
async fn spawn_fim_completion_worker(
    state: Arc<PluginState>,
    cursor_x: usize,
    cursor_y: usize,
    buffer_handle: u64,
    buffer_lines: Vec<String>,
    prev: Option<&[String]>, // speculative FIM content
) -> Result<(), nvim_oxi::Error> {
    let seq = state.increment_debounce_sequence();

    // Check debounce if we have a sequence
    let debounce_ms = {
        let config = state.config.read();
        config.debounce_ms
    };

    // This is the most recent request, check if debounce has elapsed
    let now = Instant::now();
    let last_spawn = *state.fim_worker_debounce_last_spawn.read();
    let elapsed = now.duration_since(last_spawn);
    let debounce_expired = elapsed >= Duration::from_millis(debounce_ms as u64);

    if !debounce_expired {
        // Still within debounce period. Since this is the most recent request,
        // we should wait until debounce expires and then spawn.
        let remaining_ms = debounce_ms as u64 - elapsed.as_millis() as u64;
        state.debug_manager.read().log(
            "spawn_fim_completion_worker",
            format!("Within debounce period, (seq {seq}, remaining {remaining_ms}ms)",),
        );

        // Wait for remaining debounce time
        tokio::time::sleep(Duration::from_millis(remaining_ms)).await;

        // Re-check if we're still the most recent request
        let latest_sequence = *state.fim_worker_debounce_seq.read();

        if seq < latest_sequence {
            // A newer request has come in, discard this one
            state.debug_manager.read().log(
                "spawn_fim_completion_worker",
                format!(
                    "Discarding stale worker after wait (seq {seq} < latest {latest_sequence})",
                ),
            );
            return Ok(());
        }
    }
    state.record_worker_spawn();

    state.debug_manager.read().log(
        "spawn_fim_completion_worker",
        format!("Spawning worker for ({}, {})", cursor_x, cursor_y),
    );

    //// Collect all neovim information at the start
    //let fim_state = state.fim_state.clone();

    //// Spawn async task to perform HTTP request
    //// Check if we should trigger speculative FIM
    //let speculative_fim = {
    //    let fim_state_lock = fim_state.read();
    //    fim_state_lock.hint_shown && !fim_state_lock.content.is_empty()
    //};

    //let prev_content = if speculative_fim {
    //    let fim_state_lock = fim_state.read();
    //    // Trigger Speculative FIM
    //    Some(&*fim_state_lock.content.clone())
    //} else {
    //    None
    //};

    // TODO handle error
    let _ = fim::fim_completion(state, cursor_x, cursor_y, buffer_handle, buffer_lines, prev);

    Ok(())
}

/// Process pending FIM display queue - drains and displays messages on the main thread
fn process_pending_display() -> NvimResult<()> {
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
    //// if either the hint isn't shown OR it's only whitespace then trigger another fim
    //if !state.fim_state.read().hint_shown || !state.fim_state.read().can_accept {
    //    fim_try_hint();
    //}

    Ok(())
}

/// Handle FIM completion message received from async worker
fn handle_fim_completion_message(msg: FimCompletionMessage) -> NvimResult<()> {
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
    )
}

/// FIM accept function - accepts the FIM suggestion
fn fim_accept(accept_type: FimAcceptType) -> NvimResult<Option<String>> {
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
    state
        .debug_manager
        .read()
        .log("fim_hide", "Hiding FIM hint");

    // Clear virtual text using nvim_buf_clear_namespace()
    if let Some(ns_id_val) = state.extmark_ns {
        let mut buf = Buffer::current();
        let _ = buf.clear_namespace(ns_id_val, ..);
    }

    state.fim_state.write().clear();
}

/// Debug toggle function - toggles logging
fn debug_toggle() -> NvimResult<bool> {
    let state = get_state();
    let enabled = state.debug_manager.read().is_enabled();

    // Toggle logging
    let mut debug_manager_lock = state.debug_manager.write();
    debug_manager_lock.set_enabled(!enabled);

    Ok(!enabled)
}

/// Debug clear function
fn debug_clear() -> NvimResult<()> {
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
fn enable_plugin() -> NvimResult<()> {
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
fn disable_plugin() -> NvimResult<()> {
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
fn toggle_auto_fim() -> NvimResult<bool> {
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

fn on_move() -> NvimResult<()> {
    let state = get_state();
    *state.last_move_time.write() = Instant::now();
    state.debug_manager.read().log("on_move", "Cursor moved");
    fim_hide();
    fim_try_hint()?;
    Ok(())
}

/// Handle TextYankPost event - gather chunks from yanked text
fn on_text_yank_post() -> NvimResult<()> {
    let state = get_state();

    // Get yanked text using vim.fn.getreg()
    // NOTE " is the default register for yanked text
    let reg_content: String =
        api::call_function("getreg", ("\"",)).unwrap_or_else(|_| String::new());

    // Split by newlines to get individual lines
    let yanked: Vec<String> = reg_content.split('\n').map(|s| s.to_string()).collect();

    if !yanked.is_empty() {
        let filename = Buffer::current()
            .get_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        state.debug_manager.read().log(
            "on_text_yank_post",
            format!("Yanked {} lines from {}", yanked.len(), filename),
        );

        // Pick chunk from yanked text
        let mut ring_buffer_lock = state.ring_buffer.write();
        ring_buffer_lock.pick_chunk(yanked, false, true);

        // Set filename for the last queued chunk
        if let Some(chunk) = ring_buffer_lock.queued.last_mut() {
            chunk.filename = filename;
        }
    }

    Ok(())
}

/// Handle BufEnter event - gather chunks from entered buffer
fn on_buf_enter_gather_chunks() -> NvimResult<()> {
    let state = get_state();

    let buf = Buffer::current();
    let all_lines = buf.get_lines(.., false);
    let lines: Vec<String> = match all_lines {
        Ok(iter) => iter.map(|s| s.to_string()).collect(),
        Err(_) => Vec::new(),
    };

    if lines.len() > 3 {
        let filename = buf
            .get_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        state.debug_manager.read().log(
            "on_buf_enter",
            format!("Entered buffer with {} lines: {}", lines.len(), filename),
        );

        // Pick chunk from buffer
        let mut ring_buffer_lock = state.ring_buffer.write();
        ring_buffer_lock.pick_chunk(lines, false, true);

        // Set filename for the last queued chunk
        if let Some(chunk) = ring_buffer_lock.queued.last_mut() {
            chunk.filename = filename;
        }
    }

    Ok(())
}

/// Handle BufLeave event - gather chunks from buffer before leaving
fn on_buf_leave() -> NvimResult<()> {
    let state = get_state();

    let buf = Buffer::current();
    let all_lines = buf.get_lines(.., false);
    let lines: Vec<String> = match all_lines {
        Ok(iter) => iter.map(|s| s.to_string()).collect(),
        Err(_) => Vec::new(),
    };

    if lines.len() > 3 {
        let filename = buf
            .get_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        state.debug_manager.read().log(
            "on_buf_leave",
            format!("Leaving buffer with {} lines: {}", lines.len(), filename),
        );

        // Pick chunk from buffer
        let mut ring_buffer_lock = state.ring_buffer.write();
        ring_buffer_lock.pick_chunk(lines, false, true);

        // Set filename for the last queued chunk
        if let Some(chunk) = ring_buffer_lock.queued.last_mut() {
            chunk.filename = filename;
        }
    }

    Ok(())
}

/// Trigger speculative FIM completion using async worker
fn trigger_fim() -> NvimResult<()> {
    let state = get_state();
    state.debug_manager.read().log(
        "trigger_fim",
        format!(
            "state.enabled {}, state.config.auto_fim {}",
            state.enabled.load(Ordering::SeqCst),
            state.config.read().auto_fim
        ),
    );

    // Check if FIM is enabled and auto_fim is true
    if !state.enabled.load(Ordering::SeqCst) || !state.config.read().auto_fim {
        return Ok(());
    }

    // Get CURRENT cursor position
    let (pos_x, pos_y) = get_pos();
    let lines = get_buf_lines();
    let buffer_handle: u64 = get_buffer_handle();
    let state_ = state.clone(); // Clone for async block

    // Get the current sequence number to track this request
    let tokio_runtime_lock = state.tokio_runtime.read();
    if let Some(runtime) = tokio_runtime_lock.as_ref() {
        runtime.spawn(async move {
            // TODO log error
            let _ =
                spawn_fim_completion_worker(state_, pos_x, pos_y, buffer_handle, lines, None).await;
        });
    } else {
        state.debug_manager.read().log(
            "trigger_fim",
            "Tokio runtime not initialized, falling back to blocking",
        );
    }
    Ok(())
}

/// Handle BufWritePost event - gather chunks after saving buffer
fn on_buf_write_post() -> NvimResult<()> {
    let state = get_state();

    let buf = Buffer::current();
    let all_lines = buf.get_lines(.., false);
    let lines: Vec<String> = match all_lines {
        Ok(iter) => iter.map(|s| s.to_string()).collect(),
        Err(_) => Vec::new(),
    };

    if lines.len() > 3 {
        let filename = buf
            .get_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        state.debug_manager.read().log(
            "on_buf_write_post",
            format!("Buffer saved with {} lines: {filename}", lines.len()),
        );

        // Pick chunk from buffer
        let mut ring_buffer_lock = state.ring_buffer.write();
        ring_buffer_lock.pick_chunk(lines, false, true);

        // Set filename for the last queued chunk
        if let Some(chunk) = ring_buffer_lock.queued.last_mut() {
            chunk.filename = filename;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = config::LttwConfig::new();
        assert_eq!(config.endpoint_fim, "http://127.0.0.1:8012/infill");
        assert_eq!(config.n_prefix, 256);
    }

    #[test]
    fn test_filetype_check() {
        let mut config = config::LttwConfig::new();
        assert!(config.is_filetype_enabled("rust"));

        config.disabled_filetypes.push("rust".to_string());
        assert!(!config.is_filetype_enabled("rust"));
        assert!(config.is_filetype_enabled("python"));
    }
}
