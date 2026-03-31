// src/lib.rs - Library interface for lttw Neovim plugin
//
// This module provides the entry point for the Neovim plugin using nvim-oxi.
// All core logic is implemented in Rust modules and exposed to Neovim via FFI.
pub mod cache;
pub mod config;
pub mod context;
pub mod debug;
pub mod fim;
pub mod instruction;
pub mod ring_buffer;
pub mod utils;

use {
    nvim_oxi::{
        api::{
            opts::{SetExtmarkOptsBuilder, SetKeymapOptsBuilder},
            types::Mode,
            {self, Buffer, Window},
        },
        Dictionary, Function, Result as NvimResult,
    },
    parking_lot::RwLock,
    std::{
        collections::HashMap,
        convert::TryInto,
        sync::{
            atomic::{AtomicBool, AtomicI64, Ordering},
            Arc, OnceLock,
        },
    },
};

/// Type alias for ring buffer timer handle to simplify type declarations
type RingBufferTimerHandle = Option<Arc<parking_lot::Mutex<tokio::task::JoinHandle<()>>>>;

// FIM completion channel types for async communication between worker and main thread
/// Message sent from async worker to main thread when completion is ready
#[derive(Debug, Clone)]
struct FimCompletionMessage {
    buffer_handle: u64,        // Buffer handle to ensure we're still in same buffer
    buffer_lines: Vec<String>, // All buffer lines captured at start
    cursor_x: usize,           // Cursor position X
    cursor_y: usize,           // Cursor position Y
    content: String,           // Raw JSON response content
}

use crate::instruction::InstructionStatus;

/// State for a single instruction request
pub use crate::instruction::InstructionRequestState;

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

/// Check if FIM hint is shown - internal helper for commands
fn fim_is_hint_shown() -> Result<bool, nvim_oxi::Error> {
    let state = get_state();

    let fim_state_lock = state.fim_state.read();
    {
        let debug_manager = state.debug_manager.read().clone();
        debug_manager.log(
            "fim_is_hint_shown",
            &[&(fim_state_lock.hint_shown).to_string()],
        );
    }
    Ok(fim_state_lock.hint_shown)
}

fn lttw_setup() -> NvimResult<()> {
    // Initialize plugin state
    init_state();

    // Register nvim-oxi commands
    register_commands()?;

    // Initialize persistent tokio runtime and completion channel
    init_tokio_runtime();

    // Setup keymaps
    setup_keymaps()?;

    // Setup autocmds
    setup_autocmds()?;

    Ok(())
}

/// State for FIM (Fill-in-Middle) completion
#[derive(Debug, Clone, Default)]
struct FimState {
    hint_shown: bool,
    pos_x: usize,
    pos_y: usize,
    line_cur: String,
    can_accept: bool,
    content: Vec<String>,
    cur_line: String, // the line which the FIM is for
}

// State management
#[derive(Clone)]
struct PluginState {
    config: Arc<RwLock<config::LttwConfig>>,
    cache: Arc<RwLock<cache::Cache>>,
    ring_buffer: Arc<RwLock<ring_buffer::RingBuffer>>,
    debug_manager: Arc<RwLock<debug::DebugManager>>,
    instruction_requests: Arc<RwLock<HashMap<i64, InstructionRequestState>>>,
    next_inst_req_id: Arc<AtomicI64>,
    fim_state: Arc<RwLock<FimState>>,
    extmark_ns: Option<u32>, // Namespace for extmarks (virtual text)
    inst_ns: Option<u32>,    // Namespace for instruction extmarks
    enabled: Arc<AtomicBool>,
    autocmd_ids: Arc<RwLock<Vec<u64>>>,
    ring_buffer_timer_handle: Arc<RwLock<RingBufferTimerHandle>>,
    // FIM completion channel for async worker communication
    fim_completion_tx:
        Arc<parking_lot::Mutex<Option<tokio::sync::mpsc::Sender<FimCompletionMessage>>>>,
    // Pending display queue - holds messages waiting to be rendered on main thread
    pending_display: Arc<RwLock<Vec<FimCompletionMessage>>>,
    // Persistent tokio runtime for async operations
    tokio_runtime: Arc<parking_lot::Mutex<Option<tokio::runtime::Runtime>>>,
}

impl PluginState {
    fn new() -> Self {
        let config = config::LttwConfig::from_nvim_globals();
        let enable_at_startup = config.enable_at_startup;
        let max_cache_keys = config.max_cache_keys as usize;
        let max_chunks = config.ring_n_chunks as usize;
        let chunk_size = config.ring_chunk_size as usize;

        // Create namespaces for extmarks
        let extmark_ns = Some(api::create_namespace("llama_fim"));
        let inst_ns = Some(api::create_namespace("llama_inst"));

        Self {
            config: Arc::new(RwLock::new(config)),
            cache: Arc::new(RwLock::new(cache::Cache::new(max_cache_keys))),
            ring_buffer: Arc::new(RwLock::new(ring_buffer::RingBuffer::new(
                max_chunks, chunk_size,
            ))),
            debug_manager: Arc::new(RwLock::new(debug::DebugManager::new())),
            instruction_requests: Arc::new(RwLock::new(HashMap::new())),
            next_inst_req_id: Arc::new(AtomicI64::new(0)),
            fim_state: Arc::new(RwLock::new(FimState::default())),
            extmark_ns,
            inst_ns,
            enabled: Arc::new(AtomicBool::new(enable_at_startup)),
            autocmd_ids: Arc::new(RwLock::new(Vec::new())),
            ring_buffer_timer_handle: Arc::new(RwLock::new(None)),
            // Initialize completion channel and runtime (will be set up later)
            fim_completion_tx: Arc::new(parking_lot::Mutex::new(None)),
            pending_display: Arc::new(RwLock::new(Vec::new())),
            tokio_runtime: Arc::new(parking_lot::Mutex::new(None)),
        }
    }
}

// Global state - using OnceLock for thread-safe initialization
static PLUGIN_STATE: OnceLock<Arc<PluginState>> = OnceLock::new();

/// Initialize the plugin state
fn init_state() {
    PLUGIN_STATE.get_or_init(|| Arc::new(PluginState::new()));
}

/// Get the plugin state (returns a clone of the Arc, no locking)
fn get_state() -> Arc<PluginState> {
    init_state();
    PLUGIN_STATE.get().unwrap().clone()
}

/// Get buffer lines from Neovim
fn buf_get_lines() -> Vec<String> {
    let buf = Buffer::current();
    let lines = buf.get_lines(.., false).unwrap();
    lines.map(|s| s.to_string()).collect()
}

/// Get current buffer position
fn get_pos() -> (usize, usize) {
    let (line, col) = Window::current().get_cursor().unwrap_or((0, 0));

    // NOTE nvim starts at 1, must make 0 start
    let col = col.saturating_sub(1);
    let line = line.saturating_sub(1);
    (col, line)
}

/// Get current buffer
fn get_current_buffer() -> u64 {
    let buf: u64 = Buffer::current().handle().try_into().unwrap_or(0);
    buf
}

/// Initialize persistent tokio runtime and completion channel
fn init_tokio_runtime() {
    let state = get_state();

    // Create a multi-threaded tokio runtime
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
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

    // Store the sender in state
    {
        let mut fim_completion_tx_lock = state.fim_completion_tx.lock();
        *fim_completion_tx_lock = Some(tx);
    }

    // Spawn a task that receives completion messages and adds them to the pending display queue
    // This runs on its own dedicated current-thread runtime separate from the main multi-threaded one
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

    let _ = nvim_oxi::libuv::TimerHandle::start(
        std::time::Duration::from_millis(500),
        std::time::Duration::from_millis(100), // repeat
        |_| {
            // Need this so that it executes on the main thread (or else extmarks won't display)
            nvim_oxi::schedule(|_| process_pending_display());
        },
    );

    // Store the multi-threaded runtime (used by other operations like ring buffer timer)
    {
        let mut tokio_runtime_lock = state.tokio_runtime.lock();
        *tokio_runtime_lock = Some(runtime);
    }
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
            "Received completion for buffer {} at ({}, {}), msg.content {}",
            msg.buffer_handle, msg.cursor_x, msg.cursor_y, msg.content
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
    {
        let mut fim_state_lock = state.fim_state.write();
        fim_state_lock.hint_shown = rendered.can_accept;
        fim_state_lock.pos_x = msg.cursor_x;
        fim_state_lock.pos_y = msg.cursor_y;
        fim_state_lock.line_cur = msg
            .buffer_lines
            .get(msg.cursor_y)
            .cloned()
            .unwrap_or_default();
        fim_state_lock.can_accept = rendered.can_accept;
        fim_state_lock.content = rendered.content;
    }

    // Display virtual text using extmarks
    display_fim_hint(&state)?;

    state.debug_manager.read().log(
        "handle_fim_completion_message",
        &[&format!("Displaying FIM hint: {} lines", content_len)],
    );

    Ok(())
}

/// Async worker task that collects neovim information and sends completion results through channel
/// This function is called from on_cursor_moved_i and spawns a non-blocking task
async fn spawn_fim_worker(
    state: Arc<PluginState>,
    buffer_handle: u64,
    buffer_lines: Vec<String>,
    cursor_x: usize,
    cursor_y: usize,
    is_auto: bool,
) -> Result<(), nvim_oxi::Error> {
    state.debug_manager.read().log(
        "spawn_fim_worker",
        &[&format!("Spawning worker for ({}, {})", cursor_x, cursor_y)],
    );

    // Get the channel sender from state
    let tx = {
        let fim_completion_tx_lock = state.fim_completion_tx.lock();
        fim_completion_tx_lock.clone().ok_or_else(|| {
            nvim_oxi::Error::Api(api::Error::Other(
                "Completion channel not initialized".to_string(),
            ))
        })?
    };

    // Collect all neovim information at the start
    let config = state.config.read().clone();
    let debug_manager = state.debug_manager.read().clone();
    let cache = state.cache.clone();
    let ring_buffer = state.ring_buffer.clone();
    let fim_state = state.fim_state.clone();

    // Spawn async task to perform HTTP request
    tokio::spawn(async move {
        // Check if we should trigger speculative FIM
        let speculative_fim = {
            let fim_state_lock = fim_state.read();
            fim_state_lock.hint_shown && !fim_state_lock.content.is_empty()
        };

        let result = if speculative_fim {
            let prev_content = {
                let fim_state_lock = fim_state.read();
                fim_state_lock.content.clone()
            };

            // Trigger speculative FIM
            fim::fim_completion(
                debug_manager.clone(),
                cursor_x,
                cursor_y,
                false, // Not auto - use longer timeout
                &buffer_lines,
                &config,
                cache,
                ring_buffer,
                Some(&prev_content),
            )
            .await
        } else {
            // Normal FIM
            fim::fim_completion(
                debug_manager.clone(),
                cursor_x,
                cursor_y,
                is_auto,
                &buffer_lines,
                &config,
                cache,
                ring_buffer,
                None,
            )
            .await
        };

        // Send result through channel
        if let Ok(Some(content)) = result {
            let msg = FimCompletionMessage {
                buffer_handle,
                buffer_lines,
                cursor_x,
                cursor_y,
                content,
            };

            // Use blocking_send since we're in an async context but want to ensure delivery
            if let Err(e) = tx.send(msg).await {
                debug_manager.log(
                    "spawn_fim_worker",
                    &[&format!("Failed to send completion message: {}", e)],
                );
            }
        }
    });

    Ok(())
}

/// Process pending FIM display queue - drains and displays messages on the main thread
fn process_pending_display() -> NvimResult<()> {
    let state = get_state();

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
fn fim_accept(accept_type: &str) -> NvimResult<Option<String>> {
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
    let (new_line, rest) = fim::accept_fim_suggestion(accept_type, pos_x, &line_cur, &content);

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
    let new_col = new_line.len();
    let mut window = Window::current();
    let _ = window.set_cursor(pos_y + 1, new_col + 1);

    // Clear the FIM hint - use write lock
    {
        let mut fim_state_lock = state.fim_state.write();
        fim_state_lock.hint_shown = false;
        fim_state_lock.content.clear();
    }

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
fn display_fim_hint(state: &Arc<PluginState>) -> NvimResult<()> {
    // Lock the fim_state and config to get the data we need
    let (hint_shown, content, extmark_ns, pos_y, pos_x, _config, debug_manager) = {
        let fim_state_lock = state.fim_state.read();
        let fim_state_hint_shown = fim_state_lock.hint_shown;
        let fim_state_content = fim_state_lock.content.clone();
        let fim_state_pos_y = fim_state_lock.pos_y;
        let fim_state_pos_x = fim_state_lock.pos_x;
        let extmark_ns = state.extmark_ns;
        let config = state.config.read().clone();
        let debug_manager = state.debug_manager.read().clone();
        (
            fim_state_hint_shown,
            fim_state_content,
            extmark_ns,
            fim_state_pos_y,
            fim_state_pos_x,
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
    if !api::get_mode()?
        .mode
        .as_bytes()
        .first()
        .copied()
        .expect("mode is not empty")
        == b'i'
    {
        return Ok(());
    }

    if let Some(ns_id) = extmark_ns {
        let mut buf = Buffer::current();

        // Build virtual text string - first line of suggestion
        let suggestion_text = content[0].clone();

        // Build inline info string if show_info is enabled (mode 2 = inline)
        let virt_text_vec: Vec<(String, String)> =
            { vec![(suggestion_text, "Comment".to_string())] };

        // Create extmark opts with virtual text using builder pattern
        let mut opts = SetExtmarkOptsBuilder::default();
        opts.virt_text(virt_text_vec);
        if content.len() > 1 {
            opts.virt_text_pos(nvim_oxi::api::types::ExtmarkVirtTextPosition::Overlay);
        } else {
            opts.virt_text_pos(nvim_oxi::api::types::ExtmarkVirtTextPosition::Inline);
        }

        // Add multi-line support - display rest of suggestion lines below
        if content.len() > 1 {
            let mut virt_lines: Vec<Vec<(String, String)>> = Vec::new();

            // Add remaining content lines
            for line in &content[1..] {
                virt_lines.push(vec![(line.clone(), "Comment".to_string())]);
            }

            opts.virt_lines(virt_lines);
        }

        // Set the extmark at cursor position
        match buf.set_extmark(ns_id, pos_y, pos_x + 1, &opts.build()) {
            Ok(_id) => {
                debug_manager.log(
                    "display_fim_hint",
                    &[&format!("Set extmark at line {}, col {}", pos_y, pos_x + 1)],
                );
            }
            Err(e) => {
                debug_manager.log(
                    "display_fim_hint",
                    &[&format!("Error setting extmark: {:?}", e)],
                );
            }
        }
    }

    Ok(())
}

/// Ring buffer pick chunk function
fn ring_pick_chunk(lines: Vec<String>, no_mod: bool, do_evict: bool) -> NvimResult<()> {
    let state = get_state();
    state
        .ring_buffer
        .write()
        .pick_chunk(lines, no_mod, do_evict);
    Ok(())
}

/// Ring buffer update function
fn ring_update() -> NvimResult<()> {
    let state = get_state();
    state.ring_buffer.write().update();
    Ok(())
}

/// Cache insert function
fn cache_insert(key: &str, value: &str) -> NvimResult<()> {
    let state = get_state();
    state
        .cache
        .write()
        .insert(key.to_string(), value.to_string());
    Ok(())
}

/// Ring buffer get extra function
fn ring_get_extra() -> NvimResult<Vec<Dictionary>> {
    let state = get_state();
    let ring_buffer_lock = state.ring_buffer.read();
    let extra = ring_buffer_lock.get_extra();

    let mut result = Vec::new();
    for e in extra {
        let mut dict = Dictionary::new();
        dict.insert("text", e.text);
        dict.insert("filename", e.filename);
        result.push(dict);
    }

    Ok(result)
}

/// Cache get function
fn cache_get(key: &str) -> NvimResult<Option<String>> {
    let state = get_state();
    let cache_lock = state.cache.read();
    Ok(cache_lock.get_fim(key))
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

/// Open the debug buffer - kept for compatibility but now does nothing
#[allow(dead_code)]
fn debug_open_buffer(_state: &mut PluginState) -> NvimResult<()> {
    // No longer needed - logs go to file
    Ok(())
}

/// Flush debug log to the debug buffer - kept for compatibility but now does nothing
#[allow(dead_code)]
fn debug_flush_buffer(_state: &mut PluginState) -> NvimResult<()> {
    // No longer needed - logs go to file
    Ok(())
}

/// Debug get log function - for compatibility, returns empty vec since logs are in file
fn debug_get_log() -> NvimResult<Vec<String>> {
    // Logs are now written to file, this returns empty for compatibility
    Ok(Vec::new())
}

/// Get filetype function
fn get_filetype() -> NvimResult<String> {
    let buf = Buffer::current();
    let path = buf.get_name().map_err(|_| {
        nvim_oxi::Error::Api(api::Error::Other("Failed to get buffer name".to_string()))
    })?;

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

// Expression mapping helper functions removed - using command-based callbacks instead

/// Setup keymaps function - maps keys to call nvim-oxi commands directly
fn setup_keymaps() -> NvimResult<()> {
    // FIM trigger - calls the LttwFim command
    let _ = api::set_keymap(
        Mode::Normal,
        "<leader>llf",
        ":LttwFim<CR>",
        &Default::default(),
    );

    // FIM accept word
    let _ = api::set_keymap(
        Mode::Normal,
        "<leader>ll]",
        ":LttwFimAcceptWord<CR>",
        &Default::default(),
    );

    // Instruction trigger
    let _ = api::set_keymap(
        Mode::Normal,
        "<leader>lli",
        ":LttwInst<CR>",
        &Default::default(),
    );

    // Instruction rerun
    let _ = api::set_keymap(
        Mode::Normal,
        "<leader>llr",
        ":LttwInstRerun<CR>",
        &Default::default(),
    );

    // Instruction continue
    let _ = api::set_keymap(
        Mode::Normal,
        "<leader>llc",
        ":LttwInstContinue<CR>",
        &Default::default(),
    );

    // Debug toggle
    let _ = api::set_keymap(
        Mode::Normal,
        "<leader>lld",
        ":LttwDebugToggle<CR>",
        &Default::default(),
    );

    // FIM keymaps - use command-based callbacks for proper ESC/TAB handling
    // These commands check if FIM hint is shown and act accordingly

    // FIM accept full (TAB) - check if FIM shown, accept if yes, insert tab if no
    let _ = api::set_keymap(
        Mode::Insert,
        "<Tab>",
        "",
        &SetKeymapOptsBuilder::default()
            .callback(|_| {
                if let Ok(true) = fim_is_hint_shown() {
                    if let Err(e) = fim_accept("full") {
                        // Log error but don't crash
                        let state = get_state();
                        state
                            .debug_manager
                            .read()
                            .log("Tab accept", &[&format!("Error accepting FIM: {:?}", e)]);
                    }
                }
            })
            .build(),
    );

    // Note: ESC is not mapped in Insert mode to avoid interfering with normal ESC behavior
    // ESC will naturally exit Insert mode. If FIM hint is shown, it will be hidden when
    // the user presses ESC to exit Insert mode (handled by fim_hide_on_escape autocmd if needed)

    // FIM accept line (S-Tab) - check if FIM shown, accept line if yes, re-inject S-Tab if no
    let _ = api::set_keymap(
        Mode::Insert,
        "<S-Tab>",
        "",
        &SetKeymapOptsBuilder::default()
            .callback(|_| {
                if let Ok(true) = fim_is_hint_shown() {
                    if let Err(e) = fim_accept("line") {
                        // Log error but don't crash
                        let state = get_state();
                        state.debug_manager.read().log(
                            "LttwFimAcceptFullOrTab",
                            &[&format!("Error accepting FIM: {:?}", e)],
                        );
                    }
                }
            })
            .build(),
    );

    Ok(())
}

/// Remove keymaps function - unmaps all plugin keymaps
fn remove_keymaps() -> NvimResult<()> {
    let state = get_state();
    let config = state.config.read();

    // Unmap FIM keymaps
    if !config.keymap_fim_trigger.is_empty() {
        let _ = api::del_keymap(Mode::Normal, &config.keymap_fim_trigger);
    }
    if !config.keymap_fim_accept_full.is_empty() {
        let _ = api::del_keymap(Mode::Normal, &config.keymap_fim_accept_full);
    }
    if !config.keymap_fim_accept_line.is_empty() {
        let _ = api::del_keymap(Mode::Normal, &config.keymap_fim_accept_line);
    }
    if !config.keymap_fim_accept_word.is_empty() {
        let _ = api::del_keymap(Mode::Normal, &config.keymap_fim_accept_word);
    }

    // Unmap instruction keymaps
    if !config.keymap_inst_trigger.is_empty() {
        let _ = api::del_keymap(Mode::Normal, &config.keymap_inst_trigger);
    }
    if !config.keymap_inst_rerun.is_empty() {
        let _ = api::del_keymap(Mode::Normal, &config.keymap_inst_rerun);
    }
    if !config.keymap_inst_continue.is_empty() {
        let _ = api::del_keymap(Mode::Normal, &config.keymap_inst_continue);
    }
    if !config.keymap_inst_accept.is_empty() {
        let _ = api::del_keymap(Mode::Normal, &config.keymap_inst_accept);
    }
    if !config.keymap_inst_cancel.is_empty() {
        let _ = api::del_keymap(Mode::Normal, &config.keymap_inst_cancel);
    }

    // Unmap debug keymaps
    if !config.keymap_debug_toggle.is_empty() {
        let _ = api::del_keymap(Mode::Normal, &config.keymap_debug_toggle);
    }

    // Unmap FIM insert-mode keymaps for accept/cancel (these are always set up)
    let _ = api::del_keymap(Mode::Insert, "<Tab>");
    let _ = api::del_keymap(Mode::Insert, "<Esc>");
    let _ = api::del_keymap(Mode::Insert, "<S-Tab>");

    Ok(())
}

/// Enable the plugin - sets up keymaps, autocmds, and state
fn enable_plugin() -> NvimResult<()> {
    let state = get_state();

    // Check if already enabled
    if state.enabled.load(Ordering::SeqCst) {
        return Ok(());
    }

    // Check filetype
    let filetype = get_filetype()?;
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
    setup_keymaps()?;

    // Setup autocmds
    setup_autocmds()?;

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
    remove_keymaps()?;

    // Clear autocmds (marked for cleanup)
    // Note: nvim-oxi doesn't provide direct autocmd deletion, so we just clear tracking
    {
        let mut autocmd_ids_lock = state.autocmd_ids.write();
        autocmd_ids_lock.clear();
    }

    // Mark as disabled
    state.enabled.store(false, Ordering::SeqCst);

    Ok(())
}

/// Toggle the plugin on/off
fn toggle_plugin() -> NvimResult<bool> {
    let state = get_state();
    let currently_enabled = state.enabled.load(Ordering::SeqCst);
    drop(state);

    if currently_enabled {
        disable_plugin()?;
        Ok(false)
    } else {
        enable_plugin()?;
        Ok(true)
    }
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
    setup_autocmds()?;

    Ok(new_value)
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
fn on_buf_enter() -> NvimResult<()> {
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

/// Handle CursorMovedI event - trigger speculative FIM completion using async worker
fn on_cursor_moved_i() -> NvimResult<()> {
    let _ = fim_hide();
    let state = get_state();
    state.debug_manager.read().log(
        "on_cursor_moved_i",
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

    // Get CURRENT cursor position
    let (pos_x, pos_y) = get_pos();
    let lines = buf_get_lines();
    let buffer_handle: u64 = Buffer::current().handle().try_into().unwrap_or(0);

    state.debug_manager.read().log(
        "on_cursor_moved_i",
        &[&format!(
            "Cursor moved in insert mode at ({}, {})",
            pos_x, pos_y
        )],
    );

    state.debug_manager.read().log("on_cursor_moved_i 1", &[]);

    // Try to show a cached hint (synchronous - fast)
    let hashes = fim::compute_hashes(&{
        let config_lock = state.config.read();
        context::get_local_context(&lines, pos_x, pos_y, None, &config_lock)
    });
    state.debug_manager.read().log("on_cursor_moved_i 2", &[]);

    // Check cache for primary hash
    let mut found_cached = false;
    for hash in &hashes {
        state
            .debug_manager
            .read()
            .log("on_cursor_moved_i hashes 3", &[&hash.to_string()]);
        if let Some(response_text) = {
            let cache_lock = state.cache.read();
            cache_lock.get_fim(hash)
        } {
            found_cached = true;
            state.debug_manager.read().log(
                "on_cursor_moved_i",
                &[&format!("Found cached completion for hash {}", &hash[..16])],
            );

            // Parse response and render (synchronous)
            if let Ok(response) = serde_json::from_str::<serde_json::Value>(&response_text) {
                if let Some(content) = response.get("content").and_then(|c| c.as_str()) {
                    let ctx = {
                        let config_lock = state.config.read();
                        context::get_local_context(&lines, pos_x, pos_y, None, &config_lock)
                    };
                    let rendered = {
                        let config_lock = state.config.read();
                        fim::render_fim_suggestion(
                            pos_x,
                            pos_y,
                            content,
                            &ctx.line_cur_suffix,
                            &config_lock,
                        )
                    };

                    // Update FIM state
                    {
                        let mut fim_state_lock = state.fim_state.write();
                        fim_state_lock.hint_shown = rendered.can_accept;
                        fim_state_lock.pos_x = pos_x;
                        fim_state_lock.pos_y = pos_y;
                        fim_state_lock.line_cur = lines.get(pos_y).cloned().unwrap_or_default();
                        fim_state_lock.can_accept = rendered.can_accept;
                        fim_state_lock.content = rendered.content.clone();
                    }

                    // Display virtual text using extmarks
                    if rendered.can_accept {
                        let _ = display_fim_hint(&state);

                        state.debug_manager.read().log(
                            "on_cursor_moved_i",
                            &[&format!(
                                "Showing FIM hint from cursor move: {} lines",
                                rendered.content.len()
                            )],
                        );
                    }

                    break;
                }
            }
        }
    }
    state.debug_manager.read().log("on_cursor_moved_i 4", &[]);

    // If no cached hint found and we're not already showing a hint, spawn async worker
    {
        let hint_shown = state.fim_state.read().hint_shown;
        if !found_cached && !hint_shown {
            // Only trigger FIM if we're in a reasonable position
            state.debug_manager.read().log(
                "on_cursor_moved_i 4.1",
                &[&format!(
                    "pos_y {pos_y}, pos_x {pos_x}, lines_len: {}",
                    lines.len()
                )],
            );
            if pos_y < lines.len() && pos_x <= lines.get(pos_y).map(|l| l.len()).unwrap_or(0) {
                state
                    .debug_manager
                    .read()
                    .log("on_cursor_moved_i 4.21", &[]);

                // Collect all neovim information at the start
                // This is critical - we capture everything needed before spawning async task
                let state_clone = state.clone();
                let buffer_lines_clone = lines.clone();
                let buffer_handle_clone = buffer_handle;
                let pos_x_clone = pos_x;
                let pos_y_clone = pos_y;

                // Spawn async worker task that will:
                // 1. Perform HTTP request (non-blocking)
                // 2. Send completion result through channel
                // 3. Main thread receives and renders
                {
                    let tokio_runtime_lock = state.tokio_runtime.lock();
                    if let Some(runtime) = tokio_runtime_lock.as_ref() {
                        // Use the persistent tokio runtime to spawn the worker
                        // The worker collects all neovim info at start and sends through channel
                        let _ = runtime.spawn(async move {
                            spawn_fim_worker(
                                state_clone,
                                buffer_handle_clone,
                                buffer_lines_clone,
                                pos_x_clone,
                                pos_y_clone,
                                true, // is_auto = true
                            )
                            .await
                        });
                    } else {
                        state.debug_manager.read().log(
                            "on_cursor_moved_i",
                            &["Tokio runtime not initialized, falling back to blocking"],
                        );
                    }
                }

                state
                    .debug_manager
                    .read()
                    .log("on_cursor_moved_i 4.22", &[]);
            }
        }
    }
    state.debug_manager.read().log("on_cursor_moved_i 5", &[]);

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

/// Process ring buffer updates - moves queued chunks to active ring and sends to server
fn process_ring_buffer() -> NvimResult<()> {
    let state = get_state();

    // Get configuration
    let update_interval = state.config.read().ring_update_ms;

    // Check if we have chunks before logging
    let chunk_count = {
        // Move first queued chunk to ring
        let mut ring_buffer_lock = state.ring_buffer.write();
        ring_buffer_lock.update();
        ring_buffer_lock.len()
    };

    if chunk_count > 0 {
        state.debug_manager.read().log(
            "process_ring_buffer",
            &[&format!(
                "Processing {} ring buffer chunks (interval: {}ms)",
                chunk_count, update_interval
            )],
        );

        // Build request with ring buffer context
        let extra = state.ring_buffer.read().get_extra();
        let request = serde_json::json!({
            "input_extra": extra,
            "cache_prompt": true
        });

        // Send to server (fire and forget - non-blocking)
        let config = state.config.read().clone();
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async move {
                let client = reqwest::Client::new();
                let _ = client
                    .post(&config.endpoint_fim)
                    .json(&request)
                    .bearer_auth(&config.api_key)
                    .send()
                    .await;
            });
    }

    Ok(())
}

/// Setup autocmds function - creates autocmds for auto-triggering FIM and ring buffer
fn setup_autocmds() -> NvimResult<()> {
    let state = get_state();

    // Clear existing autocmd IDs first (cleanup)
    {
        let mut autocmd_ids_lock = state.autocmd_ids.write();
        autocmd_ids_lock.clear();
    }

    // Cursor movement for auto-FIM (CursorMovedI in insert mode)
    if state.config.read().auto_fim {
        let id = api::create_autocmd(
            ["CursorMovedI", "InsertEnter", "InsertChange"],
            &nvim_oxi::api::opts::CreateAutocmdOptsBuilder::default()
                .callback(|_| {
                    let _ = on_cursor_moved_i();
                    false // DO NOT DELETE this autocommand once used
                })
                .build(),
        )
        .unwrap_or(0);
        let mut autocmd_ids_lock = state.autocmd_ids.write();
        autocmd_ids_lock.push(id as u64);
    }

    // Yank text for ring buffer (TextYankPost)
    let id = api::create_autocmd(
        ["TextYankPost"],
        &nvim_oxi::api::opts::CreateAutocmdOptsBuilder::default()
            .callback(|_| {
                let _ = on_text_yank_post();
                false // DO NOT DELETE this autocommand once used
            })
            .build(),
    )
    .unwrap_or(0);
    {
        let mut autocmd_ids_lock = state.autocmd_ids.write();
        autocmd_ids_lock.push(id as u64);
    }

    // Buffer enter for ring buffer AND filetype check
    let id = api::create_autocmd(
        ["BufEnter"],
        &nvim_oxi::api::opts::CreateAutocmdOptsBuilder::default()
            .callback(|_| {
                let _ = on_buf_enter_and_check_filetype();
                false // DO NOT DELETE this autocommand once used
            })
            .build(),
    )
    .unwrap_or(0);
    {
        let mut autocmd_ids_lock = state.autocmd_ids.write();
        autocmd_ids_lock.push(id as u64);
    }

    // Buffer leave for ring buffer
    let id = api::create_autocmd(
        ["BufLeave"],
        &nvim_oxi::api::opts::CreateAutocmdOptsBuilder::default()
            .callback(|_| {
                let _ = on_buf_leave();
                false // DO NOT DELETE this autocommand once used
            })
            .build(),
    )
    .unwrap_or(0);
    {
        let mut autocmd_ids_lock = state.autocmd_ids.write();
        autocmd_ids_lock.push(id as u64);
    }

    // Buffer write for ring buffer
    let id = api::create_autocmd(
        ["BufWritePost"],
        &nvim_oxi::api::opts::CreateAutocmdOptsBuilder::default()
            .callback(|_| {
                let _ = on_buf_write_post();
                false // DO NOT DELETE this autocommand once used
            })
            .build(),
    )
    .unwrap_or(0);
    {
        let mut autocmd_ids_lock = state.autocmd_ids.write();
        autocmd_ids_lock.push(id as u64);
    }

    // InsertLeave - hide FIM hint when leaving Insert mode
    let id = api::create_autocmd(
        ["InsertLeave"],
        &nvim_oxi::api::opts::CreateAutocmdOptsBuilder::default()
            .callback(|_| {
                let _ = fim_hide();
                false // DO NOT DELETE this autocommand once used
            })
            .build(),
    )
    .unwrap_or(0);
    {
        let mut autocmd_ids_lock = state.autocmd_ids.write();
        autocmd_ids_lock.push(id as u64);
    }

    // Setup timer-based ring buffer updates (every ring_update_ms)
    setup_ring_buffer_timer()?;

    Ok(())
}

/// Filetype check autocmd handler - enables/disables plugin based on filetype
fn on_buf_enter_and_check_filetype() -> NvimResult<()> {
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

/// Setup a repeating timer to process ring buffer updates using tokio
fn setup_ring_buffer_timer() -> NvimResult<()> {
    let state = get_state();
    let interval = state.config.read().ring_update_ms;
    let interval_duration = std::time::Duration::from_millis(interval as u64);

    // Create a new tokio runtime and spawn the timer task
    // This follows the same pattern used elsewhere in the codebase
    let timer_handle = tokio::runtime::Runtime::new().unwrap().spawn(async move {
        // Create a recurring interval timer
        let mut interval = tokio::time::interval(interval_duration);

        loop {
            // Wait for the next tick
            interval.tick().await;

            // Process the ring buffer
            let _ = process_ring_buffer();
        }
    });

    // Store the handle in the plugin state
    {
        let mut ring_buffer_timer_handle_lock = state.ring_buffer_timer_handle.write();
        *ring_buffer_timer_handle_lock = Some(Arc::new(parking_lot::Mutex::new(timer_handle)));
    }

    state.debug_manager.read().log(
        "setup_ring_buffer_timer",
        &[&format!(
            "Started ring buffer timer (interval: {}ms)",
            interval
        )],
    );

    Ok(())
}

/// Register nvim-oxi commands for the plugin
fn register_commands() -> NvimResult<()> {
    // FIM commands - use closure without args parameter
    //let _ = api::create_user_command(
    //    "LttwFim",
    //    |_| -> NvimResult<()> {
    //        fim_completion(false)?;
    //        Ok(())
    //    },
    //    &Default::default(),
    //);

    let _ = api::create_user_command(
        "LttwFimAcceptFull",
        |_| -> NvimResult<()> {
            fim_accept("full")?;
            Ok(())
        },
        &Default::default(),
    );

    let _ = api::create_user_command(
        "LttwFimAcceptLine",
        |_| -> NvimResult<()> {
            fim_accept("line")?;
            Ok(())
        },
        &Default::default(),
    );

    let _ = api::create_user_command(
        "LttwFimAcceptWord",
        |_| -> NvimResult<()> {
            fim_accept("word")?;
            Ok(())
        },
        &Default::default(),
    );

    // FIM hide command
    let _ = api::create_user_command(
        "LttwFimHide",
        |_| -> NvimResult<()> {
            fim_hide()?;
            Ok(())
        },
        &Default::default(),
    );

    // Instruction commands
    let _ = api::create_user_command(
        "LttwInst",
        |_| -> NvimResult<()> {
            // TODO: Get visual range and start instruction
            debug_log("Starting instruction editing", vec![])?;
            Ok(())
        },
        &Default::default(),
    );

    let _ = api::create_user_command(
        "LttwInstRerun",
        |_| -> NvimResult<()> {
            instruction::inst_rerun()?;
            Ok(())
        },
        &Default::default(),
    );

    let _ = api::create_user_command(
        "LttwInstContinue",
        |_| -> NvimResult<()> {
            instruction::inst_continue()?;
            Ok(())
        },
        &Default::default(),
    );

    // Debug commands
    let _ = api::create_user_command(
        "LttwDebugToggle",
        |_| -> NvimResult<()> {
            debug_toggle()?;
            Ok(())
        },
        &Default::default(),
    );

    let _ = api::create_user_command(
        "LttwDebugClear",
        |_| -> NvimResult<()> {
            debug_clear()?;
            Ok(())
        },
        &Default::default(),
    );

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
