# LTTW Agent Guidelines


## Project Overview

LTTW is a Neovim plugin for code completion using llama.cpp, written in ENTIRELY in Rust. It provides Fill-in-Middle (FIM) completion editing capabilities.

## Build & Development Commands

### Build
```bash
cargo build
```

### Test
```bash
# Run all tests (unit tests in Rust modules)
cargo test

# Run tests for a specific module
cargo test --package lttw --lib <module_name>::tests

# Run a single test
cargo test --package lttw --lib <module_name>::tests::<test_function_name>

# Example: Run config tests
cargo test --package lttw --lib config::tests

# Run with debug output
cargo test -- --nocapture
```

### Lint & Format
```bash
# Format code
cargo fmt

# Lint code
cargo clippy --all-targets --all-features -- -D warnings

# Lint specific module
cargo clippy --lib -- src/config.rs
```

### Common Task Commands
```bash
# Check specific file for syntax errors
cargo check --file src/<filename>.rs

# Build specific module
cargo build --lib --src src/<filename>.rs
```

## Code Style Guidelines

### Logging & Debugging
- Use `debug!()` macro for debug logging (timestamped, file/line info)
- Logs written to `lttw.log` in working directory
- Log file path configurable via `set_log_file()`
- Debug mode toggled via `LttwEnableDebug`/`LttwDisableDebug` commands

```rust
debug!("Processing {} completions", count);
debug!(variable_name); // Automatic "variable = value" format
```

### Neovim API Usage
- **Never call Neovim APIs from tokio worker threads** (use `assert_not_tokio_worker()`)
- Use `nvim_oxi::api` module for most operations
- Buffer positions are 0-indexed internally, SOMETIMES 1-indexed in Neovim API
  - it is very confusing always check function comments when referencing
    neovim-api 
- Use extmarks for virtual text display (FIM suggestions)

### Memory Management
- Use `parking_lot` for mutexes (`RwLock`, `Mutex`)
- Use `Arc` for shared ownership
- Use `Lazy` for global state initialization
- Avoid unnecessary cloning of large data structures
- Use `#[derive(Clone)]` judiciously

### Type Definitions
- Use `serde` with `Serialize`/`Deserialize` for config
- Use `#[derive(Debug, Clone)]` for types that need debugging

### Configuration
- Config loaded via `LttwConfig::from_object()` from Neovim globals
- Default values defined in `impl Default for LttwConfig`
- Filetype-based enable/disable via `enabled_filetypes`/`disabled_filetypes`
- Config is global state accessed via `get_state()`

### Comment Guidelines
- Explain **why**, not **what** in comments

### File Type Specifics

ALL FILES SHOULD BE WRITTEN IN RUST. This is a rust library NOT Lua.

- Follow standard Rust conventions
- Use `nvim-oxi` plugin attributes for Neovim FFI
- Test modules in same file as implementation
- Use `#[derive]` for boilerplate reduction

### Dependencies
Managed via `Cargo.toml`:
- `nvim-oxi`: Neovim FFI (with test features)
- `serde`: Serialization
- `thiserror`: Error handling
- `reqwest`: HTTP client
- `tokio`: Async runtime
- `gix-*`: Git operations
- `regex`: Pattern matching

## Notes for AGENTS
- This is a **Neovim plugin** - all user interactions go through Neovim
- The Rust code is a **library** exposed to Neovim via FFI
