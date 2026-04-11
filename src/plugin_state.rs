use {
    crate::{
        cache, config, diagnostics::DiagnosticTracker, fim::FimState,
        instruction::InstructionRequestState, ring_buffer, DisplayMessage, Error, FimState,
        LttwResult,
    },
    ahash::{HashMap, HashMapExt},
    nvim_oxi::api::create_namespace,
    papaya::HashMap as PapayaMap,
    parking_lot::{RwLock, RwLockReadGuard},
    std::{
        sync::{
            atomic::{AtomicBool, AtomicI64, AtomicU64},
            Arc, OnceLock,
        },
        time::Instant,
    },
    tokio::{
        runtime::Runtime,
        sync::{mpsc, Semaphore},
    },
};

// Global state - using OnceLock for thread-safe initialization
static PLUGIN_STATE: OnceLock<Arc<PluginState>> = OnceLock::new();

/// Initialize the plugin state
#[tracing::instrument(skip(obj))]
pub fn init_state(obj: nvim_oxi::Object) {
    PLUGIN_STATE.get_or_init(move || Arc::new(PluginState::new(obj)));
}

/// Get the plugin state (returns a clone of the Arc, no locking)
#[tracing::instrument]
pub fn get_state() -> Arc<PluginState> {
    //init_state();
    PLUGIN_STATE.get().unwrap().clone()
}

// State management
#[derive(Clone, Debug)]
pub struct PluginState {
    pub config: Arc<RwLock<config::LttwConfig>>,
    //pub otel_guard: Arc<RwLock<Option<crate::otel::OtelGuard>>>,
    pub cache: Arc<RwLock<cache::Cache>>,
    pub ring_buffer: Arc<RwLock<ring_buffer::RingBuffer>>,
    pub nvim_mode: Arc<RwLock<Vec<u8>>>, // string bytes for the mode name
    pub last_move_time: Arc<RwLock<Instant>>, // (vim s:t_last_move)
    pub instruction_requests: Arc<RwLock<HashMap<i64, InstructionRequestState>>>,
    pub tracing_enabled: Arc<AtomicBool>,
    pub enabled: Arc<AtomicBool>,
    #[allow(dead_code)]
    pub next_inst_req_id: Arc<AtomicI64>,
    pub fim_state: Arc<RwLock<FimState>>,
    pub fim_worker_debounce_seq: Arc<AtomicU64>,
    pub fim_worker_debounce_last_spawn: Arc<RwLock<Instant>>,
    pub fim_worker_semaphore: Arc<tokio::sync::Semaphore>,
    pub fim_worker_generating_for_pos: Arc<RwLock<Option<(u64, usize, usize)>>>,

    pub extmark_ns: Option<u32>, // Namespace for extmarks (virtual text)
    #[allow(dead_code)]
    pub inst_ns: Option<u32>, // Namespace for instruction extmarks
    pub cur_buf_info: Arc<RwLock<CurrentBufferInfo>>, // the current buffer and whether its modified
    // or not
    pub autocmd_ids: Arc<RwLock<Vec<u32>>>,
    pub autocmd_id_filetype_check: Arc<RwLock<Option<u32>>>,
    pub ring_buffer_timer_handle: Arc<RwLock<RingBufferTimerHandle>>,
    pub ring_updating_active: Arc<AtomicBool>,

    /// Cursor position after accepting a completion, used to allow FIM in comments
    /// immediately after accepting code that may end in a comment
    pub allow_comment_fim_cur_pos: Arc<RwLock<Option<(u64, usize, usize)>>>,

    /// Diagnostic tracker for LSP diagnostics
    pub diagnostics: Arc<RwLock<DiagnosticTracker>>,

    // File content storage - stores the most recent content of each open buffer
    // Used for calculating diffs on file save
    // key is filename, value is contents, None is there is a file but we haven't read it once yet
    file_contents: Arc<RwLock<HashMap<String, Option<String>>>>,

    // keep statistics on all the words for ordering all LSP completions
    word_statistics: Arc<PapayaMap<String, u64>>,

    // FIM completion channel for async worker communication
    pub fim_completion_tx: Arc<RwLock<Option<mpsc::Sender<DisplayMessage>>>>,
    // Pending display queue - holds messages waiting to be rendered on main thread
    pub pending_display: Arc<RwLock<Vec<DisplayMessage>>>,
    // Persistent tokio runtime for async operations
    pub tokio_runtime: Arc<RwLock<Runtime>>,
}

#[derive(Debug, Clone, Default)]
pub struct CurrentBufferInfo {
    pub filepath: String,
    pub is_modified: bool,
    pub is_loaded: bool,
    pub is_readable: bool,
}

/// Type alias for ring buffer timer handle to simplify type declarations
type RingBufferTimerHandle = Option<Arc<parking_lot::Mutex<tokio::task::JoinHandle<()>>>>;

impl PluginState {
    #[tracing::instrument(skip(obj))]
    fn new(obj: nvim_oxi::Object) -> Self {
        //let config = config::LttwConfig::from_nvim_globals();
        let config = config::LttwConfig::from_object(obj);
        let enable_at_startup = config.enable_at_startup;
        let tracing_enabled = config.tracing_enabled;
        let max_cache_keys = config.max_cache_keys as usize;
        let ring_n_chunks = config.ring_n_chunks as usize;
        let chunk_size = config.ring_chunk_size as usize;
        let max_req = config.max_concurrent_fim_requests as usize;
        let ring_queue_length = config.ring_queue_length;

        // Create namespaces for extmarks
        let extmark_ns = Some(create_namespace("lttw_fim"));
        let inst_ns = Some(create_namespace("lttw_inst"));

        // Create a multi-threaded tokio runtime
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                panic!("Failed to create tokio runtime: {}", e);
            }
        };

        Self {
            config: Arc::new(RwLock::new(config)),
            cache: Arc::new(RwLock::new(cache::Cache::new(max_cache_keys))),
            ring_buffer: Arc::new(RwLock::new(ring_buffer::RingBuffer::new(
                ring_n_chunks,
                chunk_size,
                ring_queue_length,
            ))),
            nvim_mode: Arc::new(RwLock::new(Vec::new())),
            last_move_time: Arc::new(RwLock::new(Instant::now())),
            instruction_requests: Arc::new(RwLock::new(HashMap::new())),
            inst_ns,
            cur_buf_info: Arc::new(RwLock::new(CurrentBufferInfo::default())),
            next_inst_req_id: Arc::new(AtomicI64::new(0)),
            fim_state: Arc::new(RwLock::new(FimState::default())),
            fim_worker_debounce_seq: Arc::new(AtomicU64::new(0)),
            fim_worker_debounce_last_spawn: Arc::new(RwLock::new(Instant::now())),
            fim_worker_semaphore: Arc::new(Semaphore::new(max_req)),
            fim_worker_generating_for_pos: Arc::new(RwLock::new(None)),
            extmark_ns,
            tracing_enabled: Arc::new(AtomicBool::new(tracing_enabled)),
            enabled: Arc::new(AtomicBool::new(enable_at_startup)),
            autocmd_ids: Arc::new(RwLock::new(Vec::new())),
            autocmd_id_filetype_check: Arc::new(RwLock::new(None)),
            ring_buffer_timer_handle: Arc::new(RwLock::new(None)),
            ring_updating_active: Arc::new(AtomicBool::new(false)),

            allow_comment_fim_cur_pos: Arc::new(RwLock::new(None)),

            diagnostics: Arc::new(RwLock::new(DiagnosticTracker::default())),

            file_contents: Arc::new(RwLock::new(HashMap::new())),
            word_statistics: Arc::new(PapayaMap::new()),
            // Initialize completion channel and runtime (will be set up later)
            fim_completion_tx: Arc::new(RwLock::new(None)),
            pending_display: Arc::new(RwLock::new(Vec::new())),
            tokio_runtime: Arc::new(RwLock::new(runtime)),
        }
    }
    #[tracing::instrument]
    pub fn get_fim_completion_tx(&self) -> LttwResult<mpsc::Sender<DisplayMessage>> {
        let fim_completion_tx_lock = self.fim_completion_tx.read();
        fim_completion_tx_lock
            .clone()
            .ok_or_else(|| Error::Lttw("Completion channel not initialized".to_string()))
    }
    /// Increment the debounce sequence and return the current sequence number
    #[tracing::instrument]
    pub fn increment_debounce_sequence(&self) -> u64 {
        self.fim_worker_debounce_seq
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1
    }

    /// Record that a worker was spawned (update last_spawn timestamp)
    #[tracing::instrument]
    pub fn record_worker_spawn(&self) {
        *self.fim_worker_debounce_last_spawn.write() = Instant::now();
    }

    #[tracing::instrument]
    pub fn in_insert_mode(&self) -> LttwResult<bool> {
        let bz = self.nvim_mode.read();
        let Some(mode_char) = bz.first() else {
            return Ok(false);
        };
        Ok(mode_char == &b'i')
    }
    #[tracing::instrument]
    pub fn in_normal_mode(&self) -> LttwResult<bool> {
        let bz = self.nvim_mode.read();
        let Some(mode_char) = bz.first() else {
            return Ok(false);
        };
        Ok(mode_char == &b'n')
    }

    #[tracing::instrument]
    pub fn set_cur_buffer_info(&self, info: CurrentBufferInfo) {
        *self.cur_buf_info.write() = info;
    }

    #[tracing::instrument]
    pub fn get_cur_buffer_info(&self) -> CurrentBufferInfo {
        self.cur_buf_info.read().clone()
    }

    /// Set the allow comment FIM cursor position
    #[tracing::instrument]
    pub fn set_allow_comment_fim_cur_pos(&self, buf_id: u64, pos_x: usize, pos_y: usize) {
        *self.allow_comment_fim_cur_pos.write() = Some((buf_id, pos_x, pos_y));
    }
    /// Set the allow comment FIM cursor position
    #[tracing::instrument]
    pub fn clear_allow_comment_fim_cur_pos(&self) {
        *self.allow_comment_fim_cur_pos.write() = None;
    }
    /// Get the allow comment FIM cursor position
    #[tracing::instrument]
    pub fn get_allow_comment_fim_cur_pos(&self) -> Option<(u64, usize, usize)> {
        *self.allow_comment_fim_cur_pos.read()
    }

    #[tracing::instrument]
    pub fn has_file_contents(&self, filename: &str) -> bool {
        self.file_contents.read().contains_key(filename)
    }

    // sets the filecontents also takes statistics on all the contents if this is the first time
    // adding the contents.
    #[tracing::instrument]
    pub fn set_file_contents(&self, filename: String, new_content: String) {
        self.file_contents
            .write()
            .insert(filename.clone(), Some(new_content.clone()));

        // dispatch a non-blocking thread to count word statistics on the file contents
        let rt = self.tokio_runtime.clone();
        let ws = self.word_statistics.clone();
        rt.read().spawn(async move {
            add_word_statistics(ws, new_content);
        });
    }

    #[tracing::instrument]
    pub fn adjust_word_statistics_for_diff(&self, diff_content: Vec<String>) {
        // dispatch a non-blocking thread to count word statistics on the file contents
        let rt = self.tokio_runtime.clone();
        let ws = self.word_statistics.clone();
        rt.read().spawn(async move {
            diff_word_statistics(ws, diff_content);
        });
    }

    /// set the file contents bypassing calculating word statistics
    #[tracing::instrument]
    pub fn set_file_contents_bypass_word_stats(&self, filename: String, new_content: String) {
        self.file_contents
            .write()
            .insert(filename.clone(), Some(new_content));
    }

    #[tracing::instrument]
    pub fn set_file_contents_empty(&self, filename: String) {
        self.file_contents.write().insert(filename.clone(), None);
    }

    #[tracing::instrument]
    pub fn file_contents_read(&self) -> RwLockReadGuard<'_, HashMap<String, Option<String>>> {
        self.file_contents.read()
    }

    #[tracing::instrument]
    pub fn get_word_statistic_usage(&self, word: &str) -> u64 {
        self.word_statistics
            .pin()
            .get(word)
            .copied()
            .unwrap_or(0u64)
    }

    #[tracing::instrument]
    pub fn debug_word_statistics(&self) {
        self.word_statistics.pin().iter().for_each(|(k, v)| {
            info!("{}: {}", k, v);
        });
    }
}

// add_word_statistics takes all the content then separates out all the Identifiers (words
// which must begin with a letter or underscore but then may also include numbers afterwords).
// The identifiers are then added to the word_statistics adding one for each word that exists
#[tracing::instrument]
pub fn add_word_statistics(word_stats: Arc<PapayaMap<String, u64>>, content: String) {
    info!("add_word_statistics");
    let mut current_word = String::new();

    let stats = word_stats.pin();

    for ch in content.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            if ch.is_numeric() && current_word.is_empty() {
                continue;
            }
            current_word.push(ch);
        } else if !current_word.is_empty() {
            let _ = *stats.update_or_insert(current_word.clone(), |v| v + 1, 0);
            current_word.clear();
        }
    }

    // Handle last word if any
    if !current_word.is_empty() {
        let _ = *stats.update_or_insert(current_word.clone(), |v| v + 1, 0);
    }
}

#[tracing::instrument]
pub fn sub_word_statistics(word_stats: Arc<PapayaMap<String, u64>>, content: String) {
    info!("sub_word_statistics");
    let mut current_word = String::new();

    let stats = word_stats.pin();
    for ch in content.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            if ch.is_numeric() && current_word.is_empty() {
                continue;
            }
            current_word.push(ch);
        } else if !current_word.is_empty() {
            let _ = *stats.update_or_insert(current_word.clone(), |v| v.saturating_sub(1), 0);
            current_word.clear();
        }
    }

    // Handle last word if any
    if !current_word.is_empty() {
        let _ = *stats.update_or_insert(current_word.clone(), |v| v.saturating_sub(1), 0);
    }
}

// strips a completion down to its identifier for comparison in the word statistics
#[tracing::instrument]
pub fn strip_to_first_identifier(s: &str) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            if ch.is_numeric() && out.is_empty() {
                continue;
            }
            out.push(ch);
        } else if !out.is_empty() {
            return out;
        }
    }
    out
}

// diff_word_statistics takes in a diff string and modifies
// the word statistics accordingly
#[tracing::instrument]
pub fn diff_word_statistics(word_stats: Arc<PapayaMap<String, u64>>, diff_content: Vec<String>) {
    for line in diff_content {
        // ignore all other lines (such as @@ lines)
        if let Some(line) = line.strip_prefix('+') {
            add_word_statistics(word_stats.clone(), line.to_string());
        } else if let Some(line) = line.strip_prefix('-') {
            sub_word_statistics(word_stats.clone(), line.to_string());
        }
    }
}
