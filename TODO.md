01. "line" completions SHOULD accept the NEXT line on tab if the completion only
    starts on the next line
01. random panics 
00. removed the condition that hint must be shown at the end of try_fim
     - also remove the autcommand on cursor move that triggers fim_completion on
       its own
    -> WORKS check to make sure this works (then remove commented code)
    -> ALSO figure out a way of ensuring that if a cached hint is found 
       in try_fim that the background triggered fim_completion won't just
       overwrite it!
10. our own error type

^^^^^^^^^ DONE

01. panic condition if open fim.rs and scroll really fast down and then try
    insert mode
     - https://github.com/noib3/nvim-oxi/issues/260 sheds a lot of light
       - "Essentially never call neovim's functions outside of callbacks and
         plugin entrypoints and never call neovim's functions from another
         thread. "
     - ALL neovim function calls eg. getting buffer information MUST occur from
       the neovim main thread! 
        - Callbacks which happen through an autocommand SHOULD BE OKAY
        - All spawned threads need to either have neovim-dependant information
          fed to them or access it through TimerHandle which executes on the
          main thread.
     - TODO 
        - make more function async so it will be easier to audit 
        - reduce usage of retrieving the tokio runtime and use tokio::spawn
          directly whenever already inside the runtime

01. bizzare issue with re-rendering msgs as they come where the cursor gets
    slammed to the end of the queue message. 
     - probably something to do with inline extmarks or even improper placement 
     (not checking before display if the x_pos is wrong?) 

01. Remove trailing prediction lines if they match 
     - go through one by one

01. check on the amount of llm calls make sure we're not goin crzy

01. Add (info) stats as rhs extmarks

You are in the middle of the task of translating the codebase provided under
`llama.vim/` into a new neovim plugin written in ENTIRELY rust using neovim
bindings through the use of the nvim-oxi crate. NOTE there is not, nor should
there EVER be ANY lua code in this project, all plugin functionality is achieved
PURELY by using nvim-oxi 

Add in rendering of the build info string (see build_info_string) to the rust
code. Render this extmarks which are RightHand justified. 

10. better global error management
    https://github.com/noib3/nvim-oxi/issues/231

05. add config option for debugging (default false)
05. fix tests-integrations (compile errors) 
10. easier to use debugging system 
20. option to not predict while in comments
20. allow for a more regular setup by passing config params through the setup
    function

 

