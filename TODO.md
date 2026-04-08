
05. write file contents on first enter of a buffer we haven't entered before 
05. update the single line output inside line functionality to not use 'stop'
    but just truncate the file contents

^^^^^^^^^ DONE


------------------------

05. Use TAB-TAB from normal mode to fix lines with diagnostic errors 
     - :lua print(vim.inspect(vim.diagnostic.get())) 
        - DIAGNOSTIC PIPELINE IS OVERWRITTEN BY ALE! thus the diagnostic changed
          autocmd will not be updated - it DOES actually work so long as we dont
          suppress the color changes that ale has

        config.handlers = {
        -- Override Neovim's handling of diagnostics to run through ALE's
        -- functions so all of the functionality in ALE works.
        ["textDocument/publishDiagnostics"] = function(err, result, _, _)

     - use regular completions/ endpoint not infill endpoint
     - NOTE if more tabs are received WHILE the completion is in progress they
       should be discarded
     - Whatever the whole range of the fed diagnostic is is what should be fed
       into the prompt (along with the diagnostic
     - I'm not sure if it'll be a pain to still use FIM for this or not
        - probably use a regular completion 
     - afterwords this will generate a replacement line(s) for which will be
       displayed using extmarks
     - then a 3rd TAB can accept this
     - then a 4th TAB should take the cursor to the next line which has
       diagnostic errors
     - use this entire system in reverse as well with SHIFT-TAB to move up
     - CTRL-TAB to regenerate a TAB-TAB if the user doesn't like the provided
       response
     - OPTION ONCE no more errors - save file to regenerate diagnostics
     - OPTION ONCE no more errors, go to the next file with errors in a new tab

03. git diff eviction by line number
     - because we're just saving the file changes we do not need to actually 
       calculate removed diffs, just calculate the new diffs and add those to
       the queue HOWEVER we should probably remove other diff segments by
       filename and position of the diff. For a diff with the header:
       @@ -183,6 +185,2 @@
         the new file has a modification at line 185 (1 based) for 2 lines
         so now if another diff comes in 
       @@ -184,6 +190,2 @@
         then we know that this DOES intersect with the first diff and therefor
         we should remove the old diff extra_inputs. 
       This of couse however means that we can ONLY make this evaluation for
       diff changes of the previous diff because something that happened 2
       evaluations ago may have a completely different diff location.
         ... HMMMMMM solution?
  DO THIS-> - SOLUTION 1) track the changes (by filename) and line positions and adjust any of
              the old diff locations... kind of annoying algo but probably quite good once
              it works properly
            - Look at chunk similarity 
     - When using a git-diff chunk for eviction:
        - evict other git-diff chunks by their "new" file location
        - evict other regular chunks by their similarity between the
          git-diff-chunk OLD content and the regular chunk content
     - When using a normal chunks for eviction:
        - evict git-diff chunks by comparing the chunk similarity to the
          git-diff chunks UPDATED information.

05. integrate in LSP Completions into input_prefix
     - OPTIONAL mini lag for these completions like 100ms so we're not generating
       them ruthlessly - however I want to try with this at 0ms, it may be fine!
     - automatically put the llm completion if the user hasn't moved up or down
       through the completions - HOWEVER if the user has moved up or down
       through the completions, then add the completion as the next on the list
       from whatever the users current position is in the completions list
     - supplement the llm completions with suggestions from the LSP completions
     - I WOULD actually scan all the nearby text and order them
       alphanumerically but then set the index AT any matches to nearby text
        - this will be useful in situations like structs with RwLocks for
          instance... chances are you might want to type RwLock
        - if no matches choose a random position
     - MAYBE also just provide the LSP completion as an option immediately until
       the LLM response comes in. I noticed with ALE (from insert mode go C-X
       then C-O) it gives a suggestion with a `...` in it which is probably
       where the cursor should just be inserted if the completion is accepted
       (removing the ... keeping it in insert mode THUS triggering the next
       completion).

20. integrate definitions of all nearby objects 
     - add to extra_input
     - Iterate through all the nearby words and to go-to-definition
        - one intellegent thing to do would be get the definition of 
          whatever is currently outside of the containing bracket
          for instance hello.some_fn(foo, bar, CUR-POS
          iterate backwards to the ( and then get definition for some_fn
          - also useful to iterate backwards to the first { and get what's
            directly before that. 
             - NOTE should do bracket counting to ensure that we're getting the
               entry for the thing actually above us
     - use this vim command: https://neovim.io/doc/user/lsp/#lsp-buf
     - DONT USE TREE SITTER, probably overkill given we probably only want to
       put in one or two things maximum
        - Can use treesitter [](https://neovim.io/doc/user/treesitter/#_treesitter-queries)
          - I would ONLY go to def for: 
            function.call function.method.call type constant variable variable.member 
            type.definition
          - would (could?) make a query for the specific objects I want
```
local query_string = [[
  (function_declaration
    name: (identifier) @func.name)
  (method_definition
    name: (property_identifier) @func.name)
  (class_declaration
    name: (type_identifier) @class.name)
]]
```
       - also see https://neovim.io/doc/user/treesitter/#TSNode%3Adescendant_for_range()
     - IF use treesitter probably don't have to do this:
       - probably want to have a config option for all the words which we don't
         want to get the definition for (eg. pub,struct, unwrap, usize, i64,
         Option,

10. BUG on_buf_enter_update_file_contents - for some reason its not triggered
    when openning for the first time (with vf) - subsiquent switches to the
    buffer will activate it

10. option to automatically launch llama.cpp with nohup rather than depending on
    a server already being running. 

20. better global error printing/handling https://github.com/noib3/nvim-oxi/issues/231

40. instruction system LOW priority can use CodeCompanion for now

40. reduce cognitive offloading allowing for llm calls to be ignored a % of the
    time (hence you don't know if you're waiting for an llm or waiting for
    nothing!).
     - reduce_cognitive_offloading_ratio = 25%

40. README gif of homer with the bird
     - link it to https://www.youtube.com/watch?v=R_rF4kcqLkI

------------------------
POST RELEASE

50. Offline/Online mode for getting responses locally or over the web

50. Bring the entire FIM system into LTTW for further customization
     - allow more control over cache ordering if one was to be evicted?
       - maybe this isn't an issue and we can just add something to the end of
         the cache though?
     - investigate FIM techniques used by https://huggingface.co/zed-industries/zeta-2
       - would require FIM customization
     - the indent system for generating completions is good, however, its a bit
       annoying to not be able to autogenerate a closing } in the right position
       would be nice if there was a way to have the best of both worlds here
     - yanked text should have a description before adding to ExtraArgs... Maybe
       also have it located at the end near the prefix

