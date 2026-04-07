// src/config.rs - Configuration handling for lttw
//
// This module handles the plugin configuration, translating the Vimscript
// configuration into a strongly-typed Rust struct.

use {
    nvim_oxi::conversion::FromObject,
    serde::{Deserialize, Serialize},
};

/// Configuration options for the lttw plugin
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LttwConfig {
    // FIM (Fill-in-Middle) configuration
    pub endpoint_fim: String,
    pub endpoint_inst: String,

    pub model_fim: String,
    pub model_inst: String,
    pub api_key: String,

    // Context configuration
    // NOTE even though we feed in the following 'n' number of lines for prefix and suffix,
    // those lines will be trucated further in the FIM completion system within llama.cpp which
    // balances prefix:suffix content to being 3:1 AND ensures that all of that content fits into a
    // single batch (`--batch-size` flag)
    pub n_prefix: u32, // number of prefix lines fed into the inline endpoint
    pub n_suffix: u32, // number of suffix lines fed into the inline endpoint

    // Dynamic n_predict configuration
    // n_predict_inner: tokens to predict when there are non-whitespace chars to the right of cursor
    // n_predict_end: tokens to predict when at end of line or only whitespace to the right
    pub n_predict_inner: u32,
    pub n_predict_end: u32,

    pub t_max_prompt_ms: u32,
    pub t_max_predict_ms: u32,
    pub debounce_min_ms: u64,
    pub debounce_max_ms: u64,
    pub max_concurrent_fim_requests: u32,
    pub single_line_prediction_within_line: bool,

    // Display configuration
    pub show_info: u8,
    pub auto_fim: bool,
    pub max_line_suffix: u32,

    // Cache configuration
    pub max_cache_keys: u32,

    // Ring buffer configuration
    pub ring_n_chunks: u32,
    pub ring_chunk_size: u32,
    pub ring_scope: u32,
    pub ring_update_ms: u64,
    pub ring_queue_length: usize,
    /// Number of chunks to pick from the scope when the cursor moves significantly
    /// or to a new buffer. The greater this number, the greater the scope should be
    /// to reduce overlapping picks.
    pub ring_n_picks: u32,

    // Keymap configuration

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

    // Startup configuration
    pub enable_at_startup: bool,
    pub debug_enabled_at_startup: bool,
    pub disabled_filetypes: Vec<String>,
    pub enabled_filetypes: Vec<String>,
}

impl Default for LttwConfig {
    fn default() -> Self {
        Self {
            endpoint_fim: "http://127.0.0.1:8012/infill".to_string(),
            endpoint_inst: "http://127.0.0.1:8012/v1/chat/completions".to_string(),
            model_fim: String::new(),
            model_inst: String::new(),
            api_key: String::new(),
            n_prefix: 256,
            n_suffix: 64,
            n_predict_inner: 16,
            n_predict_end: 256,
            t_max_prompt_ms: 500,
            t_max_predict_ms: 1000,
            debounce_min_ms: 20,
            debounce_max_ms: 200,
            max_concurrent_fim_requests: 3, // good to be larger than 1 to allow for speculative FIM
            single_line_prediction_within_line: true,
            show_info: 2,
            auto_fim: true,
            max_line_suffix: 8,
            max_cache_keys: 250,
            ring_n_chunks: 16,
            ring_chunk_size: 64,
            ring_scope: 1024,
            ring_update_ms: 1000,
            ring_queue_length: 16,
            ring_n_picks: 1, // Default to 1 - number of chunks to pick from scope
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
            enable_at_startup: true,
            debug_enabled_at_startup: false,
            disabled_filetypes: Vec::new(),
            enabled_filetypes: Vec::new(),
        }
    }
}

impl LttwConfig {
    /// Create a new configuration with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Load configuration from Neovim global variable vim.g.lttw_config
    /// This merges user config with defaults - only handles basic types supported by nvim_oxi
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

        // Helper to get i64 field from dictionary
        let get_i64 = |key: &str| -> Option<i64> {
            dict.get(key)
                .and_then(|obj| i64::try_from(obj.clone()).ok())
        };

        // Override string fields
        if let Some(v) = get_string("endpoint_fim") {
            config.endpoint_fim = v;
        }
        if let Some(v) = get_string("endpoint_inst") {
            config.endpoint_inst = v;
        }
        if let Some(v) = get_string("model_fim") {
            config.model_fim = v;
        }
        if let Some(v) = get_string("model_inst") {
            config.model_inst = v;
        }
        if let Some(v) = get_string("api_key") {
            config.api_key = v;
        }
        if let Some(v) = get_string("keymap_fim_trigger") {
            config.keymap_fim_trigger = v;
        }
        if let Some(v) = get_string("keymap_inst_trigger") {
            config.keymap_inst_trigger = v;
        }

        // Override numeric fields
        if let Some(v) = get_i64("n_prefix") {
            config.n_prefix = v as u32;
        }
        if let Some(v) = get_i64("n_suffix") {
            config.n_suffix = v as u32;
        }
        if let Some(v) = get_i64("n_predict_inner") {
            config.n_predict_inner = v as u32;
        }
        if let Some(v) = get_i64("n_predict_end") {
            config.n_predict_end = v as u32;
        }
        if let Some(v) = get_i64("t_max_prompt_ms") {
            config.t_max_prompt_ms = v as u32;
        }
        if let Some(v) = get_i64("t_max_predict_ms") {
            config.t_max_predict_ms = v as u32;
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
        if let Some(v) = get_i64("show_info") {
            config.show_info = v as u8;
        }
        if let Some(v) = get_i64("max_line_suffix") {
            config.max_line_suffix = v as u32;
        }
        if let Some(v) = get_i64("max_cache_keys") {
            config.max_cache_keys = v as u32;
        }
        if let Some(v) = get_i64("ring_n_chunks") {
            config.ring_n_chunks = v as u32;
        }
        if let Some(v) = get_i64("ring_chunk_size") {
            config.ring_chunk_size = v as u32;
        }
        if let Some(v) = get_i64("ring_scope") {
            config.ring_scope = v as u32;
        }
        if let Some(v) = get_i64("ring_update_ms") {
            config.ring_update_ms = v as u64;
        }
        if let Some(v) = get_i64("ring_queue_length") {
            config.ring_queue_length = v as usize;
        }
        if let Some(v) = get_i64("ring_n_picks") {
            config.ring_n_picks = v as u32;
        }

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
        if let Some(v) = get_bool("debug_enabled_at_startup") {
            config.debug_enabled_at_startup = v;
        }

        // Override array fields
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

        // Handle deprecated key names (rename old keys to new ones)
        if let Some(v) = get_string("endpoint") {
            config.endpoint_fim = v;
        }
        if let Some(v) = get_string("model") {
            config.model_fim = v;
        }
        if let Some(v) = get_string("keymap_trigger") {
            config.keymap_fim_trigger = v;
        }
        if let Some(v) = get_string("keymap_accept_full") {
            config.keymap_fim_accept_full = v;
        }
        if let Some(v) = get_string("keymap_accept_line") {
            config.keymap_fim_accept_line = v;
        }
        if let Some(v) = get_string("keymap_accept_word") {
            config.keymap_fim_accept_word = v;
        }
        if let Some(v) = get_string("keymap_debug") {
            config.keymap_debug_toggle = v;
        }

        config
    }

    /// Check if a filetype is enabled
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = LttwConfig::new();
        assert_eq!(config.endpoint_fim, "http://127.0.0.1:8012/infill");
        assert_eq!(
            config.endpoint_inst,
            "http://127.0.0.1:8012/v1/chat/completions"
        );
        assert_eq!(config.n_prefix, 256);
        assert_eq!(config.n_suffix, 64);
        assert_eq!(config.n_predict_inner, 16);
        assert_eq!(config.n_predict_end, 256);
    }

    #[test]
    fn test_filetype_enabled() {
        let mut config = LttwConfig::new();

        // Test with empty enabled_filetypes (should use disabled_filetypes)
        assert!(config.is_filetype_enabled("rust"));

        // Add rust to disabled
        config.disabled_filetypes.push("rust".to_string());
        assert!(!config.is_filetype_enabled("rust"));
        assert!(config.is_filetype_enabled("python"));

        // Test with enabled_filetypes (should override disabled)
        config.enabled_filetypes.push("python".to_string());
        assert!(config.is_filetype_enabled("python"));
        assert!(!config.is_filetype_enabled("rust"));

        // Test wildcard
        config.enabled_filetypes.clear();
        config.enabled_filetypes.push("*".to_string());
        assert!(config.is_filetype_enabled("any"));
    }

    #[test]
    fn test_config_defaults() {
        let config = LttwConfig::new();

        assert_eq!(config.endpoint_fim, "http://127.0.0.1:8012/infill");
        assert_eq!(
            config.endpoint_inst,
            "http://127.0.0.1:8012/v1/chat/completions"
        );
        assert_eq!(config.n_prefix, 256);
        assert_eq!(config.n_suffix, 64);
        assert_eq!(config.n_predict_inner, 16);
        assert_eq!(config.n_predict_end, 256);
        assert!(config.auto_fim);
        assert!(!config.diff_tracking_enabled);
    }
}
