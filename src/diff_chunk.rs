// src/diff_chunk.rs - Diff chunk tracking system for lttw
//
// This module provides diff chunk tracking functionality.
// It tracks diff chunks on file save (BufWritePost autocmd) and stores them in PluginState.
// On each recalculation, it compares new vs old diff chunks and updates the ring buffer.

use crate::{ring_buffer::Chunk, LttwResult};
use std::time::Instant;

/// Represents a single diff chunk with metadata
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffChunk {
    /// File path where the change occurred
    pub filepath: String,
    /// Original line number where the hunk starts
    pub old_start: u32,
    /// Number of lines in the original hunk
    pub old_lines: u32,
    /// New line number where the hunk starts
    pub new_start: u32,
    /// Number of lines in the new hunk
    pub new_lines: u32,
    /// The diff content (unified diff format)
    pub content: String,
    /// Timestamp when this diff was created
    pub time: Instant,
    /// Unique identifier for this chunk (assigned by PluginState)
    pub id: usize,
}

impl DiffChunk {
    /// Create a new DiffChunk from hunk data
    pub fn from_hunk_data(
        filepath: &str,
        old_start: u32,
        old_lines: u32,
        new_start: u32,
        new_lines: u32,
        content: &str,
    ) -> Self {
        Self {
            filepath: filepath.to_string(),
            old_start,
            old_lines,
            new_start,
            new_lines,
            content: content.to_string(),
            time: Instant::now(),
            id: 0, // Will be assigned by PluginState
        }
    }

    /// Convert this diff chunk into a RingBuffer Chunk for processing
    pub fn to_ring_chunk(&self) -> Chunk {
        let lines: Vec<String> = self.content.lines().map(|s| s.to_string()).collect();
        Chunk {
            data: lines,
            chunk_str: self.content.clone(),
            time: self.time,
            filename: self.filepath.clone(),
            id: self.id,
        }
    }
}

/// Calculate all diffs in the repository's working directory using git CLI
///
/// This function runs `git diff` to calculate all changed files in the repository.
/// It parses the output to extract diff chunks for all changed files.
pub fn calculate_all_repo_diffs() -> LttwResult<Vec<DiffChunk>> {
    // Check if git is available and we're in a git repo
    let git_output = std::process::Command::new("git")
        .arg("rev-parse")
        .arg("--git-dir")
        .output();

    let repo_root = match git_output {
        Ok(output) if output.status.success() => String::from_utf8(output.stdout)
            .ok()
            .and_then(|s| s.trim().strip_suffix("/.git").map(|s| s.to_string())),
        _ => None,
    };

    let repo_root = match repo_root {
        Some(r) => r,
        None => return Ok(Vec::new()), // Not in a git repo
    };

    // Run git diff to get all changes
    let diff_output = std::process::Command::new("git")
        .arg("diff")
        .arg("HEAD")
        .current_dir(&repo_root)
        .output();

    let diff_text = match diff_output {
        Ok(output) if output.status.success() => {
            String::from_utf8(output.stdout).unwrap_or_default()
        }
        _ => return Ok(Vec::new()), // Git diff failed
    };

    if diff_text.is_empty() {
        return Ok(Vec::new());
    }

    let mut chunks = Vec::new();
    let lines: Vec<&str> = diff_text.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        // Look for diff file headers: "diff --git a/file b/file"
        if lines[i].starts_with("diff --git") {
            // Extract the file path
            let filepath = extract_file_path(lines[i]);

            if filepath.is_empty() {
                i += 1;
                continue;
            }

            // Collect all lines for this file's diff
            let mut file_lines: Vec<String> = Vec::new();
            while i < lines.len() && !lines[i].starts_with("diff --git") {
                file_lines.push(lines[i].to_string());
                i += 1;
            }

            // Parse the hunk headers and content for this file
            let file_content: String = file_lines.join("\n");

            if !file_content.is_empty() {
                // Parse hunks to get line numbers
                let (old_start, old_lines, new_start, new_lines) = parse_hunk_info(&file_lines);

                chunks.push(DiffChunk::from_hunk_data(
                    &filepath,
                    old_start,
                    old_lines,
                    new_start,
                    new_lines,
                    &file_content,
                ));
            }
        } else {
            i += 1;
        }
    }

    Ok(chunks)
}

/// Extract file path from a diff header line
fn extract_file_path(line: &str) -> String {
    // Format: "diff --git a/file b/file"
    let parts: Vec<&str> = line.split(" ").collect();
    if parts.len() >= 3 {
        // Get the b/ path (new file)
        let b_path = parts[2].strip_prefix("b/").unwrap_or(parts[2]);
        return b_path.to_string();
    }
    String::new()
}

/// Parse hunk header to extract line numbers
fn parse_hunk_info(file_lines: &[String]) -> (u32, u32, u32, u32) {
    let mut old_start: u32 = 1;
    let mut old_lines: u32 = 0;
    let mut new_start: u32 = 1;
    let mut new_lines: u32 = 0;

    for line in file_lines {
        if line.starts_with("@@") {
            // Parse hunk header: "@@ -old_start,old_lines +new_start,new_lines @@"
            if let Some(hunk_part) = line.strip_prefix("@@ ") {
                let parts: Vec<&str> = hunk_part.split(" ").collect();
                if parts.len() >= 2 {
                    // Parse -old_start,old_lines
                    if let Some(old_part) = parts[0].strip_prefix("-") {
                        let old_nums: Vec<&str> = old_part.split(',').collect();
                        if old_nums.len() >= 1 {
                            old_start = old_nums[0].parse().unwrap_or(1);
                            old_lines = if old_nums.len() > 1 {
                                old_nums[1].parse().unwrap_or(0)
                            } else {
                                0
                            };
                        }
                    }
                    // Parse +new_start,new_lines
                    if let Some(new_part) = parts[1].strip_prefix("+") {
                        let new_nums: Vec<&str> = new_part.split(',').collect();
                        if new_nums.len() >= 1 {
                            new_start = new_nums[0].parse().unwrap_or(1);
                            new_lines = if new_nums.len() > 1 {
                                new_nums[1].parse().unwrap_or(0)
                            } else {
                                0
                            };
                        }
                    }
                }
            }
            break;
        }
    }

    (old_start, old_lines, new_start, new_lines)
}

/// Evaluate diff chunk changes and return additions and removals
///
/// # Arguments
/// * `new_chunks` - Newly calculated diff chunks
/// * `old_chunks` - Previously stored diff chunks
///
/// # Returns
/// * `additions` - Chunks that are new (should be added to ringbuffer)
/// * `removals` - Chunks that were removed (should be evicted from ringbuffer)
pub fn evaluate_diff_changes(
    new_chunks: &[DiffChunk],
    old_chunks: &[DiffChunk],
) -> (Vec<DiffChunk>, Vec<DiffChunk>) {
    // Create sets based on filepath and id for comparison
    // This ensures we compare by the actual diff content via its unique id
    let old_keyed: std::collections::HashMap<String, usize> = old_chunks
        .iter()
        .map(|c| (c.filepath.clone(), c.id))
        .collect();

    let new_keyed: std::collections::HashMap<String, usize> = new_chunks
        .iter()
        .map(|c| (c.filepath.clone(), c.id))
        .collect();

    // Additions: in new but not in old (by filepath)
    let additions: Vec<DiffChunk> = new_chunks
        .iter()
        .filter(|c| {
            let old_id = old_keyed.get(&c.filepath);
            // New if filepath is new OR id has changed
            old_id.is_none() || old_id != Some(&c.id)
        })
        .cloned()
        .collect();

    // Removals: in old but not in new (by filepath)
    let removals: Vec<DiffChunk> = old_chunks
        .iter()
        .filter(|c| {
            let new_id = new_keyed.get(&c.filepath);
            // Removed if filepath no longer exists OR id has changed
            new_id.is_none() || new_id != Some(&c.id)
        })
        .cloned()
        .collect();

    (additions, removals)
}

/// Log diff chunk operations for debugging
pub fn log_diff_operations(
    debug_manager: &crate::debug::DebugManager,
    additions: &[DiffChunk],
    removals: &[DiffChunk],
) {
    let add_count = additions.len();
    let rem_count = removals.len();

    debug_manager.log(
        "diff_chunk_eval",
        format!("Additions: {}, Removals: {}", add_count, rem_count),
    );

    for chunk in additions {
        debug_manager.log(
            "diff_chunk_added",
            format!(
                "{}:{}-{} ({} lines) id:{}",
                chunk.filepath,
                chunk.new_start,
                chunk.new_start + chunk.new_lines,
                chunk.new_lines,
                chunk.id
            ),
        );
    }

    for chunk in removals {
        debug_manager.log(
            "diff_chunk_removed",
            format!(
                "{}:{}-{} ({} lines) id:{}",
                chunk.filepath,
                chunk.old_start,
                chunk.old_start + chunk.old_lines,
                chunk.old_lines,
                chunk.id
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diff_chunk_creation() {
        let chunk = DiffChunk {
            filepath: "test.rs".to_string(),
            old_start: 10,
            old_lines: 5,
            new_start: 12,
            new_lines: 7,
            content: "@@ -10,5 +12,7 @@\n+line1\n+line2\n-context\n".to_string(),
            time: Instant::now(),
            id: 123,
        };

        assert_eq!(chunk.filepath, "test.rs");
        assert_eq!(chunk.old_start, 10);
        assert_eq!(chunk.old_lines, 5);
        assert_eq!(chunk.new_start, 12);
        assert_eq!(chunk.new_lines, 7);
        assert_eq!(chunk.id, 123);
    }

    #[test]
    fn test_evaluate_diff_changes() {
        // Use from_hunk_data to create consistent IDs
        let old_chunks = vec![
            DiffChunk::from_hunk_data("file1.rs", 1, 1, 1, 1, "content1"),
            DiffChunk::from_hunk_data("file2.rs", 2, 1, 2, 1, "content2"),
        ];

        let new_chunks = vec![
            DiffChunk::from_hunk_data("file1.rs", 1, 1, 1, 1, "content1"),
            DiffChunk::from_hunk_data("file3.rs", 3, 1, 3, 1, "content3"),
        ];

        let (additions, removals) = evaluate_diff_changes(&new_chunks, &old_chunks);

        assert_eq!(additions.len(), 1);
        assert_eq!(additions[0].filepath, "file3.rs");

        assert_eq!(removals.len(), 1);
        assert_eq!(removals[0].filepath, "file2.rs");
    }
}
