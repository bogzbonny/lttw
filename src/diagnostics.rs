// src/diagnostics.rs - Diagnostic tracking for LSP diagnostics
//
// This module tracks LSP diagnostics and provides utilities for accessing
// diagnostic information by buffer and line number.

use {
    crate::{get_state, LttwResult},
    ahash::{HashMap, HashMapExt},
    nvim_oxi::{api::Buffer, Dictionary, String as NvimString},
};

/// Represents a single diagnostic entry
#[derive(Debug, Clone)]
pub struct DiagnosticInfo {
    pub buffer_id: u64,   // Neovim buffer ID
    pub line: usize,      // 0-indexed line number
    pub severity: String, // error, warn, info, hint
    pub message: String,
}

/// Tracker for all diagnostics across buffers
/// Simplified to only store by buffer_id -> (line -> diagnostics)
#[derive(Debug, Clone, Default)]
pub struct DiagnosticTracker {
    /// Diagnostics: buffer_id -> (line -> list of diagnostics on that line)
    diagnostics_by_buf: HashMap<u64, HashMap<usize, Vec<DiagnosticInfo>>>,
}

impl DiagnosticTracker {
    /// Clear all tracked diagnostics
    pub fn clear(&mut self) {
        self.diagnostics_by_buf.clear();
    }

    /// Add diagnostics for a buffer
    pub fn add_diagnostics(&mut self, buffer_id: u64, diags: Vec<DiagnosticInfo>) {
        // Remove existing diagnostics for this buffer
        self.diagnostics_by_buf.remove(&buffer_id);

        // Add new diagnostics grouped by line
        if !diags.is_empty() {
            let mut by_line: HashMap<usize, Vec<DiagnosticInfo>> = HashMap::new();
            for diag in diags {
                by_line.entry(diag.line).or_default().push(diag);
            }
            self.diagnostics_by_buf.insert(buffer_id, by_line);
        }
    }

    /// Add a single diagnostic
    pub fn add_diagnostic(&mut self, diag: DiagnosticInfo) {
        let buf_id = diag.buffer_id;
        let line = diag.line;

        // Add to by-buffer then by-line
        self.diagnostics_by_buf
            .entry(buf_id)
            .or_default()
            .entry(line)
            .or_default()
            .push(diag);
    }

    /// Get diagnostics for a specific buffer
    pub fn get_buffer_diagnostics(
        &self,
        buffer_id: u64,
    ) -> Option<&HashMap<usize, Vec<DiagnosticInfo>>> {
        self.diagnostics_by_buf.get(&buffer_id)
    }

    /// Get diagnostics for a specific line in a buffer
    pub fn get_line_diagnostics(
        &self,
        buffer_id: u64,
        line: usize,
    ) -> Option<&Vec<DiagnosticInfo>> {
        self.diagnostics_by_buf
            .get(&buffer_id)
            .and_then(|lines| lines.get(&line))
    }

    /// Get all tracked diagnostics
    pub fn get_all_diagnostics(&self) -> Vec<&DiagnosticInfo> {
        self.diagnostics_by_buf
            .values()
            .flat_map(|lines| lines.values())
            .flat_map(|v| v.iter())
            .collect()
    }

    /// Count total diagnostics
    pub fn count(&self) -> usize {
        self.diagnostics_by_buf
            .values()
            .map(|lines| lines.values().map(|v| v.len()).sum::<usize>())
            .sum()
    }

    /// Count diagnostics by severity
    pub fn count_by_severity(&self) -> HashMap<String, usize> {
        let mut counts = HashMap::new();
        for diag in self
            .diagnostics_by_buf
            .values()
            .flat_map(|lines| lines.values())
            .flatten()
        {
            *counts.entry(diag.severity.clone()).or_default() += 1;
        }
        counts
    }
}

/// Handle DiagnosticChanged autocmd - get diagnostics for current buffer
///
/// This function is called when DiagnosticChanged autocmd fires.
/// It retrieves diagnostics from Neovim and stores them in the tracker.
pub fn handle_diagnostic_changed(_arg: nvim_oxi::Object) -> LttwResult<()> {
    // Get current buffer
    let buf = Buffer::current();
    let buf_id = buf.handle();
    let buf_id_u64: u64 = buf_id.try_into().unwrap_or(0);
    debug!(_arg);

    // Get diagnostics for this buffer using Neovim's vim.diagnostic.get()
    // We'll use a Lua callback to get the diagnostics directly
    // First execute a command to put the diagnostics in a register
    let _ =
        nvim_oxi::api::command("lua vim.cmd('let @+ = vim.json.encode(vim.diagnostic.get(0))')");

    // Use nvim_command to execute Lua and store diagnostics in a variable
    // We'll use a global variable approach
    let _ = nvim_oxi::api::command(
        "lua vim.g.lttw_diagnostics = vim.json.encode(vim.diagnostic.get(0))",
    );

    // Read the global variable using nvim_oxi::api::get_var with String type
    let json_str = match nvim_oxi::api::get_var::<String>("lttw_diagnostics") {
        Ok(s) => s,
        Err(_) => String::new(),
    };

    if json_str.is_empty() || json_str == "[]" {
        // No diagnostics for this buffer
        let state = get_state();
        let mut tracker = state.diagnostics.write();
        tracker.add_diagnostics(buf_id_u64, Vec::new());
        return Ok(());
    }

    // Parse JSON to get array of diagnostic dictionaries
    let diags: Vec<serde_json::Value> = match serde_json::from_str(&json_str) {
        Ok(d) => d,
        Err(_) => {
            // If parsing fails, add empty diagnostics
            let state = get_state();
            let mut tracker = state.diagnostics.write();
            tracker.add_diagnostics(buf_id_u64, Vec::new());
            return Ok(());
        }
    };

    // Convert dictionaries to DiagnosticInfo objects
    let diagnostics: Vec<DiagnosticInfo> = diags
        .into_iter()
        .filter_map(|val| {
            if let serde_json::Value::Object(map) = val {
                let message = map
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let line = map.get("lnum").and_then(|v| v.as_i64()).unwrap_or(0) as usize;
                let severity = map
                    .get("severity")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();

                Some(DiagnosticInfo {
                    buffer_id: buf_id_u64,
                    line,
                    severity,
                    message,
                })
            } else {
                None
            }
        })
        .collect();

    // Add diagnostics to tracker
    let state = get_state();
    let mut tracker = state.diagnostics.write();
    tracker.add_diagnostics(buf_id_u64, diagnostics.clone());

    debug!(
        "DiagnosticChanged: Tracked {} diagnostics for buffer {}",
        diagnostics.len(),
        buf_id_u64
    );

    Ok(())
}

/// Get diagnostics for current buffer and output with debug!()
pub fn debug_output_diagnostics(_arg: nvim_oxi::Object) -> LttwResult<()> {
    let state = get_state();
    let tracker = state.diagnostics.read();

    let buf = Buffer::current();
    let buf_id = buf.handle().try_into().unwrap_or(0);

    let buffer_diags = tracker.get_buffer_diagnostics(buf_id);

    match buffer_diags {
        Some(lines) => {
            debug!(
                "Diagnostics for buffer {}: {} lines with diagnostics",
                buf_id,
                lines.len()
            );
            for (line, diags) in lines {
                debug!("  Line {}: {} diagnostics", line, diags.len());
                for (i, diag) in diags.iter().enumerate() {
                    debug!("    [{}] [{}] {}", i + 1, diag.severity, diag.message);
                }
            }
        }
        None => {
            debug!("No diagnostics tracked for buffer {}", buf_id);
        }
    }

    Ok(())
}

// ============ DiagnosticInfo implementation ============

impl DiagnosticInfo {
    /// Helper to get string field from dictionary
    fn get_string(dict: &Dictionary, key: &str) -> Option<String> {
        dict.get(key).and_then(|obj| {
            NvimString::try_from(obj.clone())
                .ok()
                .map(|s| s.to_string())
        })
    }

    /// Helper to get i64 field from dictionary
    fn get_i64(dict: &Dictionary, key: &str) -> Option<i64> {
        dict.get(key)
            .and_then(|obj| i64::try_from(obj.clone()).ok())
    }

    /// Create a DiagnosticInfo from a Neovim diagnostic dictionary
    pub fn from_dictionary(buf_id: u64, dict: Dictionary) -> Option<Self> {
        // Extract required fields from the diagnostic dictionary
        let message = Self::get_string(&dict, "text").unwrap_or_default();
        let line = Self::get_i64(&dict, "lnum").unwrap_or(0) as usize;
        let severity = Self::get_string(&dict, "severity").unwrap_or_else(|| {
            // Fallback severity
            "unknown".to_string()
        });

        Some(Self {
            buffer_id: buf_id,
            line,
            severity,
            message,
        })
    }

    /// Get the affected line number (0-indexed)
    pub fn get_line(&self) -> usize {
        self.line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diagnostic_info_creation() {
        let diag = DiagnosticInfo {
            buffer_id: 1,
            line: 10,
            severity: "error".to_string(),
            message: "Test error".to_string(),
        };

        assert_eq!(diag.buffer_id, 1);
        assert_eq!(diag.line, 10);
        assert_eq!(diag.severity, "error");
        assert_eq!(diag.message, "Test error");
    }

    #[test]
    fn test_diagnostic_tracker() {
        let mut tracker = DiagnosticTracker::new();

        // Add some diagnostics
        tracker.add_diagnostic(DiagnosticInfo {
            buffer_id: 1,
            line: 5,
            severity: "error".to_string(),
            message: "Error on line 5".to_string(),
        });

        tracker.add_diagnostic(DiagnosticInfo {
            buffer_id: 1,
            line: 10,
            severity: "warn".to_string(),
            message: "Warning on line 10".to_string(),
        });

        // Test retrieval
        assert_eq!(tracker.count(), 2);
        assert!(tracker.get_buffer_diagnostics(1).is_some());
        assert!(tracker.get_line_diagnostics(1, 5).is_some());
        assert!(tracker.get_line_diagnostics(1, 10).is_some());
        assert!(tracker.get_line_diagnostics(1, 15).is_none());
    }
}
