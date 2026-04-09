^^^^^^^^^ DONE


10. tracing support
integrate in the tracing and tracing-opentelemetry crates into this library.
Replace all instances of this crates `debug!` with the tracing `debug!` macro.
If config.debug_enabled_at_startup is disabled, then telemetry should NOT start
during startup.  Ensure that when debugging is enabled that the new tracing
system will still write all the debug information into the `./lttw.log` file
just like the existing debug system works.

01. BUG on_buf_enter_update_file_contents - for some reason its not triggered
    when openning for the first time (with vf) - subsiquent switches to the
    buffer will activate it

01. remove matching suffix from LSP completions 

05. Integrate in better usage of lsp autocompletions
     - TODO use some of these autocompletion details better rather than just
       truncating
     - it would be nice to be able to accept Some_fn(...) and keep the closing backet
       - maybe just using the … character as the marker (nice for viewing) and
         then put the cursor there.
       - example: 
         newText = "build_info_string(${1:timings}, ${2:tokens_cached},
         ${3:truncated}, ${4:ring_chunks}, ${5:ring_n_chunks},
         ${6:ring_n_evict}, ${7:ring_queued}, ${8:ring_queue_length},
         ${9:cache_size}, ${10:max_cache_keys})$0",
       
     - NEW CONFIG OPTION something which only takes one arg, should
       automatically be filled in eg. typing Ok[CUR]some_var  then pressing tab
       should autocomplete to Ok(some_var)

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

20. git diff extra_input eviction by line number
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

10. More sophisticated statistics for lsp completion priority
     - beyond doing the global statistics, we could also do some quick stats on
       the nearby environment to wherever the completion is taking place. Nearby
       guys should have a high statistical weighting as compared to the global
       stats. This would be good for variable names.

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

40. Beefy arms link up LLM + LSP 

------------------------
POST RELEASE

30. Small Statistic completion predictions: 
     - we're definately in HLM territory here
     - predict and complete small little things such as: 
     - let askldfj String[CUR] should predict " = " 
       instead of "StringBuilder" (lsp) because "String = " probably comes up 
       more than "StringBuilder" in the codebase. 
     - use the first Identity (word) in the line and the last identity of the line to 
       predict what comes next. (lines that start with if, else, let would also
       be good predictors). ALSO be sure that (excluding whitespace) beyond the
       ident being the same the other characters must match 
     - only predict small things (such as a few characters, or .. ONE 

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

