<!--
░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░
░ꕤ                                                  (      )  )                             ꕤ ░
░                                          *       (   (       )                   *     7    ░
░   (    >>>fas like race truck>>>         *       p\      O  0 <---beautiful     *   . 7     ░
░ ( C                      ___________     .         0    o /        walnuts     *   . 7      ░
░         )   ~ ~ ~       /         ,'\    |          |                             . .  ^    ░
░  (             ______ .--------\.'   \___|__        |   |"&the crowd goes wild!" / .   |    ░
░     ))   ~    //     /       .. \ >:)      ,| ______|   |______________________ / . <--|    ░
░   (   )   ~  _______/L L T T     \_______.' /       |   |                        /     |    ░
░     (  )     |       __    W  __         |_/       /     \ R A C E T R A C K    / incredbl  ░
░        C c ==|______/  \_____/  \_______/- - - - -/  tre  \- - - - - - - -     / jump, you  ░
░                     \__/     \__/                      e                      / will be     ░
░        ______________________________________________________________________/ loved 4ever  ░
░ꕤ                                                                                          ꕤ ░
░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░
-->

# Llama Take The Wheel

A Neovim plugin for code completion using llama.cpp, written in Rust.


## Installation

Requirements:
 - Cargo installed
 - Running llama.cpp server with:
   - FIM endpoint at `http://127.0.0.1:8012/infill` (or configure custom endpoint)
   - Chat completions endpoint at `http://127.0.0.1:8012/v1/chat/completions` (or configure custom endpoint)

### Using vim-plug

```vim
Plug 'bogzbonny/lttw', { 'do': 'cd lttw && cargo build --release' }
lua require('llama').setup()
```

### Using lazy.nvim

```lua
{
  'bogzbonny/lttw',
  dependencies = { 'nvim-lua/plenary.nvim' },
  build = 'cd lttw && cargo build --release',
  config = function()
    require('llama').setup()
  end
}
```


## LSP Completion Overrides

The `lsp_overrides` configuration option allows you to transform LSP completion text
after it's generated. This is useful for cases where the LSP provides completions
that need slight modifications to be valid.

### Configuration Format

```lua
config = function()
  require('llama').setup({
    lsp_overrides = {
      -- Each entry is a pair: {pattern, replacement}
      {"Ok()", "Ok(())"},  -- Transform Ok() to Ok(()) for unit type returns
      -- Add more overrides as needed
    }
  })
end
```

### How It Works

- The plugin compares the final completion text against each pattern in order
- When a match is found, the text is replaced with the corresponding replacement string
- Only the first matching override is applied (breaks after first match)
- Patterns are matched exactly (string equality), not as regex

## Do you ever find yourself ... 

... waiting for the llm to send you a code completion only to received something
incorrect, all the while if you had just not waited you could have already had
correct code? Yeah that's the feeling of your brain rotting. LTTW prevents
cognitive offloading in three ways: 
 1. All code completions which come late in while you're typing are checked
    against to newly typed characters and SALVAGED if possible and used in
    partial form. Practically this means that you should never, not for an iota,
    stop typing to wait for the llm. Even if you make a typo, the code
    completion is always cached and you can backspace to get to the
    still-correct location and poof your cached code completion will come up. 
 2. While typing in comments all completions are disabled by default, stay sharp
    and offer your own insight into what the code base is doing rather than
    filling the space with slop. 
 3. reduce_cognitive_offloading_ratio so you can never be totally sure as to
    whether you're waiting for llm to complete of if the completion call has
    been silently cancelled. 


## CREDIT WHERE CREDIT IS DUE

Originally this codebase was translated from
[llama.vim](https://github.com/ggml-org/llama.vim) which is an excellent
lightweight code-completion system with lots of great ideas programmed by
somebody who knows more about what they're doing then I do. I've drawn lots of
inspiration from this, and I would recommend checking out it out! I honestly had
no flippin' idea what I was doing until I had the privilege of studying this
code. 

## Key Differences from llama.vim

 - LSP completion integration (whoa)
 - Ring Buffer Adjustments
   - continious ring buffer updating (when in normal mode/ inactive) instead of
     just capping it at 1 chunk per second
   - picking ring buffer chunks to the queue doesn't evict similar chunks from the live buffer
     (only once that queue entry is added to the live buffer will this occur)  
   - param to pick more from the FIM scope after fim (instead of limited to 1
     pick) NOTE that the ring scope should be larger if you increase this
 - adaptive debounce strategy for faster completions while being able to respond
   to if the user decides to type real fast (start with slow debounce then ramp
   up)
 - ability to explicitly cap the limit number of concurrent completion llm calls 
 - don't autocomplete while in code comments (option to turn that back on if
   you're cowardly ;) )
 - dynamic llm n_predict for reduce prediction tokens while inside a line
    - aka reduce the amount of tokens the llm is allowed to predict if typing
      inside of a line (as opposed to at the end of a line) 
 - attempt to still use valid late llm completions which come in after the user has
   typed more since the llm completion was requested.  
 - numerous small micro-optimizations throughout

### Example Use Cases

1. **Rust Unit Type Returns**: Transform `Ok()` to `Ok(())` when the LSP suggests a
   simple `Ok()` but you need the explicit unit type
   ```lua
   {"Ok()", "Ok(())"}
   ```

2. **Common Function Wrappers**: If the LSP suggests `Some(x)` but you often need
   `Some(Box::new(x))`
   ```lua
   {"Some(x)", "Some(Box::new(x))"}
   ```

3. **Multiple Overrides**: You can add multiple override pairs
   ```lua
   {
     {"Ok()", "Ok(())"},
     {"Err()", "Err(())"},
     {"Some(x)", "Some(Box::new(x))"}
   }
   ```

## Configuration

The plugin is configured via the `g:lttw_config` global variable in Neovim. Here's a comprehensive guide to all configuration options:

### Basic Configuration

```lua
require('llama').setup({
  -- Your configuration here
})
```

### Full Configuration Reference

#### FIM (Fill-in-Middle) Server Configuration

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `endpoint_fim` | string | `"http://127.0.0.1:8012/infill"` | llama.cpp server endpoint for FIM completion |
| `endpoint_inst` | string | `"http://127.0.0.1:8012/v1/chat/completions"` | llama.cpp server endpoint for instruction completion |
| `model_fim` | string | `""` | Model name when multiple models are loaded (optional, recommended: Qwen3 Coder 30B) |
| `model_inst` | string | `""` | Instruction model name (optional, recommended: gpt-oss-120b) |
| `api_key` | string | `""` | llama.cpp server API key (optional) |

#### Context Configuration

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `n_prefix` | integer | `256` | Number of lines before the cursor location to include in the local prefix |
| `n_suffix` | integer | `64` | Number of lines after the cursor location to include in the local suffix |
| `n_predict_inner` | integer | `16` | Max tokens to predict when there are non-whitespace chars to the right of cursor |
| `n_predict_end` | integer | `256` | Max tokens to predict when at end of line or only whitespace to the right |

#### Timing Configuration

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `t_max_prompt_ms` | integer | `500` | Max alloted time for the prompt processing (not yet supported in llama.cpp) |
| `t_max_predict_ms` | integer | `1000` | Max alloted time for the prediction |
| `debounce_min_ms` | integer | `20` | Minimum debounce time in milliseconds |
| `debounce_max_ms` | integer | `200` | Maximum debounce time in milliseconds (used when queue is full) |

#### FIM Behavior Configuration

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `max_concurrent_fim_requests` | integer | `3` | Maximum number of concurrent FIM requests (set >1 to allow speculative FIM) |
| `single_line_prediction_within_line` | boolean | `true` | Enable single-line prediction when cursor is inside a line |
| `show_info` | integer | `2` | Show extra info about the inference (0=disabled, 1=statusline, 2=inline) |
| `auto_fim` | boolean | `true` | Trigger FIM completion automatically on cursor movement |
| `max_line_suffix` | integer | `8` | Do not auto-trigger FIM completion if there are more than this number of characters to the right of the cursor |
| `no_fim_in_comments` | boolean | `true` | Disable auto FIM completion when cursor is inside code comments |

#### Cache Configuration

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `max_cache_keys` | integer | `250` | Maximum number of cached completions to keep in result_cache |

#### Ring Buffer Configuration

The ring buffer accumulates context chunks over time from:
- Completion requests
- Yank operations
- Entering a buffer
- Leaving a buffer
- Writing a file

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `ring_n_chunks` | integer | `16` | Maximum number of chunks to pass as extra context to the server (0 to disable) |
| `ring_chunk_size` | integer | `64` | Maximum size of chunks (in number of lines). Adjust to avoid context overflow - e.g., 64 chunks × 64 lines ≈ 32k context |
| `ring_scope` | integer | `1024` | Range around the cursor position (in lines) for gathering chunks after FIM |
| `ring_update_ms` | integer | `1000` | How often to process queued chunks in normal mode |
| `ring_queue_length` | integer | `16` | Maximum length of the ring chunk queue |
| `ring_n_picks` | integer | `1` | Number of chunks to pick from the scope when cursor moves significantly or to a new buffer |

#### Keymap Configuration

Set to empty string `""` to disable a keymap.

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `keymap_fim_trigger` | string | `"<leader>llf"` | Keymap to trigger FIM completion |
| `keymap_fim_accept_full` | string | `"<Tab>"` | Keymap to accept full suggestion |
| `keymap_fim_accept_line` | string | `"<S-Tab>"` | Keymap to accept line suggestion |
| `keymap_fim_accept_word` | string | `"<leader>ll]"` | Keymap to accept word suggestion |
| `keymap_debug_toggle` | string | `"<leader>lld"` | Keymap to toggle the debug pane |
| `keymap_inst_trigger` | string | `"<leader>lli"` | Keymap to trigger instruction command |
| `keymap_inst_rerun` | string | `"<leader>llr"` | Keymap to rerun the instruction |
| `keymap_inst_continue` | string | `"<leader>llc"` | Keymap to continue the instruction |
| `keymap_inst_accept` | string | `"<Tab>"` | Keymap to accept the instruction |
| `keymap_inst_cancel` | string | `"<Esc>"` | Keymap to cancel the instruction |

#### LSP Completion Configuration

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `lsp_completions` | boolean | `true` | Enable/disable LSP completions |
| `lsp_comp_truncate_vars` | boolean | `true` | Enable variable truncation in LSP completions |
| `lsp_comp_insert_one_var` | boolean | `false` | Insert one variable at a time in LSP completions |
| `lsp_overrides` | array of string pairs | `{"Ok()", "Ok(())"}` | Array of (pattern, replacement) pairs to transform LSP completion text |

#### Startup & Debug Configuration

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `enable_at_startup` | boolean | `true` | Enable llama.vim functionality at startup |
| `diff_tracking_enabled` | boolean | `true` | Enable buffer diff tracking for context |
| `tracing_enabled` | boolean | `false` | Enable tracing logging |
| `tracing_log_file` | boolean | `false` | Write traces to log file |
| `tracing_level` | string | `"DEBUG"` | Logging level (e.g., "DEBUG", "INFO", "WARN", "ERROR") |

#### Filetype Configuration

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `disabled_filetypes` | array of strings | `[]` | List of filetypes to disable code completion |
| `enabled_filetypes` | array of strings | `[]` | List of filetypes to enable code completion (overrides `disabled_filetypes`) |

### Example Configurations

#### Minimal Configuration

```lua
require('llama').setup({
  -- Only change what's needed
  auto_fim = false,  -- Disable auto FIM
})
```

#### Disable for Specific Filetypes

```lua
require('llama').setup({
  -- Disable in markdown and help files
  disabled_filetypes = { "markdown", "help" },
})
```

#### Enable Only for Specific Filetypes

```lua
require('llama').setup({
  -- Only enable for programming languages
  enabled_filetypes = { "rust", "python", "javascript", "go" },
})
```

#### Configure Ring Buffer for Large Context

```lua
require('llama').setup({
  -- For models with large context windows
  ring_n_chunks = 64,
  ring_chunk_size = 64,
  ring_scope = 2048,
})
```

#### Disable All Keymaps

```lua
require('llama').setup({
  keymap_fim_trigger = "",
  keymap_fim_accept_full = "",
  keymap_fim_accept_line = "",
  keymap_fim_accept_word = "",
  keymap_debug_toggle = "",
  keymap_inst_trigger = "",
  keymap_inst_rerun = "",
  keymap_inst_continue = "",
  keymap_inst_accept = "",
  keymap_inst_cancel = "",
})
```

#### Custom LSP Overrides

```lua
require('llama').setup({
  lsp_overrides = {
    -- Rust unit types
    {"Ok()", "Ok(())"},
  },
})
```

## Alternatives Local Code Completion

  - llama.vim
  - tabby

