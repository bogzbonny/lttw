use {
    crate::{
        cache, config, debug, instruction::InstructionRequestState, ring_buffer,
        FimCompletionMessage, FimState, FimWorkerDebounce,
    },
    nvim_oxi::api,
    parking_lot::RwLock,
    std::{
        collections::HashMap,
        sync::{
            atomic::{AtomicBool, AtomicI64},
            Arc, OnceLock,
        },
        time::Instant,
    },
    tokio::sync::mpsc,
};

// Global state - using OnceLock for thread-safe initialization
static PLUGIN_STATE: OnceLock<Arc<PluginState>> = OnceLock::new();

/// Initialize the plugin state
pub fn init_state() {
    PLUGIN_STATE.get_or_init(|| Arc::new(PluginState::new()));
}

/// Get the plugin state (returns a clone of the Arc, no locking)
pub fn get_state() -> Arc<PluginState> {
    init_state();
    PLUGIN_STATE.get().unwrap().clone()
}

// State management
#[derive(Clone)]
pub struct PluginState {
    config: Arc<RwLock<config::LttwConfig>>,
    cache: Arc<RwLock<cache::Cache>>,
    ring_buffer: Arc<RwLock<ring_buffer::RingBuffer>>,
    debug_manager: Arc<RwLock<debug::DebugManager>>,
    last_move_time: Arc<RwLock<Instant>>, // (vim s:t_last_move)
    instruction_requests: Arc<RwLock<HashMap<i64, InstructionRequestState>>>,
    enabled: Arc<AtomicBool>,
    #[allow(dead_code)]
    next_inst_req_id: Arc<AtomicI64>,
    fim_state: Arc<RwLock<FimState>>,
    fim_worker_debounce: Arc<RwLock<FimWorkerDebounce>>,
    extmark_ns: Option<u32>, // Namespace for extmarks (virtual text)
    #[allow(dead_code)]
    inst_ns: Option<u32>, // Namespace for instruction extmarks
    autocmd_ids: Arc<RwLock<Vec<u32>>>,
    autocmd_id_filetype_check: Arc<RwLock<Option<u32>>>,
    ring_buffer_timer_handle: Arc<RwLock<RingBufferTimerHandle>>,
    // FIM completion channel for async worker communication
    fim_completion_tx:
        Arc<parking_lot::Mutex<Option<tokio::sync::mpsc::Sender<FimCompletionMessage>>>>,
    // Pending display queue - holds messages waiting to be rendered on main thread
    pending_display: Arc<RwLock<Vec<FimCompletionMessage>>>,
    // Persistent tokio runtime for async operations
    tokio_runtime: Arc<parking_lot::Mutex<Option<tokio::runtime::Runtime>>>,
}

/// Type alias for ring buffer timer handle to simplify type declarations
type RingBufferTimerHandle = Option<Arc<parking_lot::Mutex<tokio::task::JoinHandle<()>>>>;

impl PluginState {
    pub fn new() -> Self {
        let config = config::LttwConfig::from_nvim_globals();
        let enable_at_startup = config.enable_at_startup;
        let max_cache_keys = config.max_cache_keys as usize;
        let max_chunks = config.ring_n_chunks as usize;
        let chunk_size = config.ring_chunk_size as usize;

        // Create namespaces for extmarks
        let extmark_ns = Some(api::create_namespace("lttw_fim"));
        let inst_ns = Some(api::create_namespace("lttw_inst"));

        Self {
            config: Arc::new(RwLock::new(config)),
            cache: Arc::new(RwLock::new(cache::Cache::new(max_cache_keys))),
            ring_buffer: Arc::new(RwLock::new(ring_buffer::RingBuffer::new(
                max_chunks, chunk_size,
            ))),
            debug_manager: Arc::new(RwLock::new(debug::DebugManager::new())),
            last_move_time: Arc::new(RwLock::new(Instant::now())),
            instruction_requests: Arc::new(RwLock::new(HashMap::new())),
            inst_ns,
            next_inst_req_id: Arc::new(AtomicI64::new(0)),
            fim_state: Arc::new(RwLock::new(FimState::default())),
            fim_worker_debounce: Arc::new(RwLock::new(FimWorkerDebounce::new())),
            extmark_ns,
            enabled: Arc::new(AtomicBool::new(enable_at_startup)),
            autocmd_ids: Arc::new(RwLock::new(Vec::new())),
            autocmd_id_filetype_check: Arc::new(RwLock::new(None)),
            ring_buffer_timer_handle: Arc::new(RwLock::new(None)),
            // Initialize completion channel and runtime (will be set up later)
            fim_completion_tx: Arc::new(parking_lot::Mutex::new(None)),
            pending_display: Arc::new(RwLock::new(Vec::new())),
            tokio_runtime: Arc::new(parking_lot::Mutex::new(None)),
        }
    }

    pub fn get_fim_completion_tx(
        &self,
    ) -> Result<mpsc::Sender<FimCompletionMessage>, nvim_oxi::Error> {
        let fim_completion_tx_lock = self.fim_completion_tx.lock();
        fim_completion_tx_lock.clone().ok_or_else(|| {
            nvim_oxi::Error::Api(api::Error::Other(
                "Completion channel not initialized".to_string(),
            ))
        })
    }
}
