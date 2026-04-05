use {
    crate::{
        cache, config, debug, instruction::InstructionRequestState, ring_buffer, Error,
        FimCompletionMessage, FimState, LttwResult,
    },
    ahash::{HashMap, HashMapExt},
    nvim_oxi::api::create_namespace,
    parking_lot::RwLock,
    std::{
        sync::{
            atomic::{AtomicBool, AtomicI64},
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
pub fn init_state() {
    PLUGIN_STATE.get_or_init(|| Arc::new(PluginState::default()));
}

/// Get the plugin state (returns a clone of the Arc, no locking)
pub fn get_state() -> Arc<PluginState> {
    init_state();
    PLUGIN_STATE.get().unwrap().clone()
}

/// Initialize the plugin state
pub fn init_config(obj: nvim_oxi::Object) {
    let state = get_state();
    *state.config.write() = config::LttwConfig::from_object(obj);
}

// State management
#[derive(Clone)]
pub struct PluginState {
    pub config: Arc<RwLock<config::LttwConfig>>,
    pub cache: Arc<RwLock<cache::Cache>>,
    pub ring_buffer: Arc<RwLock<ring_buffer::RingBuffer>>,
    pub debug_manager: Arc<RwLock<debug::DebugManager>>,
    pub nvim_mode: Arc<RwLock<Vec<u8>>>, // string bytes for the mode name
    pub last_move_time: Arc<RwLock<Instant>>, // (vim s:t_last_move)
    pub instruction_requests: Arc<RwLock<HashMap<i64, InstructionRequestState>>>,
    pub enabled: Arc<AtomicBool>,
    #[allow(dead_code)]
    pub next_inst_req_id: Arc<AtomicI64>,
    pub fim_state: Arc<RwLock<FimState>>,
    pub fim_worker_debounce_seq: Arc<RwLock<u64>>,
    pub fim_worker_debounce_last_spawn: Arc<RwLock<Instant>>,
    pub fim_worker_semaphore: Arc<tokio::sync::Semaphore>,
    pub extmark_ns: Option<u32>, // Namespace for extmarks (virtual text)
    #[allow(dead_code)]
    pub inst_ns: Option<u32>, // Namespace for instruction extmarks
    pub cur_buf_info: Arc<RwLock<CurrentBufferInfo>>, // the current buffer and whether its modified
    // or not
    pub autocmd_ids: Arc<RwLock<Vec<u32>>>,
    pub autocmd_id_filetype_check: Arc<RwLock<Option<u32>>>,
    pub ring_buffer_timer_handle: Arc<RwLock<RingBufferTimerHandle>>,
    pub ring_updating_active: Arc<AtomicBool>,
    // FIM completion channel for async worker communication
    pub fim_completion_tx: Arc<RwLock<Option<mpsc::Sender<FimCompletionMessage>>>>,
    // Pending display queue - holds messages waiting to be rendered on main thread
    pub pending_display: Arc<RwLock<Vec<FimCompletionMessage>>>,
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

impl Default for PluginState {
    fn default() -> Self {
        //let config = config::LttwConfig::from_nvim_globals();
        let config = config::LttwConfig::default();
        let enable_at_startup = config.enable_at_startup;
        let debug_enabled_at_startup = config.debug_enabled_at_startup;
        let max_cache_keys = config.max_cache_keys as usize;
        let ring_n_chunks = config.ring_n_chunks as usize;
        let chunk_size = config.ring_chunk_size as usize;
        let max_req = config.max_concurrent_fim_requests as usize;

        // Create namespaces for extmarks
        let extmark_ns = Some(create_namespace("lttw_fim"));
        let inst_ns = Some(create_namespace("lttw_inst"));

        // Create a multi-threaded tokio runtime
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4) // TODO parameterize this
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
            ))),
            debug_manager: Arc::new(RwLock::new(debug::DebugManager::new_with_enabled(
                debug_enabled_at_startup,
            ))),
            nvim_mode: Arc::new(RwLock::new(Vec::new())),
            last_move_time: Arc::new(RwLock::new(Instant::now())),
            instruction_requests: Arc::new(RwLock::new(HashMap::new())),
            inst_ns,
            cur_buf_info: Arc::new(RwLock::new(CurrentBufferInfo::default())),
            next_inst_req_id: Arc::new(AtomicI64::new(0)),
            fim_state: Arc::new(RwLock::new(FimState::default())),
            fim_worker_debounce_seq: Arc::new(RwLock::new(0)),
            fim_worker_debounce_last_spawn: Arc::new(RwLock::new(Instant::now())),
            fim_worker_semaphore: Arc::new(Semaphore::new(max_req)),
            extmark_ns,
            enabled: Arc::new(AtomicBool::new(enable_at_startup)),
            autocmd_ids: Arc::new(RwLock::new(Vec::new())),
            autocmd_id_filetype_check: Arc::new(RwLock::new(None)),
            ring_buffer_timer_handle: Arc::new(RwLock::new(None)),
            ring_updating_active: Arc::new(AtomicBool::new(false)),
            // Initialize completion channel and runtime (will be set up later)
            fim_completion_tx: Arc::new(RwLock::new(None)),
            pending_display: Arc::new(RwLock::new(Vec::new())),
            tokio_runtime: Arc::new(RwLock::new(runtime)),
        }
    }
}

impl PluginState {
    pub fn get_fim_completion_tx(&self) -> LttwResult<mpsc::Sender<FimCompletionMessage>> {
        let fim_completion_tx_lock = self.fim_completion_tx.read();
        fim_completion_tx_lock
            .clone()
            .ok_or_else(|| Error::Lttw("Completion channel not initialized".to_string()))
    }
    /// Increment the debounce sequence and return the current sequence number
    pub fn increment_debounce_sequence(&self) -> u64 {
        let mut seq = self.fim_worker_debounce_seq.write();
        *seq += 1;
        *seq
    }

    /// Record that a worker was spawned (update last_spawn timestamp)
    pub fn record_worker_spawn(&self) {
        *self.fim_worker_debounce_last_spawn.write() = Instant::now();
    }

    pub fn in_insert_mode(&self) -> LttwResult<bool> {
        let bz = self.nvim_mode.read();
        let Some(mode_char) = bz.first() else {
            return Ok(false);
        };
        Ok(mode_char == &b'i')
    }
    pub fn in_normal_mode(&self) -> LttwResult<bool> {
        let bz = self.nvim_mode.read();
        let Some(mode_char) = bz.first() else {
            return Ok(false);
        };
        Ok(mode_char == &b'n')
    }

    pub fn set_cur_buffer_info(&self, info: CurrentBufferInfo) {
        *self.cur_buf_info.write() = info;
    }

    pub fn get_cur_buffer_info(&self) -> CurrentBufferInfo {
        self.cur_buf_info.read().clone()
    }
}
