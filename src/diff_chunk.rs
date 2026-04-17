// src/diff_chunk.rs - Diff chunk tracking system for lttw
//
// This module provides diff chunk tracking functionality.
// It tracks diff chunks on file save (BufWritePost autocmd) and stores them in PluginState.
// On each recalculation, it compares new vs old diff chunks and updates the ring buffer.

use {
    crate::{LttwResult, ring_buffer::Chunk},
    gix_diff::blob::{
        Algorithm, UnifiedDiff,
        intern::InternedInput,
        unified_diff::{ConsumeBinaryHunk, ContextSize},
    },
    std::time::Instant,
};

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
    pub content: Vec<String>,
    /// Timestamp when this diff was created
    pub time: Instant,
}

impl DiffChunk {
    /// Create a new DiffChunk from hunk data
    #[tracing::instrument]
    pub fn from_hunk_data(
        filepath: &str,
        old_start: u32,
        old_lines: u32,
        new_start: u32,
        new_lines: u32,
        content: Vec<String>,
    ) -> Self {
        Self {
            filepath: filepath.to_string(),
            old_start,
            old_lines,
            new_start,
            new_lines,
            content,
            time: Instant::now(),
        }
    }

    /// Convert this diff chunk into a RingBuffer Chunk for processing
    #[tracing::instrument]
    pub fn to_ring_chunk(&self) -> Chunk {
        Chunk {
            data: self.content.clone(),
            chunk_str: self.content.join("\n"),
            time: self.time,
            filename: self.filepath.clone(),
        }
    }
}

/// Calculate diffs between two file contents and return diff chunks
///
/// This function uses gix-diff to calculate line-based diffs between the old and new content.
/// It returns diff chunks for all differences found.
#[tracing::instrument]
pub fn calculate_diff_between_contents(
    filepath: &str,
    old_content: &str,
    new_content: &str,
) -> LttwResult<Vec<DiffChunk>> {
    if old_content == new_content {
        return Ok(Vec::new());
    }

    let mut chunks = Vec::new();

    // Use gix_diff::blob::diff with simple unified diff output
    let interner = InternedInput::new(old_content, new_content);
    let unified = UnifiedDiff::new(
        &interner,
        ConsumeBinaryHunk::new(String::new(), "\n"),
        ContextSize::symmetrical(0),
    );

    let diff_output = gix_diff::blob::diff(Algorithm::Myers, &interner, unified)?;
    info!("state {diff_output}");

    // Parse the diff output to extract hunks
    // The output is a unified diff string
    if !diff_output.is_empty() {
        // Parse hunk information from the unified diff
        let diff_lines: Vec<&str> = diff_output.lines().collect();
        let (old_start, old_lines, new_start, new_lines) = parse_hunk_info_from_diff(&diff_lines);

        chunks.push(DiffChunk::from_hunk_data(
            filepath,
            old_start,
            old_lines,
            new_start,
            new_lines,
            diff_lines.into_iter().map(|s| s.to_string()).collect(),
        ));
    }

    Ok(chunks)
}

/// Parse hunk header to extract line numbers from a unified diff string
#[tracing::instrument]
fn parse_hunk_info_from_diff(diff_lines: &[&str]) -> (u32, u32, u32, u32) {
    let mut old_start: u32 = 1;
    let mut old_lines: u32 = 0;
    let mut new_start: u32 = 1;
    let mut new_lines: u32 = 0;

    for line in diff_lines {
        if line.starts_with("@@") {
            // Parse hunk header: "@@ -old_start,old_lines +new_start,new_lines @@"
            if let Some(hunk_part) = line.strip_prefix("@@ ") {
                let parts: Vec<&str> = hunk_part.split(" ").collect();
                if parts.len() >= 2 {
                    // Parse -old_start,old_lines
                    if let Some(old_part) = parts[0].strip_prefix("-") {
                        let old_nums: Vec<&str> = old_part.split(',').collect();
                        if !old_nums.is_empty() {
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
                        if !new_nums.is_empty() {
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
            content: "@@ -10,5 +12,7 @@\n+line1\n+line2\n-context\n"
                .lines()
                .map(|s| s.to_string())
                .collect(),
            time: Instant::now(),
        };

        assert_eq!(chunk.filepath, "test.rs");
        assert_eq!(chunk.old_start, 10);
        assert_eq!(chunk.old_lines, 5);
        assert_eq!(chunk.new_start, 12);
        assert_eq!(chunk.new_lines, 7);
    }

    #[test]
    fn test_parse_hunk_info_from_diff() {
        let diff_lines = vec!["@@ -1,10 +1,11 @@", " line1", " line2"];

        let (old_start, old_lines, new_start, new_lines) = parse_hunk_info_from_diff(&diff_lines);

        assert_eq!(old_start, 1);
        assert_eq!(old_lines, 10);
        assert_eq!(new_start, 1);
        assert_eq!(new_lines, 11);
    }

    // TODO uncomment once there is a better debugging system so I don't need to init state
    //#[test]
    //fn test_diff_between_contents() {
    //    let old = "line1\nline2\nline3\n";
    //    let new = "line1\nmodified\nline3\n";
    //    //init_state();

    //    let chunks = calculate_diff_between_contents("test.rs", old, new);
    //    assert!(chunks.is_ok());
    //    // Should have at least one chunk
    //    assert!(!chunks.unwrap().is_empty());
    //}
}
