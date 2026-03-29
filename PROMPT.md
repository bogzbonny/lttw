You are in the middle of the task of translating the codebase provided under
`llama.vim/` into a new neovim plugin written in ENTIRELY rust using neovim
bindings through the use of the nvim-oxi crate. NOTE there is not, nor should
there EVER be ANY lua code in this project, all plugin functionality is achieved
PURELY by using nvim-oxi 

Evaluate all code, function by function within the origin codebase
(`llama.vim/`) and make sure they both exist and are actually usable. Fill in
any gaps if there are "work in progress" implementations.

You are allowed to look at nvim-oxi docs. 

Be sure to update the readme and installation instructions based on how a
nvim-oxi neovim plugin is supposed to be installed (read the docs). 

Also be sure to write full integration tests for FIM using `#[nvim_oxi::test]`
which spawns a neovim instance with the text code. NOTE the llama-server IS
currently running so you can expect results from llama.cpp 

--------------

You are in the middle of the task of translating the codebase provided under
`llama.vim/` into a new neovim plugin written in ENTIRELY rust using neovim
bindings through the use of the nvim-oxi crate. NOTE there is not, nor should
there EVER be ANY lua code in this project, all plugin functionality is achieved
PURELY by using nvim-oxi 

Evaluate all code, function by function within the origin codebase
(`llama.vim/`) and make sure they both exist and are actually usable. Fill in
any gaps if there are "work in progress" implementations. As a part of this
process resolve all Clippy warnings, there is lots of code which is never
called, all this code should either be called somewhere or deleted if its truly
not necessary 

Be sure to write/assess full integration tests for FIM using `#[nvim_oxi::test]`
which spawns a neovim instance with the text code. This is DIFFERENT than the
built-in `#[test]`. You are allowed to look at nvim-oxi docs. NOTE the
llama-server IS currently running so you can expect results from llama.cpp 

--------------

You have been translating the codebase provided under `llama.vim/` into a new
neovim plugin written in ENTIRELY rust using neovim bindings through the use of
the nvim-oxi crate. NOTE there is not, nor should there EVER be ANY lua code in
this project, all plugin functionality is achieved PURELY by using nvim-oxi 

Complete the full fim integration tests for FIM which use `#[nvim_oxi::test]`
which spawns a neovim instance with the text code. NOTE the llama-server IS
currently running so you can expect results from llama.cpp. Currently these
tests are incomplete they need to be completed to: 
 - ensure that FIM is activated and generates code. 
 - ensure that the ring buffer system works and allows for llm caching

By writing these tests you may uncover bugs in the functionality of the
translated code, if this is the case, fix the bugs. You may always refer to the
original working code (written in vimscript) to understand what should be
happening. However you should work by first trying to complete the integration tests,
by understanding the rust code exclusively at first. 

--------------

You have been translating the codebase provided under `llama.vim/` into a new
neovim plugin written in ENTIRELY rust using neovim bindings through the use of
the nvim-oxi crate. NOTE there is not, nor should there EVER be ANY lua code in
this project, all plugin functionality is achieved PURELY by using nvim-oxi 

The following issues have been identified which should be resolved:

 - in lib.rs there is a `on_cursor_moved_i` function which is called within
   setup_autocmds() however this function has not been implemented. refer to
   `llama.vim` to understand how this function should be implemented then
   implement it and add it to the functions defined within lib.rs `lttw()`.

 - fim_accept function in lib.rs is incomplete. 
```
// In a real implementation, this would:
// 1. Set the buffer lines with the accepted content
// 2. Move the cursor to the end of the accepted text
// 3. Clear the FIM hint
```
 - fim_try_hint function in lib.rs is incomplete: 
```
// This would be an async call in the real implementation
```
 - inst_send is incomplete: 
```
// In a real implementation, this would read chunks from the response stream
// and update visual text in real-time
```
 - process_ring_buffer function in lib.rs is incomplete: 
```
// In a full implementation, we would send these to the server here
// For now, just log that we processed them
```


