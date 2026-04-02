01. ring buffer at end of fim (see XXX) 
You are in the middle of the task of translating the codebase provided under
`llama.vim/` into a new neovim plugin written in ENTIRELY rust using neovim
bindings through the use of the nvim-oxi crate. NOTE there is not, nor should
there EVER be ANY lua code in this project, all plugin functionality is achieved
PURELY by using nvim-oxi 

The following issue has been identified which should be resolved: In the
original vim code at the end of the fim function (llama#fim) there is ring
buffer pick logic (lines 940 - 946 of llama.vim). This same logic does not yet
exist within the lib.rs trigger_fim function, it should exist at the end much
like in the llama#fim

^^^^^^^^^ DONE

01. "line" completions SHOULD accept the NEXT line on tab if the completion only
    starts on the next line

01. Add (info) stats as rhs extmarks

05. Remove trailing prediction lines if they match 
     - go through one by one
05. add config option for debugging (default false)
05. fix tests-integrations (compile errors) 
10. easier to use debugging system 
10. our own error type
20. option to not predict while in comments

 

