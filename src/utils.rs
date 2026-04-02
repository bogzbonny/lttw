// src/utils.rs - Utility functions
//
// This module provides various utility functions used throughout the plugin.

use {
    crate::NvimResult,
    nvim_oxi::api::{self, Buffer, Window},
    rand::Rng,
    sha2::{Digest, Sha256},
    std::fs,
    std::ops::RangeBounds,
    std::path::Path,
};

// are we in insert mode
pub fn in_insert_mode() -> NvimResult<bool> {
    Ok(api::get_mode()?
        .mode
        .as_bytes()
        .first()
        .copied()
        .expect("mode is not empty")
        == b'i')
}

// are we in insert mode
pub fn in_normal_mode() -> NvimResult<bool> {
    Ok(api::get_mode()?
        .mode
        .as_bytes()
        .first()
        .copied()
        .expect("mode is not empty")
        == b'n')
}

/// Get current buffer position ([0,0]-indexed)
pub fn get_pos() -> (usize, usize) {
    let (line, col) = Window::current().get_cursor().unwrap_or((0, 0));

    // NOTE this is (1, 0) indexing (CONFUSING!)
    // hence we must subtract 1 from the col but not the line
    // to be consistent with our (0, 0) indexing
    let line = line.saturating_sub(1);
    (col, line)
}

/// Get buffer lines from Neovim
pub fn get_buf_lines<R>(line_range: R) -> Vec<String>
where
    R: RangeBounds<usize>,
{
    let buf = Buffer::current();
    let lines = buf.get_lines(line_range, false).unwrap();
    lines.map(|s| s.to_string()).collect()
}

/// Get buffer lines from Neovim
pub fn get_buf_line_count() -> usize {
    let buf = Buffer::current();
    buf.line_count().unwrap_or(0)
}

/// Get buffer lines from Neovim
pub fn buffer_modified() -> bool {
    let buf = Buffer::current();
    // TODO test that this get_var is working
    let is_modified: bool = buf.get_var("modified").unwrap_or(false);
    is_modified
}

/// Get buffer lines from Neovim
pub fn get_buf_filename() -> NvimResult<String> {
    let buf = Buffer::current();
    let buf_file_name = buf.get_name()?;
    // convert to string
    let filename = buf_file_name.to_string_lossy().to_string();
    Ok(filename)
}
/// Get buffer lines from Neovim
pub fn buffer_active_and_readable() -> NvimResult<bool> {
    let buf = Buffer::current();
    let loaded = buf.is_loaded(); // acts like buf_listed
    let buf_file_name = buf.get_name()?;
    let is_readable = is_readable(buf_file_name.as_path());
    Ok(loaded && is_readable)
}
fn is_readable(path: &Path) -> bool {
    path.exists() && fs::metadata(path).map(|m| m.is_file()).unwrap_or(false)
}

/// Get buffer lines from Neovim
/// pos_y is zero indexed
pub fn get_buf_line(pos_y: usize) -> String {
    let buf = Buffer::current();
    let Ok(lines) = buf.get_lines(pos_y..=pos_y, false) else {
        return "".to_string();
    };
    let lines: Vec<String> = lines.map(|s| s.to_string()).collect();
    if lines.is_empty() {
        "".to_string()
    } else {
        lines.into_iter().next().expect("should be one record")
    }
}

/// Get current buffer
pub fn get_current_buffer() -> u64 {
    let buf: u64 = Buffer::current().handle().try_into().unwrap_or(0);
    buf
}

pub fn get_buffer_handle() -> u64 {
    Buffer::current().handle().try_into().unwrap_or(0)
}

/// Generate a random number in the range [i0, i1]
pub fn random_range(i0: usize, i1: usize) -> usize {
    let mut rng = rand::thread_rng();
    rng.gen_range(i0..=i1)
}

/// Compute SHA256 hash of a string
pub fn sha256(input: &str) -> String {
    let hash = Sha256::digest(input.as_bytes());
    format!("{:x}", hash)
}

/// Get current working directory
pub fn get_current_directory() -> String {
    std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| ".".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sha256() {
        let hash = sha256("hello");
        assert_eq!(hash.len(), 64); // SHA256 produces 64 hex characters
        assert_eq!(
            hash,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }
}
