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
            opts::SetExtmarkOptsBuilder,
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
            atomic::{AtomicBool, AtomicI64},
            Arc, OnceLock,
        },
    },
};

/// Type alias for ring buffer timer handle to simplify type declarations
type RingBufferTimerHandle = Option<Arc<parking_lot::Mutex<tokio::task::JoinHandle<()>>>>;

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

    // FIM functions
    functions.insert::<&str, Function<Dictionary, Option<String>>>(
        "fim_completion",
        Function::from(|_: Dictionary| fim_completion(false)),
    );

    functions.insert::<&str, Function<Dictionary, Option<String>>>(
        "fim_completion_auto",
        Function::from(|_: Dictionary| fim_completion(true)),
    );

    // Note: fim_render is now handled internally via display_fim_hint

    functions.insert::<&str, Function<String, Option<String>>>(
        "fim_accept",
        Function::from(|accept_type: String| fim_accept(&accept_type)),
    );

    functions.insert::<&str, Function<(), ()>>("fim_hide", Function::from(|_| fim_hide()));

    functions.insert::<&str, Function<(), Option<String>>>(
        "fim_try_hint",
        Function::from(|_| fim_try_hint()),
    );

    // Instruction functions
    functions.insert::<&str, Function<(Vec<String>, i64, i64, String), Dictionary>>(
        "inst_build",
        Function::from(|(lines, l0, l1, inst): (Vec<String>, i64, i64, String)| {
            inst_build(lines, l0, l1, &inst)
        }),
    );

    // New instruction API with proper state tracking
    functions.insert::<&str, Function<(i64, i64, String), NvimResult<i64>>>(
        "inst_start",
        Function::from(|(l0, l1, inst): (i64, i64, String)| -> NvimResult<i64> {
            inst_start(l0, l1, &inst)
        }),
    );

    functions.insert::<&str, Function<(i64, String), String>>(
        "inst_update",
        Function::from(|(req_id, response): (i64, String)| -> NvimResult<String> {
            inst_update(req_id, &response)
        }),
    );

    functions.insert::<&str, Function<i64, Option<String>>>(
        "inst_send",
        Function::from(|req_id: i64| inst_send(req_id)),
    );

    functions.insert::<&str, Function<i64, ()>>(
        "inst_finalize",
        Function::from(|req_id: i64| {
            let _ = inst_finalize(req_id);
        }),
    );

    functions.insert::<&str, Function<(), ()>>("inst_accept", Function::from(|_| inst_accept()));

    functions.insert::<&str, Function<(), ()>>("inst_cancel", Function::from(|_| inst_cancel()));

    functions.insert::<&str, Function<(), Option<String>>>(
        "inst_rerun",
        Function::from(|_| inst_rerun()),
    );

    functions.insert::<&str, Function<(), Option<String>>>(
        "inst_continue",
        Function::from(|_| inst_continue()),
    );

    // Warm-up function
    functions.insert::<&str, Function<(), ()>>(
        "inst_warmup",
        Function::from(|_| {
            let state = get_state();
            let config = state.config.read().clone();
            let _ = tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(instruction::send_instruction_warmup(&config));
        }),
    );

    // Ring buffer functions
    functions.insert::<&str, Function<(Vec<String>, bool, bool), ()>>(
        "ring_pick_chunk",
        Function::from(|(lines, no_mod, do_evict): (Vec<String>, bool, bool)| {
            ring_pick_chunk(lines, no_mod, do_evict)
        }),
    );

    functions.insert::<&str, Function<(), ()>>("ring_update", Function::from(|_| ring_update()));

    functions.insert::<&str, Function<(), Vec<Dictionary>>>(
        "ring_get_extra",
        Function::from(|_| ring_get_extra()),
    );

    // Timer-based ring buffer processing
    functions.insert::<&str, Function<(), ()>>(
        "process_ring_buffer",
        Function::from(|_| {
            let _ = process_ring_buffer();
        }),
    );

    functions.insert::<&str, Function<(), ()>>(
        "on_text_yank_post",
        Function::from(|_| {
            let _ = on_text_yank_post();
        }),
    );

    functions.insert::<&str, Function<(), ()>>(
        "on_buf_enter_and_check_filetype",
        Function::from(|_| {
            let _ = on_buf_enter_and_check_filetype();
        }),
    );

    functions.insert::<&str, Function<(), ()>>(
        "on_buf_write_post",
        Function::from(|_| {
            let _ = on_buf_write_post();
        }),
    );

    functions.insert::<&str, Function<(), ()>>(
        "on_buf_leave",
        Function::from(|_| {
            let _ = on_buf_leave();
        }),
    );

    functions.insert::<&str, Function<(), ()>>(
        "on_cursor_moved_i",
        Function::from(|_| {
            let _ = on_cursor_moved_i();
        }),
    );

    // Cache functions
    functions.insert::<&str, Function<(String, String), ()>>(
        "cache_insert",
        Function::from(|(key, value): (String, String)| cache_insert(&key, &value)),
    );

    functions.insert::<&str, Function<String, Option<String>>>(
        "cache_get",
        Function::from(|key: String| cache_get(&key)),
    );

    // Debug functions
    functions.insert::<&str, Function<(String, Vec<String>), ()>>(
        "debug_log",
        Function::from(|(msg, details): (String, Vec<String>)| {
            debug_log(&msg, details.iter().map(|s| s.as_str()).collect::<Vec<_>>())
        }),
    );

    functions
        .insert::<&str, Function<(), bool>>("debug_toggle", Function::from(|_| debug_toggle()));

    functions.insert::<&str, Function<(), ()>>("debug_clear", Function::from(|_| debug_clear()));

    functions.insert::<&str, Function<(), Vec<String>>>(
        "debug_get_log",
        Function::from(|_| debug_get_log()),
    );

    // Utility functions
    functions
        .insert::<&str, Function<(), String>>("get_filetype", Function::from(|_| get_filetype()));

    functions.insert::<&str, Function<(), bool>>(
        "is_filetype_enabled",
        Function::from(|_| is_filetype_enabled()),
    );

    // Plugin lifecycle management
    functions
        .insert::<&str, Function<(), ()>>("enable_plugin", Function::from(|_| enable_plugin()));

    functions
        .insert::<&str, Function<(), ()>>("disable_plugin", Function::from(|_| disable_plugin()));

    functions
        .insert::<&str, Function<(), bool>>("toggle_plugin", Function::from(|_| toggle_plugin()));

    functions.insert::<&str, Function<(), bool>>(
        "toggle_auto_fim",
        Function::from(|_| toggle_auto_fim()),
    );

    // FIM state query
    functions.insert::<&str, Function<(), bool>>(
        "is_fim_hint_shown",
        Function::from(|_| {
            let state = get_state();
            let fim_state_lock = state.fim_state.read();
            fim_state_lock.hint_shown
        }),
    );

    Ok(functions)
}

/// Check if FIM hint is shown - internal helper for commands
fn fim_is_hint_shown() -> Result<bool, nvim_oxi::Error> {
    let state = get_state();
    let fim_state_lock = state.fim_state.read();
    Ok(fim_state_lock.hint_shown)
}

fn lttw_setup() -> NvimResult<()> {
    // Initialize plugin state
    init_state();

    // Register nvim-oxi commands
    register_commands()?;

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

/// FIM completion function
fn fim_completion(is_auto: bool) -> NvimResult<Option<String>> {
    let (pos_x, pos_y) = get_pos();
    let lines = buf_get_lines();

    // Check if we should trigger speculative FIM after showing a cached hint
    let state = get_state();

    // Check if there's a displayed hint that needs speculative follow-up
    let speculative_fim = {
        let fim_state_lock = state.fim_state.read();
        fim_state_lock.hint_shown && !fim_state_lock.content.is_empty()
    };

    if speculative_fim {
        let prev_content = {
            let fim_state_lock = state.fim_state.read();
            fim_state_lock.content.clone()
        };

        let (config, debug_manager) = {
            let config_lock = state.config.read();
            let debug_lock = state.debug_manager.read();
            (config_lock.clone(), debug_lock.clone())
        };

        let result = tokio::runtime::Runtime::new().unwrap().block_on(async {
            // Trigger speculative FIM with previous content as prev parameter
            let result = fim::fim_completion(
                debug_manager,
                pos_x,
                pos_y,
                false, // Not auto - use longer timeout
                &lines,
                &config,
                state.cache.clone(),
                state.ring_buffer.clone(),
                Some(&prev_content),
            )
            .await;

            // If we got a new suggestion, render and display it
            if let Ok(Some(ref content)) = result {
                // Parse response and render
                let ctx = context::get_local_context(&lines, pos_x, pos_y, None, &config);
                let rendered = fim::render_fim_suggestion(
                    pos_x,
                    pos_y,
                    content,
                    &ctx.line_cur_suffix,
                    &config,
                );

                // Update FIM state
                {
                    let mut fim_state_lock = state.fim_state.write();
                    fim_state_lock.hint_shown = rendered.can_accept;
                    fim_state_lock.pos_x = pos_x;
                    fim_state_lock.pos_y = pos_y;
                    fim_state_lock.line_cur = lines.get(pos_y).cloned().unwrap_or_default();
                    fim_state_lock.can_accept = rendered.can_accept;
                    fim_state_lock.content = rendered.content;
                }

                // Display the virtual text using extmarks
                let _ = display_fim_hint(&state);
            }

            result
        });

        return result.map_err(|e| nvim_oxi::Error::Api(api::Error::Other(e.to_string())));
    }

    // Normal FIM (no speculative hint)
    let (config, debug_manager) = {
        let config_lock = state.config.read();
        let debug_lock = state.debug_manager.read();
        (config_lock.clone(), debug_lock.clone())
    };

    let result = tokio::runtime::Runtime::new().unwrap().block_on(async {
        let result = fim::fim_completion(
            debug_manager,
            pos_x,
            pos_y,
            is_auto,
            &lines,
            &config,
            state.cache.clone(),
            state.ring_buffer.clone(),
            None,
        )
        .await;

        // If we got a suggestion from server, display it
        if let Ok(Some(ref content)) = result {
            // Parse response and render
            if let Ok(response) = serde_json::from_str::<serde_json::Value>(content) {
                if let Some(content_str) = response.get("content").and_then(|c| c.as_str()) {
                    let ctx = context::get_local_context(&lines, pos_x, pos_y, None, &config);
                    let rendered = fim::render_fim_suggestion(
                        pos_x,
                        pos_y,
                        content_str,
                        &ctx.line_cur_suffix,
                        &config,
                    );

                    // Update FIM state
                    {
                        let mut fim_state_lock = state.fim_state.write();
                        fim_state_lock.hint_shown = rendered.can_accept;
                        fim_state_lock.pos_x = pos_x;
                        fim_state_lock.pos_y = pos_y;
                        fim_state_lock.line_cur = lines.get(pos_y).cloned().unwrap_or_default();
                        fim_state_lock.can_accept = rendered.can_accept;
                        fim_state_lock.content = rendered.content;
                    }

                    // Display virtual text using extmarks
                    if rendered.can_accept {
                        let _ = display_fim_hint(&state);
                    }

                    // Return the original content string
                    return Ok(Some(content_str.to_string()));
                }
            } else {
                // JSON string content (from speculative FIM)
                let ctx = context::get_local_context(&lines, pos_x, pos_y, None, &config);
                let rendered = fim::render_fim_suggestion(
                    pos_x,
                    pos_y,
                    content,
                    &ctx.line_cur_suffix,
                    &config,
                );

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
                }

                return Ok(Some(content.clone()));
            }
        }

        result
    });

    result.map_err(|e| nvim_oxi::Error::Api(api::Error::Other(e.to_string())))
}

/// FIM accept function - accepts the FIM suggestion
fn fim_accept(accept_type: &str) -> NvimResult<Option<String>> {
    let state = get_state();

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
        pos_y..end_line,
        true,
        all_lines_modified[pos_y..end_line].to_vec(),
    )?;

    // Move the cursor to the end of the accepted text
    let new_col = new_line.len();
    let mut window = Window::current();
    let _ = window.set_cursor(pos_y, new_col);

    // Clear the FIM hint - use write lock
    {
        let mut fim_state_lock = state.fim_state.write();
        fim_state_lock.hint_shown = false;
        fim_state_lock.content.clear();
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
    let (hint_shown, content, extmark_ns, pos_y, pos_x, config, debug_manager) = {
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

    if let Some(ns_id) = extmark_ns {
        let mut buf = Buffer::current();

        // Build virtual text string - first line of suggestion
        let suggestion_text = content[0].clone();

        // Build inline info string if show_info is enabled (mode 2 = inline)
        let virt_text_vec: Vec<(String, String)> = {
            if config.show_info == 2 {
                // Display suggestion with inline stats/info
                vec![
                    (suggestion_text.clone(), "Comment".to_string()),
                    // Info will be displayed on the next line via virt_lines
                ]
            } else {
                // Just display suggestion without info
                vec![(suggestion_text, "Comment".to_string())]
            }
        };

        // Create extmark opts with virtual text using builder pattern
        let mut opts = SetExtmarkOptsBuilder::default();
        opts.virt_text(virt_text_vec);
        opts.virt_text_pos(nvim_oxi::api::types::ExtmarkVirtTextPosition::Inline);

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
        match buf.set_extmark(ns_id, pos_y, pos_x, &opts.build()) {
            Ok(_id) => {
                debug_manager.log(
                    "display_fim_hint",
                    &[&format!("Set extmark at line {}, col {}", pos_y, pos_x)],
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

/// FIM try hint function - tries to show a hint from cache
fn fim_try_hint() -> NvimResult<Option<String>> {
    let state = get_state();
    let (pos_x, pos_y) = get_pos();
    let lines = buf_get_lines();

    // Get local context
    let ctx = {
        let config_lock = state.config.read();
        context::get_local_context(&lines, pos_x, pos_y, None, &config_lock)
    };

    // Compute hash
    let hashes = fim::compute_hashes(&ctx);

    // Check cache for primary hash
    for hash in &hashes {
        if let Some(response_text) = {
            let cache_lock = state.cache.read();
            cache_lock.get_fim(hash)
        } {
            state.debug_manager.read().log(
                "fim_try_hint",
                &[&format!("Found cached completion for hash {}", &hash[..16])],
            );

            // Parse response and render
            if let Ok(response) = serde_json::from_str::<serde_json::Value>(&response_text) {
                if let Some(content) = response.get("content").and_then(|c| c.as_str()) {
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
                            "fim_try_hint",
                            &[&format!(
                                "Showing FIM hint: {} lines",
                                rendered.content.len()
                            )],
                        );

                        // Trigger speculative FIM in background
                        // Use cloned data to avoid borrow conflicts
                        let dbg = state.debug_manager.read().clone();
                        let speculative_config = state.config.clone();
                        let speculative_cache = state.cache.clone();
                        let speculative_ring = state.ring_buffer.clone();
                        let speculative_lines = lines.clone();
                        let speculative_content = {
                            let fim_state_lock = state.fim_state.read();
                            fim_state_lock.content.clone()
                        };

                        // Spawn speculative FIM task
                        tokio::runtime::Runtime::new().unwrap().spawn(async move {
                            let pos_x = pos_x;
                            let pos_y = pos_y;

                            // Clone config to avoid holding lock across await point
                            let speculative_config_val = {
                                let config_lock = speculative_config.read();
                                config_lock.clone()
                            };

                            let result = tokio::runtime::Runtime::new().unwrap().block_on(async {
                                fim::fim_completion(
                                    dbg,
                                    pos_x,
                                    pos_y,
                                    true, // is_auto - shorter timeout
                                    &speculative_lines,
                                    &speculative_config_val,
                                    speculative_cache,
                                    speculative_ring,
                                    Some(&speculative_content),
                                )
                                .await
                            });

                            // Update state with result (if needed)
                            if let Ok(Some(_content)) = result {
                                // Result is cached by fim_completion
                                // Note: We can't easily update the main state from a spawned task
                                // In a real implementation, we'd use channels or a shared state with proper locking
                            }
                        });

                        return Ok(Some("hint_shown".to_string()));
                    }
                }
            }
        }
    }

    // Also check nearby completions (cursor moved slightly)
    let pm = format!("{}{}", ctx.prefix, ctx.middle);
    for i in 0..std::cmp::min(128, pm.len()) {
        if pm.len() < 2 + i {
            break;
        }
        let removed = &pm[pm.len() - (1 + i)..];
        let ctx_new = format!("{}{}", &pm[..pm.len() - (2 + i)], ctx.suffix);
        let hash_new = utils::sha256(&ctx_new);

        if let Some(response_text) = {
            let cache_lock = state.cache.read();
            cache_lock.get_fim(&hash_new)
        } {
            if let Ok(response) = serde_json::from_str::<serde_json::Value>(&response_text) {
                if let Some(content_str) = response.get("content").and_then(|c| c.as_str()) {
                    if content_str.starts_with(removed) {
                        let new_content = &content_str[i + 1..];
                        if !new_content.is_empty() {
                            state.debug_manager.read().log(
                                "fim_try_hint_nearby",
                                &[&format!("Found nearby completion at offset {}", i)],
                            );

                            let mut new_response = response.clone();
                            new_response["content"] =
                                serde_json::Value::String(new_content.to_string());
                            let response_text =
                                serde_json::to_string(&new_response).unwrap_or_default();

                            if let Ok(response) =
                                serde_json::from_str::<serde_json::Value>(&response_text)
                            {
                                if let Some(content) =
                                    response.get("content").and_then(|c| c.as_str())
                                {
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

                                    {
                                        let mut fim_state_lock = state.fim_state.write();
                                        fim_state_lock.hint_shown = rendered.can_accept;
                                        fim_state_lock.pos_x = pos_x;
                                        fim_state_lock.pos_y = pos_y;
                                        fim_state_lock.line_cur =
                                            lines.get(pos_y).cloned().unwrap_or_default();
                                        fim_state_lock.can_accept = rendered.can_accept;
                                        fim_state_lock.content = rendered.content.clone();
                                    }

                                    if rendered.can_accept {
                                        let _ = display_fim_hint(&state);
                                        return Ok(Some("hint_shown_nearby".to_string()));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(None)
}

/// Instruction start function - creates a new instruction request with visual markers
fn inst_start(l0: i64, l1: i64, inst: &str) -> NvimResult<i64> {
    let state = get_state();
    let bufnr = get_current_buffer();
    let lines = buf_get_lines();

    // Create new instruction request
    let req_id = state
        .next_inst_req_id
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

    let mut req =
        InstructionRequestState::new(req_id, bufnr, (l0 as usize, l1 as usize), inst.to_string());

    // Set namespace for extmarks
    req.ns_id = state.inst_ns;

    // Add visual marker at the end of the range
    if let Some(ns_id) = req.ns_id {
        let mut buf = Buffer::current();

        // Create extmark at end of range to show instruction status
        let opts = nvim_oxi::api::opts::SetExtmarkOptsBuilder::default()
            .virt_text(vec![(
                format!("[Instr: {}]", inst),
                "llama_hl_inst_virt_proc".to_string(),
            )])
            .virt_text_pos(nvim_oxi::api::types::ExtmarkVirtTextPosition::Eol)
            .build();

        match buf.set_extmark(ns_id, l1 as usize, 0, &opts) {
            Ok(id) => {
                req.extmark_id = Some(id);
                state.debug_manager.read().log(
                    "inst_start",
                    &[&format!(
                        "Created extmark {} for instruction {}",
                        id, req_id
                    )],
                );
            }
            Err(e) => {
                state.debug_manager.read().log(
                    "inst_start",
                    &[&format!("Failed to create extmark: {:?}", e)],
                );
            }
        }
    }

    // Build messages for server request
    let messages = instruction::build_instruction_payload(
        &lines,
        l0 as usize,
        l1 as usize,
        inst,
        &state.config.read(),
    );

    req.inst_prev = messages;

    // Store request
    {
        let mut instruction_requests_lock = state.instruction_requests.write();
        instruction_requests_lock.insert(req_id, req);
    }

    state.debug_manager.read().log(
        "inst_start",
        &[&format!(
            "Started instruction {} at range ({}, {})",
            req_id, l0, l1
        )],
    );

    Ok(req_id)
}

/// Instruction build function - builds payload without starting request
fn inst_build(lines: Vec<String>, l0: i64, l1: i64, inst: &str) -> NvimResult<Dictionary> {
    let state = get_state();
    let messages = instruction::build_instruction_payload(
        &lines,
        l0 as usize,
        l1 as usize,
        inst,
        &state.config.read(),
    );

    let mut result = Dictionary::new();
    let mut messages_dict = Vec::new();

    for msg in messages {
        let mut msg_dict = Dictionary::new();
        msg_dict.insert("role", msg.role);
        msg_dict.insert("content", msg.content);
        messages_dict.push(msg_dict);
    }

    let messages_array: nvim_oxi::Array = messages_dict.into_iter().collect();
    result.insert("messages", messages_array);
    Ok(result)
}

/// Instruction send function - sends request and streams response
#[allow(clippy::await_holding_lock)] // Uses state access within block_on for async call
fn inst_send(req_id: i64) -> NvimResult<Option<String>> {
    let state = get_state();

    // Get the request
    let (_req, messages, debug_manager, config) = {
        let instruction_requests_lock = state.instruction_requests.read();
        let r = match instruction_requests_lock.get(&req_id) {
            Some(r) => r,
            None => {
                let debug_manager = state.debug_manager.read().clone();
                debug_manager.log("inst_send", &[&format!("Request {} not found", req_id)]);
                return Ok(None);
            }
        };

        (
            r.clone(),
            r.inst_prev.clone(),
            state.debug_manager.read().clone(),
            state.config.read().clone(),
        )
    };

    debug_manager.log(
        "inst_send",
        &[&format!(
            "Sending instruction request {} with {} messages",
            req_id,
            messages.len()
        )],
    );

    // Send request asynchronously
    let result = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async { instruction::send_instruction_stream(&messages, &config, req_id).await });

    match result {
        Ok(response) => {
            // Process streaming response
            let req_id_clone = req_id;

            // Spawn a task to process the stream
            tokio::runtime::Runtime::new().unwrap().spawn(async move {
                // Read the response body
                let body = match response.text().await {
                    Ok(b) => b,
                    Err(e) => {
                        let _ = inst_update(req_id_clone, &format!("Error: {}", e));
                        return;
                    }
                };

                // Process SSE stream and update state
                for line in body.lines() {
                    if let Ok(updated_content) = inst_update(req_id_clone, line) {
                        // Content has been updated
                        let _ = updated_content;
                    }
                }

                // Mark as finalized
                let _ = inst_finalize(req_id_clone);
            });

            // Update request status
            {
                let mut instruction_requests_lock = state.instruction_requests.write();
                if let Some(req) = instruction_requests_lock.get_mut(&req_id) {
                    req.status = InstructionStatus::Generating;
                }
            }
            inst_update_virt_text(req_id)?;
            Ok(Some("streaming".to_string()))
        }
        Err(e) => {
            // Log the error
            let debug_manager = state.debug_manager.read().clone();
            debug_manager.log("inst_send", &[&format!("Error: {:?}", e)]);

            // Update request status
            {
                let mut instruction_requests_lock = state.instruction_requests.write();
                if let Some(req) = instruction_requests_lock.get_mut(&req_id) {
                    req.status = InstructionStatus::Error(e.to_string());
                }
            }
            inst_update_virt_text(req_id)?;
            Ok(None)
        }
    }
}

/// Update virtual text for instruction request
fn inst_update_virt_text(req_id: i64) -> NvimResult<()> {
    let state = get_state();

    // Get request info first, then release borrow for logging
    let (ns_id, extmark_id, range_1, virt_text) = {
        let instruction_requests_lock = state.instruction_requests.read();
        match instruction_requests_lock.get(&req_id) {
            Some(r) => {
                if let Some(ns_id) = r.ns_id {
                    if let Some(_extmark_id) = r.extmark_id {
                        let virt_text = instruction::build_instruction_virt_text(r, 50);
                        (ns_id, r.extmark_id, r.range.1, virt_text)
                    } else {
                        return Ok(());
                    }
                } else {
                    return Ok(());
                }
            }
            None => return Ok(()),
        }
    };

    let mut buf = Buffer::current();

    // Clear old extmark
    if let Some(old_id) = extmark_id {
        let _ = buf.del_extmark(ns_id, old_id);
    }

    // Create new extmark with updated status
    let opts = nvim_oxi::api::opts::SetExtmarkOptsBuilder::default()
        .virt_text(virt_text)
        .virt_text_pos(nvim_oxi::api::types::ExtmarkVirtTextPosition::Eol)
        .build();

    match buf.set_extmark(ns_id, range_1, 0, &opts) {
        Ok(new_id) => {
            // Update the request with new extmark id
            let mut instruction_requests_lock = state.instruction_requests.write();
            if let Some(req) = instruction_requests_lock.get_mut(&req_id) {
                req.extmark_id = Some(new_id);
            }
            // Log after releasing borrow
        }
        Err(_e) => {
            // Error case - nothing to log here
        }
    }

    Ok(())
}

/// Instruction update function - processes streaming response chunk and updates state
fn inst_update(req_id: i64, response_chunk: &str) -> NvimResult<String> {
    let state = get_state();

    // Get the request and accumulate response
    let new_content = {
        let mut instruction_requests_lock = state.instruction_requests.write();
        if let Some(req) = instruction_requests_lock.get_mut(&req_id) {
            // Parse the SSE chunk and extract content
            let new_content = instruction::process_streaming_response(response_chunk, &req.result);

            req.result = new_content.clone();
            req.n_gen += 1;
            req.status = InstructionStatus::Generating;

            new_content
        } else {
            let debug_manager = state.debug_manager.read().clone();
            debug_manager.log(
                "inst_update",
                &[&format!(
                    "Request {} not found for streaming update",
                    req_id
                )],
            );
            return Ok(String::new());
        }
    };

    // Update virtual text to show new content
    inst_update_virt_text(req_id)?;

    Ok(new_content)
}

/// Instruction finalize function - marks request as ready after streaming completes
fn inst_finalize(req_id: i64) -> NvimResult<()> {
    let state = get_state();

    let result_len = {
        let mut instruction_requests_lock = state.instruction_requests.write();
        if let Some(req) = instruction_requests_lock.get_mut(&req_id) {
            req.status = InstructionStatus::Ready;
            req.result.len()
        } else {
            let debug_manager = state.debug_manager.read().clone();
            debug_manager.log(
                "inst_finalize",
                &[&format!("Request {} not found for finalize", req_id)],
            );
            return Ok(());
        }
    };

    // Log after updating state
    {
        let state = get_state();
        state.debug_manager.read().log(
            "inst_finalize",
            &[&format!(
                "Request {} finalized with {} chars",
                req_id, result_len
            )],
        );
    }

    // Update virtual text to show ready status
    inst_update_virt_text(req_id)?;

    Ok(())
}

/// Instruction accept function - applies the generated result to the buffer
fn inst_accept() -> NvimResult<()> {
    let state = get_state();
    let bufnr = get_current_buffer();

    // Find instruction request for current buffer (prioritize Ready status)
    let (req_id_to_accept, req) = {
        let mut instruction_requests_lock = state.instruction_requests.write();
        let req_to_accept = instruction_requests_lock
            .iter()
            .find(|(_, req)| {
                req.bufnr == bufnr
                    && (req.status == InstructionStatus::Ready
                        || req.status == InstructionStatus::Generating)
            })
            .map(|(id, req)| (*id, req.clone()));

        if let Some((req_id, req)) = req_to_accept {
            instruction_requests_lock.remove(&req_id);
            (Some(req_id), Some(req))
        } else {
            (None, None)
        }
    };

    if let Some(req_id) = req_id_to_accept {
        if let Some(req) = req {
            if req.result.is_empty() {
                state.debug_manager.read().log(
                    "inst_accept",
                    &[&format!(
                        "Request {} has empty result, skipping apply",
                        req_id
                    )],
                );
                // Still clean up the visual marker
                if let Some(ns_id) = req.ns_id {
                    if let Some(extmark_id) = req.extmark_id {
                        let mut buf = Buffer::current();
                        let _ = buf.del_extmark(ns_id, extmark_id);
                    }
                }
                return Ok(());
            }

            let result_lines: Vec<String> = req.result.split('\n').map(|s| s.to_string()).collect();
            let (l0, l1) = req.range;

            state.debug_manager.read().log(
                "inst_accept",
                &[&format!(
                    "Applying {} lines to buffer {} at range ({}, {})",
                    result_lines.len(),
                    bufnr,
                    l0,
                    l1
                )],
            );

            // Apply the result to the buffer using current buffer (assuming we're on the right buffer)
            let mut buf = Buffer::current();

            // Delete the original range and insert new lines in one operation
            // set_lines replaces lines in range [start, end) with new lines
            match buf.set_lines(l0..(l1 + 1), true, result_lines) {
                Ok(_) => {
                    let state = get_state();
                    state.debug_manager.read().log(
                        "inst_accept",
                        &["Successfully applied instruction result to buffer"],
                    );
                }
                Err(e) => {
                    let state = get_state();
                    state.debug_manager.read().log(
                        "inst_accept",
                        &[&format!("Failed to set buffer lines: {:?}", e)],
                    );
                }
            }

            // Clear the visual marker from the original location
            if req.ns_id.is_some() && req.extmark_id.is_some() {
                let mut buf = Buffer::current();
                if let (Some(ns_id), Some(extmark_id)) = (req.ns_id, req.extmark_id) {
                    let _ = buf.del_extmark(ns_id, extmark_id);
                }
            }

            return Ok(());
        }
    }

    state.debug_manager.read().log(
        "inst_accept",
        &["No ready instruction request found for current buffer"],
    );

    Ok(())
}

/// Instruction cancel function - cancels an instruction request and removes markers
fn inst_cancel() -> NvimResult<()> {
    let state = get_state();
    let bufnr = get_current_buffer();
    let (_, pos_y) = get_pos();

    // Find and cancel the instruction request at the current line
    let (req_id_to_cancel, req) = {
        let mut instruction_requests_lock = state.instruction_requests.write();
        let req_to_cancel = instruction_requests_lock
            .iter()
            .find(|(_, req)| req.bufnr == bufnr && pos_y >= req.range.0 && pos_y <= req.range.1)
            .map(|(id, req)| (*id, req.clone()));

        if let Some((req_id, req)) = req_to_cancel {
            instruction_requests_lock.remove(&req_id);
            (Some(req_id), Some(req))
        } else {
            (None, None)
        }
    };

    if let Some(req_id) = req_id_to_cancel {
        if let Some(req) = req {
            state
                .debug_manager
                .read()
                .log("inst_cancel", &[&format!("Cancelling request {}", req_id)]);

            // Delete the visual marker
            if let Some(ns_id) = req.ns_id {
                if let Some(extmark_id) = req.extmark_id {
                    let mut buf = Buffer::current();
                    match buf.del_extmark(ns_id, extmark_id) {
                        Ok(_) => {
                            let state = get_state();
                            state.debug_manager.read().log(
                                "inst_cancel",
                                &[&format!("Deleted extmark for request {}", req_id)],
                            );
                        }
                        Err(e) => {
                            let state = get_state();
                            state.debug_manager.read().log(
                                "inst_cancel",
                                &[&format!("Failed to delete extmark: {:?}", e)],
                            );
                        }
                    }
                }
            }

            return Ok(());
        }
    }

    Ok(())
}

/// Instruction rerun function - re-runs the last instruction
fn inst_rerun() -> NvimResult<Option<String>> {
    let state = get_state();
    let bufnr = get_current_buffer();
    let (_, pos_y) = get_pos();

    // Find the instruction request at the current line
    let req_id_to_rerun = {
        let instruction_requests_lock = state.instruction_requests.read();
        instruction_requests_lock
            .iter()
            .find(|(_, req)| {
                req.bufnr == bufnr
                    && pos_y >= req.range.0
                    && pos_y <= req.range.1
                    && req.status == InstructionStatus::Ready
            })
            .map(|(id, _)| *id)
    };

    if let Some(req_id) = req_id_to_rerun {
        let mut instruction_requests_lock = state.instruction_requests.write();
        if let Some(req) = instruction_requests_lock.get_mut(&req_id) {
            // Reset status and result
            req.status = InstructionStatus::Processing;
            req.result.clear();
            req.n_gen = 0;

            // Remove the last assistant message from inst_prev
            if let Some(pos) = req.inst_prev.iter().position(|m| m.role == "assistant") {
                req.inst_prev.remove(pos);
            }
        }

        state
            .debug_manager
            .read()
            .log("inst_rerun", &[&format!("Re-running request {}", req_id)]);
        return Ok(Some(format!("Re-running request {}", req_id)));
    }

    Ok(None)
}

/// Instruction continue function - continues with a new instruction
fn inst_continue() -> NvimResult<Option<String>> {
    let state = get_state();
    let bufnr = get_current_buffer();
    let (_, pos_y) = get_pos();

    // Find the instruction request at the current line
    let req_id_to_continue = {
        let instruction_requests_lock = state.instruction_requests.read();
        instruction_requests_lock
            .iter()
            .find(|(_, req)| {
                req.bufnr == bufnr
                    && pos_y >= req.range.0
                    && pos_y <= req.range.1
                    && req.status == InstructionStatus::Ready
            })
            .map(|(id, _)| *id)
    };

    if let Some(req_id) = req_id_to_continue {
        let mut instruction_requests_lock = state.instruction_requests.write();
        if let Some(req) = instruction_requests_lock.get_mut(&req_id) {
            // Reset for continuation
            req.status = InstructionStatus::Processing;
            req.result.clear();
            req.n_gen = 0;
        }

        state.debug_manager.read().log(
            "inst_continue",
            &[&format!("Continuing request {}", req_id)],
        );
        return Ok(Some(format!("Continuing request {}", req_id)));
    }

    Ok(None)
}

/// Ring buffer pick chunk function
fn ring_pick_chunk(lines: Vec<String>, no_mod: bool, do_evict: bool) -> NvimResult<()> {
    let state = get_state();
    let mut ring_buffer_lock = state.ring_buffer.write();
    ring_buffer_lock.pick_chunk(lines, no_mod, do_evict);
    Ok(())
}

/// Ring buffer update function
fn ring_update() -> NvimResult<()> {
    let state = get_state();
    let mut ring_buffer_lock = state.ring_buffer.write();
    ring_buffer_lock.update();
    Ok(())
}

/// Cache insert function
fn cache_insert(key: &str, value: &str) -> NvimResult<()> {
    let state = get_state();
    let mut cache_lock = state.cache.write();
    cache_lock.insert(key.to_string(), value.to_string());
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
    let mut debug_manager_lock = state.debug_manager.write();
    debug_manager_lock.clear();
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
        "<C-O>:LttwFimAcceptFullOrTab<CR>",
        &Default::default(),
    );

    // Note: ESC is not mapped in Insert mode to avoid interfering with normal ESC behavior
    // ESC will naturally exit Insert mode. If FIM hint is shown, it will be hidden when
    // the user presses ESC to exit Insert mode (handled by fim_hide_on_escape autocmd if needed)

    // FIM accept line (S-Tab) - check if FIM shown, accept line if yes, re-inject S-Tab if no
    let _ = api::set_keymap(
        Mode::Insert,
        "<S-Tab>",
        "<C-O>:LttwFimAcceptLineOrSTab<CR>",
        &Default::default(),
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
    if state.enabled.load(std::sync::atomic::Ordering::SeqCst) {
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
    state
        .enabled
        .store(true, std::sync::atomic::Ordering::SeqCst);

    Ok(())
}

/// Disable the plugin - removes keymaps, clears autocmds, and hides hints
fn disable_plugin() -> NvimResult<()> {
    let state = get_state();

    // Check if already disabled
    if !state.enabled.load(std::sync::atomic::Ordering::SeqCst) {
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
    state
        .enabled
        .store(false, std::sync::atomic::Ordering::SeqCst);

    Ok(())
}

/// Toggle the plugin on/off
fn toggle_plugin() -> NvimResult<bool> {
    let state = get_state();
    let currently_enabled = state.enabled.load(std::sync::atomic::Ordering::SeqCst);
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

/// Handle CursorMovedI event - trigger speculative FIM completion
fn on_cursor_moved_i() -> NvimResult<()> {
    let state = get_state();
    state.debug_manager.read().log(
        "on_cursor_moved_i",
        &[&format!(
            "state.enabled {}, state.config.auto_fim {}",
            state.enabled.load(std::sync::atomic::Ordering::SeqCst),
            state.config.read().auto_fim
        )],
    );

    // Check if FIM is enabled and auto_fim is true
    if !state.enabled.load(std::sync::atomic::Ordering::SeqCst) || !state.config.read().auto_fim {
        return Ok(());
    }

    // Get CURRENT cursor position
    let (pos_x, pos_y) = get_pos();
    let lines = buf_get_lines();

    state.debug_manager.read().log(
        "on_cursor_moved_i",
        &[&format!(
            "Cursor moved in insert mode at ({}, {})",
            pos_x, pos_y
        )],
    );

    state.debug_manager.read().log("on_cursor_moved_i 1", &[]);

    // Try to show a cached hint
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

            // Parse response and render
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

    // If no cached hint found and we're not already showing a hint, try normal FIM
    {
        let fim_state_lock = state.fim_state.read();
        if !found_cached && !fim_state_lock.hint_shown {
            drop(fim_state_lock);
            // Only trigger FIM if we're in a reasonable position
            state.debug_manager.read().log(
                "on_cursor_moved_i 4.1",
                &[&format!(
                    "pos_y {pos_y}, pos_x {pos_x}, lines_len: {}",
                    lines.len()
                )],
            );
            if pos_y < lines.len() && pos_x <= lines.get(pos_y).map(|l| l.len()).unwrap_or(0) {
                // Use the synchronous fim_completion wrapper
                state
                    .debug_manager
                    .read()
                    .log("on_cursor_moved_i 4.21", &[]);
                let result = fim_completion(true); // is_auto = true

                // If we got a suggestion from server, display it
                if let Ok(Some(ref content)) = result {
                    state.debug_manager.read().log("on_cursor_moved_i 4.3", &[]);
                    // Parse response and render
                    if let Ok(response) = serde_json::from_str::<serde_json::Value>(content) {
                        state.debug_manager.read().log("on_cursor_moved_i 4.4", &[]);
                        if let Some(content_str) = response.get("content").and_then(|c| c.as_str())
                        {
                            state.debug_manager.read().log("on_cursor_moved_i 4.5", &[]);
                            let ctx = {
                                let config_lock = state.config.read();
                                context::get_local_context(&lines, pos_x, pos_y, None, &config_lock)
                            };
                            let rendered = {
                                let config_lock = state.config.read();
                                fim::render_fim_suggestion(
                                    pos_x,
                                    pos_y,
                                    content_str,
                                    &ctx.line_cur_suffix,
                                    &config_lock,
                                )
                            };
                            state.debug_manager.read().log("on_cursor_moved_i 4.6", &[]);

                            // Update FIM state
                            {
                                let mut fim_state_lock = state.fim_state.write();
                                fim_state_lock.hint_shown = rendered.can_accept;
                                fim_state_lock.pos_x = pos_x;
                                fim_state_lock.pos_y = pos_y;
                                fim_state_lock.line_cur =
                                    lines.get(pos_y).cloned().unwrap_or_default();
                                fim_state_lock.can_accept = rendered.can_accept;
                                fim_state_lock.content = rendered.content;
                            }

                            state.debug_manager.read().log("on_cursor_moved_i 4.7", &[]);
                            // Display virtual text using extmarks
                            let _ = display_fim_hint(&state);
                            state.debug_manager.read().log("on_cursor_moved_i 4.8", &[]);
                        }
                    }
                }
            }
        }
    }
    let state = get_state();
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

    // Move first queued chunk to ring
    let mut ring_buffer_lock = state.ring_buffer.write();
    ring_buffer_lock.update();

    // Check if we have chunks before logging
    let chunk_count = ring_buffer_lock.len();

    if chunk_count > 0 {
        state.debug_manager.read().log(
            "process_ring_buffer",
            &[&format!(
                "Processing {} ring buffer chunks (interval: {}ms)",
                chunk_count, update_interval
            )],
        );

        // Build request with ring buffer context
        let extra = ring_buffer_lock.get_extra();
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
            ["CursorMovedI"],
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
    let mut autocmd_ids_lock = state.autocmd_ids.write();
    autocmd_ids_lock.push(id as u64);

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
    let mut autocmd_ids_lock = state.autocmd_ids.write();
    autocmd_ids_lock.push(id as u64);

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
    let mut autocmd_ids_lock = state.autocmd_ids.write();
    autocmd_ids_lock.push(id as u64);

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
    let mut autocmd_ids_lock = state.autocmd_ids.write();
    autocmd_ids_lock.push(id as u64);

    // InsertLeavePre - hide FIM hint when leaving Insert mode
    let id = api::create_autocmd(
        ["InsertLeavePre"],
        &nvim_oxi::api::opts::CreateAutocmdOptsBuilder::default()
            .callback(|_| {
                let _ = fim_hide();
                false // DO NOT DELETE this autocommand once used
            })
            .build(),
    )
    .unwrap_or(0);
    let mut autocmd_ids_lock = state.autocmd_ids.write();
    autocmd_ids_lock.push(id as u64);

    // Setup timer-based ring buffer updates (every ring_update_ms)
    setup_ring_buffer_timer()?;

    Ok(())
}

/// Filetype check autocmd handler - enables/disables plugin based on filetype
fn on_buf_enter_and_check_filetype() -> NvimResult<()> {
    let state = get_state();
    let is_enabled = state.enabled.load(std::sync::atomic::Ordering::SeqCst);
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
    let _ = api::create_user_command(
        "LttwFim",
        |_| -> NvimResult<()> {
            fim_completion(false)?;
            Ok(())
        },
        &Default::default(),
    );

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

    // FIM accept full or insert tab - for TAB key handling
    let _ = api::create_user_command(
        "LttwFimAcceptFullOrTab",
        |_| -> NvimResult<()> {
            if let Ok(true) = fim_is_hint_shown() {
                let _ = fim_accept("full");
            } else {
                // Insert tab character by calling vim.feedkeys
                let _ =
                    api::call_function::<(&str, &str, bool), ()>("feedkeys", ("\t", "i", false));
            }
            Ok(())
        },
        &Default::default(),
    );

    // Note: LttwFimCancelOrEsc command removed - ESC is no longer mapped in Insert mode
    // to avoid interfering with normal ESC behavior

    // FIM accept line or re-inject S-Tab - for S-Tab key handling
    let _ = api::create_user_command(
        "LttwFimAcceptLineOrSTab",
        |_| -> NvimResult<()> {
            if let Ok(true) = fim_is_hint_shown() {
                let _ = fim_accept("line");
                // Key is consumed
            } else {
                // Re-inject S-Tab key by calling vim.feedkeys
                // S-Tab is \x1bOP3~ in terminal
                let _ = api::call_function::<(&str, &str, bool), ()>(
                    "feedkeys",
                    ("\x1bOP3~", "n", false),
                );
            }
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
            inst_rerun()?;
            Ok(())
        },
        &Default::default(),
    );

    let _ = api::create_user_command(
        "LttwInstContinue",
        |_| -> NvimResult<()> {
            inst_continue()?;
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
