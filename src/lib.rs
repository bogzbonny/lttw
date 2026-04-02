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
            del_autocmd,
            opts::SetExtmarkOptsBuilder,
            types::ExtmarkVirtTextPosition,
            {self, Buffer, Window},
        },
        Dictionary, Function, Result as NvimResult,
    },
    plugin_state::{get_state, init_state, PluginState},
    std::{
        convert::TryInto,
        sync::{atomic::Ordering, Arc},
        time::{Duration, Instant},
    },
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

    // Register nvim-oxi commands
    commands::register_commands()?;

    // Initialize persistent tokio runtime and completion channel
    init_tokio_runtime();

    // Setup keymaps
    keymap::setup_keymaps()?;

    // Setup filetype
    autocommands::setup_filetype_autocmd()?;

    // Setup timer-based ring buffer updates (every ring_update_ms)
    ring_buffer::setup_ring_buffer_timer()?;

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

/// State for FIM worker debouncing
#[derive(Debug, Clone)]
pub struct FimWorkerDebounce {
    /// Timestamp of the last worker spawn attempt
    last_spawn_ms: Instant,
    /// Sequence number for tracking most recent request
    next_sequence: u64,
}

impl FimWorkerDebounce {
    fn new() -> Self {
        Self {
            last_spawn_ms: Instant::now(),
            next_sequence: 0,
        }
    }
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
                &[&format!("Failed to create tokio runtime: {}", e)],
            );
            return;
        }
    };

    // Create channel for completion messages
    let (tx, rx) = tokio::sync::mpsc::channel::<FimCompletionMessage>(16);
    *state.fim_completion_tx.write() = Some(tx);

    // Spawn a task that receives completion messages and adds them to the pending display queue
    // This runs on its own dedicated current-thread runtime separate from the main multi-threaded one
    // TODO use a tokio thread?
    let state_for_receiver = state.clone();
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Failed to create queue receiver runtime: {}", e);
                return;
            }
        };

        rt.block_on(async move {
            let mut rx = rx; // make mutable
            while let Some(msg) = rx.recv().await {
                // Push to pending display queue (this is thread-safe)
                state_for_receiver
                    .debug_manager
                    .read()
                    //.log("pending_queue msg", &[&format!("msg {msg:?}")]);
                    .log("pending_queue msg", &[]);
                let mut pending_queue = state_for_receiver.pending_display.write();
                pending_queue.push(msg);
                // Release lock automatically when pending_queue goes out of scope
            }
        });
    });

    // Set up a Neovim timer to periodically process the pending display queue
    // This ensures display updates happen on the main thread

    // NOTE I tested this with a tokio thread and it didn't work
    let _ = nvim_oxi::libuv::TimerHandle::start(
        Duration::from_millis(500),
        Duration::from_millis(50), // repeat
        |_| {
            // Need this so that it executes on the main thread (or else extmarks won't display)
            nvim_oxi::schedule(|_| process_pending_display());
        },
    );

    *state.tokio_runtime.write() = Some(runtime);
}

/// Handle FIM completion message received from async worker
fn handle_fim_completion_message(msg: FimCompletionMessage) -> NvimResult<()> {
    let state = get_state();

    // Check if we're still in the same buffer
    let current_buf: u64 = Buffer::current().handle().try_into().unwrap_or(0);
    if current_buf != msg.buffer_handle {
        state.debug_manager.read().log(
            "handle_fim_completion_message",
            &[&format!(
                "Buffer changed, ignoring completion (expected {}, got {})",
                msg.buffer_handle, current_buf
            )],
        );
        return Ok(());
    }

    state.debug_manager.read().log(
        "handle_fim_completion_message",
        &[&format!(
            "Received completion for buffer {} at ({}, {})",
            msg.buffer_handle, msg.cursor_x, msg.cursor_y
        )],
    );

    // Parse response and render
    let ctx = context::get_local_context(
        &msg.buffer_lines,
        msg.cursor_x,
        msg.cursor_y,
        None,
        &state.config.read(),
    );
    let rendered = fim::render_fim_suggestion(
        msg.cursor_x,
        msg.cursor_y,
        &msg.content,
        &ctx.line_cur_suffix,
        &state.config.read(),
    );

    // Get line count before moving content
    let content_len = rendered.content.len();

    // Update FIM state
    state.fim_state.write().update(
        rendered.can_accept,
        msg.cursor_x,
        msg.cursor_y,
        msg.buffer_lines
            .get(msg.cursor_y)
            .cloned()
            .unwrap_or_default(),
        rendered.can_accept,
        rendered.content,
    );

    // Display virtual text using extmarks
    display_fim_text(&state)?;

    state.debug_manager.read().log(
        "handle_fim_completion_message",
        &[&format!("Displaying FIM hint: {} lines", content_len)],
    );

    Ok(())
}

/// Implementation of FIM worker with optional debounce sequence tracking
async fn spawn_fim_worker(
    state: Arc<PluginState>,
    buffer_handle: u64,
    buffer_lines: Vec<String>,
    cursor_x: usize,
    cursor_y: usize,
    sequence: u64,
) -> Result<(), nvim_oxi::Error> {
    // Check debounce if we have a sequence
    let debounce_ms = {
        let config = state.config.read();
        config.debounce_ms
    };

    // This is the most recent request, check if debounce has elapsed
    let now = Instant::now();
    let last_spawn = state.fim_worker_debounce.read().last_spawn_ms;
    let elapsed = now.duration_since(last_spawn);
    let debounce_expired = elapsed >= Duration::from_millis(debounce_ms as u64);

    if !debounce_expired {
        // Still within debounce period. Since this is the most recent request,
        // we should wait until debounce expires and then spawn.
        let remaining_ms = debounce_ms as u64 - elapsed.as_millis() as u64;
        state.debug_manager.read().log(
            "spawn_fim_worker",
            &[&format!(
                "Within debounce period, (seq {sequence}, remaining {remaining_ms}ms)",
            )],
        );

        // Wait for remaining debounce time
        tokio::time::sleep(Duration::from_millis(remaining_ms)).await;

        // Re-check if we're still the most recent request
        let latest_sequence = {
            let debounce_lock = state.fim_worker_debounce.read();
            debounce_lock.next_sequence - 1
        };

        if sequence < latest_sequence {
            // A newer request has come in, discard this one
            state.debug_manager.read().log(
                "spawn_fim_worker",
                &[&format!(
                    "Discarding stale worker after wait (seq {sequence} < latest {latest_sequence})",
                )],
            );
            return Ok(());
        }
    }
    record_worker_spawn(&state);

    state.debug_manager.read().log(
        "spawn_fim_worker",
        &[&format!("Spawning worker for ({}, {})", cursor_x, cursor_y)],
    );

    // Collect all neovim information at the start
    let fim_state = state.fim_state.clone();

    // Spawn async task to perform HTTP request
    // Check if we should trigger speculative FIM
    let speculative_fim = {
        let fim_state_lock = fim_state.read();
        fim_state_lock.hint_shown && !fim_state_lock.content.is_empty()
    };

    let prev_content = if speculative_fim {
        let fim_state_lock = fim_state.read();
        // Trigger Speculative FIM
        Some(&*fim_state_lock.content.clone())
    } else {
        None
    };

    // TODO handle error
    let _ = fim::fim_completion(
        state,
        cursor_x,
        cursor_y,
        buffer_handle,
        buffer_lines,
        prev_content,
    );

    Ok(())
}

/// Process pending FIM display queue - drains and displays messages on the main thread
fn process_pending_display() -> NvimResult<()> {
    let state = get_state();

    // Only display if we are in insert mode
    if !in_insert_mode()? {
        fim_hide()?; // failsafe if somehow a hint weezled its way in there
        return Ok(());
    }

    // Take all pending messages (clear the queue)
    let messages: Vec<FimCompletionMessage> = {
        let mut pending_queue = state.pending_display.write();
        std::mem::take(&mut *pending_queue)
    };

    if !messages.is_empty() {
        state.debug_manager.read().log(
            "process_pending_display",
            &[&format!(
                "Processing {} pending display messages",
                messages.len()
            )],
        );
    }

    // Process each message
    for msg in messages {
        handle_fim_completion_message(msg)?;
    }

    Ok(())
}

/// FIM accept function - accepts the FIM suggestion
fn fim_accept(accept_type: FimAcceptType) -> NvimResult<Option<String>> {
    // Log before releasing the lock
    let state = get_state();
    {
        let debug_manager = state.debug_manager.read().clone();
        debug_manager.log("fim_accept_triggered", &[]);
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

    // Log before releasing the lock
    {
        let debug_manager = state.debug_manager.read().clone();
        debug_manager.log(
            "fim_accept",
            &[&format!("Accepting {} suggestion", accept_type)],
        );
    }

    // Use the accept_fim_suggestion function from fim module
    let (new_line, rest, inline_loc) =
        fim::accept_fim_suggestion(accept_type, pos_x, &line_cur, &content);

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
fn fim_hide() -> NvimResult<()> {
    let state = get_state();

    let (hint_shown, pos_x, pos_y, debug_manager, ns_id) = {
        let fim_state_lock = state.fim_state.read();
        (
            fim_state_lock.hint_shown,
            fim_state_lock.pos_x,
            fim_state_lock.pos_y,
            state.debug_manager.read().clone(),
            state.extmark_ns,
        )
    };

    if hint_shown {
        debug_manager.log(
            "fim_hide",
            &[&format!("Hiding FIM hint at ({}, {})", pos_x, pos_y)],
        );

        // Clear virtual text using nvim_buf_clear_namespace()
        if let Some(ns_id_val) = ns_id {
            let mut buf = Buffer::current();
            let _ = buf.clear_namespace(ns_id_val, ..);
        }

        // Clear the FIM hint
        {
            let mut fim_state_lock = state.fim_state.write();
            fim_state_lock.hint_shown = false;
            fim_state_lock.content.clear();
        }
    }

    Ok(())
}

/// Display FIM hint as virtual text using extmarks with optional inline info
fn display_fim_text(state: &Arc<PluginState>) -> NvimResult<()> {
    // Lock the fim_state and config to get the data we need
    let (hint_shown, content, extmark_ns, pos_y, pos_x, line_cur, _config, debug_manager) = {
        let fs = state.fim_state.read();
        let config = state.config.read().clone();
        let debug_manager = state.debug_manager.read().clone();
        (
            fs.hint_shown,
            fs.content.clone(),
            state.extmark_ns,
            fs.pos_y,
            fs.pos_x,
            fs.line_cur.clone(),
            config,
            debug_manager,
        )
    };

    if !hint_shown || content.is_empty() {
        return Ok(());
    }

    // Clear any existing extmarks in the namespace before setting new ones
    if let Some(ns_id) = extmark_ns {
        let mut buf = Buffer::current();
        let _ = buf.clear_namespace(ns_id, ..);
    }

    // Only display if we are in insert mode
    if !in_insert_mode()? {
        return Ok(());
    }

    if let Some(ns_id) = extmark_ns {
        let mut buf = Buffer::current();

        // Build virtual text string - first line of suggestion
        let suggestion_text = content[0].clone();
        let (suggestion_text, use_inline) =
            fim::trim_suggestion_curr_line(&suggestion_text, pos_x, &line_cur);

        // Build inline info string if show_info is enabled (mode 2 = inline)
        let virt_text_vec: Vec<(String, String)> =
            { vec![(suggestion_text.to_string(), "Comment".to_string())] };

        // Create extmark opts with virtual text using builder pattern
        let mut opts = SetExtmarkOptsBuilder::default();
        opts.virt_text(virt_text_vec);

        let mut text_pos = ExtmarkVirtTextPosition::Overlay;
        if content.len() == 1 && use_inline {
            text_pos = ExtmarkVirtTextPosition::Inline;
        }

        opts.virt_text_pos(text_pos);

        // Add multi-line support - display rest of suggestion lines below
        if content.len() > 1 {
            let mut virt_lines: Vec<Vec<(String, String)>> = Vec::new();

            // Add remaining content lines
            for line in &content[1..] {
                virt_lines.push(vec![(line.clone(), "Comment".to_string())]);
            }

            opts.virt_lines(virt_lines);
        }

        if !in_insert_mode()? {
            return Ok(());
        }

        // Set the extmark at cursor position
        match buf.set_extmark(ns_id, pos_y, pos_x + 1, &opts.build()) {
            Ok(_id) => {
                debug_manager.log(
                    "display_fim_text",
                    &[&format!("Set extmark at line {}, col {}", pos_y, pos_x + 1)],
                );
            }
            Err(e) => {
                debug_manager.log(
                    "display_fim_text",
                    &[&format!("Error setting extmark: {:?}", e)],
                );
            }
        }
    }

    Ok(())
}

/// Debug log function
fn debug_log(msg: &str, details: Vec<&str>) -> NvimResult<()> {
    let state = get_state();
    state.debug_manager.read().log(msg, &details);
    Ok(())
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
            &[&format!("Plugin not enabled for filetype: {}", filetype)],
        );
        return Ok(());
    }

    state
        .debug_manager
        .read()
        .log("enable_plugin", &["Enabling plugin"]);

    // Setup keymaps
    keymap::setup_keymaps()?;

    // Setup autocmds
    autocommands::setup_non_filetype_autocmds()?;

    // Hide any existing FIM hints
    fim_hide()?;

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
        .log("disable_plugin", &["Disabling plugin"]);

    // Hide FIM hints
    fim_hide()?;

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
    fim_hide()?;
    fim_try_hint()?;
    Ok(())
}

/// Handle TextYankPost event - gather chunks from yanked text
fn on_text_yank_post() -> NvimResult<()> {
    let state = get_state();

    // Get yanked text using vim.fn.getreg() which returns a string
    // Split by newlines to get individual lines
    let reg_content: String =
        api::call_function("getreg", ("\"",)).unwrap_or_else(|_| String::new());
    let yanked: Vec<String> = reg_content.split('\n').map(|s| s.to_string()).collect();

    if !yanked.is_empty() {
        let filename = Buffer::current()
            .get_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        state.debug_manager.read().log(
            "on_text_yank_post",
            &[&format!("Yanked {} lines from {}", yanked.len(), filename)],
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
            &[&format!(
                "Entered buffer with {} lines: {}",
                lines.len(),
                filename
            )],
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
            &[&format!(
                "Leaving buffer with {} lines: {}",
                lines.len(),
                filename
            )],
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

/// Increment the debounce sequence and return the current sequence number
fn increment_debounce_sequence(state: &PluginState) -> u64 {
    let mut debounce_lock = state.fim_worker_debounce.write();
    let seq = debounce_lock.next_sequence;
    debounce_lock.next_sequence += 1;
    seq
}

/// Record that a worker was spawned (update last_spawn timestamp)
fn record_worker_spawn(state: &PluginState) {
    let mut debounce_lock = state.fim_worker_debounce.write();
    debounce_lock.last_spawn_ms = Instant::now();
}

/// Trigger speculative FIM completion using async worker
fn trigger_fim() -> NvimResult<()> {
    let state = get_state();
    state.debug_manager.read().log(
        "trigger_fim",
        &[&format!(
            "state.enabled {}, state.config.auto_fim {}",
            state.enabled.load(Ordering::SeqCst),
            state.config.read().auto_fim
        )],
    );

    // Check if FIM is enabled and auto_fim is true
    if !state.enabled.load(Ordering::SeqCst) || !state.config.read().auto_fim {
        return Ok(());
    }
    state.debug_manager.read().log("hey!", &[]);

    // Get CURRENT cursor position
    let (pos_x, pos_y) = get_pos();
    let lines = get_buf_lines();
    let buffer_handle: u64 = get_buffer_handle();

    state.debug_manager.read().log(
        "trigger_fim",
        &[&format!(
            "Cursor moved in insert mode at ({}, {})",
            pos_x, pos_y
        )],
    );

    state.debug_manager.read().log("trigger_fim 1", &[]);

    // Try to show a cached hint (synchronous - fast)
    let hashes = fim::compute_hashes(&{
        let config_lock = state.config.read();
        context::get_local_context(&lines, pos_x, pos_y, None, &config_lock)
    });
    state.debug_manager.read().log("trigger_fim 2", &[]);

    // Check cache for primary hash
    for hash in &hashes {
        state
            .debug_manager
            .read()
            .log("trigger_fim hashes 3", &[&hash.to_string()]);
        if let Some(response) = {
            let cache_lock = state.cache.read();
            cache_lock.get_fim(hash)
        } {
            state.debug_manager.read().log(
                "trigger_fim",
                &[&format!("Found cached completion for hash {}", &hash[..16])],
            );

            // Parse response and render (synchronous)
            let content = response.content;
            let ctx = {
                let config_lock = state.config.read();
                context::get_local_context(&lines, pos_x, pos_y, None, &config_lock)
            };
            let rendered = {
                let config_lock = state.config.read();
                fim::render_fim_suggestion(
                    pos_x,
                    pos_y,
                    &content,
                    &ctx.line_cur_suffix,
                    &config_lock,
                )
            };

            // Update FIM state
            state.fim_state.write().update(
                rendered.can_accept,
                pos_x,
                pos_y,
                lines.get(pos_y).cloned().unwrap_or_default(),
                rendered.can_accept,
                rendered.content.clone(),
            );

            // Display virtual text using extmarks
            if rendered.can_accept {
                let _ = display_fim_text(&state);

                state.debug_manager.read().log(
                    "trigger_fim",
                    &[&format!(
                        "Showing FIM from cursor move: {} lines",
                        rendered.content.len()
                    )],
                );
            }
            break;
        }
    }
    state.debug_manager.read().log("trigger_fim 4", &[]);

    //let _ = fim_hide();

    // Only trigger FIM if we're in a reasonable position
    if pos_y < lines.len() && pos_x <= lines.get(pos_y).map(|l| l.len()).unwrap_or(0) {
        state.debug_manager.read().log("trigger_fim 4.21", &[]);

        // Get the current sequence number to track this request
        let seq = increment_debounce_sequence(&state);
        let tokio_runtime_lock = state.tokio_runtime.read();
        // Clone lines for the async worker since we need lines for ring buffer logic above
        let state_ = state.clone();
        let lines_clone = lines.clone();
        if let Some(runtime) = tokio_runtime_lock.as_ref() {
            runtime.spawn(async move {
                // TODO log error
                let _ =
                    spawn_fim_worker(state_, buffer_handle, lines_clone, pos_x, pos_y, seq).await;
            });
        } else {
            state.debug_manager.read().log(
                "trigger_fim",
                &["Tokio runtime not initialized, falling back to blocking"],
            );
        }
    }

    //// Ring buffer pick logic - gather extra context when cursor moves significantly
    //// This mirrors the logic in llama#fim (llama.vim lines 930-946)
    //let last_pick_pos_y = state.fim_state.read().get_last_pick_pos_y();
    //let delta_y = last_pick_pos_y
    //    .map(|last_pos| {
    //        pos_y
    //            .saturating_sub(last_pos)
    //            .max(last_pos.saturating_sub(pos_y))
    //    })
    //    .unwrap_or(33); // If no last position, treat as large delta to gather initial chunks

    //// Only gather chunks if cursor has moved more than 32 lines
    //let ring_buffer_pick_needed = delta_y > 32;

    //if ring_buffer_pick_needed {
    //    let max_y = lines.len().saturating_sub(1); // line('$') - 1 (0-indexed)

    //    // Get ring configuration
    //    let config_lock = state.config.read();
    //    let ring_scope = config_lock.ring_scope as usize;
    //    let n_prefix = config_lock.n_prefix as usize;
    //    let n_suffix = config_lock.n_suffix as usize;
    //    let ring_chunk_size = config_lock.ring_chunk_size as usize;

    //    // Expand the prefix even further
    //    // Vim: getline(max([1, l:pos_y - g:llama_config.ring_scope]), max([1, l:pos_y - g:llama_config.n_prefix]))
    //    // In Rust with 0-indexed lines:
    //    let prefix_start = (pos_y.saturating_sub(ring_scope)).max(0);
    //    let prefix_end = (pos_y.saturating_sub(n_prefix)).max(0);

    //    let prefix_lines =
    //        if prefix_start <= max_y && prefix_end <= max_y && prefix_start <= prefix_end {
    //            lines
    //                .get(prefix_start..=prefix_end)
    //                .map(|slice| slice.to_vec())
    //                .unwrap_or_default()
    //        } else {
    //            Vec::new()
    //        };

    //    // Log prefix chunk info before moving
    //    if !prefix_lines.is_empty() {
    //        let mut ring_buffer_lock = state.ring_buffer.write();
    //        ring_buffer_lock.pick_chunk(prefix_lines, false, false);
    //    }

    //    // Pick a suffix chunk
    //    // Vim: getline(min([l:max_y, l:pos_y + g:llama_config.n_suffix]), min([l:max_y, l:pos_y + g:llama_config.n_suffix + g:llama_config.ring_chunk_size]))
    //    // In Rust with 0-indexed lines:
    //    let suffix_start = pos_y.saturating_add(n_suffix).min(max_y);
    //    let suffix_end = (pos_y
    //        .saturating_add(n_suffix)
    //        .saturating_add(ring_chunk_size))
    //    .min(max_y);

    //    let suffix_lines =
    //        if suffix_start <= max_y && suffix_end <= max_y && suffix_start <= suffix_end {
    //            lines
    //                .get(suffix_start..=suffix_end)
    //                .map(|slice| slice.to_vec())
    //                .unwrap_or_default()
    //        } else {
    //            Vec::new()
    //        };

    //    // Log suffix chunk info before moving
    //    if !suffix_lines.is_empty() {
    //        let mut ring_buffer_lock = state.ring_buffer.write();
    //        ring_buffer_lock.pick_chunk(suffix_lines, false, false);
    //    }

    //    // Update the last pick position
    //    state.fim_state.write().set_last_pick_pos_y(pos_y);
    //}

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
            &[&format!(
                "Buffer saved with {} lines: {}",
                lines.len(),
                filename
            )],
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
