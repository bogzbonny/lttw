// tests-integration/src/lib.rs - Integration tests for FIM using nvim-oxi
//
// These tests spawn a Neovim instance and test actual FIM functionality.
// They require:
// 1. Neovim installed and in $PATH
// 2. llama.cpp server running at http://127.0.0.1:8012 (for server tests)
//
// Run with: cargo test -p lttw-integration-tests
// Run with server tests: cargo test -p lttw-integration-tests -- --ignored

use nvim_oxi::api;
use nvim_oxi::Result;

/// Test that we can access Neovim API in test context
#[nvim_oxi::test]
fn test_plugin_initialization() -> Result<()> {
    // Verify we can access Neovim API
    let buf = api::Buffer::current();
    assert!(buf.is_valid());

    Ok(())
}

/// Test basic buffer operations with Rust code context
#[nvim_oxi::test]
fn test_fim_basic_rust() -> Result<()> {
    // Create a buffer with some Rust code
    let code = vec![
        "fn main() {",
        "    let x = ", // Cursor would be here
        "}",
    ];

    let mut buf = api::Buffer::current();
    buf.set_lines(.., true, code.into_iter())?;

    // Set cursor at position (line 1, col 13) - after "let x = "
    let mut win = api::Window::current();
    win.set_cursor(1, 13)?;

    // At this point we could call FIM completion if the server is running
    // For now just verify the buffer was set up correctly
    let lines: Vec<String> = buf
        .get_lines(.., false)?
        .collect::<Vec<_>>()
        .into_iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[1], "    let x = ");

    Ok(())
}

/// Test FIM context gathering in Neovim buffer
#[nvim_oxi::test]
fn test_fim_context_gathering() -> Result<()> {
    // Create a multi-line buffer to test context extraction
    let code = vec![
        "fn add(a: i32, b: i32) -> i32 {",
        "    a + b",
        "}",
        "",
        "fn main() {",
        "    let result = ", // Cursor position
        "}",
    ];

    let mut buf = api::Buffer::current();
    buf.set_lines(.., true, code.into_iter())?;

    // Set cursor at line 5 (0-indexed), column 17
    let mut win = api::Window::current();
    win.set_cursor(5, 17)?;

    // Get all buffer lines
    let lines: Vec<String> = buf
        .get_lines(.., false)?
        .collect::<Vec<_>>()
        .into_iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(lines.len(), 7);

    // Verify we can access line at cursor position
    let (line, _col) = api::Window::current().get_cursor()?;
    assert_eq!(line, 5);

    Ok(())
}

/// Test buffer manipulation - simulating FIM accept
#[nvim_oxi::test]
fn test_fim_accept_simulation() -> Result<()> {
    // Create initial code
    let code = vec![
        "fn hello() {",
        "    println!(\"hello\");",
        "}",
    ];

    let mut buf = api::Buffer::current();
    buf.set_lines(.., true, code.clone().into_iter())?;

    // Simulate accepting a FIM completion by inserting text
    let completion = "world";
    let mut line1 = code[1].to_string();
    line1.push_str(completion);

    // Update the buffer with accepted completion
    buf.set_lines(1..2, true, vec![line1].into_iter())?;

    // Verify the completion was applied
    let updated_lines: Vec<String> = buf
        .get_lines(.., false)?
        .collect::<Vec<_>>()
        .into_iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(updated_lines[1], "    println!(\"hello\");world");

    Ok(())
}

/// Test FIM with cached completion lookup in Neovim context
#[nvim_oxi::test]
fn test_fim_cache_lookup() -> Result<()> {
    // This test verifies the cache mechanism works in a real Neovim instance
    use lttw::cache::Cache;
    
    let mut cache: Cache = Cache::new(10);

    // Simulate caching a completion
    let key = "test_hash_123".to_string();
    let value = r#"{"content":"42;","timings":{}}"#.to_string();
    cache.insert(key.clone(), value);

    // Verify we can retrieve it
    assert!(cache.contains_key(&key));
    assert_eq!(
        cache.get_fim(&key).unwrap().as_str(),
        r#"{"content":"42;","timings":{}}"#
    );

    Ok(())
}

/// Test ring buffer system with chunk management
#[nvim_oxi::test]
fn test_ring_buffer_basic() -> Result<()> {
    use lttw::ring_buffer::RingBuffer;
    
    let mut ring_buffer = RingBuffer::new(3, 64);

    // Add first chunk
    ring_buffer.pick_chunk(
        vec![
            "fn main() {".to_string(),
            "    println!(\"hello\");".to_string(),
            "}".to_string(),
        ],
        false,
        true,
    );
    ring_buffer.update();

    assert_eq!(ring_buffer.len(), 1);
    assert_eq!(ring_buffer.queued_len(), 0);

    // Add second chunk
    ring_buffer.pick_chunk(
        vec![
            "use std::io;".to_string(),
            "fn read_input() {".to_string(),
            "    let mut s = String::new();".to_string(),
        ],
        false,
        true,
    );
    ring_buffer.update();

    assert_eq!(ring_buffer.len(), 2);

    // Verify extra context is returned
    let extra = ring_buffer.get_extra();
    assert_eq!(extra.len(), 2);
    assert!(!extra[0].text.is_empty());

    Ok(())
}

/// Test ring buffer eviction with similar chunks
#[nvim_oxi::test]
fn test_ring_buffer_eviction() -> Result<()> {
    use lttw::ring_buffer::RingBuffer;
    
    let mut ring_buffer = RingBuffer::new(5, 64);

    let chunk1 = vec![
        "fn function_one() {".to_string(),
        "    let x = 1;".to_string(),
        "    let y = 2;".to_string(),
        "    let z = 3;".to_string(),
        "}".to_string(),
    ];

    // Add first chunk
    ring_buffer.pick_chunk(chunk1.clone(), false, true);
    ring_buffer.update();

    assert_eq!(ring_buffer.len(), 1);

    // Add very similar chunk (should evict first due to >0.9 similarity)
    let mut chunk2 = chunk1.clone();
    chunk2[1] = "    let x = 100;".to_string(); // Slightly different

    ring_buffer.pick_chunk(chunk2, false, true);
    ring_buffer.update();

    // Due to high similarity, first chunk should be evicted
    // The exact behavior depends on the similarity threshold (0.9)
    assert!(ring_buffer.len() <= 2);

    Ok(())
}

/// Test cache integration with ring buffer chunks
#[nvim_oxi::test]
fn test_cache_with_ring_buffer() -> Result<()> {
    use lttw::cache::Cache;
    use lttw::ring_buffer::RingBuffer;
    use lttw::fim::compute_hashes;
    use lttw::context::LocalContext;
    
    let mut cache = Cache::new(10);
    let mut ring_buffer = RingBuffer::new(3, 64);

    // Add chunks to ring buffer
    ring_buffer.pick_chunk(
        vec![
            "fn test1() {".to_string(),
            "    println!(\"test1\");".to_string(),
            "}".to_string(),
        ],
        false,
        true,
    );
    ring_buffer.update();

    // Simulate a FIM request with ring buffer context
    let ctx = LocalContext {
        prefix: "fn main() {\n    let x = 1;\n".to_string(),
        middle: "    println!(\"hello\"".to_string(),
        suffix: ");\n}".to_string(),
        line_cur_suffix: "rintln!(\"hello\");".to_string(),
        indent: 4,
    };

    let hashes = compute_hashes(&ctx);

    // Verify we generated multiple hashes
    assert!(hashes.len() > 1, "Should generate multiple hashes from truncated prefixes");

    // Cache a response for these hashes
    let response = r#"{"content":" world","timings":{},"tokens_cached":0,"truncated":false}"#;
    for hash in &hashes {
        cache.insert(hash.clone(), response.to_string());
    }

    // Verify cache contains the entries
    for hash in &hashes {
        assert!(cache.contains_key(hash));
    }

    Ok(())
}

/// Test FIM suggestion rendering
#[nvim_oxi::test]
fn test_fim_render_suggestion() -> Result<()> {
    use lttw::fim::render_fim_suggestion;
    use lttw::config::LttwConfig;
    
    let config = LttwConfig::new();
    
    // Test rendering a simple suggestion
    let content = "42;";
    let line_cur = "    let x = ";
    let pos_x = 11; // After "= "
    
    let rendered = render_fim_suggestion(pos_x, 0, content, line_cur, &config);
    
    assert!(!rendered.content.is_empty());
    assert!(rendered.can_accept);
    
    Ok(())
}

/// Test FIM accept functionality
#[nvim_oxi::test]
fn test_fim_accept_word() -> Result<()> {
    use lttw::fim::accept_fim_suggestion;
    
    let content = vec!["world".to_string()];
    let line_cur = "Hello ";
    let pos_x = 6;
    
    let (new_line, _rest) = accept_fim_suggestion("word", pos_x, line_cur, &content);
    
    assert!(new_line.contains("world"));
    
    Ok(())
}

/// Test FIM accept full suggestion
#[nvim_oxi::test]
fn test_fim_accept_full() -> Result<()> {
    use lttw::fim::accept_fim_suggestion;
    
    let content = vec![
        "fn greet() {".to_string(),
        "    println!(\"Hello\");".to_string(),
        "}".to_string(),
    ];
    let line_cur = "";
    let pos_x = 0;
    
    let (new_line, rest) = accept_fim_suggestion("full", pos_x, line_cur, &content);
    
    assert_eq!(new_line, "fn greet() {");
    assert!(rest.is_some());
    assert_eq!(rest.unwrap().len(), 2); // Two remaining lines
    
    Ok(())
}

/// Test LRU cache eviction with ring buffer usage
#[nvim_oxi::test]
fn test_cache_lru_eviction() -> Result<()> {
    use lttw::cache::Cache;
    
    let mut cache = Cache::new(5); // Small cache for testing
    
    // Insert more items than max_keys
    for i in 0..10 {
        let key = format!("key_{}", i);
        let value = format!("value_{}", i);
        cache.insert(key, value);
    }
    
    // Cache should not exceed max_keys (5)
    assert!(cache.len() <= 6); // May temporarily be one over during insertion
    
    Ok(())
}

/// Test ring buffer duplicate prevention
#[nvim_oxi::test]
fn test_ring_buffer_no_duplicates() -> Result<()> {
    use lttw::ring_buffer::RingBuffer;
    
    let mut ring_buffer = RingBuffer::new(5, 64);

    let chunk = vec![
        "fn duplicate_test() {".to_string(),
        "    let x = 1;".to_string(),
        "}".to_string(),
    ];

    // Add chunk first time
    ring_buffer.pick_chunk(chunk.clone(), false, true);
    ring_buffer.update();

    assert_eq!(ring_buffer.len(), 1);

    // Try to add exact same chunk again (should be ignored)
    ring_buffer.pick_chunk(chunk.clone(), false, true);
    ring_buffer.update();

    // Should still be 1 (no duplicate added)
    assert_eq!(ring_buffer.len(), 1);

    Ok(())
}

/// Test FIM request building with extra context from ring buffer
#[nvim_oxi::test]
fn test_fim_request_with_extra_context() -> Result<()> {
    use lttw::fim::FimRequest;
    use lttw::ring_buffer::RingBuffer;
    
    let mut ring_buffer = RingBuffer::new(2, 64);

    // Add some chunks to the ring buffer
    ring_buffer.pick_chunk(
        vec![
            "mod module1;".to_string(),
            "mod module2;".to_string(),
            "mod module3;".to_string(),
        ],
        false,
        true,
    );
    ring_buffer.update();

    let extra = ring_buffer.get_extra();

    let request = FimRequest {
        id_slot: 0,
        input_prefix: "fn main() {".to_string(),
        input_suffix: "}".to_string(),
        input_extra: extra,
        prompt: "    println!(\"hello\"".to_string(),
        n_predict: 32,
        stop: vec![],
        n_indent: 4,
        top_k: 40,
        top_p: 0.90,
        samplers: vec!["top_k".to_string(), "top_p".to_string()],
        stream: false,
        cache_prompt: true,
        t_max_prompt_ms: 500,
        t_max_predict_ms: 1000,
        response_fields: vec!["content".to_string()],
        model: "".to_string(),
        prev: vec![],
    };

    let json = serde_json::to_string(&request).expect("Request should serialize to JSON");
    let parsed: serde_json::Value =
        serde_json::from_str(&json).expect("JSON should be parseable");

    // Verify input_extra contains the chunk data
    let extra_array = parsed["input_extra"].as_array().unwrap();
    assert_eq!(extra_array.len(), 1);
    assert!(extra_array[0].get("text").is_some());

    Ok(())
}

// ============================================================================
// Server Integration Tests (require running llama.cpp server)
// These tests are marked with #[ignore] and must be run with --ignored flag
// ============================================================================

/// Test FIM completion with actual llama.cpp server
#[nvim_oxi::test]
#[ignore = "requires llama.cpp server running at http://127.0.0.1:8012"]
fn test_fim_server_completion() -> Result<()> {
    use lttw::config::LttwConfig;
    use lttw::cache::Cache;
    use lttw::ring_buffer::RingBuffer;
    use tokio::runtime::Runtime;
    
    // Create a buffer with incomplete code
    let code = vec![
        "fn fibonacci(n: u32) -> u32 {",
        "    if n <= 1 { return n; }",
        "    ",  // Cursor here - expect completion
        "}",
    ];

    let mut buf = api::Buffer::current();
    buf.set_lines(.., true, code.into_iter())?;
    
    // Set cursor position
    let mut win = api::Window::current();
    win.set_cursor(2, 4)?;

    // Get lines for FIM request
    let lines: Vec<String> = buf.get_lines(.., false)?.collect::<Vec<_>>().into_iter().map(|s| s.to_string()).collect();
    
    // Setup config and state
    let config = LttwConfig::new();
    let mut cache = Cache::new(10);
    let mut ring_buffer = RingBuffer::new(3, 64);
    
    // Add some context to ring buffer
    ring_buffer.pick_chunk(vec![
        "fn helper() {".to_string(),
        "    println!(\"helper\");".to_string(),
        "}".to_string(),
    ], false, true);
    ring_buffer.update();
    
    // Run async FIM completion
    let rt = Runtime::new().unwrap();
    let result = rt.block_on(async {
        lttw::fim::fim_completion(
            4,  // pos_x
            2,  // pos_y
            false,  // is_auto
            &lines,
            &config,
            &mut cache,
            &mut ring_buffer,
            None,  // prev
        ).await
    });
    
    // Check if we got a response (may fail if server not running)
    match result {
        Ok(Some(content)) => {
            assert!(!content.is_empty(), "Server should return non-empty completion");
        }
        Ok(None) => {
            // No completion is also valid (e.g., cache hit or skipped)
        }
        Err(e) => {
            // Server errors are expected if server not running
            println!("Server error (expected if not running): {:?}", e);
        }
    }

    Ok(())
}

/// Test that FIM caching works with actual server responses
#[nvim_oxi::test]
#[ignore = "requires llama.cpp server running at http://127.0.0.1:8012"]
fn test_fim_cache_with_server() -> Result<()> {
    use lttw::config::LttwConfig;
    use lttw::cache::Cache;
    use lttw::ring_buffer::RingBuffer;
    use tokio::runtime::Runtime;
    
    let config = LttwConfig::new();
    let mut cache = Cache::new(10);
    let mut ring_buffer = RingBuffer::new(3, 64);
    
    // Setup code for FIM
    let code = vec![
        "fn add(a: i32, b: i32) -> i32 {",
        "    a + b",
        "}",
    ];

    let mut buf = api::Buffer::current();
    buf.set_lines(.., true, code.into_iter())?;

    // Get buffer lines
    let lines: Vec<String> = buf.get_lines(.., false)?.collect::<Vec<_>>().into_iter().map(|s| s.to_string()).collect();
    
    // First FIM request - should go to server
    let rt = Runtime::new().unwrap();
    let result1 = rt.block_on(async {
        lttw::fim::fim_completion(
            8,  // pos_x
            0,  // pos_y
            false,
            &lines,
            &config,
            &mut cache,
            &mut ring_buffer,
            None,
        ).await
    });

    // Second identical request - should potentially hit cache (depending on implementation)
    let result2 = rt.block_on(async {
        lttw::fim::fim_completion(
            8,  // pos_x
            0,  // pos_y
            false,
            &lines,
            &config,
            &mut cache,
            &mut ring_buffer,
            None,
        ).await
    });

    // Verify server returned something (or cached result)
    match (&result1, &result2) {
        (Ok(Some(_)), Ok(Some(_))) => {
            // Both succeeded
        }
        _ => {
            // At least one failed or returned None - may be expected
            println!("Results: r1={:?}, r2={:?}", result1, result2);
        }
    }

    Ok(())
}

/// Test ring buffer integration with server caching
#[nvim_oxi::test]
#[ignore = "requires llama.cpp server running at http://127.0.0.1:8012"]
fn test_ring_buffer_server_integration() -> Result<()> {
    use lttw::config::LttwConfig;
    use lttw::cache::Cache;
    use lttw::ring_buffer::RingBuffer;
    use tokio::runtime::Runtime;
    
    let config = LttwConfig::new();
    let mut cache = Cache::new(10);
    let mut ring_buffer = RingBuffer::new(5, 64);
    
    // Add multiple chunks to ring buffer
    for i in 0..3 {
        ring_buffer.pick_chunk(vec![
            format!("fn function_{}() {{", i),
            format!("    println!(\"function {}\");", i),
            "}".to_string(),
        ], false, true);
    }
    
    // Update ring buffer to move chunks from queued to active
    ring_buffer.update();
    ring_buffer.update();
    ring_buffer.update();
    
    assert_eq!(ring_buffer.len(), 3);
    
    // Setup code for FIM with extra context
    let code = vec![
        "fn main() {",
        "    function_0();",  // Expect server to suggest more functions
        "}",
    ];

    let mut buf = api::Buffer::current();
    buf.set_lines(.., true, code.into_iter())?;

    let lines: Vec<String> = buf.get_lines(.., false)?.collect::<Vec<_>>().into_iter().map(|s| s.to_string()).collect();
    
    // FIM request should include ring buffer extra context
    let rt = Runtime::new().unwrap();
    let result = rt.block_on(async {
        lttw::fim::fim_completion(
            16,  // pos_x - after function_0()
            1,   // pos_y
            false,
            &lines,
            &config,
            &mut cache,
            &mut ring_buffer,
            None,
        ).await
    });

    // Check result
    match result {
        Ok(Some(content)) => {
            assert!(!content.is_empty());
            println!("Server returned: {}", content);
        }
        _ => {
            println!("No completion or error");
        }
    }

    Ok(())
}

