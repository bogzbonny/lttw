// src/lib.rs - Library interface for lttw Neovim plugin
//
// This module provides the entry point for the Neovim plugin using nvim-oxi.
// All core logic is implemented in Rust modules and exposed to Neovim via FFI.

/// Highlight group name for FIM generated text
pub const LTTW_FIM_HIGHLIGHT: &str = "LttwFIM";

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
pub mod lsp_completion;
pub mod plugin_state;
pub mod ring_buffer;
pub mod utils;

pub use error::{Error, LttwResult};

use {
    diagnostics::{debug_output_diagnostics, handle_diagnostic_changed},
    diff_chunk::calculate_diff_between_contents,
    fim::{
        FimAcceptType, FimResponse, FimTimings, fim_cycle_next, fim_cycle_prev, fim_try_hint,
        fim_try_hint_skip_debounce, render_fim_suggestion,
    },
    lsp_completion::retrieve_lsp_completions,
    nvim_oxi::{Dictionary, Function},
    plugin_state::{PluginState, get_state, init_state},
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
    buffer_id: u64, // Buffer handle to ensure we're still in same buffer
    //ctx: LocalContext,       // All buffer lines captured at start
    line_cur: String, // the current line where the completion was calculated (without completion)
    cursor_x: usize,  // Cursor position X
    cursor_y: usize,  // Cursor position Y
    completion: FimResponse, // All available completions for cycling
    do_render: bool,
    retry: Option<usize>, // the retry count for this completion
}

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

    functions.insert::<&str, Function<nvim_oxi::Object, ()>>("setup", Function::from(lttw_setup));

    // Export functions for diagnostic tracking
    functions.insert::<&str, Function<nvim_oxi::Object, ()>>(
        "handle_diagnostic_changed",
        Function::from(handle_diagnostic_changed),
    );
    functions.insert::<&str, Function<nvim_oxi::Object, ()>>(
        "debug_output_diagnostics",
        Function::from(debug_output_diagnostics),
    );

    Ok(functions)
}

/// Initialize the plugin setup with tracing
#[tracing::instrument(skip(c))]
fn lttw_setup(c: nvim_oxi::Object) {
    let _span = tracing::info_span!("plugin_setup").entered();
    // Initialize plugin state
    init_state(c);

    let state = get_state();
    let (tracing_enabled, log_file, tracing_level) = {
        let config = state.config.read();
        (
            config.tracing_enabled,
            config.tracing_log_file,
            config.tracing_level.clone(),
        )
    };

    // Initialize persistent tokio runtime and completion channel
    init_completion_processing_and_tracing_thread(tracing_enabled, log_file, tracing_level);

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
    /// Collection of completions for cycling (longest to shortest)
    completion_cycle: Vec<FimResponse>,
    /// Index of currently displayed completion in the cycle
    completion_index: usize,
}

impl FimState {
    #[allow(clippy::too_many_arguments)]
    #[allow(dead_code)]
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
        self.completion_cycle.clear();
        self.completion_index = 0;
    }

    /// Update the last pick position
    fn set_last_pick_buf_id_pos_y(&mut self, buf_id: u64, pos_y: usize) {
        self.last_pick_buf_id_pos_y = Some((buf_id, pos_y));
    }

    /// Get the last pick position
    fn get_last_pick_buf_id_pos_y(&self) -> Option<(u64, usize)> {
        self.last_pick_buf_id_pos_y
    }

    /// Set the completion cycle list
    fn set_completion_cycle(&mut self, completions: Vec<FimResponse>, idx: usize) {
        self.completion_cycle = completions;
        self.completion_index = idx;
    }

    /// Set the completion cycle list
    fn set_completion_idx(&mut self, idx: usize) {
        self.completion_index = idx;
    }

    /// Set the completion cycle list
    fn push_completion_cycle(&mut self, completions: FimResponse) {
        self.completion_cycle.push(completions);
    }

    fn push_completion_idx_to_tail(&mut self) {
        self.set_completion_idx(self.completion_cycle.len() - 1);
    }

    /// Cycle to next completion
    fn cycle_next(&mut self) -> Option<FimResponse> {
        if self.completion_cycle.is_empty() {
            return None;
        }
        self.completion_index = (self.completion_index + 1) % self.completion_cycle.len();
        // Update content to match the current completion
        let current = &self.completion_cycle[self.completion_index];
        Some(current.clone())
    }

    /// Cycle to previous completion
    fn cycle_prev(&mut self) -> Option<FimResponse> {
        if self.completion_cycle.is_empty() {
            return None;
        }
        self.completion_index = if self.completion_index == 0 {
            self.completion_cycle.len() - 1
        } else {
            self.completion_index - 1
        };
        // Update content to match the current completion
        let current = &self.completion_cycle[self.completion_index];
        Some(current.clone())
    }
}

/// Initialize persistent tokio runtime and completion channel
/// also start tracing.
#[tracing::instrument]
fn init_completion_processing_and_tracing_thread(
    tracing_enabled: bool,
    log_file: bool,
    trace_level: String,
) {
    let state = get_state();

    // Create channel for completion messages
    let (tx, mut rx) = mpsc::channel::<FimCompletionMessage>(16);
    *state.fim_completion_tx.write() = Some(tx);

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
            //state_.pending_display.write().push(msg);
            // do try write here for when we occassionally lock up (we will just lose the odd
            // message)
            let Some(mut pending_queue) = state_.pending_display.try_write() else {
                continue;
            };
            pending_queue.push(msg);
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
            nvim_oxi::schedule(|_| {
                if let Err(e) = process_pending_display() {
                    info!("process_pending_display() error: {}", e);
                }
            });
        },
    );
}

/// NOTE this occurs on the neovim thread
/// Process pending FIM display queue - drains and displays messages on the main thread
#[tracing::instrument]
fn process_pending_display() -> LttwResult<()> {
    let state = get_state();

    // Only display if we are in insert mode
    if !in_insert_mode()? {
        fim_hide()?; // failsafe if somehow a hint weezled its way in there
        return Ok(());
    }

    // Take all pending messages (clear the queue)
    let queued_messages: Vec<FimCompletionMessage> = {
        let Some(mut pending_queue) = state.pending_display.try_write() else {
            return Ok(());
        };
        std::mem::take(&mut *pending_queue)
    };

    //info!("process_pending_display");
    let mut messages = match retrieve_lsp_completions(&state) {
        Ok(c) => c,
        Err(e) => {
            info!("retrieve_lsp_completions error: {}", e);
            Vec::new()
        }
    };
    //info!("process_pending_display");

    //info!("process_pending_display");

    messages.extend(queued_messages);
    //info!("process_pending_display");

    if messages.is_empty() {
        return Ok(());
    }

    info!("Processing {} pending display messages", messages.len(),);

    // accept the most recent message which has content and isn't only whitespace
    let mut msg = None;
    for msg_ in messages.into_iter().rev() {
        if msg_is_valid_to_display(&msg_) {
            // because the msg is valie we already know that the message is for the cursor position
            state
                .fim_state
                .write()
                .push_completion_cycle(msg_.completion.clone());
            if msg.is_none() {
                if msg_.do_render {
                    state.fim_state.write().push_completion_idx_to_tail();
                }
                msg = Some(msg_);
            }
        }
    }

    let mut retry = 0;
    if let Some(msg) = msg {
        if msg.do_render {
            render_fim_suggestion(
                state.clone(),
                msg.cursor_x,
                msg.cursor_y,
                &msg.completion,
                msg.line_cur,
            )?;
        }
        retry = msg.retry.unwrap_or(0);
    }

    // NOTE there were messages nomatter what at this point in the function (even if none were
    // valid to display)
    //
    // if either the hint isn't shown OR it's only whitespace then trigger another fim
    // only retry a llm call 3 times before giving up
    if !state.fim_state.read().hint_shown && retry <= 3 {
        retry += 1;
        info!("rerendering fim suggestion");
        fim_try_hint(Some(retry))?;
    }

    Ok(())
}

// should we abort the completion because the content has changed since we started this completion
#[tracing::instrument]
fn msg_is_valid_to_display(msg: &FimCompletionMessage) -> bool {
    if msg.completion.content.is_empty() || msg.completion.content.trim().is_empty() {
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
    if curr_line != msg.line_cur {
        return false;
    }

    true
}

/// FIM accept function - can be used to accept real changes or virtually accept changes in order
/// to run speculative FIM for future rounds.
///
/// Returns new_x_pos, new_y_pos, combined content to write
#[tracing::instrument]
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
#[tracing::instrument]
fn fim_accept(accept_type: FimAcceptType) -> LttwResult<()> {
    // Log before releasing the lock
    let state = get_state();
    info!("fim_accept_triggered for {}", accept_type);

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

    info!("Accepting {} suggestion", accept_type);

    let (new_x, new_y, final_content) =
        fim_accept_inner(accept_type, pos_x, pos_y, line_cur, content)?;

    // replace the one line with all the new content (can be multiple lines)
    set_buf_lines(pos_y..=pos_y, final_content)?;

    // Move the cursor to the end of the accepted text
    set_window_cursor(new_x, new_y)?;

    // Set allow_comment_fim_cur_pos to allow FIM in comments immediately after accepting completion
    let buf_id = get_current_buffer_id();
    state.set_allow_comment_fim_cur_pos(buf_id, new_x, new_y);

    fim_hide_inner(&state)?;

    // immediately start a new FIM request skipping the debounce
    fim_try_hint_skip_debounce()?;

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
fn fim_hide_inner(state: &PluginState) -> LttwResult<()> {
    // Clear virtual text using nvim_buf_clear_namespace()
    if let Some(ns_id_val) = state.extmark_ns {
        clear_buf_namespace_objects(ns_id_val)?
    }

    state.fim_state.write().clear();
    Ok(())
}

/// Enable info display (set show_info = 2)
#[tracing::instrument]
fn enable_info() -> LttwResult<()> {
    let state = get_state();
    state.config.write().show_info = 2;
    Ok(())
}

/// Disable info display (set show_info = 0)
#[tracing::instrument]
fn disable_info() -> LttwResult<()> {
    let state = get_state();
    state.config.write().show_info = 0;
    Ok(())
}

#[tracing::instrument]
fn is_enabled() -> bool {
    let state = get_state();
    if state.enabled.load(Ordering::SeqCst) {
        return true;
    }
    false
}

/// Enable the plugin - sets up keymaps, autocmds, and state
#[tracing::instrument]
fn enable_plugin() -> LttwResult<()> {
    let state = get_state();

    // Check if already enabled
    if state.enabled.load(Ordering::SeqCst) {
        return Ok(());
    }

    // Check filetype
    let filetype = get_current_filetype()?;
    if !state.config.read().is_filetype_enabled(&filetype) {
        info!("Plugin not enabled for filetype: {}", filetype);
        return Ok(());
    }

    info!("Enabling plugin");

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
#[tracing::instrument]
fn disable_plugin() -> LttwResult<()> {
    let state = get_state();

    // Check if already disabled
    if !state.enabled.load(Ordering::SeqCst) {
        return Ok(());
    }

    info!("Disabling plugin");

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
#[tracing::instrument]
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

#[tracing::instrument]
fn on_move() -> LttwResult<()> {
    let state = get_state();
    *state.last_move_time.write() = Instant::now();

    // Check if cursor has moved to a different position than allow_comment_fim_cur_pos
    let (pos_x, pos_y) = get_pos();
    let buf_id = get_current_buffer_id();

    if let Some((allowed_buf, allowed_x, allowed_y)) = state.get_allow_comment_fim_cur_pos()
        && (buf_id != allowed_buf || pos_x != allowed_x || pos_y != allowed_y)
    {
        info!(
            "on_move clearing allow_comment_fim_cur_pos buf_id={buf_id}, pos_x={pos_x}, \
           pos_y={pos_y}, allowed_buf={allowed_buf}, allowed_x={allowed_x}, allowed_y={allowed_y}",
        );
        state.clear_allow_comment_fim_cur_pos();
    }

    info!("Cursor moved");
    fim_hide()?;
    fim_try_hint(None)?;
    Ok(())
}

/// Toggle auto_fim configuration
#[tracing::instrument]
fn set_mode_in_state() -> LttwResult<()> {
    let state = get_state();
    *state.nvim_mode.write() = get_mode_bz()?;
    Ok(())
}

/// Toggle auto_fim configuration
#[tracing::instrument]
fn set_cur_buffer_info_in_state() -> LttwResult<()> {
    let info = get_current_buffer_info()?;
    get_state().set_cur_buffer_info(info);
    Ok(())
}

/// Handle TextYankPost event - gather chunks from yanked text
#[tracing::instrument]
fn on_text_yank_post() -> LttwResult<()> {
    let state = get_state();

    // Get yanked text using vim.fn.getreg()
    // NOTE " is the default register for yanked text
    let reg_content = get_yanked_text()?;

    // Split by newlines to get individual lines
    let yanked: Vec<String> = reg_content.split('\n').map(|s| s.to_string()).collect();

    if !yanked.is_empty() {
        let filename = get_buf_filename().unwrap_or_default();

        info!("Yanked {} lines from {}", yanked.len(), filename,);

        // Pick chunk from yanked text
        let mut ring_buffer_lock = state.ring_buffer.write();
        ring_buffer_lock.pick_chunk(&state, &yanked, filename);
    }

    Ok(())
}

/// Handle BufEnter event - track file content and gather chunks from entered buffer
#[tracing::instrument]
fn on_buf_enter_gather_chunks() -> LttwResult<()> {
    let state = get_state();

    let lines = get_buf_lines(..);

    if lines.len() > 3 {
        let filename = get_buf_filename().unwrap_or_default();

        info!("Entered buffer with {} lines: {}", lines.len(), filename,);

        // Pick chunk from buffer
        let mut ring_buffer_lock = state.ring_buffer.write();
        ring_buffer_lock.pick_chunk(&state, &lines, filename.clone());
    }

    Ok(())
}

/// Handle BufLeave event - track file content and gather chunks from buffer before leaving
#[tracing::instrument]
fn on_buf_leave() -> LttwResult<()> {
    let state = get_state();

    let lines = get_buf_lines(..);

    if lines.len() > 3 {
        let filename = get_buf_filename().unwrap_or_default();

        info!("Leaving buffer with {} lines: {}", lines.len(), filename,);

        // Pick chunk from buffer
        let mut ring_buffer_lock = state.ring_buffer.write();
        ring_buffer_lock.pick_chunk(&state, &lines, filename);
    }

    Ok(())
}

/// Handle BufEnter event - update file contents if not already stored
/// This only reads from disk if there's no existing content saved for this file
#[tracing::instrument]
fn on_buf_enter_update_file_contents() -> LttwResult<()> {
    info!("on_buf_enter_update_file_contents");
    let state = get_state();

    // Only update file contents if diff tracking is enabled
    if !state.config.read().diff_tracking_enabled {
        return Ok(());
    }

    let filename = get_buf_filename()?;

    // If we already have content saved for this file, do nothing
    if state.has_file_contents(&filename) {
        return Ok(());
    }

    let new_content = std::fs::read_to_string(&filename)?;

    // Save the current file content for future diff comparison
    state.set_file_contents(filename.clone(), new_content);

    Ok(())
}

/// Handle BufWritePost event - track file content and evaluate diff chunks after saving buffer
#[tracing::instrument]
fn on_buf_write_post() -> LttwResult<()> {
    let state = get_state();

    let lines = get_buf_lines(..);

    if lines.len() > 3 {
        let filename = get_buf_filename().unwrap_or_default();

        info!("Buffer saved with {} lines: {filename}", lines.len(),);

        // Pick chunk from buffer
        {
            let mut ring_buffer_lock = state.ring_buffer.write();
            ring_buffer_lock.pick_chunk(&state, &lines, filename.clone());
        }

        if state.config.read().diff_tracking_enabled {
            let has_file = state.has_file_contents(&filename);
            if !has_file {
                state.set_file_contents_empty(filename);
            }

            let mut to_write = Vec::new();
            for (filename_, old_content) in state.file_contents_read().iter() {
                // get the new file contents from the filesystem
                let Ok(new_content) = std::fs::read_to_string(filename_) else {
                    continue;
                };

                // Get saved content for this file
                let diff_chunks = {
                    // Calculate diff between saved content and current content
                    if let Some(old_content) = old_content {
                        calculate_diff_between_contents(filename_, old_content, &new_content)?
                    } else {
                        // No previous content - return empty
                        Vec::new()
                    }
                };

                // TODO should check ALL the files that we've ever looked at.

                to_write.push((filename_.clone(), new_content));

                info!("diff_chunks: {:#?}", diff_chunks);

                // Process diff chunks
                if !diff_chunks.is_empty() {
                    // Apply changes to ring buffer in a separate locked section
                    let mut ring_buffer_lock = state.ring_buffer.write();

                    // Perform additions (after removals)
                    // TODO delete old intersecting chunks too
                    for chunk in &diff_chunks {
                        //let ring_chunk = chunk.to_ring_chunk();
                        ring_buffer_lock.pick_chunk_inner(&chunk.content, chunk.filepath.clone());
                        info!("diff_chunk_added Added to queued: {}", chunk.filepath,);
                    }
                }

                // process word statistics for diff chunks
                for c in diff_chunks {
                    state.adjust_word_statistics_for_diff(c.content);
                }
            }

            for (filename_, new_content) in to_write.into_iter() {
                // Save the current file content for future diff comparison
                state.set_file_contents_bypass_word_stats(filename_.clone(), new_content);
            }
        }
    }

    Ok(())
}

pub fn debug_word_statistics() {
    let state = get_state();
    state.debug_word_statistics();
}
