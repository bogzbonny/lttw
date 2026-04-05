03. continious ring_buffer processing rather than just 1 per second while in
    normal/mode or nothing is happening
20. add config option for debugging (default false)
20. allow for a more regular setup by passing config params through the setup
    function

^^^^^^^^^ DONE

03. integrate git diff system into extra_inputs 
     - definitely should integrate with extra_input ring_buffer system -
       ordering is important
     - should be a part of the same ring buffer 

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


05. Use TAB from normal mode to fix lines with diagnostic errors (THEN I can
    simply jump using [[ / ]] and then tab to fix those lines! yay) 
     - I'm not sure if it'll be a pain to still use FIM for this or not
     - Kinda thinking that it should maybe be TAB-TAB from normal mode to
       activate the FIM completion?

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
    
20. Iff there are only two lines and the second line is all whitespace (new
    empty line) then discard that from the prediction... seems janky when it
    shows up

30. investigate FIM techniques used by https://huggingface.co/zed-industries/zeta-2
     - I think would require a implementing my own FIM system, which would be
       useful anyways

40. instruction system LOW priority can use CodeCompanion for now

40. reduce cognitive offloading allowing for llm calls to be ignored a % of the
    time (hence you don't know if you're waiting for an llm or waiting for
    nothing!).
