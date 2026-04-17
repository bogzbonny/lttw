use {
    super::render::trim_suggestion_and_suffix_on_curr_line,
    crate::{
        LttwResult,
        fim::fim_try_hint_skip_debounce,
        fim_hide_inner,
        plugin_state::get_state,
        utils::{get_current_buffer_id, set_buf_lines, set_window_cursor},
    },
};

#[derive(Clone, Debug)]
pub enum FimAcceptType {
    Full,
    Line,
    Word,
}

impl std::fmt::Display for FimAcceptType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FimAcceptType::Full => write!(f, "full"),
            FimAcceptType::Line => write!(f, "line"),
            FimAcceptType::Word => write!(f, "word"),
        }
    }
}

/// FIM accept function - accepts the FIM suggestion
//
/// NOTE Processed on main Neovim thread
#[tracing::instrument]
pub fn fim_accept(accept_type: FimAcceptType) -> LttwResult<()> {
    // Log before releasing the lock
    let state = get_state();
    info!("fim_accept_triggered for {}", accept_type);

    let (hint_shown, pos_x, pos_y, line_cur, content) = {
        let fim_state_lock = state.fim_state.read();
        (
            fim_state_lock.hint_shown,
            fim_state_lock.pos_x,
            fim_state_lock.pos_y,
            fim_state_lock.line_cur.clone(),
            fim_state_lock.content.clone(),
        )
    };

    if !hint_shown {
        return Ok(());
    }

    info!("Accepting {} suggestion", accept_type);

    let (new_x, new_y, final_content) =
        fim_accept_inner(accept_type, pos_x, pos_y, line_cur, content)?;

    // replace the one line with all the new content (can be multiple lines)
    set_buf_lines(pos_y..=pos_y, final_content)?;

    // Move the cursor to the end of the accepted text
    set_window_cursor(new_x, new_y)?;

    // Set allow_comment_fim_cur_pos to allow FIM in comments immediately after accepting completion
    let buf_id = get_current_buffer_id();
    state.set_allow_comment_fim_cur_pos(buf_id, new_x, new_y);

    fim_hide_inner(&state)?;

    // immediately start a new FIM request skipping the debounce
    fim_try_hint_skip_debounce()?;

    Ok(())
}

/// FIM accept function - can be used to accept real changes or virtually accept changes in order
/// to run speculative FIM for future rounds.
///
/// Returns new_x_pos, new_y_pos, combined content to write
//
/// NOTE NOT processed on main Neovim thread
#[tracing::instrument]
pub fn fim_accept_inner(
    accept_type: FimAcceptType,
    pos_x: usize,
    pos_y: usize,
    line_cur: String,
    content: Vec<String>,
) -> LttwResult<(usize, usize, Vec<String>)> {
    // Use the accept_fim_suggestion function from fim module
    let (new_line, rest, inline_loc) =
        accept_fim_suggestion(accept_type, pos_x, &line_cur, &content);

    // Check if the new_line contains "…" character and find its position
    let ellipsis_pos = new_line.find('…');

    let (new_x, new_y, combined) = if let Some(ellipsis_pos) = ellipsis_pos {
        // Delete the "…" character
        let mut new_line_without_ellipsis = new_line.clone();
        new_line_without_ellipsis.remove(ellipsis_pos);

        // Calculate the position of "…" in the final output
        // The ellipsis is at ellipsis_pos in new_line, and new_line is the first line in combined
        let ellipsis_x = ellipsis_pos;
        let ellipsis_y = 0; // new_line is the first line

        let mut combined = vec![new_line_without_ellipsis];
        if let Some(rest_lines) = &rest {
            combined.extend(rest_lines.clone());
        }

        (ellipsis_x, pos_y + ellipsis_y, combined)
    } else {
        // Move the cursor to the end of the accepted text
        let (new_x, new_y) = if let Some(rest_lines) = &rest {
            let new_pos_y = pos_y + rest_lines.len();
            let new_pos_x = rest_lines.last().map_or(0, |line| line.len());
            (new_pos_x, new_pos_y)
        } else if let Some(inline) = inline_loc {
            (inline, pos_y)
        } else {
            let new_col = new_line.len();
            (new_col, pos_y)
        };

        let mut combined = vec![new_line];
        if let Some(rest_lines) = rest {
            combined.extend(rest_lines);
        }

        (new_x, new_y, combined)
    };

    Ok((new_x, new_y, combined))
}

/// Accept FIM suggestion - returns the modified line
// returns if inline should be used
#[tracing::instrument]
pub fn accept_fim_suggestion(
    accept_type: FimAcceptType,
    pos_x: usize,
    line_cur: &str,
    content: &[String],
) -> (
    String,              // first line
    Option<Vec<String>>, // rest lines (None if not needed)
    Option<usize>,       // inline-end (NONE if not inline)
) {
    // Safety: check content length before accessing content[0]
    if content.is_empty() {
        return (line_cur.to_string(), None, None);
    }

    let first_line = content[0].clone();

    // Safety: ensure pos_x is within bounds
    let line_cur_len = line_cur.len();
    let safe_pos_x = pos_x.min(line_cur_len);
    let prefix = if safe_pos_x <= line_cur_len {
        &line_cur[..safe_pos_x]
    } else {
        ""
    }
    .to_string();

    let (new_line, inline) = if content.len() == 1 {
        // If only one line, just replace the current line
        let suffix = if safe_pos_x <= line_cur_len {
            &line_cur[safe_pos_x..]
        } else {
            ""
        };
        let (first_line, new_suffix, infill) =
            trim_suggestion_and_suffix_on_curr_line(&first_line, suffix);

        // NOTE even though when we get a new suffix (under a partial bracket match) we
        // don't render the content with infill but we still are rendering "inline"
        // so we still need to calculate the final location upon acceptance
        let inline = if infill || new_suffix.is_some() {
            Some(prefix.len() + first_line.len())
        } else {
            None
        };

        let suffix = if let Some(suffix) = new_suffix {
            suffix
        } else {
            suffix.to_string()
        };
        (prefix + first_line + &suffix, inline)
    } else {
        (prefix + &first_line, None)
    };

    // Handle accept type
    match accept_type {
        FimAcceptType::Full => {
            // Insert rest of suggestion
            if content.len() > 1 {
                let rest: Vec<String> = content[1..].to_vec();
                (new_line, Some(rest), inline)
            } else {
                (new_line, None, inline)
            }
        }
        FimAcceptType::Line => {
            if new_line == line_cur && content.len() > 1 {
                // accept the next line - safety check for content[1]
                let rest = vec![content[1].clone()];
                (new_line, Some(rest), inline)
            } else {
                (new_line, None, inline)
            }
        }
        FimAcceptType::Word => {
            // Accept only the first word
            let suffix = if safe_pos_x <= line_cur_len {
                &line_cur[safe_pos_x..]
            } else {
                ""
            };
            if let Some(word_match) = first_line.split_whitespace().next() {
                let _new_word = word_match.to_string() + suffix;
                (new_line + word_match, None, inline)
            } else {
                (new_line, None, inline)
            }
        }
    }
}
