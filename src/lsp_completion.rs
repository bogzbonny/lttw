use {
    crate::{
        plugin_state::strip_to_first_identifier,
        utils::{self, filter_tail_chars, get_buf_line, get_current_buffer_id, get_pos},
        PluginState, {FimCompletionMessage, FimResponse, LttwResult},
    },
    ahash::HashSet,
    regex::Regex,
};

/// Get LSP completions for the cursor position and log them
///
/// This is a test command to debug LSP completion behavior.
/// It uses Neovim's built-in LSP functionality via `vim.lsp.buf_request_sync`.
// TODO make async by using buf_request_all with a handler
// TODO make '500' here a param, (500ms max wait time for the result)
#[tracing::instrument]
pub fn trigger_lsp_completions_async() -> LttwResult<()> {
    utils::assert_not_tokio_worker();
    info!("Requesting LSP completions at current cursor position");

    // For testing directly in neovim
    // :lua vim.print(vim.lsp.buf_request_sync(0, 'textDocument/completion', vim.lsp.util.make_position_params(0, 'utf-8'), 1000))

    // get the completions through LUA.
    // NOTE we filter the content in lua before sending to rust and if we didn't we would be
    // encoding/decoding a huge amount of information (descriptions etc.)
    // NOTE must -1 from pos_y (line) as of dumb (1,0) indexing
    nvim_oxi::api::command(
        r#"lua
local bufnr = vim.api.nvim_get_current_buf()
local pos = vim.api.nvim_win_get_cursor(0)
local pos_y_ = math.max(0, pos[1] - 1)
local pos_x_ = pos[2]

vim.lsp.buf_request_all(bufnr, 'textDocument/completion', vim.lsp.util.make_position_params(0, 'utf-8'),
  function(responses)
    local items = {}
    for _, resp in ipairs(responses) do
      if resp.result and resp.result.items then
        for _, it in ipairs(resp.result.items) do
          table.insert(items, {
            text = it.textEdit and it.textEdit.newText or it.insertText or it.label,
            start_char = it.textEdit and it.textEdit.range.start.character or 0,
            start_line = it.textEdit and it.textEdit.range.start.line or 0
          })
        end
      end
    end
    local result = {
      items = items,
      pos_x = pos_x_,
      pos_y = pos_y_,
      buffer_id = bufnr
    }
    vim.g.lttw_completion = vim.json.encode(result)
  end)
"#,
    )?;
    Ok(())
}

#[tracing::instrument(skip(state))]
pub fn retrieve_lsp_completions(state: &PluginState) -> LttwResult<Vec<FimCompletionMessage>> {
    let Ok(json_str) = nvim_oxi::api::get_var::<String>("lttw_completion") else {
        return Ok(vec![]); // no completions available, nbd
    };
    nvim_oxi::api::del_var("lttw_completion")?; // clear the var now that we've gotten it

    //info!("retrieved lsp completions: {}", json_str);
    let (truncate_vars, insert_one_var) = {
        let config = state.config.read();
        (
            config.lsp_comp_truncate_vars,
            config.lsp_comp_insert_one_var,
        )
    };

    let response: utils::CompletionResponse = serde_json::from_str(&json_str)?;
    //info!("response: {:?}", response);

    let (pos_x, pos_y) = (response.pos_x, response.pos_y);
    let (x, y) = get_pos();
    if pos_x != x || pos_y != y {
        info!("retrieve_lsp_completions");
        return Ok(vec![]);
    };
    if get_current_buffer_id() != response.buffer_id {
        info!("retrieve_lsp_completions");
        return Ok(vec![]);
    }
    info!("retrieve_lsp_completions");
    let line_cur = get_buf_line(pos_y);
    let suffix = line_cur.chars().skip(pos_x).collect::<String>();
    let mut seen = HashSet::default();
    let non_split_chs: Vec<char> = vec!['_', '.', '(', '[', '{', '<', '>', ')', '}', ']']; // TODO parameterize

    let mut last_start_char = None;

    // we try not to recompute next_var unless the start_chars change (rare)
    // start with not computing it at all to begin with and only compute
    // it on the first time we ACTUALLY would need to
    let mut next_var: Option<Option<String>> = None;

    let mut filtered_comps: Vec<(FimCompletionMessage, u64)> = response
        .items
        .into_iter()
        .filter_map(|comp| {
            if comp.start_char > pos_x || comp.start_line != pos_y {
                return None;
            }

            let prefix = line_cur
                .chars()
                .skip(comp.start_char)
                .take(pos_x - comp.start_char)
                .collect::<String>();

            if let Some(last_start_char) = last_start_char
                && last_start_char != comp.start_char
            {
                next_var = None;
            }
            last_start_char = Some(comp.start_char);

            let text = trim_completion(
                comp.text.as_str(),
                &prefix,
                &suffix,
                truncate_vars,
                insert_one_var,
                &non_split_chs,
                &mut next_var,
            )?;

            let span = tracing::span!(tracing::Level::DEBUG, "lsp completion");
            let _enter = span.enter();
            debug!(comp.text);
            debug!(suffix);
            debug!(prefix);
            debug!(next_var);

            // filter out duplicates
            if seen.contains(&text) {
                return None;
            }
            seen.insert(text.clone());

            // NOTE use the full suggestion here, NOT the prefix stripped text!
            let ident = strip_to_first_identifier(&comp.text);
            let usage = state.get_word_statistic_usage(&ident);

            let fim_resp = FimResponse {
                content: text,
                timings: None,
                tokens_cached: 0,
                truncated: false,
            };
            info!("text: {}, usage: {}", fim_resp.content, usage);

            Some((
                FimCompletionMessage {
                    buffer_id: response.buffer_id,
                    line_cur: line_cur.clone(),
                    cursor_x: pos_x,
                    cursor_y: pos_y,
                    completion: fim_resp, // All available completions for cycling
                    do_render: true,
                    retry: None,
                },
                usage,
            ))
        })
        .collect();

    // first reverse the order so that earlier items suggested by the LSP have higher priority
    filtered_comps.reverse();

    // sort the completions from least common to most common
    // NOTE later FimCompletionMessages are considered higher priority
    filtered_comps.sort_by(|a, b| a.1.cmp(&b.1));
    let filtered_comps: Vec<_> = filtered_comps.into_iter().map(|x| x.0).collect();

    // save in caches
    //let hashes = compute_hashes(&ctx.prefix, &ctx.middle, &ctx.suffix);
    //let mut cache_lock = state.cache.write();
    //for hash in &hashes {
    //    cache_lock.insert(hash.clone(), resp.clone());
    //}
    info!("retrieve_lsp_completions");

    Ok(filtered_comps)
}

/// Check if all open brackets match closed brackets for < [ ( { characters
///
/// Uses a stack-based approach to track opening brackets and verify they match
/// with corresponding closing brackets in the correct order.
pub fn brackets_matching(s: &str) -> bool {
    let mut stack: Vec<char> = Vec::new();

    for c in s.chars() {
        match c {
            '<' | '[' | '(' | '{' => stack.push(c),
            '>' => {
                if stack.pop() != Some('<') {
                    return false;
                }
            }
            ']' => {
                if stack.pop() != Some('[') {
                    return false;
                }
            }
            ')' => {
                if stack.pop() != Some('(') {
                    return false;
                }
            }
            '}' => {
                if stack.pop() != Some('{') {
                    return false;
                }
            }
            _ => {}
        }
    }

    // Stack should be empty if all brackets matched
    stack.is_empty()
}

// NOTE this DOESN'T trim trailing open brackets!
// assert_eq!(trim_trailing_unmatched_closing_brackets("((var))("), "((var))(");
// assert_eq!(trim_trailing_unmatched_closing_brackets("((var))(("), "((var))((");
fn trim_trailing_unmatched_closing_brackets(s: &str) -> String {
    let mut chars: Vec<char> = s.chars().collect();
    let mut matched = vec![false; chars.len()];
    let mut stack = Vec::new();

    // Match brackets: record matched pairs
    for (i, &c) in chars.iter().enumerate() {
        match c {
            '(' | '[' | '{' | '<' => stack.push(i),
            ')' if matches!(stack.last(), Some(&j) if chars[j] == '(') => {
                matched[stack.pop().unwrap()] = true;
                matched[i] = true;
            }
            ']' if matches!(stack.last(), Some(&j) if chars[j] == '[') => {
                matched[stack.pop().unwrap()] = true;
                matched[i] = true;
            }
            '}' if matches!(stack.last(), Some(&j) if chars[j] == '{') => {
                matched[stack.pop().unwrap()] = true;
                matched[i] = true;
            }
            '>' if matches!(stack.last(), Some(&j) if chars[j] == '<') => {
                matched[stack.pop().unwrap()] = true;
                matched[i] = true;
            }
            _ => {}
        }
    }

    // Trim trailing unmatched closing brackets
    let mut end = chars.len();
    while end > 0 {
        let i = end - 1;
        match chars[i] {
            ')' | ']' | '}' | '>' if !matched[i] => end = i,
            _ => break,
        }
    }

    chars.truncate(end);
    chars.into_iter().collect()
}

fn trim_completion(
    completion: &str,
    prefix: &str,
    suffix: &str,
    truncate: bool,
    insert_one_var: bool,
    non_split_chs: &[char],
    next_var: &mut Option<Option<String>>,
) -> Option<String> {
    // Only complete partially written text
    if prefix.is_empty() {
        return None;
    }
    // only keep if strips the prefix
    let mut text = completion.strip_prefix(prefix).map(|s| s.to_string())?;

    // TODO use some of these autocompletion details better rather than just
    // truncating
    // - it would be nice to be able to accept Some_fn(...) and keep the closing backet
    // - something which only takes one arg, should automatically be filled in eg.
    //    typing Ok[CUR]some_var  then pressing tab should autocomplete to Ok(some_var)
    // Find the first occurrence of:
    // - $NN (where NN is 1-2 digits)
    // - ${NN:...} with optional ", " at end
    // and truncate at that position
    let re = Regex::new(r"\$\d{1,2}|\$\{\d{1,2}:[^}]*,?\s*\}").unwrap();
    // Regex for ${NN:...} patterns only
    let curly_re = Regex::new(r"\$\{\d{1,2}:[^}]*,?\s*\}").unwrap();

    if truncate {
        // Truncate at first marker (current behavior)
        if let Some(mat) = re.find(&text) {
            text.truncate(mat.start());
        }
    } else {
        // strip the final $0 if it exists
        text = text.strip_suffix("$0").unwrap_or(&text).to_string();

        // Remove all matches and record position of first match
        let mut first_match_pos: Option<usize> = None;
        let matches: Vec<_> = re.find_iter(&text).collect();

        // Find position of first match
        if let Some(mat) = matches.first() {
            first_match_pos = Some(mat.start());
        }

        // Count curly matches and dollar-only matches
        let curly_matches: Vec<_> = curly_re.find_iter(&text).collect();
        let curly_count = curly_matches.len();

        // Count only $NN matches (not ${...})
        // Find all $NN matches and filter out those that are part of ${...}
        let dollar_simple_re = Regex::new(r"\$\d{1,2}").unwrap();
        let dollar_only_matches: Vec<_> = dollar_simple_re
            .find_iter(&text)
            .filter(|mat| {
                // Check if this match is NOT inside a ${...} pattern
                // The character immediately before the $ should not be {
                let start_pos = mat.start();
                start_pos == 0 || text.chars().nth(start_pos - 1) != Some('{')
            })
            .collect();
        let dollar_only_count = dollar_only_matches.len();

        // Determine if next_var should be inserted:
        // - Exactly 1 curly match (${NN:...}), OR
        // - No curly matches AND exactly 1 dollar-only match ($NN)
        let should_insert_next_var =
            curly_count == 1 || (curly_count == 0 && dollar_only_count == 1);

        // Remove all matches by iterating in reverse to maintain positions
        let mut text_to_modify = text.clone();
        for mat in matches.iter().rev() {
            text_to_modify.replace_range(mat.start()..mat.end(), "");
        }

        text = text_to_modify;

        // Determine what to insert at first match position
        if let Some(first_pos) = first_match_pos
            && first_pos < text.len()
        {
            if should_insert_next_var {
                // get the next word chunk seperated by space/ from the suffix

                // compute next_var if it doesn't exist yet
                if next_var.is_none() {
                    let to_write = if insert_one_var {
                        suffix
                            .split(|c: char| !(c.is_alphanumeric() || non_split_chs.contains(&c)))
                            .next()
                            .map(trim_trailing_unmatched_closing_brackets)
                            .filter(|next_var| brackets_matching(next_var))
                            .map(|s| s.to_string())
                    } else {
                        None
                    };
                    *next_var = Some(to_write);
                }

                match next_var {
                    Some(Some(v)) => {
                        text.insert_str(first_pos, v);
                    }
                    Some(None) => {
                        text.insert(first_pos, '…');
                    }
                    None => {
                        error!("impossible");
                    }
                }
            } else {
                text.insert(first_pos, '…');
            }
        }
    }

    // discard multiline
    if let Some(pos) = text.find('\n') {
        text.truncate(pos);
    }

    // trim the end of the completion for any matching suffix chars
    let text = filter_tail_chars(&text, suffix); // strip the prefix from the text
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trim_completion_basic_strip_prefix() {
        // Basic case: prefix is stripped, no suffix match
        let completion = "HelloWorld";
        let prefix = "Hello";
        let suffix = "";
        assert_eq!(
            trim_completion(completion, prefix, suffix, true, None),
            Some("World".to_string())
        );
    }

    #[test]
    fn test_trim_completion_with_dollar_zero() {
        // $0 marker should truncate the text at that position
        let completion = "Hello$0World";
        let prefix = "Hello";
        let suffix = "";
        // After stripping prefix: "$0World", find("$0") returns 0, truncate(0) gives ""
        assert_eq!(
            trim_completion(completion, prefix, suffix, true, None),
            Some("".to_string())
        );
    }

    #[test]
    fn test_trim_completion_with_dollar_zero_after_content() {
        // $0 marker after content should truncate at that position
        let completion = "HelloWorld$0";
        let prefix = "Hello";
        let suffix = "";
        // After stripping prefix: "World$0", find("$0") returns 5, truncate(5) gives "World"
        assert_eq!(
            trim_completion(completion, prefix, suffix, true, None),
            Some("World".to_string())
        );
    }

    #[test]
    fn test_trim_completion_with_dollar_curly_brace() {
        // ${ marker should truncate the text at that position
        let completion = "Hello${1:World}";
        let prefix = "Hello";
        let suffix = "";
        // After stripping prefix: "${1:World}", find("${") returns 0, truncate(0) gives ""
        assert_eq!(
            trim_completion(completion, prefix, suffix, true, None),
            Some("".to_string())
        );
    }

    #[test]
    fn test_trim_completion_with_dollar_curly_brace_after_content() {
        // ${ marker after content should truncate at that position
        let completion = "HelloWorld${1:more}";
        let prefix = "Hello";
        let suffix = "";
        // After stripping prefix: "World${1:more}", find("${") returns 5, truncate(5) gives "World"
        assert_eq!(
            trim_completion(completion, prefix, suffix, true, None),
            Some("World".to_string())
        );
    }

    #[test]
    fn test_trim_completion_with_dollar_one() {
        // $1 marker should truncate the text at that position
        let completion = "Hello$1World";
        let prefix = "Hello";
        let suffix = "";
        // After stripping prefix: "$1World", find("$1") returns 0, truncate(0) gives ""
        assert_eq!(
            trim_completion(completion, prefix, suffix, true, None),
            Some("".to_string())
        );
    }

    #[test]
    fn test_trim_completion_with_dollar_one_after_content() {
        // $1 marker after content should truncate at that position
        let completion = "HelloWorld$1";
        let prefix = "Hello";
        let suffix = "";
        // After stripping prefix: "World$1", find("$1") returns 5, truncate(5) gives "World"
        assert_eq!(
            trim_completion(completion, prefix, suffix, true, None),
            Some("World".to_string())
        );
    }

    #[test]
    fn test_trim_completion_with_dollar_two() {
        // $2 marker should truncate the text at that position
        let completion = "Hello$2World";
        let prefix = "Hello";
        let suffix = "";
        // After stripping prefix: "$2World", find("$2") returns 0, truncate(0) gives ""
        assert_eq!(
            trim_completion(completion, prefix, suffix, true, None),
            Some("".to_string())
        );
    }

    #[test]
    fn test_trim_completion_with_dollar_two_after_content() {
        // $2 marker after content should truncate at that position
        let completion = "HelloWorld$2";
        let prefix = "Hello";
        let suffix = "";
        // After stripping prefix: "World$2", find("$2") returns 5, truncate(5) gives "World"
        assert_eq!(
            trim_completion(completion, prefix, suffix, true, None),
            Some("World".to_string())
        );
    }

    #[test]
    fn test_trim_completion_multiline_truncates() {
        // Multiline text should be truncated at newline
        let completion = "Hello\nWorld";
        let prefix = "Hello";
        let suffix = "";
        // After stripping prefix: "\nWorld", find('\n') returns 0, truncate(0) gives ""
        assert_eq!(
            trim_completion(completion, prefix, suffix, true, None),
            Some("".to_string())
        );
    }

    #[test]
    fn test_trim_completion_multiline_with_content_before_newline() {
        // Multiline text with content before newline
        let completion = "HelloWorld\nMore";
        let prefix = "Hello";
        let suffix = "";
        // After stripping prefix: "World\nMore", find('\n') returns 5, truncate(5) gives "World"
        assert_eq!(
            trim_completion(completion, prefix, suffix, true, None),
            Some("World".to_string())
        );
    }

    #[test]
    fn test_trim_completion_with_dollar_two_digits() {
        // $55 marker (two digits) should truncate at that position
        let completion = "HelloWorld$55";
        let prefix = "Hello";
        let suffix = "";
        // After stripping prefix: "World$55", find("$55") returns 5, truncate(5) gives "World"
        assert_eq!(
            trim_completion(completion, prefix, suffix, true, None),
            Some("World".to_string())
        );
    }

    #[test]
    fn test_trim_completion_with_dollar_single_digit() {
        // $7 marker (single digit) should truncate at that position
        let completion = "HelloWorld$7";
        let prefix = "Hello";
        let suffix = "";
        // After stripping prefix: "World$7", find("$7") returns 5, truncate(5) gives "World"
        assert_eq!(
            trim_completion(completion, prefix, suffix, true, None),
            Some("World".to_string())
        );
    }

    #[test]
    fn test_trim_completion_with_dollar_zero_single_digit() {
        // $0 marker (single digit) should still work like before
        let completion = "Hello$0World";
        let prefix = "Hello";
        let suffix = "";
        // After stripping prefix: "$0World", find("$0") returns 0, truncate(0) gives ""
        assert_eq!(
            trim_completion(completion, prefix, suffix, true, None),
            Some("".to_string())
        );
    }

    #[test]
    fn test_trim_completion_first_marker_wins() {
        // When multiple markers exist, only the first one should truncate
        let completion = "HelloWorld$10More$55End";
        let prefix = "Hello";
        let suffix = "";
        // After stripping prefix: "World$10More$55End", find("$10") returns 5, truncate(5) gives "World"
        assert_eq!(
            trim_completion(completion, prefix, suffix, true, None),
            Some("World".to_string())
        );
    }

    #[test]
    fn test_trim_completion_dollar_marker_vs_suffix() {
        // $NN marker should truncate before suffix matching
        let completion = "HelloWorld$1";
        let prefix = "Hello";
        let suffix = "World";
        // After stripping prefix: "World$1", find("$1") returns 5, truncate(5) gives "World"
        // Then suffix matching removes "World" because it matches suffix
        assert_eq!(
            trim_completion(completion, prefix, suffix, true, None),
            Some("".to_string())
        );
    }

    #[test]
    fn test_trim_completion_dollar_marker_with_partial_suffix_match() {
        // $NN marker with partial suffix match
        let completion = "HelloWorld$55";
        let prefix = "Hello";
        let suffix = "orld";
        // After stripping prefix: "World$55", find("$55") returns 5, truncate(5) gives "World"
        // Then suffix matching removes "orld" because it matches suffix end
        assert_eq!(
            trim_completion(completion, prefix, suffix, true, None),
            Some("W".to_string())
        );
    }

    #[test]
    fn test_trim_completion_with_dollar_curly_brace_nn_anything() {
        // ${NN:anything} marker should truncate at that position
        let completion = "Hello${1:World}";
        let prefix = "Hello";
        let suffix = "";
        // After stripping prefix: "${1:World}", find("${1:World}") returns 0, truncate(0) gives ""
        assert_eq!(
            trim_completion(completion, prefix, suffix, true, None),
            Some("".to_string())
        );
    }

    #[test]
    fn test_trim_completion_with_dollar_curly_brace_nn_anything_after_content() {
        // ${NN:anything} marker after content should truncate at that position
        let completion = "HelloWorld${1:more}";
        let prefix = "Hello";
        let suffix = "";
        // After stripping prefix: "World${1:more}", find("${1:more}") returns 5, truncate(5) gives "World"
        assert_eq!(
            trim_completion(completion, prefix, suffix, true, None),
            Some("World".to_string())
        );
    }

    #[test]
    fn test_trim_completion_with_dollar_curly_brace_nn_anything_two_digits() {
        // ${NN:anything} marker with two digits should truncate at that position
        let completion = "HelloWorld${55:more}";
        let prefix = "Hello";
        let suffix = "";
        // After stripping prefix: "World${55:more}", find("${55:more}") returns 5, truncate(5) gives "World"
        assert_eq!(
            trim_completion(completion, prefix, suffix, true, None),
            Some("World".to_string())
        );
    }

    #[test]
    fn test_trim_completion_with_dollar_curly_brace_nn_anything_comma_space() {
        // ${NN:anything, } marker with comma space at end should truncate at that position
        let completion = "HelloWorld${1:more, }";
        let prefix = "Hello";
        let suffix = "";
        // After stripping prefix: "World${1:more, }", find("${1:more, }") returns 5, truncate(5) gives "World"
        assert_eq!(
            trim_completion(completion, prefix, suffix, true, None),
            Some("World".to_string())
        );
    }

    #[test]
    fn test_trim_completion_with_dollar_curly_brace_nn_anything_comma_space_no_trailing() {
        // ${NN:anything, } marker without actual comma space should still match
        let completion = "HelloWorld${1:more}";
        let prefix = "Hello";
        let suffix = "";
        // After stripping prefix: "World${1:more}", find("${1:more}") returns 5, truncate(5) gives "World"
        assert_eq!(
            trim_completion(completion, prefix, suffix, true, None),
            Some("World".to_string())
        );
    }

    #[test]
    fn test_trim_completion_dollar_curly_vs_suffix() {
        // ${NN:anything, } marker should truncate before suffix matching
        let completion = "HelloWorld${1:more, }";
        let prefix = "Hello";
        let suffix = "World";
        // After stripping prefix: "World${1:more, }", find("${1:more, }") returns 5, truncate(5) gives "World"
        // Then suffix matching removes "World" because it matches suffix
        assert_eq!(
            trim_completion(completion, prefix, suffix, true, None),
            Some("".to_string())
        );
    }

    #[test]
    fn test_trim_completion_dollar_curly_vs_suffix_partial() {
        // ${NN:anything, } marker with partial suffix match
        let completion = "HelloWorld${55:more}";
        let prefix = "Hello";
        let suffix = "orld";
        // After stripping prefix: "World${55:more}", find("${55:more}") returns 5, truncate(5) gives "World"
        // Then suffix matching removes "orld" because it matches suffix end
        assert_eq!(
            trim_completion(completion, prefix, suffix, true, None),
            Some("W".to_string())
        );
    }

    // Tests for truncate: false mode
    #[test]
    fn test_trim_completion_truncate_false_basic() {
        // In truncate: false mode, remove all matches and record position of first match
        let completion = "HelloWorld$1More";
        let prefix = "Hello";
        let suffix = "";
        // After stripping prefix: "World$1More", remove $1 -> "WorldMore"
        // First match was at position 5, and it's not at end, so insert "…" at position 5
        assert_eq!(
            trim_completion(completion, prefix, suffix, false, None),
            Some("World…More".to_string())
        );
    }

    #[test]
    fn test_trim_completion_truncate_false_at_end() {
        // When first match is at end, no ellipsis should be added
        let completion = "HelloWorld$1";
        let prefix = "Hello";
        let suffix = "";
        // After stripping prefix: "World$1", remove $1 -> "World"
        // First match was at position 5, and it IS at end (5 == 5), so no ellipsis
        assert_eq!(
            trim_completion(completion, prefix, suffix, false, None),
            Some("World".to_string())
        );
    }

    #[test]
    fn test_trim_completion_truncate_false_multiple_markers() {
        // Multiple markers: remove all and record first position
        let completion = "HelloWorld$1More$55End";
        let prefix = "Hello";
        let suffix = "";
        // After stripping prefix: "World$1More$55End"
        // Remove all markers: "WorldMoreEnd"
        // First match was at position 5
        assert_eq!(
            trim_completion(completion, prefix, suffix, false, None),
            Some("World…MoreEnd".to_string())
        );
    }

    #[test]
    fn test_trim_completion_truncate_false_marker_at_start() {
        // Marker at start of stripped text
        let completion = "Hello$1World";
        let prefix = "Hello";
        let suffix = "";
        // After stripping prefix: "$1World", remove $1 -> "World"
        // First match was at position 0, and it's not at end (0 < 5), so insert "…" at position 0
        assert_eq!(
            trim_completion(completion, prefix, suffix, false, None),
            Some("…World".to_string())
        );
    }

    #[test]
    fn test_trim_completion_truncate_false_curly_markers() {
        // ${NN:...} markers in truncate: false mode
        let completion = "HelloWorld${1:more}End";
        let prefix = "Hello";
        let suffix = "";
        // After stripping prefix: "World${1:more}End"
        // Remove ${1:more} -> "WorldEnd"
        // First match was at position 5
        assert_eq!(
            trim_completion(completion, prefix, suffix, false, None),
            Some("World…End".to_string())
        );
    }

    #[test]
    fn test_trim_completion_truncate_false_with_suffix_match() {
        // Suffix matching should still apply in truncate: false mode
        let completion = "HelloWorld$1";
        let prefix = "Hello";
        let suffix = "World";
        // After stripping prefix: "World$1", remove $1 -> "World"
        // First match was at position 5, and it IS at end (5 == 5), so no ellipsis
        // Then suffix matching removes "World" because it matches suffix
        assert_eq!(
            trim_completion(completion, prefix, suffix, false, None),
            Some("".to_string())
        );
    }

    #[test]
    fn test_trim_completion_truncate_false_partial_suffix_match() {
        // Partial suffix match in truncate: false mode
        let completion = "HelloWorld$55";
        let prefix = "Hello";
        let suffix = "orld";
        // After stripping prefix: "World$55", remove $55 -> "World"
        // First match was at position 5, and it IS at end (5 == 5), so no ellipsis
        // Then suffix matching removes "orld" because it matches suffix end
        assert_eq!(
            trim_completion(completion, prefix, suffix, false, None),
            Some("W".to_string())
        );
    }

    // Tests for next_var parameter
    #[test]
    fn test_trim_completion_next_var_single_curly_match() {
        // Single ${NN:...} match with next_var Some - insert next_var content
        let completion = "HelloWorld${1:more}End";
        let prefix = "Hello";
        let suffix = "";
        let next_var = Some("replaced".to_string());
        // After stripping prefix: "World${1:more}End"
        // Remove ${1:more} -> "WorldEnd"
        // First match was at position 5, exactly 1 curly match, so insert next_var
        assert_eq!(
            trim_completion(completion, prefix, suffix, false, next_var),
            Some("WorldreplacedEnd".to_string())
        );
    }

    #[test]
    fn test_trim_completion_next_var_multiple_curly_matches() {
        // Multiple ${NN:...} matches with next_var Some - use ellipsis instead
        let completion = "HelloWorld${1:more}End${2:another}";
        let prefix = "Hello";
        let suffix = "";
        let next_var = Some("replaced".to_string());
        // After stripping prefix: "World${1:more}End${2:another}"
        // Remove both curly matches -> "WorldEnd"
        // 2 curly matches, so use ellipsis at first position (5)
        assert_eq!(
            trim_completion(completion, prefix, suffix, false, next_var),
            Some("World…End".to_string())
        );
    }

    #[test]
    fn test_trim_completion_next_var_single_curly_with_none() {
        // Single ${NN:...} match with next_var None - use ellipsis
        let completion = "HelloWorld${1:more}End";
        let prefix = "Hello";
        let suffix = "";
        let next_var = None;
        // After stripping prefix: "World${1:more}End"
        // Remove ${1:more} -> "WorldEnd"
        // First match was at position 5, so insert "…" at position 5
        assert_eq!(
            trim_completion(completion, prefix, suffix, false, next_var),
            Some("World…End".to_string())
        );
    }

    #[test]
    fn test_trim_completion_next_var_only_dollar_markers() {
        // Only $NN markers (no curly) with next_var Some - insert next_var
        // Per new requirement: NO curly matches + exactly 1 $NN match = insert next_var
        let completion = "HelloWorld$1More";
        let prefix = "Hello";
        let suffix = "";
        let next_var = Some("replaced".to_string());
        // After stripping prefix: "World$1More"
        // Remove $1 -> "WorldMore"
        // 0 curly matches, 1 dollar match ($1), so insert next_var at position 5
        assert_eq!(
            trim_completion(completion, prefix, suffix, false, next_var),
            Some("WorldreplacedMore".to_string())
        );
    }

    #[test]
    fn test_trim_completion_next_var_curly_at_end() {
        // Single ${NN:...} match at end of stripped text - no ellipsis/insertion needed
        let completion = "HelloWorld${1:more}";
        let prefix = "Hello";
        let suffix = "";
        let next_var = Some("replaced".to_string());
        // After stripping prefix: "World${1:more}"
        // Remove ${1:more} -> "World"
        // First match was at position 5, and it IS at end (5 == 5), so no insertion
        assert_eq!(
            trim_completion(completion, prefix, suffix, false, next_var),
            Some("World".to_string())
        );
    }

    // Tests for brackets_matching function
    #[test]
    fn test_brackets_matching_empty() {
        // Empty string should be valid
        assert!(brackets_matching(""));
    }

    #[test]
    fn test_brackets_matching_single_brackets() {
        // Single pairs should match
        assert!(brackets_matching("<>"));
        assert!(brackets_matching("[]"));
        assert!(brackets_matching("()"));
        assert!(brackets_matching("{}"));
    }

    #[test]
    fn test_brackets_matching_nested() {
        // Nested brackets should match
        assert!(brackets_matching("<[()]>"));
        assert!(brackets_matching("{[()]}"));
        assert!(brackets_matching("(<{}>)"));
    }

    #[test]
    fn test_brackets_matching_multiple() {
        // Multiple sibling brackets should match
        assert!(brackets_matching("<><>"));
        assert!(brackets_matching("[][]"));
        assert!(brackets_matching("()()"));
        assert!(brackets_matching("{}{}"));
    }

    #[test]
    fn test_brackets_matching_mismatched() {
        // Mismatched brackets should fail
        assert!(!brackets_matching("<]"));
        assert!(!brackets_matching("[>"));
        assert!(!brackets_matching("{)"));
        assert!(!brackets_matching("(}"));
    }

    #[test]
    fn test_brackets_matching_unclosed() {
        // Unclosed brackets should fail
        assert!(!brackets_matching("<"));
        assert!(!brackets_matching("["));
        assert!(!brackets_matching("("));
        assert!(!brackets_matching("{"));
    }

    #[test]
    fn test_brackets_matching_unopened() {
        // Unopened brackets should fail
        assert!(!brackets_matching(">"));
        assert!(!brackets_matching("]"));
        assert!(!brackets_matching(")"));
        assert!(!brackets_matching("}"));
    }

    #[test]
    fn test_brackets_matching_with_text() {
        // Brackets with text in between should work
        assert!(brackets_matching("func(a, b)"));
        assert!(brackets_matching("arr[index]"));
        assert!(brackets_matching("func<a, b>()"));
        assert!(brackets_matching("{ [ ( < > ) ] }"));
        assert!(brackets_matching("if (x) { return true; }"));
    }

    #[test]
    fn test_brackets_matching_wrong_order() {
        // Wrong order should fail
        assert!(!brackets_matching(">["));
        assert!(!brackets_matching(")("));
        assert!(!brackets_matching("}{"));
        assert!(!brackets_matching("]["));
    }

    #[test]
    fn test_brackets_matching_complex_valid() {
        // Complex valid nested structures
        assert!(brackets_matching("{[<(abc)>]}"));
        assert!(brackets_matching("class<T> { method() { <T> } }"));
    }

    #[test]
    fn test_basic_trimming() {
        assert_eq!(trim_trailing_unmatched_closing_brackets("var"), "var");
        assert_eq!(trim_trailing_unmatched_closing_brackets("var)"), "var");
        assert_eq!(trim_trailing_unmatched_closing_brackets("var))"), "var");
        assert_eq!(trim_trailing_unmatched_closing_brackets("var())"), "var()");
        assert_eq!(trim_trailing_unmatched_closing_brackets("var()"), "var()");
        assert_eq!(trim_trailing_unmatched_closing_brackets("var[]"), "var[]");
        assert_eq!(trim_trailing_unmatched_closing_brackets("var{}"), "var{}");
        assert_eq!(trim_trailing_unmatched_closing_brackets("var<>"), "var<>");
    }

    #[test]
    fn test_mismatched_brackets() {
        assert_eq!(trim_trailing_unmatched_closing_brackets("var)"), "var");
        assert_eq!(trim_trailing_unmatched_closing_brackets("var]"), "var");
        assert_eq!(trim_trailing_unmatched_closing_brackets("var}"), "var");
        assert_eq!(trim_trailing_unmatched_closing_brackets("var>"), "var");
        assert_eq!(trim_trailing_unmatched_closing_brackets("var)]"), "var");
        assert_eq!(trim_trailing_unmatched_closing_brackets("var}]"), "var");
        assert_eq!(trim_trailing_unmatched_closing_brackets("var}>"), "var");
    }

    #[test]
    fn test_nested_matching_brackets_with_trailing_unmatched() {
        assert_eq!(
            trim_trailing_unmatched_closing_brackets("a(b[c{d}e]f)"),
            "a(b[c{d}e]f)"
        );
        assert_eq!(
            trim_trailing_unmatched_closing_brackets("a(b[c{d}e]f))"),
            "a(b[c{d}e]f)"
        );
        assert_eq!(
            trim_trailing_unmatched_closing_brackets("a(b[c{d}e]f)))"),
            "a(b[c{d}e]f)"
        );
        assert_eq!(
            trim_trailing_unmatched_closing_brackets("a(b[c{d}e]f)))g"),
            "a(b[c{d}e]f)))g"
        );
    }

    #[test]
    fn test_unbalanced_with_partial_trim() {
        assert_eq!(trim_trailing_unmatched_closing_brackets("(var))"), "(var)");
        assert_eq!(
            trim_trailing_unmatched_closing_brackets("((var)))"),
            "((var))"
        );

        // NOTE the following should not match as they are openning brackets
        assert_eq!(
            trim_trailing_unmatched_closing_brackets("((var))("),
            "((var))("
        );
        assert_eq!(
            trim_trailing_unmatched_closing_brackets("((var))(("),
            "((var))(("
        );
    }

    #[test]
    fn test_angle_brackets() {
        assert_eq!(trim_trailing_unmatched_closing_brackets("a<b>"), "a<b>");
        assert_eq!(trim_trailing_unmatched_closing_brackets("a<b>>"), "a<b>");
        assert_eq!(trim_trailing_unmatched_closing_brackets("a<b>>>"), "a<b>");
        assert_eq!(trim_trailing_unmatched_closing_brackets("a<b>>c"), "a<b>>c");
    }

    #[test]
    fn test_empty_and_edge_cases() {
        assert_eq!(trim_trailing_unmatched_closing_brackets(""), "");
        assert_eq!(trim_trailing_unmatched_closing_brackets(")"), "");
        assert_eq!(trim_trailing_unmatched_closing_brackets("))"), "");
        assert_eq!(trim_trailing_unmatched_closing_brackets("()"), "()");
    }
}
