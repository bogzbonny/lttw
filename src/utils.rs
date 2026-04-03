// src/utils.rs - Utility functions
//
// This module provides various utility functions used throughout the plugin.

use {
    crate::LttwResult,
    nvim_oxi::{
        api::{
            self, get_option_value,
            opts::{CreateAutocmdOpts, OptionOpts, SetExtmarkOpts},
            Buffer, Window,
        },
        conversion::FromObject,
    },
    rand::Rng,
    sha2::{Digest, Sha256},
    std::fs,
    std::ops::RangeBounds,
    std::path::Path,
};

// NOTE important we cannot safely call into neovim from tokio worker threads
//
// https://github.com/noib3/nvim-oxi/issues/260
//     "Essentially never call neovim's functions outside of callbacks and plugin entrypoints and
//      never call neovim's functions from another thread."

fn assert_not_tokio_worker() {
    let t = std::thread::current();
    if let Some(n) = t.name() {
        if n.contains("tokio") {
            panic!("function must not be called from Tokio runtime worker thread (name: {n})",);
        }
    }
}

// are we in insert mode
pub fn in_insert_mode() -> LttwResult<bool> {
    assert_not_tokio_worker();
    let mode_result = api::get_mode()?;
    let mode_bytes = mode_result.mode.as_bytes();
    let mode_char = mode_bytes.first().copied().unwrap_or(b'?'); // Default to '?' if empty
    Ok(mode_char == b'i')
}

// are we in normal mode
pub fn in_normal_mode() -> LttwResult<bool> {
    assert_not_tokio_worker();
    let mode_result = api::get_mode()?;
    let mode_bytes = mode_result.mode.as_bytes();
    let mode_char = mode_bytes.first().copied().unwrap_or(b'?'); // Default to '?' if empty
    Ok(mode_char == b'n')
}

/// Get current buffer position ([0,0]-indexed)
pub fn get_pos() -> (usize, usize) {
    assert_not_tokio_worker();
    // Safety: handle cursor error gracefully
    let (line, col) = match Window::current().get_cursor() {
        Ok((l, c)) => (l, c),
        Err(_) => return (0, 0), // Return default position on error
    };

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
    assert_not_tokio_worker();
    // Safety: handle get_lines error gracefully
    // Use Buffer::current() directly in the match to avoid lifetime issues
    match Buffer::current().get_lines(line_range, false) {
        Ok(iter) => iter.map(|s| s.to_string()).collect(),
        // TODO log error
        Err(_) => Vec::new(), // Return empty vec on error
    }
}

/// Get buffer lines from Neovim
pub fn set_buf_lines<R>(line_range: R, replacement: Vec<String>) -> LttwResult<()>
where
    R: RangeBounds<usize>,
{
    assert_not_tokio_worker();

    let mut buf = Buffer::current();
    buf.set_lines(line_range, true, replacement)?;
    Ok(())
}

pub fn set_buf_extmark(
    ns_id: u32,
    line: usize,
    col: usize,
    opts: &SetExtmarkOpts,
) -> LttwResult<u32> {
    assert_not_tokio_worker();

    let mut buf = Buffer::current();
    Ok(buf.set_extmark(ns_id, line, col, opts)?)
}

pub fn del_buf_extmark(ns_id: u32, extmark_id: u32) -> LttwResult<()> {
    assert_not_tokio_worker();

    let mut buf = Buffer::current();
    buf.del_extmark(ns_id, extmark_id)?;
    Ok(())
}

/// Get buffer lines from Neovim
pub fn get_buf_line_count() -> usize {
    assert_not_tokio_worker();
    let buf = Buffer::current();
    buf.line_count().unwrap_or(0)
}

pub fn create_autocmd<'a, I>(events: I, opts: &CreateAutocmdOpts) -> LttwResult<u32>
where
    I: IntoIterator<Item = &'a str>,
{
    assert_not_tokio_worker();
    Ok(nvim_oxi::api::create_autocmd(events, opts)?)
}

pub fn del_autocmd(id: u32) -> LttwResult<()> {
    assert_not_tokio_worker();
    nvim_oxi::api::del_autocmd(id)?;
    Ok(())
}

pub fn get_var<Var>(name: &str) -> LttwResult<Var>
where
    Var: FromObject,
{
    assert_not_tokio_worker();
    Ok(nvim_oxi::api::get_var(name)?)
}

pub fn get_yanked_text() -> LttwResult<String> {
    assert_not_tokio_worker();
    // Get yanked text using vim.fn.getreg()
    // NOTE " is the default register for yanked text
    let reg_content: String =
        api::call_function("getreg", ("\"",)).unwrap_or_else(|_| String::new());
    Ok(reg_content)
}

/// Get buffer lines from Neovim
pub fn buffer_modified() -> bool {
    assert_not_tokio_worker();
    let buf = Buffer::current();
    // TODO test that this get_var is working
    let is_modified: bool = buf.get_var("modified").unwrap_or(false);
    is_modified
}

/// Get buffer lines from Neovim
pub fn get_buf_filename() -> LttwResult<String> {
    assert_not_tokio_worker();
    let buf = Buffer::current();
    let buf_file_name = buf.get_name()?;
    // convert to string
    let filename = buf_file_name.to_string_lossy().to_string();
    Ok(filename)
}
/// Get buffer lines from Neovim
pub fn buffer_active_and_readable() -> LttwResult<bool> {
    assert_not_tokio_worker();
    let buf = Buffer::current();
    let loaded = buf.is_loaded(); // acts like buf_listed
    let buf_file_name = buf.get_name()?;
    let is_readable = is_readable(buf_file_name.as_path());
    Ok(loaded && is_readable)
}

/// Get buffer lines from Neovim
/// pos_y is zero indexed
pub fn get_buf_line(pos_y: usize) -> String {
    assert_not_tokio_worker();
    let buf = Buffer::current();
    let Ok(lines) = buf.get_lines(pos_y..=pos_y, false) else {
        return "".to_string();
    };
    let lines: Vec<String> = lines.map(|s| s.to_string()).collect();
    lines.into_iter().next().unwrap_or_default()
}

/// Get current buffer
pub fn get_current_buffer_id() -> u64 {
    assert_not_tokio_worker();
    Buffer::current().handle().try_into().unwrap_or(0)
}

/// Get current buffer
pub fn clear_buf_namespace_objects(ns_id: u32) -> LttwResult<()> {
    assert_not_tokio_worker();
    let mut buf = Buffer::current();
    buf.clear_namespace(ns_id, ..)?;
    Ok(())
}

/// Set the window cursor,
/// pos_x and pos_y are 0 indexed
pub fn set_window_cursor(pos_x: usize, pos_y: usize) -> LttwResult<()> {
    assert_not_tokio_worker();
    let mut window = Window::current();
    window.set_cursor(pos_y + 1, pos_x)?;
    Ok(())
}

pub fn get_current_filetype() -> LttwResult<String> {
    assert_not_tokio_worker();
    let ft = get_option_value::<String>("filetype", &OptionOpts::default())?;
    Ok(ft)
}

// --------------------------

fn is_readable(path: &Path) -> bool {
    path.exists() && fs::metadata(path).map(|m| m.is_file()).unwrap_or(false)
}
/// Generate a random number in the range [i0, i1]
pub fn random_range(i0: usize, i1: usize) -> usize {
    let mut rng = rand::thread_rng();
    // Safety: ensure valid range
    if i0 > i1 {
        return i0; // Return lower bound if invalid range
    }
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
