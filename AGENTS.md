# lttw Agent Guidelines

## Build Commands

```bash
# Build the Rust library (for Neovim plugin)
cargo build --release

# Build the CLI binary
cargo build --bin lttw --release

# Run all unit tests
cargo test

# Run a specific test
cargo test test_name

# Run ignored integration tests (requires llama.cpp server)
cargo test -- --ignored

# Build with debug symbols
cargo build
```

## Linting

```bash
# Check code for issues without building
cargo check

# Format code
cargo fmt

# Lint Rust code
cargo clippy
```

## Testing

```bash
# Run all tests
cargo test

# Run specific test module
cargo test --lib cache

# Run specific test
cargo test test_cache_basic

# Run tests with output
cargo test -- --nocapture

# Run tests with profile
cargo test --profile bench
```

## Code Style Guidelines

### Rust

**General Style:**
- Follow official [Rust Style Guide](https://doc.rust-lang.org/style-guide/)
- Use `cargo fmt` for automatic formatting
- Use 4-space indentation (matches Neovim tabstop)
- Prefer `&str` over `String` for string slices
- Use `Vec<T>` for owned vectors, `&[T]` for slices

**Imports:**
```rust
// Standard library first
use std::collections::HashMap;

// External crates second
use serde::{Deserialize, Serialize};

// Local modules last
use crate::config::LttwConfig;
```

**Naming Conventions:**
- Types: \x60PascalCase\x60 (e.g., \x60LttwConfig\x60, \x60FimRequest\x60)
- Functions/Methods: `snake_case` (e.g., `fim_completion`, `get_indent`)
- Constants: `SCREAMING_SNAKE_CASE` (e.g., `MAX_KEYS`)
- Modules: `snake_case` (e.g., `context.rs`, `cache.rs`)

**Error Handling:**
- Use `thiserror::Error` for custom error types
- Use `anyhow::Error` for general error propagation
- Provide descriptive error messages
- Handle HTTP errors separately from JSON errors

**Comments:**
- Document all public functions with Rustdoc comments
- Use `//` for inline comments
- Explain "why" not "what" in comments
- Include usage examples in doc comments when helpful

**Structs & Enums:**
- Derive `Debug`, `Clone` where appropriate
- Use `Serialize`/`Deserialize` for data transfer objects
- Group related fields logically
- Use `Option<T>` for nullable fields

### Lua

**General Style:**
- Follow [Lua Style Guide](https://github.com/Olivine-Labs/lua-style-guide)
- Use 2-space indentation
- Use `snake_case` for function and variable names
- Use `PascalCase` for module names (e.g., `llama.init`)

**Imports:**
```lua
-- Standard libraries first
local vim = vim

-- Local modules second
local M = {}

-- Default configuration (table structure)
M.default_config = {
    endpoint_fim = 'http://127.0.0.1:8012/infill',
    -- ... other config
}
```

**Naming Conventions:**
- Module tables: `camelCase` or `PascalCase` (e.g., `llama`, `llama.fim`)
- Public functions: `snake_case` (e.g., `fim_completion`)
- Private functions: `_snake_case` (e.g., `_send_request`)
- Config keys: `snake_case` (e.g., `n_prefix`, `max_cache_keys`)

**Error Handling:**
- Return `(nil, error)` pattern for errors
- Use `vim.notify()` for user-facing errors
- Log errors to debug pane when available

**Comments:**
- Document all public functions with comment blocks
- Include argument types and return values
- Explain complex logic in comments

### Formatting

**File Headers:**
All Rust files should start with a comment block:
```rust
// src/module.rs - Brief description
//
// Detailed description of what this module does.
```

All Lua files should start with a comment block:
```lua
-- lua/llama/module.lua - Brief description
--
-- Detailed description of what this module does.
```

**Line Length:**
- Max 100 characters per line
- Break long function signatures across lines
- Keep related code grouped together

### Error Messages

- Use clear, actionable error messages
- Include context (e.g., "HTTP error: failed to connect")
- Provide troubleshooting hints when possible
- Use `thiserror` for structured Rust errors

### API Design

**Rust:**
- Use async/await for network requests
- Return \x60LttwResult<T, Error>\x60 for fallible operations
- Use builder patterns for complex configurations
- Keep public API minimal and focused

**Lua:**
- Use tables for configuration objects
- Provide sensible defaults
- Support both global and per-buffer configuration
- Use `vim.keymap.set()` for keybindings

## Testing Requirements

**Unit Tests:**
- Test all public functions
- Test edge cases (empty inputs, out-of-bounds)
- Use `#[cfg(test)]` modules
- Mock external dependencies where possible

**Integration Tests:**
- Mark with `#[ignore]` flag
- Require running llama.cpp server
- Document server requirements in test comments
- Use `#[tokio::test]` for async tests

## Project Structure

```
lttw/
├── Cargo.toml              # Rust dependencies
├── src/
│   ├── lib.rs             # Library entry point
│   ├── config.rs          # Configuration handling
│   ├── context.rs         # Context gathering
│   ├── cache.rs           # LRU cache
│   ├── ring_buffer.rs     # Ring buffer for chunks
│   ├── fim.rs             # FIM completion
│   ├── instruction.rs     # Instruction editing
│   ├── debug.rs           # Debug management
│   └── utils.rs           # Utility functions
├── lua/
│   └── llama/
│       ├── init.lua       # Plugin initialization
│       ├── fim.lua        # FIM integration
│       ├── instruction.lua # Instruction integration
│       └── debug.lua      # Debug integration
└── tests/                 # Test files (if added)
```

## Keymap Conventions

All keymaps use `<leader>ll` prefix:
- `llf` - FIM trigger
- `ll]` - FIM accept word
- `lli` - Instruction trigger
- `llr` - Instruction rerun
- `llc` - Instruction continue
- `lld` - Debug toggle

## Configuration

Configuration uses \x60vim.g.lttw_config\x60 table with defaults in \x60M.default_config\x60. Values should be deeply extended using \x60vim.tbl_deep_extend('force', {}, M.default_config, vim.g.lttw_config or {})\x60.

## Neovim API Usage

- Use \x60vim.api.nvim_buf_get_lines()\x60 for buffer access
- Use `vim.api.nvim_buf_set_lines()` for modifications
- Use `vim.notify()` for messages
- Use `vim.fn.wordcount()` for position info
- Use `vim.opt_local.tabstop = 4` for consistent indentation

## Debugging

- Enable debug mode with `<leader>lld`
- Use `require('llama').debug_toggle()` programmatically
- Debug logs appear in dedicated debug pane
- Include timestamps and relevant context in log messages