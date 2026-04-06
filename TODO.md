03. continious ring_buffer processing rather than just 1 per second while in
    normal/mode or nothing is happening
20. add config option for debugging (default false)
20. allow for a more regular setup by passing config params through the setup
    function
20. strange error where sometimes there are code completions in a markdown file
    right at the beginning even though markdown is disabled.
      - probably should add a simple failsafe check right before actually
        sending prompts
20. Iff there are only two lines and the second line is all whitespace (new
    empty line) then discard that from the prediction... seems janky when it
    shows up
     - I think the tail reduction already trims the final whitespace
03. optionize the git diff functionality

^^^^^^^^^ DONE

03. RING BUFFER UPDATES
     - pick_chunk_inner: 
            // TODO probably only actually evict from the ring_buffer once
            // the chunk enters the buffer. So here we should only be evicting from
            // the similar from the queue.
            self.evict_similar(chunk, 0.9);
     - allow for multiple chunks to be picked once insert mode is entered. 
       - do not evict similar chunks to the text on fim_completion?
          -> this acts only to slow down completions, chunks should be evicted
          passively when ring_buffer processing is active once each chunk is
          added to the queue. 
           - fim completion: 
               // TODO strange that we only evict a single chunk here I imagine that we could evict more if the
               // text.len > chunk_size. ALSO it seems like this eviction process is going to slow down
               // code completions,
               // TODO OPTIONALY queue up the chunk deletions rather than deleting them at FIM time
               //       -> they should still be evicted from the ring_buffer queue at this moment however
        DO -> just (as an option) just don't remove this chunk and
           don't even actually queue it up for deletion, just wait for the ring
           buffer pick function which occurs at the end of the fim_completion
           section to add new chunks which may end up bumping existing chunks
           out.

     - fim_completion: 
          // TODO option to allow picking more than one chunk from the scope here
     - parameterize the queue length! 
     - IF we are removing from the main ring buffer chunk once we actually put
       it in the queue, we should batch buffer the queue because everytime there
       is an earlier removal 
        - new parameter of the max amount of queue entries to batch process
        - OKAY WAIT COULD still dynamically process the queue ASSUMING that
          there isn't any removals right? SO maybe the thing to do, is to
          process all the removals in batch right as the first batch process
          THEN process the remainder of the queue entries in order one by one
          (were we know we're not adding anything) 
            - the removals-processing moment should NOT add any of the queue
              entries which removed them, those should still be processed one by
              one in the post-removal ring-buffer updating.
        - NOTE important - We need to take a snapshot of all the queued chunks
          which are being processed for the removal processing and subsiquent
          additions processing SO THAT if a new chunk gets picked somehow during
          these proceedings it doesn't get processed until the NEXT
          removal-round. A new queue'd chunk which gets added post-removal
          during the updating SHOULD HOWEVER be allowed to remove chunks from
          the list of actively processing chunks.  
        - Keep an BTreeMap of all the chunks-ids that have already been compared
          similarity between (for eviction) which we can quickly check before
          each process in order to reduce recomputations of similarity

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




03. option to not predict while in comments
     - should ALLOW comment predictions immediately after 
       accepting a code completion, because the code completion 
       may end in a comment, and if that's the case then we want to allow for 
       further code completions
Use the synID() and synIDattr() functions to check the syntax ID under the cursor:
function! IsInComment()
  let l:syn_id = synID(line('.'), col('.'), 1)
  let l:syn_name = synIDattr(l:syn_id, 'name')
  return l:syn_name =~? '^comment$'
endfunction

05. integrate in LSP diagnostics into input_prefix (?)
     - because the diagnostics are per-line and are likely not to get reused
       then adding them to the beginning of input prefix is likely the best
       strategy... 
       - NOTE If we added them as the final input_extra entry then they would
         get priority positioning, but if we removed it on the next cache call
         then it would break the caching mechanism and all subsiquent calls
         would recalculate all the cached entries (which would not have had to
         happen if we never removed this input_extra).

       - HOWEVER the issue with adding it to the input_prefix, is that the
         content CAN ACTUALLY GET TRUNCATED once it reaches llama.cpp which
         truncates this information to fit within a batch size (for prefix and
         suffix lines) -> We could risk it and try and add it to the prefix but
         then make n_prefix small so that hopefully it doesn't truncate the
         diagostic information. 

       - could experiment with re-adding in the token "<FIM_PRE>" after the
         diagnostic information is provided.. might be a problem though
       - Caching: Your diagnostic would be cached and reused across requests,
         potentially contaminating future completions
     - Use autocmd and keep our own map 
     - https://neovim.io/doc/user/diagnostic/#diagnostic-events


05. Use TAB-TAB from normal mode to fix lines with diagnostic errors 
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

------------------------

05. integrate in LSP Completions into input_prefix (?)
     - suppliment the llm completions with suggestions from the LSP completions
     - MAYBE also just provide the LSP completion as an option immediately until
       the LLM response comes in. I noticed with ALE (from insert mode go C-X
       then C-O) it gives a suggestion with a `...` in it which is probably
       where the cursor should just be inserted if the completion is accepted.

10. integrate definitions of all nearby objects 
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

40. instruction system LOW priority can use CodeCompanion for now

40. reduce cognitive offloading allowing for llm calls to be ignored a % of the
    time (hence you don't know if you're waiting for an llm or waiting for
    nothing!).
     - reduce_cognitive_offloading_ratio = 25%

40. README gif of homer with the bird
     - link it to https://www.youtube.com/watch?v=R_rF4kcqLkI
