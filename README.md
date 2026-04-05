# Llama Take The Wheel

A Neovim plugin for code completion using llama.cpp, written in Rust.

## CREDIT WHERE CREDIT IS DUE

Originally this codebase was translated from
[llama.vim](https://github.com/ggml-org/llama.vim) which is an excellent
lightweight code-completion system with lots of great systems. I've drawn lots
of inspiration from this, and I would def recommend ALSO checking out it out! At
the time of making this plugin this was def the best option, however code
completion is too important for me not to get my mits more grubby.

Key Differences: 
 - process ring buffer from most recent to oldest
 - adaptive debounce strategy for fast typing
 - explicit ability to limit number of concurrent FIM calls 

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


## Usage

### FIM Completion

1. **Manual trigger**: Press `<leader>llf` in insert mode
2. **Auto trigger**: Move cursor (when `auto_fim = true`)
3. **Accept suggestion**:
   - `<Tab>` - Accept full suggestion
   - `<S-Tab>` - Accept line suggestion
   - `<leader>ll]` - Accept word suggestion


## Others
 - llama.vim
 - tabby

