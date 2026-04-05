// src/lib.rs - Library interface for lttw Neovim plugin
//
// This module provides the entry point for the Neovim plugin using nvim-oxi.
// All core logic is implemented in Rust modules and exposed to Neovim via FFI.
pub mod autocmd;
pub mod cache;
pub mod commands;
pub mod config;
pub mod context;
pub mod debug;
pub mod diff_chunk;
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
    context::LocalContext,
    diff_chunk::{calculate_diff_between_contents, evaluate_diff_changes, log_diff_operations},
    fim::{
        fim_try_hint, fim_try_hint_skip_debounce, render_fim_suggestion, FimAcceptType, FimTimings,
    },
    nvim_oxi::{Dictionary, Function},
    plugin_state::{get_state, init_state},
    std::{
        sync::atomic::Ordering,
        time::{Duration, Instant},
    },
    tokio::sync::mpsc,
    utils::{
        clear_buf_namespace_objects, del_autocmd, get_buf_filename, get_buf_line, get_buf_lines,
        get_current_buffer_id, get_current_buffer_info, get_current_filetype, get_mode_bz, get_pos,
        get_yanked_text, in_insert_mode, set_buf_lines, set_window_cursor,
    },
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

impl FimTimingsData {
    pub fn new(t: FimTimings, tokens_cached: u64, truncated: bool) -> Self {
        Self {
            n_prompt: t.prompt_n.unwrap_or(0),
            t_prompt_ms: t.prompt_ms.unwrap_or(0.0),
            s_prompt: t.prompt_per_second.unwrap_or(0.0),
            n_predict: t.predicted_n.unwrap_or(0),
            t_predict_ms: t.predicted_ms.unwrap_or(0.0),
            s_predict: t.predicted_per_second.unwrap_or(0.0),
            tokens_cached,
            truncated,
        }
    }
}

/// Message sent from async worker to main thread when completion is ready
#[derive(Debug, Clone)]
pub struct FimCompletionMessage {
    buffer_id: u64,                  // Buffer handle to ensure we're still in same buffer
    ctx: LocalContext,               // All buffer lines captured at start
    cursor_x: usize,                 // Cursor position X
    cursor_y: usize,                 // Cursor position Y
    content: String,                 // FIM response content
    timings: Option<FimTimingsData>, // Timing information from server response
    retry: Option<usize>,            // the retry count for this completion
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

    functions.insert::<&str, Function<nvim_oxi::Object, ()>>("setup", Function::from(lttw_setup));

    Ok(functions)
}

// TODO process errors in this function
fn lttw_setup(c: nvim_oxi::Object) {
    // Initialize plugin state
    init_state(c);

    // Initialize persistent tokio runtime and completion channel
    init_completion_processing_thread();

    // Setup timer-based ring buffer updates (every ring_update_ms)
    let _ = ring_buffer::setup_ring_buffer_timer();

    // Register nvim-oxi commands
    let _ = commands::register_commands();

    // Setup keymaps
    let _ = keymap::setup_keymaps();

    // Setup autocmds
    let _ = autocmd::setup_filetype_autocmd();
    let _ = autocmd::setup_non_filetype_autocmds();
}

// ---------------------------

/// State for FIM (Fill-in-Middle) completion
#[derive(Debug, Clone, Default)]
pub struct FimState {
    hint_shown: bool,
    /// Last buffer id and cursor Y position where ring buffer chunks were picked
    last_pick_buf_id_pos_y: Option<(u64, usize)>,

    pos_x: usize,
    pos_y: usize,
    line_cur: String,
    content: Vec<String>,
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
        content: Vec<String>,
        timings: Option<FimTimingsData>,
    ) {
        self.hint_shown = hint_shown;
        self.pos_x = pos_x;
        self.pos_y = pos_y;
        self.line_cur = line_cur;
        self.content.clear();
        self.content = content;
        self.timings = timings;
    }

    fn clear(&mut self) {
        self.hint_shown = false;
        self.pos_x = 0;
        self.pos_y = 0;
        self.line_cur.clear();
        self.content.clear();
        self.last_pick_buf_id_pos_y = None;
        self.timings = None;
    }

    /// Update the last pick position
    fn set_last_pick_buf_id_pos_y(&mut self, buf_id: u64, pos_y: usize) {
        self.last_pick_buf_id_pos_y = Some((buf_id, pos_y));
    }
    /// Get the last pick position
    fn get_last_pick_buf_id_pos_y(&self) -> Option<(u64, usize)> {
        self.last_pick_buf_id_pos_y
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

/// NOTE this occurs on the neovim thread
/// Process pending FIM display queue - drains and displays messages on the main thread
fn process_pending_display() -> LttwResult<()> {
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
    if messages.is_empty() {
        return Ok(());
    }

    state.debug_manager.read().log(
        "process_pending_display",
        format!("Processing {} pending display messages", messages.len()),
    );
    // process the most recent message which has content and isn't only whitespace
    let mut msg = None;
    for msg_ in messages.into_iter().rev() {
        if msg_is_valid_to_display(&msg_) {
            msg = Some(msg_);
            break;
        }
    }

    let mut retry = 0;
    if let Some(msg) = msg {
        state.debug_manager.read().log(
            "process_pending_display",
            format!("valid message found {msg:?}"),
        );
        render_fim_suggestion(
            state.clone(),
            msg.cursor_x,
            msg.cursor_y,
            &msg.content,
            msg.ctx.line_cur,
            msg.timings,
        )?;
        retry = msg.retry.unwrap_or(0);
    }

    // NOTE there were messages nomatter what at this point in the function (even if none were
    // valid to display)
    //
    // if either the hint isn't shown OR it's only whitespace then trigger another fim
    // only retry a llm call 3 times before giving up
    if !state.fim_state.read().hint_shown && retry <= 3 {
        retry += 1;
        state
            .debug_manager
            .read()
            .log("process_pending_display", "rerendering fim suggestion");
        fim_try_hint(Some(retry))?;
    }

    Ok(())
}

// should we abort the completion because the content has changed since we started this completion
#[allow(dead_code)] // TODO fix or delete (needs to not make neovim calls not on the main thread)
fn msg_is_valid_to_display(msg: &FimCompletionMessage) -> bool {
    if msg.content.is_empty() || msg.content.trim().is_empty() {
        return false;
    }
    let id = get_current_buffer_id();
    if id != msg.buffer_id {
        return false;
    }

    let (x, y) = get_pos();
    if msg.cursor_y != y || msg.cursor_x != x {
        return false;
    };
    let curr_line = get_buf_line(y);
    if curr_line != msg.ctx.line_cur {
        return false;
    }

    true
}

/// FIM accept function - can be used to accept real changes or virtually accept changes in order
/// to run speculative FIM for future rounds.
///
/// Returns new_x_pos, new_y_pos, combined content to write
fn fim_accept_inner(
    accept_type: FimAcceptType,
    pos_x: usize,
    pos_y: usize,
    line_cur: String,
    content: Vec<String>,
) -> LttwResult<(usize, usize, Vec<String>)> {
    // Use the accept_fim_suggestion function from fim module
    let (new_line, rest, inline_loc) =
        fim::accept_fim_suggestion(accept_type, pos_x, &line_cur, &content);

    // Move the cursor to the end of the accepted text
    let (new_x, new_y) = if let Some(rest_lines) = &rest {
        let new_pos_y = pos_y + rest_lines.len();
        let new_pos_x = rest_lines.last().map_or(0, |line| line.len());
        (new_pos_x, new_pos_y)
    } else if let Some(inline) = inline_loc {
        (inline, pos_y)
    } else {
        let new_col = new_line.len();
        (new_col, pos_y)
    };

    let mut combined = vec![new_line];
    if let Some(rest_lines) = rest {
        combined.extend(rest_lines);
    }

    Ok((new_x, new_y, combined))
}

/// FIM accept function - accepts the FIM suggestion
fn fim_accept(accept_type: FimAcceptType) -> LttwResult<()> {
    // Log before releasing the lock
    let state = get_state();
    {
        let debug_manager = state.debug_manager.read().clone();
        debug_manager.log("fim_accept_triggered", "");
    }

    let (hint_shown, pos_x, pos_y, line_cur, content) = {
        let fim_state_lock = state.fim_state.read();
        (
            fim_state_lock.hint_shown,
            fim_state_lock.pos_x,
            fim_state_lock.pos_y,
            fim_state_lock.line_cur.clone(),
            fim_state_lock.content.clone(),
        )
    };

    if !hint_shown {
        return Ok(());
    }

    state.debug_manager.read().log(
        "fim_accept",
        format!("Accepting {} suggestion", accept_type),
    );

    let (new_x, new_y, final_content) =
        fim_accept_inner(accept_type, pos_x, pos_y, line_cur, content)?;

    // replace the one line with all the new content (can be multiple lines)
    set_buf_lines(pos_y..=pos_y, final_content)?;

    // Move the cursor to the end of the accepted text
    set_window_cursor(new_x, new_y)?;

    state.fim_state.write().clear();

    // Clear virtual text from display
    if let Some(ns_id) = state.extmark_ns {
        clear_buf_namespace_objects(ns_id)?
    }

    // immediately start a new FIM request skipping the debounce
    fim_try_hint_skip_debounce()?;

    Ok(())
}

/// FIM hide function - clears the FIM hint from display
fn fim_hide() -> LttwResult<()> {
    let state = get_state();
    //state
    //    .debug_manager
    //    .read()
    //    .log("fim_hide", "Hiding FIM hint");

    // Clear virtual text using nvim_buf_clear_namespace()
    if let Some(ns_id_val) = state.extmark_ns {
        clear_buf_namespace_objects(ns_id_val)?
    }

    state.fim_state.write().clear();
    Ok(())
}

fn debug_enable() -> LttwResult<()> {
    let state = get_state();
    state.debug_manager.write().enabled = true;
    Ok(())
}

fn debug_disable() -> LttwResult<()> {
    let state = get_state();
    state.debug_manager.write().enabled = true;
    Ok(())
}

/// Debug clear function
fn debug_clear() -> LttwResult<()> {
    let state = get_state();
    state.debug_manager.write().clear();
    Ok(())
}

/// Enable info display (set show_info = 2)
fn enable_info() -> LttwResult<()> {
    let state = get_state();
    state.config.write().show_info = 2;
    Ok(())
}

/// Disable info display (set show_info = 0)
fn disable_info() -> LttwResult<()> {
    let state = get_state();
    state.config.write().show_info = 0;
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
    let filetype = get_current_filetype()?;
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
    autocmd::setup_non_filetype_autocmds()?;

    // Hide any existing FIM hints
    fim_hide()?;

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
fn toggle_auto_fim() -> LttwResult<bool> {
    let state = get_state();

    // Toggle auto_fim in config
    let new_value = !state.config.read().auto_fim;
    {
        let mut config_lock = state.config.write();
        config_lock.auto_fim = new_value;
    }

    // Re-setup autocmds with new config
    autocmd::setup_non_filetype_autocmds()?;

    Ok(new_value)
}

fn on_move() -> LttwResult<()> {
    let state = get_state();
    *state.last_move_time.write() = Instant::now();
    state.debug_manager.read().log("on_move", "Cursor moved");
    fim_hide()?;
    fim_try_hint(None)?;
    Ok(())
}

/// Toggle auto_fim configuration
fn set_mode_in_state() -> LttwResult<()> {
    let state = get_state();
    *state.nvim_mode.write() = get_mode_bz()?;
    Ok(())
}

/// Toggle auto_fim configuration
fn set_cur_buffer_info_in_state() -> LttwResult<()> {
    let info = get_current_buffer_info()?;
    get_state().set_cur_buffer_info(info);
    Ok(())
}

/// Handle TextYankPost event - gather chunks from yanked text
fn on_text_yank_post() -> LttwResult<()> {
    let state = get_state();

    // Get yanked text using vim.fn.getreg()
    // NOTE " is the default register for yanked text
    let reg_content = get_yanked_text()?;

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
        ring_buffer_lock.pick_chunk(&state, &yanked, filename, false, true)?;
    }

    Ok(())
}

/// Handle BufEnter event - track file content and gather chunks from entered buffer
fn on_buf_enter_gather_chunks() -> LttwResult<()> {
    let state = get_state();

    let lines = get_buf_lines(..);

    if lines.len() > 3 {
        let filename = get_buf_filename().unwrap_or_default();

        state.debug_manager.read().log(
            "on_buf_enter",
            format!("Entered buffer with {} lines: {}", lines.len(), filename),
        );

        // Pick chunk from buffer
        let mut ring_buffer_lock = state.ring_buffer.write();
        ring_buffer_lock.pick_chunk(&state, &lines, filename.clone(), false, true)?;

        // Track file content for future diff comparison
        let content = lines.join("\n");
        {
            let mut file_contents_lock = state.file_contents.write();
            file_contents_lock.insert(filename.clone(), content);
        }
    }

    Ok(())
}

/// Handle BufLeave event - track file content and gather chunks from buffer before leaving
fn on_buf_leave() -> LttwResult<()> {
    let state = get_state();

    let lines = get_buf_lines(..);

    if lines.len() > 3 {
        let filename = get_buf_filename().unwrap_or_default();

        state.debug_manager.read().log(
            "on_buf_leave",
            format!("Leaving buffer with {} lines: {}", lines.len(), filename),
        );

        // Track file content for future diff comparison
        let content = lines.join("\n");
        {
            let mut file_contents_lock = state.file_contents.write();
            file_contents_lock.insert(filename.clone(), content);
        }

        // Pick chunk from buffer
        let mut ring_buffer_lock = state.ring_buffer.write();
        ring_buffer_lock.pick_chunk(&state, &lines, filename, false, true)?;
    }

    Ok(())
}

/// Handle BufWritePost event - track file content and evaluate diff chunks after saving buffer
fn on_buf_write_post() -> LttwResult<()> {
    let state = get_state();

    let lines = get_buf_lines(..);

    if lines.len() > 3 {
        let filename = get_buf_filename().unwrap_or_default();

        state.debug_manager.read().log(
            "on_buf_write_post",
            format!("Buffer saved with {} lines: {filename}", lines.len()),
        );

        // Pick chunk from buffer
        let mut ring_buffer_lock = state.ring_buffer.write();
        ring_buffer_lock.pick_chunk(&state, &lines, filename.clone(), false, true)?;

        // Convert lines to string for diff comparison
        let new_content = lines.join("\n");

        // Save the current file content for future diff comparison
        {
            let mut file_contents_lock = state.file_contents.write();
            file_contents_lock.insert(filename.clone(), new_content.clone());
        }

        // Get saved content for this file
        let old_content = {
            let file_contents_lock = state.file_contents.read();
            file_contents_lock.get(&filename).cloned()
        };

        // Calculate diff between saved content and current content
        let mut diff_chunks = if let Some(ref old_content) = old_content {
            calculate_diff_between_contents(&filename, old_content, &new_content)?
        } else {
            // No previous content - return empty
            Vec::new()
        };

        // Process diff chunks
        if !diff_chunks.is_empty() {
            // Assign sequential IDs to new chunks and evaluate changes
            let (additions, removals) = {
                let mut old_chunks_lock = state.diff_chunks.write();
                let old_chunks: Vec<diff_chunk::DiffChunk> = old_chunks_lock.clone();

                // Assign sequential IDs to new chunks
                let mut next_id = state.next_diff_chunk_id.load(std::sync::atomic::Ordering::SeqCst);
                for chunk in &mut diff_chunks {
                    // Check if this filepath was in old chunks
                    let was_in_old = old_chunks.iter().any(|c| c.filepath == chunk.filepath);
                    if was_in_old {
                        // Find the old chunk for this filepath and reuse its id if content unchanged
                        let mut found = false;
                        for old_chunk in &old_chunks {
                            if old_chunk.filepath == chunk.filepath && old_chunk.content == chunk.content {
                                // Content unchanged - reuse old id
                                chunk.id = old_chunk.id;
                                found = true;
                                break;
                            }
                        }
                        if !found && chunk.id == 0 {
                            // Content changed - assign new id
                            chunk.id = next_id;
                            next_id += 1;
                        }
                    } else {
                        // New filepath - assign new id
                        chunk.id = next_id;
                        next_id += 1;
                    }
                }
                state.next_diff_chunk_id.store(next_id, std::sync::atomic::Ordering::SeqCst);

                // Evaluate changes
                let (additions, removals) = evaluate_diff_changes(&diff_chunks, &old_chunks);

                // Log operations for debugging
                log_diff_operations(&state.debug_manager.read(), &additions, &removals);

                // Update stored chunks
                *old_chunks_lock = diff_chunks;

                (additions, removals)
            };

            // Apply changes to ring buffer in a separate locked section
            let mut ring_buffer_lock = state.ring_buffer.write();
            
            // Perform removals first (as per requirements)
            for chunk in &removals {
                // Evict chunks by id from both queued and ring
                ring_buffer_lock.evict_by_id(chunk.id);
                state.debug_manager.read().log(
                    "diff_chunk_evicted",
                    format!("Evicted from buffer: {} (id: {})", chunk.filepath, chunk.id),
                );
            }

            // Perform additions (after removals)
            for chunk in &additions {
                let ring_chunk = chunk.to_ring_chunk();
                ring_buffer_lock.queued.push(ring_chunk);
                state.debug_manager.read().log(
                    "diff_chunk_added",
                    format!("Added to queued: {} (id: {})", chunk.filepath, chunk.id),
                );
            }
        }
    } else {
        // Small file - just track content
        let filename = get_buf_filename().unwrap_or_default();
        let new_content = lines.join("\n");
        {
            let mut file_contents_lock = state.file_contents.write();
            file_contents_lock.insert(filename.clone(), new_content);
        }
    }

    Ok(())
}

/// Evaluate all repository diffs and update ring buffer
///
/// This function:
/// 1. Calculates all diff chunks by comparing saved file contents with current buffer contents
/// 2. Compares with previously stored chunks
/// 3. Assigns unique sequential IDs to new/changed chunks
/// 4. Adds new chunks to ringbuffer.queued (additions)
/// 5. Removes chunks from ringbuffer by id (removals) 
/// 6. Updates stored chunks in PluginState
///
/// NOTE: This function avoids holding multiple locks simultaneously to prevent deadlocks.
/// It performs calculations first, then applies changes in separate locked sections.
///
/// NOTE: This function is called when evaluating diffs on demand (e.g., from a command).
/// The on_buf_write_post function handles diffs on file save.
#[allow(dead_code)]
fn evaluate_all_repo_diffs() -> LttwResult<()> {
    let state = get_state();

    // Phase 1: Calculate new chunks from current buffer contents
    let mut new_chunks = Vec::new();

    // Get all file contents from the file_contents map (these are the buffers we're tracking)
    {
        let file_contents_lock = state.file_contents.read();
        let old_chunks_lock = state.diff_chunks.read();
        let old_chunks: Vec<diff_chunk::DiffChunk> = old_chunks_lock.clone();

        for (filepath, current_content) in file_contents_lock.iter() {
            // Get the saved content (if any)
            let saved_content = file_contents_lock.get(filepath);

            if let Some(saved) = saved_content {
                // Calculate diff between saved content and current content
                match calculate_diff_between_contents(filepath, saved, current_content) {
                    Ok(mut diff_chunks) => {
                        // Assign IDs to new chunks
                        let mut next_id = state.next_diff_chunk_id.load(std::sync::atomic::Ordering::SeqCst);
                        for chunk in &mut diff_chunks {
                            // Check if this filepath was in old chunks
                            let was_in_old = old_chunks.iter().any(|c| c.filepath == chunk.filepath);
                            if was_in_old {
                                // Find the old chunk for this filepath and reuse its id if content unchanged
                                let mut found = false;
                                for old_chunk in &old_chunks {
                                    if old_chunk.filepath == chunk.filepath && old_chunk.content == chunk.content {
                                        // Content unchanged - reuse old id
                                        chunk.id = old_chunk.id;
                                        found = true;
                                        break;
                                    }
                                }
                                if !found && chunk.id == 0 {
                                    // Content changed - assign new id
                                    chunk.id = next_id;
                                    next_id += 1;
                                }
                            } else {
                                // New filepath - assign new id
                                chunk.id = next_id;
                                next_id += 1;
                            }
                        }
                        state.next_diff_chunk_id.store(next_id, std::sync::atomic::Ordering::SeqCst);

                        new_chunks.extend(diff_chunks);
                    }
                    Err(e) => {
                        state.debug_manager.read().log(
                            "calculate_diff_error",
                            format!("Failed to calculate diff for {}: {}", filepath, e),
                        );
                    }
                }
            }
        }
    }

    // Phase 2: Evaluate changes
    let (additions, removals) = {
        let mut new_chunks_lock = state.diff_chunks.write();
        let old_chunks: Vec<diff_chunk::DiffChunk> = new_chunks_lock.clone();

        // Evaluate changes
        let (additions, removals) = evaluate_diff_changes(&new_chunks, &old_chunks);

        // Log operations for debugging
        log_diff_operations(&state.debug_manager.read(), &additions, &removals);

        // Update stored chunks
        *new_chunks_lock = new_chunks;

        (additions, removals)
    };

    // Phase 3: Apply changes to ring buffer (separate locked section)
    {
        let mut ring_buffer_lock = state.ring_buffer.write();

        // Perform removals first (as per requirements)
        for chunk in &removals {
            // Evict chunks by id from both queued and ring
            ring_buffer_lock.evict_by_id(chunk.id);
            state.debug_manager.read().log(
                "diff_chunk_evicted",
                format!("Evicted from buffer: {} (id: {})", chunk.filepath, chunk.id),
            );
        }

        // Perform additions (after removals)
        for chunk in &additions {
            let ring_chunk = chunk.to_ring_chunk();
            ring_buffer_lock.queued.push(ring_chunk);
            state.debug_manager.read().log(
                "diff_chunk_added",
                format!("Added to queued: {} (id: {})", chunk.filepath, chunk.id),
            );
        }
    }

    // Log summary
    let chunk_count = state.diff_chunks.read().len();
    state.debug_manager.read().log(
        "evaluate_all_repo_diffs",
        format!("Evaluated {} chunks ({} additions, {} removals)", 
            chunk_count, additions.len(), removals.len()),
    );

    Ok(())
}
