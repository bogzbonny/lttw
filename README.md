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

 - continious ring buffer updating (when in normal mode/ inactive) instead of
   just capping it at 1 chunk per second
 - picking ring buffer chunks to the queue doesn't evict similar chunks from the live buffer
   (only once that queue entry is added to the live buffer will this occur)  
 - param to pick more from the FIM scope after fim (instead of limited to 1
   pick) NOTE that the ring scope should be larger if you increase this
 - adaptive debounce strategy for fast typing
 - ability to explicitly cap the limit number of concurrent FIM calls 
 - don't autocomplete while in code comments (option to turn that on)
 - dynamic n_predict for reduce prediction tokens while inside a line

## Alternatives Local Code Completion 

 - llama.vim
 - tabby

