use crate::{
    fim::{fim_try_hint, render::render_fim_suggestion},
    fim_hide,
    lsp_completion::retrieve_lsp_completions,
    plugin_state::get_state,
    utils::{get_buf_line, get_current_buffer_id, get_pos, in_insert_mode},
    FimResponse, LttwResult,
};

/// Message to be passed for displaying
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum DisplayMessage {
    ClearFIM,
    TriggerLSPCompletion,
    CompletionMsg(FimCompletionMessage),
    Msgs(Vec<DisplayMessage>),
}

impl From<FimCompletionMessage> for DisplayMessage {
    fn from(msg: FimCompletionMessage) -> Self {
        DisplayMessage::CompletionMsg(msg)
    }
}

impl From<Vec<DisplayMessage>> for DisplayMessage {
    fn from(msgs: Vec<DisplayMessage>) -> Self {
        DisplayMessage::Msgs(msgs)
    }
}

/// Message sent from async worker to main thread when completion is ready
#[derive(Debug, Clone)]
pub struct FimCompletionMessage {
    pub buffer_id: u64, // Buffer handle to ensure we're still in same buffer
    //ctx: LocalContext,       // All buffer lines captured at start
    pub line_cur: String, // the current line where the completion was calculated (without completion)
    pub cursor_x: usize,  // Cursor position X
    pub cursor_y: usize,  // Cursor position Y
    pub completion: FimResponse, // All available completions for cycling
    pub do_render: bool,
    pub retry: Option<usize>, // the retry count for this completion
}

/// NOTE this occurs on the neovim thread
/// Process pending FIM display queue - drains and displays messages on the main thread
#[tracing::instrument]
pub fn process_pending_display() -> LttwResult<()> {
    let state = get_state();

    // Only display if we are in insert mode
    if !in_insert_mode()? {
        fim_hide()?; // failsafe if somehow a hint weezled its way in there
        return Ok(());
    }

    // Take all pending messages (clear the queue)
    let queued_messages: Vec<DisplayMessage> = {
        let Some(mut pending_queue) = state.pending_display.try_write() else {
            return Ok(());
        };
        std::mem::take(&mut *pending_queue)
    };

    // read the LSP messages
    let mut messages = if state.config.read().lsp_completions {
        match retrieve_lsp_completions(&state) {
            Ok(c) => c,
            Err(e) => {
                info!("retrieve_lsp_completions error: {}", e);
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };

    messages.extend(queued_messages);
    if messages.is_empty() {
        return Ok(());
    }

    info!("Processing {} pending display messages", messages.len(),);

    // accept the most recent (last) message which has content and isn't only whitespace
    let mut msg_to_render = None;
    let mut do_clear = false;
    let mut trigger_lsp_completions = false;
    let mut disp_msgs = Vec::new();
    for msg_ in messages.into_iter() {
        match msg_ {
            DisplayMessage::ClearFIM => {
                do_clear = true;
            }

            DisplayMessage::TriggerLSPCompletion => {
                trigger_lsp_completions = true;
            }
            DisplayMessage::CompletionMsg(msg_) => {
                disp_msgs.push(msg_);
            }
            DisplayMessage::Msgs(msgs_inner) => {
                for msg_ in msgs_inner {
                    match msg_ {
                        DisplayMessage::ClearFIM => {
                            do_clear = true;
                        }
                        DisplayMessage::TriggerLSPCompletion => {
                            trigger_lsp_completions = true;
                        }
                        DisplayMessage::CompletionMsg(msg_) => {
                            disp_msgs.push(msg_);
                        }
                        DisplayMessage::Msgs(_msgs) => {} // only allow for depth of 1
                    }
                }
            }
        }
    }
    if trigger_lsp_completions {
        // trigger the async lsp completion
        if let Err(e) = crate::lsp_completion::trigger_lsp_completions_async() {
            error!("trigger_lsp_completions_async error: {}", e)
        }
    }

    for msg_ in disp_msgs.into_iter() {
        if msg_is_valid_to_display(&msg_) {
            // because the msg is valid we already know that the message is for the cursor position
            let is_unique = state
                .fim_state
                .write()
                .push_completion_cycle_if_unique(msg_.completion.clone());
            // only trigger renders if unique messages added
            //if is_unique && msg_to_render.is_none() {
            if is_unique {
                if msg_.do_render {
                    state.fim_state.write().push_completion_idx_to_tail();
                }
                msg_to_render = Some(msg_); // always render the last message
            }
        }
    }

    if do_clear {
        fim_hide()?;
    }
    let mut retry = 0;
    if let Some(msg) = msg_to_render {
        info!("valid message about to render: {:?}", msg);
        if msg.do_render {
            render_fim_suggestion(
                state.clone(),
                msg.cursor_x,
                msg.cursor_y,
                &msg.completion,
                msg.line_cur,
            )?;
        }
        retry = msg.retry.unwrap_or(0);
    }

    // NOTE there were messages nomatter what at this point in the function (even if none were
    // valid to display)
    //
    // if either the hint isn't shown OR it's only whitespace then trigger another fim
    // only retry a llm call 3 times before giving up
    if !state.fim_state.read().hint_shown && retry <= 3 {
        retry += 1;
        info!("rerendering fim suggestion");
        fim_try_hint(Some(retry))?;
    }

    Ok(())
}

// should we abort the completion because the content has changed since we started this completion
#[tracing::instrument]
fn msg_is_valid_to_display(msg: &FimCompletionMessage) -> bool {
    info!("{:?}", msg);
    if msg.completion.content.is_empty() || msg.completion.content.trim().is_empty() {
        info!("returning false");
        return false;
    }
    let id = get_current_buffer_id();
    if id != msg.buffer_id {
        info!("returning false");
        return false;
    }

    let (x, y) = get_pos();
    if msg.cursor_y != y || msg.cursor_x != x {
        info!("returning false");
        return false;
    };
    let curr_line = get_buf_line(y);
    if curr_line != msg.line_cur {
        info!("returning false");
        return false;
    }

    info!("returning true");
    true
}
