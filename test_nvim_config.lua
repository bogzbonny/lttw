-- run with: 
-- nvim -u test_nvim_config.lua src/fim.rs
vim.opt.runtimepath:append('./testing_plugin/lttw')
require("lttw").lttw_setup()

