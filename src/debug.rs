// src/debug.rs - Debug management for lttw
//
// This module provides debug logging functionality for the plugin.
// Logs are written to a file named 'lttw.log' in the working directory.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use crate::utils;

/// Debug manager
#[derive(Debug, Clone)]
pub struct DebugManager {
    log_file_path: String,
    enabled: bool,
}

impl DebugManager {
    /// Create a new debug manager
    pub fn new() -> Self {
        let log_file_path = Self::get_log_file_path();

        // Clear the log file on startup
        Self::clear_log_file(&log_file_path);

        Self {
            log_file_path,
            enabled: true,
        }
    }

    /// Get the path to the log file (static method)
    #[allow(dead_code)]
    fn get_log_file_path() -> String {
        // Use current working directory
        let cwd = utils::get_current_directory();
        let log_path = Path::new(&cwd).join("lttw.log");
        log_path.to_string_lossy().to_string()
    }

    /// Clear the log file
    fn clear_log_file(path: &str) {
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(path)
        {
            // Just open and truncate - file is now empty
            let _ = writeln!(file, "=== lttw debug log started ===");
        }
    }

    /// Log a message to the file
    pub fn log(&self, msg: &str, details: &[&str]) {
        if !self.enabled {
            return;
        }

        let mut header = format!("{msg}");

        let mut block = Vec::new();

        if !details.is_empty() {
            header.push_str(" | ");
            header.push_str(details.first().unwrap_or(&""));
            block.push(header.clone());

            for detail in details {
                block.push(detail.to_string());
            }

            block.push("}".to_string());
        } else {
            block.push(header);
        }

        let log_entry = block.join("\n");

        // Append to log file
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_file_path)
        {
            let _ = writeln!(file, "{}", log_entry);
        }
    }

    /// Clear the log file
    pub fn clear(&mut self) {
        Self::clear_log_file(&self.log_file_path);
    }

    /// Enable or disable logging
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Check if logging is enabled
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Get log entries (for compatibility with Neovim - reads from file)
    pub fn get_log(&self) -> Vec<String> {
        // For file-based logging, this returns an empty vec
        // The actual logs are in the file
        Vec::new()
    }
}

impl Default for DebugManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Format a value for logging
pub fn format_for_log(value: &dyn std::fmt::Debug) -> String {
    format!("{:?}", value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_debug_manager() {
        let mut manager = DebugManager::new();

        // Test basic logging
        manager.log("test1", &[]);
        assert!(manager.is_enabled());

        // Test enabling/disabling
        manager.set_enabled(false);
        assert!(!manager.is_enabled());
        manager.set_enabled(true);
        assert!(manager.is_enabled());
    }
}
