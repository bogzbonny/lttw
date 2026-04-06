10. fim_completion, option to pick more
       // TODO option to allow picking more than one chunk from the scope here
03. option to not predict while in comments
Add an new feature which prevents FIM prediction while in comments 
 - new config option no_fim_in_comments (default true)
 - Use the synID() and synIDattr() functions to check the syntax ID under the cursor:
   - need to use equivalent within nvim_oxi
 - should however ALLOW comment predictions immediately after accepting a code
   completion, because the code completion may end in a comment, and if that's
   the case then we want to allow for further code completions
 - everytime a completion is accepted, update a new plugin state
   allow_comment_fim_cur_pos (Option<>) which contains the final cursor position which
   the accept function moves the cursor to upon accepting a completion. Whenever
   checking against code comments, ignore checking if we're in a code comment if
   this value is set. Everytime the on_move function is triggered, check the
   cursor pos, if the cursor pos is different from allow_comment_fim_cur_pos
   then set allow_comment_fim_cur_pos to None.

^^^^^^^^^ DONE

10. n_predict changes
dynamically change n_predict during each FIM call (number of tokens to predict)
 - replaces existing n_predict config option
 - When in a line and there are non-whitespace characters to the right of the
   cursor set to n_predict_inner (default value 16)
 - for at the end of a line or where there's only whitespace left to the right
   of the cursor set to a new config param n_prefict_end (default value 256)

05. completion cycling
New keymaps; use CTRL-j and CTRL-k from insert mode to cycle through the
completions options. Now whenever we compile autocompletions from previous
nearby autocompletions, we should keep a list of all the autocompletions
(ordered from longest to shortest) and start by displaying the first FIM but
then allow cycling through these

01. when debugging is disabled the lttw.log file is still created/cleared, this
    shouldn't happen

05. regenerate
New keymaps; Ctrl+l from insert move to regenerate the completion at the
location. NOTE add this to the list of completions at this location, so one can
cycle back through them if necessary. Keep the existing completion visibly and
only trigger changing to the newly generated completion once the response for
this has been received.

03. integrate git diff system into extra_inputs 
     - git diff --no-ext-diff --unified=0
         -> use unified=0 for concise chunks
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

integrate a new system which keeps track of diff chunks each time there is a
filesave. the diff of a single file may contain several small diff chunks if
there are disconnect edits. trigger diff evaluation in autoccmd.rs with
bufwritepost. use the gix-diff crate for calculating the diffs on the codebase
for all buffers which we have open. save an array of all the diff
chunks in the pluginstate. each time the diff is recalculated compare it to the
previously saved diff chunks and add compile the diff-chunk-changes. for
diff-chunk-changes additions add the diff chunks to the ringbuffer.queued, for
removals evict the diff from ringbuffer.queued and ringbuffer.chunks (note those
chunks may have already been evicted for other reasons by the time we go to
evict those chunks). perform the removals before the additions and add debug
output for these operations

DO NOT revert to using CLI git, that is forbidden. Review
https://github.com/GitoxideLabs/gitoxide/blob/main/gix-diff/tests/diff/blob/unified_diff.rs
to see a basic example of how to diff between two strings, this is very simple!
Use no context like this:

let actual = gix_diff::blob::diff(
        Algorithm::Myers,
        &interner,
        UnifiedDiff::new(
            &interner,
            ConsumeBinaryHunk::new(String::new(), "\n"),
            ContextSize::symmetrical(0),
        ),
    )?; 
Our approach should be to simply save the most recent buffers we encounter by
filename in the PluginState and then everytime BufWritePost is executed we
compare all the files we have to what we've previously saved and calculate the
diff based on that

05. integrate in LSP Completions into input_prefix (?)
     - probably add a mini lag for these completions like 100ms so we're not
       generating them unnecessarily. 
        - automatically put the llm completion if the user hasn't moved up or
          down through the completions - HOWEVER if the user has moved up or
          down through the completions, then add the completion as the next on
          the list from whatever the users current position is in the
          completions list
     - supplement the llm completions with suggestions from the LSP completions
     - MAYBE also just provide the LSP completion as an option immediately until
       the LLM response comes in. I noticed with ALE (from insert mode go C-X
       then C-O) it gives a suggestion with a `...` in it which is probably
       where the cursor should just be inserted if the completion is accepted
       (removing the ... keeping it in insert mode THUS triggering the next
       completion).


------------------------

05. Use TAB-TAB from normal mode to fix lines with diagnostic errors 
     - use regular completions/ endpoint not infill endpoint
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


20. integrate definitions of all nearby objects 
     - Iterate through all the nearby words and to go-to-definition
     - use this vim command: https://neovim.io/doc/user/lsp/#lsp-buf
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

10. option to automatically launch llama.cpp with nohup rather than depending on
    a server already being running. 

20. easier to use debugging system (like debug! macro)

20. better global error printing 
    https://github.com/noib3/nvim-oxi/issues/231

20. Option to ONLY accept single line inline suggestions if typing within a fully
    closed bracket system within a line example: "#[derive(Debug, Cl[CURSOR], Default)]"

30. investigate FIM techniques used by https://huggingface.co/zed-industries/zeta-2
     - I think would require a implementing my own FIM system, which would be
       useful anyways

40. Bring the entire FIM system into LTTW for customization

40. instruction system LOW priority can use CodeCompanion for now

40. reduce cognitive offloading allowing for llm calls to be ignored a % of the
    time (hence you don't know if you're waiting for an llm or waiting for
    nothing!).
     - reduce_cognitive_offloading_ratio = 25%

40. README gif of homer with the bird
     - link it to https://www.youtube.com/watch?v=R_rF4kcqLkI
