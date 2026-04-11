01. fix filetype prediction logic
01. add debounce
01. Should not be Inline extmarks when there are no ending matches
01. sometimes sending two updates (not just one) probably on 'o' because it is
    both a cursor move and entering insert mode. Solved with debounce
01. Panic condition when tab from an empty line 
01. fix cursor positioning on accepted text
02. sometimes doesn't display if on empty line (tab still works)
     - Seems like this only happens when we're on the very first character of a
       line some edge case biz
01. When typing on top of a suggestion, that suggestion should still be there
    IFF we're typing the same content... wonder if this has to do with the ring
    buffer
     - I think this may have to do with speculative FIM actually
01. Do not render the virtual text if it nolonger matches what's actually in the
    line

01. ring buffer at end of fim 
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

01. TOSS FIMs which are nolonger for the correct buffer location.
     -> ensure that when a cached FIM is used, the location is updated
     appropriately (don't want to toss these precious caches accidently)

01. bizzare issue with re-rendering msgs as they come where the cursor gets
    slammed to the end of the queue message. 
     - probably something to do with inline extmarks or even improper placement 
     (not checking before display if the x_pos is wrong?) 
01. fix should abort to prevent unnecessary llm calls
     - streamlined the debounce system a bit. 
01. Add (info) stats as lsp progress messages 
     - send this information through LSP progress messages. 


01. Remove trailing prediction lines if they match 
     - go through one by one
     - NOTE this could use the stop_strings, however that seems problematic if
       the actual completion DOES really have a duplication of the stop string
       which its meant to generate. 
     - changes in fim.rs accept_fim_suggestion
       - ACTUALLY I think this one will be a bit easier given the information
         available to us to filter out directly in fim_completion once we get
         the response (line 600) 
05. info disappears once completion is done (should only disappear once leaving
    insert mode, or next completion displayed) 
      - should also appear as the FIM is displayed not when FIM is accepted
05. track the number of llm calls currently running. 
     - if the max number of concurrent llm calls is reached then the debounce 
       should simply wait until this goes down before launching.. ALSO all
       waiting debounced llm calls should abort unless they are the top of the
       seq after waiting for the llm calls to go down.
     - used a semaphore, no tracking required clearly works great
01. debug to see if rerender-fim suggestion is causing infinite loops (see
    process_pending_display)
     - added retry count to explicitly prevent this

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
03. RING BUFFER UPDATES
     - DONE parameterize the queue length! 
     - DONE pick_chunk_inner: 
            // TODO probably only actually evict from the ring_buffer once
            // the chunk enters the buffer. So here we should only be evicting from
            // the similar from the queue.
            self.evict_similar(chunk, 0.9);
        - SO according to https://github.com/ggml-org/llama.cpp/pull/9787 the
          eviction doesn't actually increase the computation time significantly!
     - DON"T DO ALL THIS COMPLEXITY - just evict the queue chunks as we process
       them.
       IF we are removing from the main ring buffer chunk once we actually put
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
10. n_predict changes
dynamically change n_predict during each FIM call (number of tokens to predict)
 - replaces existing n_predict config option
 - When in a line and there are non-whitespace characters to the right of the
   cursor set to n_predict_inner (default value 16)
 - for at the end of a line or where there's only whitespace left to the right
   of the cursor set to a new config param n_prefict_end (default value 256)
01. when debugging is disabled the lttw.log file is still created/cleared, this
    shouldn't happen
20. easier to use debugging system (like info! macro)
05. completion cycling
New keymaps; use CTRL-j and CTRL-k from insert mode to cycle through the
completions options. Now whenever we compile autocompletions from previous
nearby autocompletions, we should keep a list of all the autocompletions
(ordered from longest to shortest) and start by displaying the first FIM but
then allow cycling through these
05. regenerate
New keymaps; Ctrl+l from insert move to regenerate the completion at the
location. NOTE add this to the list of completions at this location, so one can
cycle back through them if necessary. 
05. regeneration removes everything but most recent 

05. single_line_prediction_within_line
 - Option to ONLY show the first line of code completions unless you're in an empty line
     -> could still predict more, but just dont show it
 - Option to ONLY accept single line inline suggestions if typing within a fully
   closed bracket system within a line example: "#[derive(Debug, Cl[CURSOR], Default)]"
set the fim request `stop` value '\n' when inside a new line if a new config option
single_line_prediction_within_line is set to true (default is true). Integrate
this logic into get_dynamic_n_predict
03. integrate git diff system into extra_inputs 
     - DONE if we use a super simple approach were we don't calculate any of this biz
       and just evict like normal, the queue ordering should probably be
       rectified to process in order again (instead of popping) 
     - git diff --no-ext-diff --unified=0
         -> use unified=0 for concise chunks
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

05. write file contents on first enter of a buffer we haven't entered before 
05. update the single line output inside line functionality to not use 'stop'
    but just truncate the file contents
05. integrate in LSP Completions into input_prefix
     - use vim.lsp.Client.request_sync directly
          - https://neovim.io/doc/user/lsp/#_lua-module%3a-vim.lsp.client
          - https://neovim.io/doc/user/lsp/#Client%3Arequest()
          - OR for sync https://neovim.io/doc/user/lsp/#Client%3Arequest_sync()
     - nvim_oxi doesn't currently support lsp natively - could probably still
       call the sync function. directly 
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
01. lsp completions - do not do any on empty string
01. use sort order for the items coming in 
     - keep a passive map of all words in the files to sort by most common 
       for the top of the list
01. deadlock bug hiding in the lsp completion system 
     - seems to have come up in this refactor to include word statistics

10. tracing support
integrate in the tracing and tracing-opentelemetry crates into this library.
Replace all instances of this crates \x60info!\x60 with the tracing \x60info!\x60 macro.
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
01. This situation is for some reason not putting things inside: 
       if let Err(...)e = fim_hide();
     comp.text = "Err($1)$0" - need to strip the final $0
01. Allow for suffix trimming IFF A SINGLE character is removed at the end. 
    Eg. if match is Option<String> and the suffix is String then the suffix is a
    match up to that character. 
00. bracket matching suffix removal goes to the end of the line

00. feeling a bit slow should probably NOT initiate the infill until a pause has
    completed. Test by holding the backspace on code vs comment
      - THE ISSUE is that there actually the cache computation which doesn't
        happen async
         - DONE
      - there is currently NO ignoring repeated keystrokes for cache completion 
        OR LSP completion - should add a small delay using a last keystroke biz. 
         - maybe use "try_read" on the "last move time" and just skip the
           completion if its we can't try
      - DONE - ONLY compute the next_var at the time when we know we'll need it when
        processing a completion. (use Option Option)
      - on my computer the key repeat rate is about 66 ms maybe make this special debouce
        80ms? - or fold lsp into the other debounce?
05. ensure that when messages come in they aren't duplicating existing messages
    already around 
05. telemetry doesn't work if tracing logfile is disabled (it should)
00. regression on typing maintaining the same thing on the screen due to moving
    the cache logic into async. 
     - now the LSP is being computed all the time (maybe the issue) 
     - the whole group of commits is not being written (only the most recent one
       is being sent through a message.
01. reduce flicker - on_move fim_hide should actually take place inside of
    try_fim_hint as the first message sent in (modify messages to be able to
    take in alternative biz)
00. do NOT do lsp-completion is there is a cached guy found

00. if lsp_comp_insert_one_var=true AND a variable match was found 
     then we should skip matching removing any matching suffix characters from
     the match - this sometimes removes final ')' undesirably 
01. LSP rematch options eg. Ok() is predicted a decent amount which should
    probably re rerouted to Ok(()) (config option this) 
    - comp.text = "let mut $1 = $0;" is funny and turns into let mut ... = ;
    it would almost make sense to have it just be 'let mut '[truncated]
     - having a predictable reusable pattern for this is funny though
     - maybe this is one for the rematch routine
Add a new configuration option "lsp_overrides" which is an array of string pairs.
For the default value of this add one override pair: ("Ok()", "Ok(())"). 
In lsp_completions.rs right at the end of generating the lsp text, add compare
the final text generated against this list of rematches, if a match is found
then modify the text to the override provided. For example if Ok() was found
modify to Ok(()). Add comments as to how one would use this in their config in
README.md
