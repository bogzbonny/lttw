01. "line" completions SHOULD accept the NEXT line on tab if the completion only
    starts on the next line
01. random panics 

^^^^^^^^^ DONE

00. removed the condition that hint must be shown at the end of try_fim
     - also remove the autcommand on cursor move that triggers fim_completion on
       its own
    -> check to make sure this works (then remove commented code)
    -> ALSO figure out a way of ensuring that if a cached hint is found 
       in try_fim that the background triggered fim_completion won't just
       overwrite it!

01. should do a try_fim as soon as something is accepted

01. panic condition if open fim.rs and scroll really fast down and then try
    insert mode
    https://github.com/noib3/nvim-oxi/issues/231

01. check on the amount of llm calls make sure we're not goin crzy

01. Add (info) stats as rhs extmarks

05. Remove trailing prediction lines if they match 
     - go through one by one
05. add config option for debugging (default false)
05. fix tests-integrations (compile errors) 
10. easier to use debugging system 
10. our own error type
20. option to not predict while in comments

 

