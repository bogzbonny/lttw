// src/debug.rs - Debug management for lttw
//
// This module provides debug logging functionality for the plugin.
// Logs are written to a file named 'lttw.log' in the working directory.

use crate::log;

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
            log::reset_log_file("./lttw.log".to_string());
        }

        Self { enabled }
    }

    /// Create a new debug manager without initializing tracing
    /// This is useful for tests where tracing may already be initialized
    #[cfg(test)]
    pub fn new_without_tracing(enabled: bool) -> Self {
        Self { enabled }
    }

    /// Clear the log file
    pub fn clear(&mut self) {
        if !self.enabled {
            return;
        }
        log::clear()
    }

    /// Enable or disable logging
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;

        // If enabling, also enable the log system
        if enabled {
            log::enable();
            // Don't re-initialize tracing if already initialized
            // Tracing is a global singleton and can only be initialized once
        } else {
            log::disable();
        }
    }

    /// Check if logging is enabled
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_debug_manager_creation() {
        let manager = DebugManager::new_without_tracing(true);
        assert!(manager.is_enabled());
    }

    #[test]
    fn test_debug_manager_disable() {
        let mut manager = DebugManager::new_without_tracing(true);
        manager.set_enabled(false);
        assert!(!manager.is_enabled());
    }

    #[test]
    fn test_debug_manager_enable() {
        let mut manager = DebugManager::new_without_tracing(false);
        manager.set_enabled(true);
        assert!(manager.is_enabled());
    }
}
