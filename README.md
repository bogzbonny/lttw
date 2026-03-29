# lttw

A Neovim plugin for code completion using llama.cpp, written in Rust and Lua.

## Features

- **Fill-in-Middle (FIM) completion**: Get code suggestions between prefix and suffix context
- **Instruction-based editing**: Apply natural language instructions to selected text
- **Ring buffer context**: Automatically gather and reuse relevant code chunks
- **Caching**: Cache completions to avoid redundant API calls
- **Debug logging**: Track plugin activity and troubleshooting

## Requirements

- Cargo installed
- Running llama.cpp server with:
  - FIM endpoint at `http://127.0.0.1:8012/infill` (or configure custom endpoint)
  - Chat completions endpoint at `http://127.0.0.1:8012/v1/chat/completions` (or configure custom endpoint)

## Installation

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

## Configuration

```lua
require('llama').setup({
  -- Server endpoints
  endpoint_fim = 'http://127.0.0.1:8012/infill',
  endpoint_inst = 'http://127.0.0.1:8012/v1/chat/completions',
  
  -- Model names (optional, for multi-model servers)
  model_fim = '',  -- e.g., 'Qwen3 Coder 30B'
  model_inst = '', -- e.g., 'gpt-oss-120b'
  
  -- API key (optional)
  api_key = '',
  
  -- Context window sizes
  n_prefix = 256,  -- Lines before cursor
  n_suffix = 64,   -- Lines after cursor
  n_predict = 128, -- Max tokens to predict
  
  -- Stop strings
  stop_strings = {},
  
  -- Timing limits (ms)
  t_max_prompt_ms = 500,
  t_max_predict_ms = 1000,
  
  -- Show info (0=disabled, 1=statusline, 2=inline)
  show_info = 2,
  
  -- Auto FIM settings
  auto_fim = true,
  max_line_suffix = 8, -- Max chars to right of cursor for auto FIM
  
  -- Cache settings
  max_cache_keys = 250,
  
  -- Ring buffer settings
  ring_n_chunks = 16,
  ring_chunk_size = 64,
  ring_scope = 1024,
  ring_update_ms = 1000,
  
  -- Keymaps (empty string to disable)
  keymap_fim_trigger = '<leader>llf',
  keymap_fim_accept_full = '<Tab>',
  keymap_fim_accept_line = '<S-Tab>',
  keymap_fim_accept_word = '<leader>ll]',
  keymap_debug_toggle = '<leader>lld',
  keymap_inst_trigger = '<leader>lli',
  keymap_inst_rerun = '<leader>llr',
  keymap_inst_continue = '<leader>llc',
  keymap_inst_accept = '<Tab>',
  keymap_inst_cancel = '<Esc>',
  
  -- Filetype filtering
  enable_at_startup = true,
  disabled_filetypes = {},
  enabled_filetypes = {}, -- Overrides disabled_filetypes
})
```

## Usage

### FIM Completion

1. **Manual trigger**: Press `<leader>llf` in insert mode
2. **Auto trigger**: Move cursor (when `auto_fim = true`)
3. **Accept suggestion**:
   - `<Tab>` - Accept full suggestion
   - `<S-Tab>` - Accept line suggestion
   - `<leader>ll]` - Accept word suggestion

### Instruction-based Editing

1. **Select text** in visual mode
2. **Trigger instruction**: `<leader>lli`
3. **Enter instruction** (e.g., "make it shorter", "fix typos")
4. **Accept result**: `<Tab>`
5. **Cancel**: `<Esc>`

## License

MIT License

## Acknowledgments

- Original project: [llama.vim](https://github.com/ggml-org/llama.vim)
- Built with [llama.cpp](https://github.com/ggerganov/llama.cpp)
