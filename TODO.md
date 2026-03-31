You have been translating the codebase provided under `llama.vim/` into a new
neovim plugin written in ENTIRELY rust using neovim bindings through the use of
the nvim-oxi crate. NOTE there is not, nor should there EVER be ANY lua code in
this project, all plugin functionality is achieved PURELY by using nvim-oxi 

The following issues have been identified which should be resolved:
 - When an auto-completion is auto suggested in FIM while neovim is in Insert
   Mode it should be able to be accepted with the TAB key, however this
   functionality does not appear to be working (TAB doesn't accept)
 - should trigger autocomplete when insert mode is entered, not just on when the
   cursor is moved
 - For every new FIM suggested, all the old virtual text should be removed
   before the new FIM is presented

--------

You have been translating the codebase provided under `llama.vim/` into a new
neovim plugin written in ENTIRELY rust using neovim bindings through the use of
the nvim-oxi crate. NOTE there is not, nor should there EVER be ANY lua code in
this project, all plugin functionality is achieved PURELY by using nvim-oxi 

The issue: the worker thread created at plugin setup seems to not be actually
displaying any fim_hint text as soon as they return, only after another
keystroke or two will they render from the cache, explore and understand this
issue then attempt to fix. Maybe if we just check for updates every X ms from a
nvim schedule it would render
