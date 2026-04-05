// src/utils.rs - Utility functions
//
// This module provides various utility functions used throughout the plugin.

use {
    crate::{get_state, plugin_state::CurrentBufferInfo, LttwResult},
    ahash::AHasher,
    nvim_oxi::{
        api::{
            self, get_option_value,
            opts::{CreateAutocmdOpts, OptionOpts, SetExtmarkOpts, SetExtmarkOptsBuilder},
            types::ExtmarkVirtTextPosition,
            Buffer, Window,
        },
        conversion::FromObject,
    },
    rand::RngExt,
    std::{
        hash::Hasher,
        {backtrace::Backtrace, fs, ops::RangeBounds, path::Path},
    },
};

// NOTE important we cannot safely call into neovim from tokio worker threads
//
// https://github.com/noib3/nvim-oxi/issues/260
//     "Essentially never call neovim's functions outside of callbacks and plugin entrypoints and
//      never call neovim's functions from another thread."

fn assert_not_tokio_worker() {
    let t = std::thread::current();
    if let Some(n) = t.name()
        && n.contains("tokio")
    {
        let state = get_state();
        let bt = Backtrace::force_capture();
        state
            .debug_manager
            .read()
            .log("assert_not_tokio_worker", format!("Backtrace:\n{bt:?}"));

        panic!("function must not be called from Tokio runtime worker thread (name: {n})");
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

// are we in normal mode
pub fn get_mode_bz() -> LttwResult<Vec<u8>> {
    assert_not_tokio_worker();
    let mode_result = api::get_mode()?;
    Ok(mode_result.mode.as_bytes().to_vec())
}

/// Get current buffer position ([0,0]-indexed)
/// returns x position, y position
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

pub fn set_buf_extmark_top_right(ns_id: u32, message: String) -> LttwResult<u32> {
    assert_not_tokio_worker();

    let mut info_opts = SetExtmarkOptsBuilder::default();
    let info_virt_text = vec![(message, "Comment")];
    info_opts.virt_text(info_virt_text);

    // Use RightAlign positioning for the info string
    // This displays the info at the right side of the window
    info_opts.virt_text_pos(ExtmarkVirtTextPosition::RightAlign);

    info_opts.virt_text_pos(ExtmarkVirtTextPosition::RightAlign);
    let top_line: usize = api::call_function("line", ("w0",)).unwrap_or(0);
    let top_line = top_line.saturating_sub(1); // Adjust for 0-based indexing in Neovim API

    let mut buf = Buffer::current();
    Ok(buf.set_extmark(ns_id, top_line, 0, &info_opts.build())?)
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
// id, filename, is_modified, is_readable
pub fn get_current_buffer_info() -> LttwResult<CurrentBufferInfo> {
    assert_not_tokio_worker();
    let buf = Buffer::current();
    let buf_file_path = buf.get_name()?;
    // TODO test that this get_var is working
    let is_modified: bool = buf.get_var("modified").unwrap_or(false);
    let filepath = buf_file_path.to_string_lossy().to_string();
    let is_loaded = buf.is_loaded(); // acts like buf_listed
    let is_readable = is_readable(buf_file_path.as_path());
    let out = CurrentBufferInfo {
        filepath,
        is_modified,
        is_loaded,
        is_readable,
    };
    Ok(out)
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
    let mut rng = rand::rng();
    // Safety: ensure valid range
    if i0 > i1 {
        return i0; // Return lower bound if invalid range
    }
    rng.random_range(i0..=i1)
}

/// Compute SHA256 hash of a string
pub fn hash_input(input: &str) -> String {
    //let hash = Sha256::digest(input.as_bytes());
    let mut hasher = AHasher::default();
    hasher.write(input.as_bytes());
    let hash = hasher.finish();
    format!("{hash:x}")
}

/// Get current working directory
pub fn get_current_directory() -> String {
    std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| ".".to_string())
}

// --------------------------

pub fn filter_tail<'a>(arr1: &'a [String], arr2: &[String]) -> &'a [String] {
    let n = arr1.len();
    let m = arr2.len();

    // Find max k such that arr1[n-k..] == arr2[0..k]
    let mut max_k = 0;
    for k in 1..=m.min(n) {
        if arr1[n - k..].iter().eq(arr2[..k].iter()) {
            max_k = k;
        }
    }

    &arr1[..n - max_k]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_tail_example_1() {
        let arr1 = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let arr2 = vec!["C".to_string(), "D".to_string()];
        assert_eq!(
            filter_tail(&arr1, &arr2),
            vec!["A".to_string(), "B".to_string()]
        );
    }

    #[test]
    fn test_filter_tail_example_2() {
        let arr1 = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let arr2 = vec!["B".to_string(), "D".to_string()];
        assert_eq!(
            filter_tail(&arr1, &arr2),
            vec!["A".to_string(), "B".to_string(), "C".to_string()]
        );
    }

    #[test]
    fn test_filter_tail_example_3() {
        let arr1 = vec![
            "A".to_string(),
            "B".to_string(),
            "C".to_string(),
            "D".to_string(),
        ];
        let arr2 = vec!["C".to_string(), "D".to_string(), "E".to_string()];
        assert_eq!(
            filter_tail(&arr1, &arr2),
            vec!["A".to_string(), "B".to_string()]
        );
    }

    #[test]
    fn test_filter_tail_no_matches() {
        let arr1 = vec!["X".to_string(), "Y".to_string(), "Z".to_string()];
        let arr2 = vec!["A".to_string(), "B".to_string()];
        assert_eq!(filter_tail(&arr1, &arr2), arr1);
    }

    #[test]
    fn test_filter_tail_all_match_and_indices_satisfy() {
        let arr1 = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let arr2 = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        assert_eq!(filter_tail(&arr1, &arr2), Vec::<String>::new());
    }

    #[test]
    fn test_filter_tail_partial_match_at_end() {
        let arr1 = vec![
            "A".to_string(),
            "B".to_string(),
            "C".to_string(),
            "D".to_string(),
        ];
        let arr2 = vec!["B".to_string(), "C".to_string()];
        // arr2_idx: B->0, C->1
        // i=3, s="D": not in arr2 → break → suffix_len=0
        assert_eq!(filter_tail(&arr1, &arr2), arr1);
    }

    #[test]
    fn test_filter_tail_match_at_equal_index() {
        let arr1 = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let arr2 = vec!["B".to_string(), "C".to_string(), "D".to_string()];
        // arr2_idx: B->0, C->1, D->2
        // i=2, s="C": j=1 ≤ 2 → suffix_len=1
        // i=1, s="B": j=0 ≤ 1 → suffix_len=2
        // i=0, s="A": not in arr2 → break
        // suffix_len=2 → keep first 1 → ["A"]
        assert_eq!(filter_tail(&arr1, &arr2), vec!["A".to_string()]);
    }

    #[test]
    fn test_filter_tail_empty_arr2() {
        let arr1 = vec!["A".to_string(), "B".to_string()];
        let arr2: Vec<String> = vec![];
        assert_eq!(filter_tail(&arr1, &arr2), arr1);
    }

    #[test]
    fn test_filter_tail_empty_arr1() {
        let arr1: Vec<String> = vec![];
        let arr2 = vec!["A".to_string()];
        assert_eq!(filter_tail(&arr1, &arr2), Vec::<String>::new());
    }

    #[test]
    fn test_filter_tail_duplicate_in_arr2() {
        let arr1 = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let arr2 = vec!["B".to_string(), "B".to_string(), "C".to_string()];
        assert_eq!(filter_tail(&arr1, &arr2), arr1);
    }
    #[test]
    fn test_filter_tail_single_char_match() {
        let arr1 = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let arr2 = vec!["C".to_string()];
        assert_eq!(
            filter_tail(&arr1, &arr2),
            vec!["A".to_string(), "B".to_string()]
        );
    }

    #[test]
    fn test_filter_tail_longest_possible_match() {
        let arr1 = vec!["X".to_string(), "Y".to_string(), "Z".to_string()];
        let arr2 = vec!["Y".to_string(), "Z".to_string()];
        assert_eq!(filter_tail(&arr1, &arr2), vec!["X".to_string()]);
    }

    #[test]
    fn test_filter_tail_no_contiguous_match_but_noncontiguous_chars_match() {
        // arr1 = [A,B,C,D], arr2 = [A,C,D] — suffix [C,D] is not prefix of arr2
        let arr1 = vec![
            "A".to_string(),
            "B".to_string(),
            "C".to_string(),
            "D".to_string(),
        ];
        let arr2 = vec!["A".to_string(), "C".to_string(), "D".to_string()];
        assert_eq!(filter_tail(&arr1, &arr2), arr1); // no suffix of arr1 equals prefix of arr2
    }

    #[test]
    fn test_filter_tail_long_arr2() {
        // arr1 = [A,B,C,D], arr2 = [A,C,D] — suffix [C,D] is not prefix of arr2
        let arr1 = vec!["A".to_string(), "B".to_string()];
        let arr2 = vec![
            "B".to_string(),
            "C".to_string(),
            "D".to_string(),
            "E".to_string(),
            "F".to_string(),
            "G".to_string(),
            "H".to_string(),
        ];
        assert_eq!(filter_tail(&arr1, &arr2), vec!["A".to_string()]); // no suffix of arr1 equals prefix of arr2
    }
}
