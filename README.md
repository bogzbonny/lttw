# Llama Take The Wheel

A Neovim plugin for code completion using llama.cpp, written in Rust.

## CREDIT WHERE CREDIT IS DUE

Originally this codebase was translated from
[llama.vim](https://github.com/ggml-org/llama.vim) which is an excellent
lightweight code-completion system with lots of great ideas programmed by
somebody who knows more about what they're doing then I do. I've drawn lots of
inspiration from this, and I would recommend checking out it out! At the time of
making this plugin this was def the best option, however code completion is too
important for me not to get my mits more grubby (see the key differences below).

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

## Key Differences from llama.vim

 - process ring buffer queue from most recent updates to oldest (instead of oldest to newest)
 - continious ring buffer updating (when in normal mode/ inactive) instead of
   just capping it at 1 chunk per second
 - adaptive debounce strategy for fast typing
 - explicit ability to limit number of concurrent FIM calls 


## Alternatives Local Code Completion 

 - llama.vim
 - tabby

