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
