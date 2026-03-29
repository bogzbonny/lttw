// src/debug.rs - Debug management for lttw
//
// This module provides debug logging functionality for the plugin.

/// Debug manager
#[derive(Debug, Clone)]
pub struct DebugManager {
    log: Vec<String>,
    max_lines: usize,
    enabled: bool,
}

impl DebugManager {
    /// Create a new debug manager
    pub fn new(max_lines: usize) -> Self {
        Self {
            log: Vec::new(),
            max_lines,
            enabled: true,
        }
    }

    /// Log a message
    pub fn log(&mut self, msg: &str, details: &[&str]) {
        if !self.enabled {
            return;
        }

        //let timestamp = time::UtcDateTime::now().format("%H:%M:%S");
        let now = time::OffsetDateTime::now_utc();
        let timestamp = format!("{:02}:{:02}:{:02}", now.hour(), now.minute(), now.second());
        let mut header = format!("{} | {}", timestamp, msg);

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

        // Insert at beginning (newest first)
        self.log.insert(0, block.join("\n"));

        // Trim if too long
        if self.log.len() > self.max_lines {
            self.log.truncate(self.max_lines);
        }
    }

    /// Get all log entries
    pub fn get_log(&self) -> &[String] {
        &self.log
    }

    /// Clear the log
    pub fn clear(&mut self) {
        self.log.clear();
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

impl Default for DebugManager {
    fn default() -> Self {
        Self::new(1024)
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
        let mut manager = DebugManager::new(3);

        manager.log("test1", &[]);
        manager.log("test2", &["detail1", "detail2"]);

        let log = manager.get_log();
        assert_eq!(log.len(), 2);

        // Test max lines
        manager.log("test3", &[]);
        manager.log("test4", &[]);

        let log = manager.get_log();
        assert_eq!(log.len(), 3);
        assert!(log[0].contains("test4"));
        assert!(!log.iter().any(|l| l.contains("test1")));
    }
}
