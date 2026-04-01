01. Should not be Inline extmarks when there are no ending matches
01. sometimes sending two updates (not just one) probably on 'o' because it is
    both a cursor move and entering insert mode. Solved with debounce
01. Panic condition when tab from an empty line 
01. fix cursor positioning on accepted text

^^^^^^^^^ DONE

01. When typing on top of a suggestion, that suggestion should still be there
    IFF we're typing the same content... wonder if this has to do with the ring
    buffer

01. Do not render the virtual text if it nolonger matches what's actually in the
    line

02. sometimes doesn't display if on empty line (tab still works)
     - Seems like this only happens when we're on the very first character of a
       line some edge case biz

05. Remove trailing prediction lines if they match 
     - go through one by one
05. add config option for debugging (default false)
05. Add info string
05. fix tests-integrations (compile errors) 
 
20. option to not predict while in comments
