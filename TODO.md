
^^^^^^^^^ DONE

00. commented out XXXs for pick_chunk

01. bizzare issue with re-rendering msgs as they come where the cursor gets
    slammed to the end of the queue message. 
     - probably something to do with inline extmarks or even improper placement 
     (not checking before display if the x_pos is wrong?) 

01. Remove trailing prediction lines if they match 
     - go through one by one

01. check on the amount of llm calls make sure we're not goin crzy

01. Add (info) stats as rhs extmarks
 - alternatively maybe send this information through LSP progress messages. 

You are in the middle of the task of translating the codebase provided under
`llama.vim/` into a new neovim plugin written in ENTIRELY rust using neovim
bindings through the use of the nvim-oxi crate. NOTE there is not, nor should
there EVER be ANY lua code in this project, all plugin functionality is achieved
PURELY by using nvim-oxi 

Add in rendering of the build info string (see build_info_string) to the rust
code. Render this extmarks which are RightHand justified. 

05. integrate git diff system into extra 

05. integrate in LSP diagnostics into extra 
     - Use autocmd and keep our own map 
     - https://neovim.io/doc/user/diagnostic/#diagnostic-events

05. add config option for debugging (default false)
05. fix tests-integrations (compile errors) 

------------------------

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

10. easier to use debugging system (like debug! macro)
20. option to not predict while in comments
20. better global error printing 
    https://github.com/noib3/nvim-oxi/issues/231
20. allow for a more regular setup by passing config params through the setup
    function

 

