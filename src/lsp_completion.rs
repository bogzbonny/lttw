use {
    crate::{
        plugin_state::strip_to_first_identifier,
        utils::{self, get_buf_line, get_current_buffer_id, get_pos},
        PluginState, {FimCompletionMessage, FimResponse, LttwResult},
    },
    ahash::HashSet,
};

/// Get LSP completions for the cursor position and log them
///
/// This is a test command to debug LSP completion behavior.
/// It uses Neovim's built-in LSP functionality via `vim.lsp.buf_request_sync`.
// TODO make async by using buf_request_all with a handler
// TODO make '500' here a param, (500ms max wait time for the result)
pub fn trigger_lsp_completions_async() -> LttwResult<()> {
    utils::assert_not_tokio_worker();
    debug!("Requesting LSP completions at current cursor position");

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

pub fn retrieve_lsp_completions(state: &PluginState) -> LttwResult<Vec<FimCompletionMessage>> {
    let Ok(json_str) = nvim_oxi::api::get_var::<String>("lttw_completion") else {
        return Ok(vec![]); // no completions available, nbd
    };
    nvim_oxi::api::del_var("lttw_completion")?; // clear the var now that we've gotten it

    //debug!("retrieved lsp completions: {}", json_str);

    let response: utils::CompletionResponse = serde_json::from_str(&json_str)?;
    //debug!("response: {:?}", response);

    let (pos_x, pos_y) = (response.pos_x, response.pos_y);
    let (x, y) = get_pos();
    if pos_x != x || pos_y != y {
        debug!("retrieve_lsp_completions");
        return Ok(vec![]);
    };
    if get_current_buffer_id() != response.buffer_id {
        debug!("retrieve_lsp_completions");
        return Ok(vec![]);
    }
    debug!("retrieve_lsp_completions");
    let line_cur = get_buf_line(pos_y);
    let mut seen = HashSet::default();
    let mut filtered_comps: Vec<(FimCompletionMessage, u64)> = response
        .items
        .into_iter()
        .filter_map(|comp| {
            if comp.start_char > pos_x || comp.start_line != pos_y {
                return None;
            }

            let line_chars = line_cur
                .chars()
                .skip(comp.start_char)
                .take(pos_x - comp.start_char)
                .collect::<String>();

            // Only complete partially written text
            if line_chars.is_empty() {
                return None;
            }

            // only keep if strips the prefix
            let mut text = comp.text.strip_prefix(&line_chars).map(|s| s.to_string())?;

            // TODO use some of these autocompletion details better rather than just
            // truncating
            // - it would be nice to be able to accept Some_fn(...) and keep the closing backet
            // - something which only takes one arg, should automatically be filled in eg.
            //    typing Ok[CUR]some_var  then pressing tab should autocomplete to Ok(some_var)
            if let Some(pos) = text.find("$0") {
                text.truncate(pos);
            }
            if let Some(pos) = text.find("${") {
                text.truncate(pos);
            }
            if let Some(pos) = text.find("$1") {
                text.truncate(pos);
            }
            if let Some(pos) = text.find("$2") {
                text.truncate(pos);
            }
            // discard multiline
            if let Some(pos) = text.find('\n') {
                text.truncate(pos);
            }

            // filter out duplicates
            if seen.contains(&text) {
                return None;
            }
            seen.insert(text.clone());

            // NOTE use the full suggestion here, NOT the prefix stripped text!
            debug!("hi");
            let ident = strip_to_first_identifier(&comp.text);
            debug!(ident);
            let usage = state.get_word_statistic_usage(&ident);
            debug!(usage);

            let fim_resp = FimResponse {
                content: text,
                timings: None,
                tokens_cached: 0,
                truncated: false,
            };
            debug!("text: {}, usage: {}", fim_resp.content, usage);

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
    debug!("retrieve_lsp_completions");

    Ok(filtered_comps)
}
