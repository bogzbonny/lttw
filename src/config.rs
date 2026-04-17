// src/config.rs - Configuration handling for lttw
//
// This module handles the plugin configuration, translating the Vimscript
// configuration into a strongly-typed Rust struct.

use {
    crate::fim::FimLLM,
    nvim_oxi::conversion::FromObject,
    serde::{Deserialize, Serialize},
    std::{collections::BTreeMap, str::FromStr},
};

/// Configuration options for the lttw plugin
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LttwConfig {
    //-------------------------------------------
    // GENERAL CONFIG
    pub debounce_min_ms: u64,
    pub debounce_max_ms: u64,
    pub max_concurrent_fim_requests: u32,
    pub single_line_prediction_within_line: bool,

    // Display configuration
    pub show_info: u8,
    pub auto_fim: bool,

    // Cache configuration
    pub max_cache_keys: u32,

    // how often to check if we should start the ring update sequence
    pub ring_update_ms: u64,

    // TODO actually use
    // Keymap configuration
    pub keymap_fim_trigger: String,
    pub keymap_fim_accept_full: String,
    pub keymap_fim_accept_line: String,
    pub keymap_fim_accept_word: String,
    pub keymap_debug_toggle: String,
    pub keymap_inst_trigger: String,
    pub keymap_inst_rerun: String,
    pub keymap_inst_continue: String,
    pub keymap_inst_accept: String,
    pub keymap_inst_cancel: String,

    // Diff tracking configuration
    pub diff_tracking_enabled: bool,

    // Comment detection configuration
    pub no_fim_in_comments: bool,

    pub llm_completions: bool,
    pub reduce_cognitive_offloading_percentage: u8,

    pub duel_model_mode: bool,
    pub run_duel_models_concurrently: bool,
    pub duel_models_ring_buffer_prioritization: DuelModelPrioritization, // "concurrent", "series", "series_zip"

    // Startup configuration
    pub enable_at_startup: bool,
    pub tracing_enabled: bool,
    pub tracing_log_file: bool,
    pub tracing_level: String,

    // cleanup of old virt text (used for debugging)
    // TODO eventually remove this
    pub disable_cleanup: bool,

    pub disabled_filetypes: Vec<String>,
    pub enabled_filetypes: Vec<String>,

    //-------------------------------------------
    // LSP
    pub lsp_completions: bool,
    pub lsp_comp_truncate_vars: bool,
    pub lsp_comp_insert_one_var: bool,

    /// LSP override pairs for transforming completion text, keyed by filetype.
    /// Each key is a Neovim filetype (e.g. "rust", "typescript") and the value
    /// is a list of (pattern, replacement) pairs. If a completion text matches
    /// the pattern, it will be replaced with the replacement string.
    /// Example: {"rust": [("Ok()", "Ok(())")]} will transform Ok() to Ok(()) for Rust files.
    pub lsp_overrides: BTreeMap<String, Vec<(String, String)>>,
    //-------------------------------------------
    // Local word statistics for LSP completion priority
    /// Multiplier applied to local (around-cursor) word occurrences vs global occurrences.
    /// A word found in the local scope is treated as if it occurred this many times globally.
    pub lsp_local_occurrence_weight: u64,

    /// Number of recent diff hunks to keep for LSP completion weighting.
    /// When new diffs are added, the oldest diff is evicted if the list exceeds this length.
    pub lsp_diff_history_length: u32,

    /// Multiplier applied to word occurrences from recent diff additions (+ lines).
    /// Similar to lsp_local_occurrence_weight but for the diff history.
    pub lsp_diff_occurrence_weight: u64,

    //-------------------------------------------
    // INSTRUCTION
    pub instr_endpoint: String,
    pub instr_model: Option<String>,
    pub instr_api_key: Option<String>,
    pub instr_n_prefix: u32, // number of prefix lines fed into the inline endpoint
    pub instr_n_suffix: u32, // number of suffix lines fed into the inline endpoint

    //-------------------------------------------
    // FIM Context configuration
    // NOTE even though we feed in the following 'n' number of lines for prefix and suffix,
    // those lines will be trucated further in the FIM completion system within llama.cpp which
    // balances prefix:suffix content to being 3:1 AND ensures that all of that content fits into a
    // single batch (`--batch-size` flag)
    pub n_prefix: u32, // number of prefix lines fed into the inline endpoint
    pub n_suffix: u32, // number of suffix lines fed into the inline endpoint


    //-------------------------------------------
    // PER MODEL CONFIG
    default_fim_config: FimModelConfig,
    fast_fim_config: FimModelConfigOverrides,
    slow_fim_config: FimModelConfigOverrides,
}

impl Default for LttwConfig {
    fn default() -> Self {
        Self {
            debounce_min_ms: 20,
            debounce_max_ms: 200,
            max_concurrent_fim_requests: 3, // good to be larger than 1 to allow for speculative FIM
            single_line_prediction_within_line: true,
            show_info: 2,
            auto_fim: true,
            max_cache_keys: 32, // can be small due to recaching
            ring_update_ms: 1000,
            keymap_fim_trigger: "<leader>llf".to_string(),
            keymap_fim_accept_full: "<Tab>".to_string(),
            keymap_fim_accept_line: "<S-Tab>".to_string(),
            keymap_fim_accept_word: "<leader>ll]".to_string(),
            keymap_debug_toggle: "<leader>lld".to_string(),
            keymap_inst_trigger: "<leader>lli".to_string(),
            keymap_inst_rerun: "<leader>llr".to_string(),
            keymap_inst_continue: "<leader>llc".to_string(),
            keymap_inst_accept: "<Tab>".to_string(),
            keymap_inst_cancel: "<Esc>".to_string(),
            diff_tracking_enabled: true,
            no_fim_in_comments: true,
            llm_completions: true,
            duel_model_mode: true,
            run_duel_models_concurrently: false,
            duel_models_ring_buffer_prioritization: DuelModelPrioritization::SeriesZip,
            reduce_cognitive_offloading_percentage: 0,
            lsp_completions: true,
            lsp_comp_truncate_vars: true,
            lsp_comp_insert_one_var: false,
            // Default overrides keyed by filetype (currently only rust defaults)
            lsp_overrides: {
                let mut map = BTreeMap::new();
                map.insert(
                    "rust".to_string(),
                    vec![
                        ("Ok()".to_string(), "Ok(())".to_string()),
                        ("unwrap_or()".to_string(), "unwrap_or(…)".to_string()),
                        ("if … {".to_string(), "if ".to_string()),
                        ("if let … =  {".to_string(), "if let ".to_string()),
                        ("match … {".to_string(), "match ".to_string()),
                        ("let … = ;".to_string(), "let ".to_string()),
                        ("let mut … = ;".to_string(), "let mut ".to_string()),
                        ("for … in  {".to_string(), "for ".to_string()),
                        ("while … {".to_string(), "while ".to_string()),
                        ("fn …() {".to_string(), "fn ".to_string()),
                        ("trait … {".to_string(), "trait ".to_string()),
                        ("enum … {".to_string(), "enum ".to_string()),
                        ("impl … {".to_string(), "impl ".to_string()),
                        ("Arc<>".to_string(), "Arc<".to_string()),
                        ("RwLock<>".to_string(), "RwLock<".to_string()),
                        ("Box<>".to_string(), "Box<".to_string()),
                        ("Option<>".to_string(), "Option<".to_string()),
                        ("Result<>".to_string(), "Result<".to_string()),
                        ("Mutex<>".to_string(), "Mutex<".to_string()),
                        ("Rc<>".to_string(), "Rc<".to_string()),
                        ("RefCell<>".to_string(), "RefCell<".to_string()),
                        ("Vec<>".to_string(), "Vec<".to_string()),
                    ],
                );
                map
            },
            enable_at_startup: true,
            tracing_enabled: false,
            tracing_log_file: false,
            tracing_level: "DEBUG".to_string(),
            disable_cleanup: false,
            disabled_filetypes: Vec::new(),
            enabled_filetypes: Vec::new(),

            instr_endpoint: "http://127.0.0.1:8012/v1/chat/completions".to_string(),
            instr_model: None,
            instr_api_key: None,
            instr_n_prefix: 256,
            instr_n_suffix: 64,

            n_prefix: 256,
            n_suffix: 64,
            lsp_local_occurrence_weight: 10,
            lsp_diff_history_length: 7,
            lsp_diff_occurrence_weight: 10,

            default_fim_config: FimModelConfig::default(),
            fast_fim_config: FimModelConfigOverrides::default(),
            slow_fim_config: FimModelConfigOverrides::default(),
        }
    }
}

/// Configuration options for each model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FimModelConfig {
    pub endpoint: String,

    pub model_name: Option<String>,
    pub api_key: Option<String>,

    // Dynamic n_predict configuration
    // n_predict_inner: tokens to predict when there are non-whitespace chars to the right of cursor
    // n_predict_end: tokens to predict when at end of line or only whitespace to the right
    pub n_predict_inner: u32,
    pub n_predict_end: u32,

    pub t_max_prompt_ms: u32,
    pub t_max_predict_ms: u32,

    pub max_line_suffix: u32,

    // Ring buffer configuration
    pub ring_n_chunks: u32,
    pub ring_chunk_size: u32,
    pub ring_scope: u32,
    pub ring_queue_length: usize,
    /// Number of chunks to pick from the scope when the cursor moves significantly
    /// or to a new buffer. The greater this number, the greater the scope should be
    /// to reduce overlapping picks.
    pub ring_n_picks: u32,
}

/// Configuration options for each model
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FimModelConfigOverrides {
    pub endpoint: Option<String>,
    pub model_name: Option<String>,
    pub api_key: Option<String>,
    pub n_prefix: Option<u32>,
    pub n_suffix: Option<u32>,
    pub n_predict_inner: Option<u32>,
    pub n_predict_end: Option<u32>,
    pub t_max_prompt_ms: Option<u32>,
    pub t_max_predict_ms: Option<u32>,
    pub max_line_suffix: Option<u32>,
    pub ring_n_chunks: Option<u32>,
    pub ring_chunk_size: Option<u32>,
    pub ring_scope: Option<u32>,
    pub ring_queue_length: Option<usize>,
    pub ring_n_picks: Option<u32>,
}

impl Default for FimModelConfig {
    fn default() -> Self {
        Self {
            endpoint: "http://127.0.0.1:8012/infill".to_string(),
            model_name: None,
            api_key: None,
            n_predict_inner: 16,
            n_predict_end: 256,
            t_max_prompt_ms: 500,
            t_max_predict_ms: 1000,
            max_line_suffix: 8,
            ring_n_chunks: 16,
            ring_chunk_size: 64,
            ring_scope: 1024,
            ring_queue_length: 16,
            ring_n_picks: 1, // Default to 1 - number of chunks to pick from scope
        }
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum DuelModelPrioritization {
    Concurrent,
    Series,
    SeriesZip,
}

impl DuelModelPrioritization {
    pub fn as_str(&self) -> &str {
        match self {
            DuelModelPrioritization::Concurrent => "concurrent",
            DuelModelPrioritization::Series => "series",
            DuelModelPrioritization::SeriesZip => "series_zip",
        }
    }
}

impl std::str::FromStr for DuelModelPrioritization {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "concurrent" => Ok(Self::Concurrent),
            "series" => Ok(Self::Series),
            "series_zip" => Ok(Self::SeriesZip),
            _ => Ok(Self::SeriesZip),
        }
    }
}

impl LttwConfig {
    pub fn get_endpoint(&self, m: FimLLM) -> String {
        match m {
            FimLLM::Fast => self
                .fast_fim_config
                .endpoint
                .clone()
                .unwrap_or(self.default_fim_config.endpoint.clone()),
            FimLLM::Slow => self
                .slow_fim_config
                .endpoint
                .clone()
                .unwrap_or(self.default_fim_config.endpoint.clone()),
        }
    }

    pub fn get_fim_model_name(&self, m: FimLLM) -> Option<String> {
        match m {
            FimLLM::Fast => {
                if let Some(ref model_name) = self.fast_fim_config.model_name {
                    Some(model_name.to_string())
                } else {
                    self.default_fim_config.model_name.clone()
                }
            }
            FimLLM::Slow => {
                if let Some(ref model_name) = self.slow_fim_config.model_name {
                    Some(model_name.to_string())
                } else {
                    self.default_fim_config.model_name.clone()
                }
            }
        }
    }

    pub fn get_api_key(&self, m: FimLLM) -> Option<String> {
        match m {
            FimLLM::Fast => {
                if let Some(ref api_key) = self.fast_fim_config.api_key {
                    Some(api_key.to_string())
                } else {
                    self.default_fim_config.api_key.clone()
                }
            }
            FimLLM::Slow => {
                if let Some(ref api_key) = self.slow_fim_config.api_key {
                    Some(api_key.to_string())
                } else {
                    self.default_fim_config.api_key.clone()
                }
            }
        }
    }

    pub fn get_n_predict_inner(&self, m: FimLLM) -> u32 {
        match m {
            FimLLM::Fast => self
                .fast_fim_config
                .n_predict_inner
                .unwrap_or(self.default_fim_config.n_predict_inner),
            FimLLM::Slow => self
                .slow_fim_config
                .n_predict_inner
                .unwrap_or(self.default_fim_config.n_predict_inner),
        }
    }

    pub fn get_n_predict_end(&self, m: FimLLM) -> u32 {
        match m {
            FimLLM::Fast => self
                .fast_fim_config
                .n_predict_end
                .unwrap_or(self.default_fim_config.n_predict_end),
            FimLLM::Slow => self
                .slow_fim_config
                .n_predict_end
                .unwrap_or(self.default_fim_config.n_predict_end),
        }
    }

    pub fn get_t_max_prompt_ms(&self, m: FimLLM) -> u32 {
        match m {
            FimLLM::Fast => self
                .fast_fim_config
                .t_max_prompt_ms
                .unwrap_or(self.default_fim_config.t_max_prompt_ms),
            FimLLM::Slow => self
                .slow_fim_config
                .t_max_prompt_ms
                .unwrap_or(self.default_fim_config.t_max_prompt_ms),
        }
    }

    pub fn get_t_max_predict_ms(&self, m: FimLLM) -> u32 {
        match m {
            FimLLM::Fast => self
                .fast_fim_config
                .t_max_predict_ms
                .unwrap_or(self.default_fim_config.t_max_predict_ms),
            FimLLM::Slow => self
                .slow_fim_config
                .t_max_predict_ms
                .unwrap_or(self.default_fim_config.t_max_predict_ms),
        }
    }

    pub fn get_max_line_suffix(&self, m: FimLLM) -> u32 {
        match m {
            FimLLM::Fast => self
                .fast_fim_config
                .max_line_suffix
                .unwrap_or(self.default_fim_config.max_line_suffix),
            FimLLM::Slow => self
                .slow_fim_config
                .max_line_suffix
                .unwrap_or(self.default_fim_config.max_line_suffix),
        }
    }

    pub fn get_ring_n_chunks(&self, m: FimLLM) -> u32 {
        match m {
            FimLLM::Fast => self
                .fast_fim_config
                .ring_n_chunks
                .unwrap_or(self.default_fim_config.ring_n_chunks),
            FimLLM::Slow => self
                .slow_fim_config
                .ring_n_chunks
                .unwrap_or(self.default_fim_config.ring_n_chunks),
        }
    }

    pub fn get_ring_chunk_size(&self, m: FimLLM) -> u32 {
        match m {
            FimLLM::Fast => self
                .fast_fim_config
                .ring_chunk_size
                .unwrap_or(self.default_fim_config.ring_chunk_size),
            FimLLM::Slow => self
                .slow_fim_config
                .ring_chunk_size
                .unwrap_or(self.default_fim_config.ring_chunk_size),
        }
    }

    pub fn get_ring_scope(&self, m: FimLLM) -> u32 {
        match m {
            FimLLM::Fast => self
                .fast_fim_config
                .ring_scope
                .unwrap_or(self.default_fim_config.ring_scope),
            FimLLM::Slow => self
                .slow_fim_config
                .ring_scope
                .unwrap_or(self.default_fim_config.ring_scope),
        }
    }

    pub fn get_ring_queue_length(&self, m: FimLLM) -> usize {
        match m {
            FimLLM::Fast => self
                .fast_fim_config
                .ring_queue_length
                .unwrap_or(self.default_fim_config.ring_queue_length),
            FimLLM::Slow => self
                .slow_fim_config
                .ring_queue_length
                .unwrap_or(self.default_fim_config.ring_queue_length),
        }
    }
    pub fn get_ring_n_picks(&self, m: FimLLM) -> u32 {
        match m {
            FimLLM::Fast => self
                .fast_fim_config
                .ring_n_picks
                .unwrap_or(self.default_fim_config.ring_n_picks),
            FimLLM::Slow => self
                .slow_fim_config
                .ring_n_picks
                .unwrap_or(self.default_fim_config.ring_n_picks),
        }
    }
}

impl LttwConfig {
    /// Create a new configuration with default values
    #[tracing::instrument]
    pub fn new() -> Self {
        Self::default()
    }

    /// Load configuration from Neovim global variable vim.g.lttw_config
    /// This merges user config with defaults - only handles basic types supported by nvim_oxi
    #[tracing::instrument(skip(obj))]
    pub fn from_object(obj: nvim_oxi::Object) -> Self {
        // Start with defaults
        let mut config = Self::default();

        // Convert Object to Dictionary
        let dict: nvim_oxi::Dictionary = match obj.try_into() {
            Ok(d) => d,
            Err(_) => return config, // Return defaults on conversion error
        };

        // Helper to get string field from dictionary
        let get_string = |key: &str| -> Option<String> {
            dict.get(key).and_then(|obj| {
                nvim_oxi::String::try_from(obj.clone())
                    .ok()
                    .map(|s| s.to_string())
            })
        };

        // Helper to get bool field from dictionary
        let get_bool = |key: &str| -> Option<bool> {
            dict.get(key)
                .and_then(|obj| nvim_oxi::Boolean::from_object(obj.clone()).ok())
        };

        // Helper to get array of strings from dictionary
        let get_string_array = |key: &str| -> Option<Vec<String>> {
            dict.get(key).and_then(|obj| {
                nvim_oxi::Array::from_object(obj.clone()).ok().map(|a| {
                    a.into_iter()
                        .filter_map(|item| nvim_oxi::String::try_from(item).ok())
                        .map(|s| s.to_string())
                        .collect()
                })
            })
        };

        // Helper to get i64 field from dictionary
        let get_i64 = |key: &str| -> Option<i64> {
            dict.get(key)
                .and_then(|obj| i64::try_from(obj.clone()).ok())
        };

        if let Some(v) = get_string("instr_endpoint") {
            config.instr_endpoint = v;
        }
        if let Some(v) = get_string("instr_model_name")
            && !v.is_empty()
        {
            config.instr_model = Some(v);
        }
        if let Some(v) = get_string("instr_api_key")
            && !v.is_empty()
        {
            config.instr_api_key = Some(v);
        }
        if let Some(v) = get_i64("instr_n_prefix") {
            config.instr_n_prefix = v as u32;
        }
        if let Some(v) = get_i64("instr_n_suffix") {
            config.instr_n_suffix = v as u32;
        }

        if let Some(v) = get_i64("show_info") {
            let v = v.clamp(0, 2) as u8;
            config.show_info = v;
        }
        if let Some(v) = get_string("keymap_fim_trigger") {
            config.keymap_fim_trigger = v;
        }
        if let Some(v) = get_string("keymap_inst_trigger") {
            config.keymap_inst_trigger = v;
        }
        if let Some(v) = get_i64("debounce_min_ms") {
            config.debounce_min_ms = v as u64;
        }
        if let Some(v) = get_i64("debounce_max_ms") {
            config.debounce_max_ms = v as u64;
        }
        if let Some(v) = get_i64("max_concurrent_fim_requests") {
            config.max_concurrent_fim_requests = v as u32;
        }
        if let Some(v) = get_i64("max_cache_keys") {
            config.max_cache_keys = v as u32;
        }
        if let Some(v) = get_i64("ring_update_ms") {
            config.ring_update_ms = v as u64;
        }
        if let Some(v) = get_i64("n_prefix") {
            config.n_prefix = v as u32;
        }
        if let Some(v) = get_i64("n_suffix") {
            config.n_suffix = v as u32;
        }
        if let Some(v) = get_i64("lsp_local_occurrence_weight") {
            config.lsp_local_occurrence_weight = v as u64;
        }
        if let Some(v) = get_i64("lsp_diff_history_length") {
            config.lsp_diff_history_length = v as u32;
        }
        if let Some(v) = get_i64("lsp_diff_occurrence_weight") {
            config.lsp_diff_occurrence_weight = v as u64;
        }

        // ------------------
        if let Some(v) = get_string("fim_endpoint") {
            config.default_fim_config.endpoint = v;
        }
        if let Some(v) = get_string("fim_model_name")
            && !v.is_empty()
        {
            config.default_fim_config.model_name = Some(v);
        }
        if let Some(v) = get_string("fim_api_key")
            && !v.is_empty()
        {
            config.default_fim_config.api_key = Some(v);
        }
        if let Some(v) = get_i64("n_predict_inner") {
            config.default_fim_config.n_predict_inner = v as u32;
        }
        if let Some(v) = get_i64("n_predict_end") {
            config.default_fim_config.n_predict_end = v as u32;
        }
        if let Some(v) = get_i64("t_max_prompt_ms") {
            config.default_fim_config.t_max_prompt_ms = v as u32;
        }
        if let Some(v) = get_i64("t_max_predict_ms") {
            config.default_fim_config.t_max_predict_ms = v as u32;
        }
        if let Some(v) = get_i64("max_line_suffix") {
            config.default_fim_config.max_line_suffix = v as u32;
        }
        if let Some(v) = get_i64("ring_n_chunks") {
            config.default_fim_config.ring_n_chunks = v as u32;
        }
        if let Some(v) = get_i64("ring_chunk_size") {
            config.default_fim_config.ring_chunk_size = v as u32;
        }
        if let Some(v) = get_i64("ring_scope") {
            config.default_fim_config.ring_scope = v as u32;
        }
        if let Some(v) = get_i64("ring_queue_length") {
            config.default_fim_config.ring_queue_length = v as usize;
        }
        if let Some(v) = get_i64("ring_n_picks") {
            config.default_fim_config.ring_n_picks = v as u32;
        }
        // ------------------
        if let Some(v) = get_string("fast_fim_endpoint") {
            config.fast_fim_config.endpoint = Some(v);
        }
        if let Some(v) = get_string("fast_fim_model_name")
            && !v.is_empty()
        {
            config.fast_fim_config.model_name = Some(v);
        }
        if let Some(v) = get_string("fast_fim_api_key")
            && !v.is_empty()
        {
            config.fast_fim_config.api_key = Some(v);
        }
        if let Some(v) = get_i64("fast_fim_n_prefix") {
            config.fast_fim_config.n_prefix = Some(v as u32);
        }
        if let Some(v) = get_i64("fast_fim_n_suffix") {
            config.fast_fim_config.n_suffix = Some(v as u32);
        }
        if let Some(v) = get_i64("fast_fim_n_predict_inner") {
            config.fast_fim_config.n_predict_inner = Some(v as u32);
        }
        if let Some(v) = get_i64("fast_fim_n_predict_end") {
            config.fast_fim_config.n_predict_end = Some(v as u32);
        }
        if let Some(v) = get_i64("fast_fim_t_max_prompt_ms") {
            config.fast_fim_config.t_max_prompt_ms = Some(v as u32);
        }
        if let Some(v) = get_i64("fast_fim_t_max_predict_ms") {
            config.fast_fim_config.t_max_predict_ms = Some(v as u32);
        }
        if let Some(v) = get_i64("fast_fim_max_line_suffix") {
            config.fast_fim_config.max_line_suffix = Some(v as u32);
        }
        if let Some(v) = get_i64("fast_fim_ring_n_chunks") {
            config.fast_fim_config.ring_n_chunks = Some(v as u32);
        }
        if let Some(v) = get_i64("fast_fim_ring_chunk_size") {
            config.fast_fim_config.ring_chunk_size = Some(v as u32);
        }
        if let Some(v) = get_i64("fast_fim_ring_scope") {
            config.fast_fim_config.ring_scope = Some(v as u32);
        }
        if let Some(v) = get_i64("fast_fim_ring_queue_length") {
            config.fast_fim_config.ring_queue_length = Some(v as usize);
        }
        if let Some(v) = get_i64("fast_fim_ring_n_picks") {
            config.fast_fim_config.ring_n_picks = Some(v as u32);
        }
        // ------------------
        if let Some(v) = get_string("slow_fim_endpoint") {
            config.slow_fim_config.endpoint = Some(v);
        }
        if let Some(v) = get_string("slow_fim_model_name")
            && !v.is_empty()
        {
            config.slow_fim_config.model_name = Some(v);
        }
        if let Some(v) = get_string("slow_fim_api_key")
            && !v.is_empty()
        {
            config.slow_fim_config.api_key = Some(v);
        }
        if let Some(v) = get_i64("slow_fim_n_prefix") {
            config.slow_fim_config.n_prefix = Some(v as u32);
        }
        if let Some(v) = get_i64("slow_fim_n_suffix") {
            config.slow_fim_config.n_suffix = Some(v as u32);
        }
        if let Some(v) = get_i64("slow_fim_n_predict_inner") {
            config.slow_fim_config.n_predict_inner = Some(v as u32);
        }
        if let Some(v) = get_i64("slow_fim_n_predict_end") {
            config.slow_fim_config.n_predict_end = Some(v as u32);
        }
        if let Some(v) = get_i64("slow_fim_t_max_prompt_ms") {
            config.slow_fim_config.t_max_prompt_ms = Some(v as u32);
        }
        if let Some(v) = get_i64("slow_fim_t_max_predict_ms") {
            config.slow_fim_config.t_max_predict_ms = Some(v as u32);
        }
        if let Some(v) = get_i64("slow_fim_max_line_suffix") {
            config.slow_fim_config.max_line_suffix = Some(v as u32);
        }
        if let Some(v) = get_i64("slow_fim_ring_n_chunks") {
            config.slow_fim_config.ring_n_chunks = Some(v as u32);
        }
        if let Some(v) = get_i64("slow_fim_ring_chunk_size") {
            config.slow_fim_config.ring_chunk_size = Some(v as u32);
        }
        if let Some(v) = get_i64("slow_fim_ring_scope") {
            config.slow_fim_config.ring_scope = Some(v as u32);
        }
        if let Some(v) = get_i64("slow_fim_ring_queue_length") {
            config.slow_fim_config.ring_queue_length = Some(v as usize);
        }
        if let Some(v) = get_i64("slow_fim_ring_n_picks") {
            config.slow_fim_config.ring_n_picks = Some(v as u32);
        }

        // Override bool fields
        if let Some(v) = get_bool("single_line_prediction_within_line") {
            config.single_line_prediction_within_line = v;
        }
        if let Some(v) = get_bool("auto_fim") {
            config.auto_fim = v;
        }
        if let Some(v) = get_bool("enable_at_startup") {
            config.enable_at_startup = v;
        }
        if let Some(v) = get_bool("tracing_enabled") {
            config.tracing_enabled = v;
        }
        if let Some(v) = get_bool("tracing_log_file") {
            config.tracing_log_file = v;
        }
        if let Some(v) = get_string("tracing_level") {
            config.tracing_level = v;
        }

        // Override array fields
        if let Some(v) = get_bool("disable_cleanup") {
            config.disable_cleanup = v;
        }
        if let Some(v) = get_string_array("disabled_filetypes") {
            config.disabled_filetypes = v;
        }
        if let Some(v) = get_string_array("enabled_filetypes") {
            config.enabled_filetypes = v;
        }
        // Override bool fields
        if let Some(v) = get_bool("diff_tracking_enabled") {
            config.diff_tracking_enabled = v;
        }
        if let Some(v) = get_bool("no_fim_in_comments") {
            config.no_fim_in_comments = v;
        }

        // LLM general settings
        if let Some(v) = get_bool("llm_completions") {
            config.llm_completions = v;
        }
        if let Some(v) = get_bool("duel_model_mode") {
            config.duel_model_mode = v;
        }
        if let Some(v) = get_bool("run_duel_models_concurrently") {
            config.run_duel_models_concurrently = v;
        }
        if let Some(v) = get_string("duel_models_ring_buffer_prioritization") {
            config.duel_models_ring_buffer_prioritization =
                DuelModelPrioritization::from_str(&v).expect("Invalid DuelModelPrioritization");
        }
        if let Some(v) = get_i64("reduce_cognitive_offloading_percentage") {
            let v = v.clamp(0, 100) as u8;
            config.reduce_cognitive_offloading_percentage = v;
        }

        // LSP
        if let Some(v) = get_bool("lsp_completions") {
            config.lsp_completions = v;
        }
        if let Some(v) = get_bool("lsp_comp_truncate_vars") {
            config.lsp_comp_truncate_vars = v;
        }
        if let Some(v) = get_bool("lsp_comp_insert_one_var") {
            config.lsp_comp_insert_one_var = v;
        }

        // Helper to get array of string pairs from an array (flat)
        let get_string_pairs = |key: &str| -> Option<Vec<(String, String)>> {
            dict.get(key).and_then(|obj| {
                nvim_oxi::Array::from_object(obj.clone()).ok().map(|a| {
                    a.into_iter()
                        .filter_map(|item| {
                            nvim_oxi::Array::from_object(item).ok().map(|pair| {
                                if pair.len() >= 2 {
                                    let first = nvim_oxi::String::try_from(pair[0].clone())
                                        .ok()
                                        .map(|s| s.to_string());
                                    let second = nvim_oxi::String::try_from(pair[1].clone())
                                        .ok()
                                        .map(|s| s.to_string());
                                    match (first, second) {
                                        (Some(f), Some(s)) => Some((f, s)),
                                        _ => None,
                                    }
                                } else {
                                    None
                                }
                            })
                        })
                        .flatten()
                        .collect()
                })
            })
        };

        // Helper to get a BTreeMap of filetype -> string pairs from dictionary
        let get_string_pairs_by_filetype = |key: &str| -> Option<BTreeMap<String, Vec<(String, String)>>> {
            dict.get(key).and_then(|obj| {
                nvim_oxi::Dictionary::from_object(obj.clone()).ok().map(|d| {
                    let mut map = BTreeMap::new();
                    for (filetype_key, filetype_val) in d.iter() {
                        if let Ok(arr) = nvim_oxi::Array::from_object(filetype_val.clone()) {
                            let pairs: Vec<(String, String)> = arr.into_iter()
                                .filter_map(|item| {
                                    nvim_oxi::Array::from_object(item).ok().and_then(|pair| {
                                        if pair.len() >= 2 {
                                            let first = nvim_oxi::String::try_from(pair[0].clone())
                                                .ok()
                                                .map(|s| s.to_string());
                                            let second = nvim_oxi::String::try_from(pair[1].clone())
                                                .ok()
                                                .map(|s| s.to_string());
                                            match (first, second) {
                                                (Some(f), Some(s)) => Some((f, s)),
                                                _ => None,
                                            }
                                        } else {
                                            None
                                        }
                                    })
                                })
                                .collect();
                            map.insert(filetype_key.to_string(), pairs);
                        }
                    }
                    map
                })
            })
        };

        // Try the new filetype-keyed format first, fall back to flat array for backwards compat
        if let Some(v) = get_string_pairs_by_filetype("lsp_overrides") {
            config.lsp_overrides = v;
        } else if let Some(v) = get_string_pairs("lsp_overrides") {
            // Backwards compat: if user provides a flat array, put it under "rust"
            if !v.is_empty() {
                config.lsp_overrides.insert("rust".to_string(), v);
            }
        }

        config
    }

    /// Check if a filetype is enabled
    #[tracing::instrument]
    pub fn is_filetype_enabled(&self, filetype: &str) -> bool {
        // If enabled_filetypes is empty, check disabled_filetypes
        let mut enabled = !self
            .disabled_filetypes
            .iter()
            .any(|ft| ft == filetype || ft == "*");

        // If enabled_filetypes is not empty, only allow those types
        if !self.enabled_filetypes.is_empty() {
            enabled = self
                .enabled_filetypes
                .iter()
                .any(|ft| ft == filetype || ft == "*");
        }
        enabled
    }

    /// Get LSP overrides for a specific filetype.
    /// Returns an empty slice if no overrides are configured for that filetype.
    #[tracing::instrument]
    pub fn get_lsp_overrides(&self, filetype: &str) -> &[(String, String)] {
        self.lsp_overrides
            .get(filetype)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }
}