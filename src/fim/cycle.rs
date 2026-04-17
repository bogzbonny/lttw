use {
    super::render::render_fim_suggestion,
    crate::{LttwResult, plugin_state::get_state},
};

/// Cycle to next completion
#[tracing::instrument]
pub fn fim_cycle_next() -> LttwResult<()> {
    let state = get_state();

    // Get current state
    let (hint_shown, _pos_x, _pos_y, _line_cur, _cycle_empty) = {
        let fim_state = state.fim_state.read();
        if !fim_state.hint_shown {
            return Ok(());
        }
        (
            fim_state.hint_shown,
            fim_state.pos_x,
            fim_state.pos_y,
            fim_state.line_cur.clone(),
            fim_state.completion_cycle.is_empty(),
        )
    };

    if !hint_shown {
        return Ok(());
    }

    // Cycle to next
    let completion = {
        let mut fim_state = state.fim_state.write();
        let Some(completion) = fim_state.cycle_next() else {
            return Ok(());
        };
        completion
    };
    info!(
        "Cycled to next completion ({} chars)",
        completion.resp.content.len()
    );

    // Re-display with new completion
    let (pos_x, pos_y, line_cur) = {
        let fim_state = state.fim_state.read();
        (fim_state.pos_x, fim_state.pos_y, fim_state.line_cur.clone())
    };

    render_fim_suggestion(state.clone(), pos_x, pos_y, &completion, line_cur)?;

    Ok(())
}

/// Cycle to previous completion
#[tracing::instrument]
pub fn fim_cycle_prev() -> LttwResult<()> {
    let state = get_state();

    // Get current state
    let (hint_shown, _pos_x, _pos_y, _line_cur) = {
        let fim_state = state.fim_state.read();
        if !fim_state.hint_shown {
            return Ok(());
        }
        (
            fim_state.hint_shown,
            fim_state.pos_x,
            fim_state.pos_y,
            fim_state.line_cur.clone(),
        )
    };

    if !hint_shown {
        return Ok(());
    }

    // Cycle to previous
    let completion = {
        let mut fim_state = state.fim_state.write();
        let Some(completion) = fim_state.cycle_prev() else {
            return Ok(());
        };
        completion
    };

    info!(
        "Cycled to previous completion ({} chars)",
        completion.resp.content.len()
    );

    // Re-display with new completion
    let (pos_x, pos_y, line_cur) = {
        let fim_state = state.fim_state.read();
        (fim_state.pos_x, fim_state.pos_y, fim_state.line_cur.clone())
    };

    render_fim_suggestion(state.clone(), pos_x, pos_y, &completion, line_cur)?;

    Ok(())
}
