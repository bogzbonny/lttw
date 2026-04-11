/// FIM accept function - accepts the FIM suggestion
//
/// NOTE Processed on main Neovim thread
#[tracing::instrument]
fn fim_accept(accept_type: FimAcceptType) -> LttwResult<()> {
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
/// NOTE Processed on main Neovim thread
#[tracing::instrument]
fn fim_accept_inner(
    accept_type: FimAcceptType,
    pos_x: usize,
    pos_y: usize,
    line_cur: String,
    content: Vec<String>,
) -> LttwResult<(usize, usize, Vec<String>)> {
    // Use the accept_fim_suggestion function from fim module
    let (new_line, rest, inline_loc) =
        fim::accept_fim_suggestion(accept_type, pos_x, &line_cur, &content);

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
