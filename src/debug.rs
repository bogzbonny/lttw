// src/debug.rs - Debug management for lttw
//
// This module provides debug logging functionality for the plugin.
// Logs are written to a file named 'lttw.log' in the working directory.

/// Debug manager
#[derive(Debug, Clone)]
pub struct DebugManager {
    pub enabled: bool,
}

impl DebugManager {
    /// Create a new debug manager with specified enabled state
    pub fn new(enabled: bool) -> Self {
        // Only clear the log file on startup if debug is enabled
        if enabled {
            crate::log::reset_log_file("./lttw.log".to_string());
        }

        Self { enabled }
    }

    /// Clear the log file
    pub fn clear(&mut self) {
        if !self.enabled {
            return;
        }
        crate::log::clear()
    }

    /// Enable or disable logging
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Check if logging is enabled
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }
}
