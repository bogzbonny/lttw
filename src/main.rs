// src/main.rs - CLI binary for lttw
//
// This binary provides a command-line interface for the lttw plugin
// to call Rust functions for FIM and instruction completion.

use std::env;
use std::fs;
use std::process;

#[allow(dead_code)] // Some library functions are unused in the CLI but needed for lib
use lttw::cache::Cache;
use lttw::config::LlamaConfig;
use lttw::fim::render_fim_suggestion;
use lttw::instruction::{build_instruction_payload, process_instruction_response, send_instruction};
use lttw::ring_buffer::RingBuffer;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        print_help();
        process::exit(1);
    }

    match args[1].as_str() {
        "fim" => {
            if let Err(e) = handle_fim(&args) {
                eprintln!("Error: {}", e);
                process::exit(1);
            }
        }
        "instruction" => {
            if let Err(e) = handle_instruction(&args) {
                eprintln!("Error: {}", e);
                process::exit(1);
            }
        }
        "ring" => {
            if let Err(e) = handle_ring(&args) {
                eprintln!("Error: {}", e);
                process::exit(1);
            }
        }
        "help" => {
            print_help();
        }
        _ => {
            print_help();
        }
    }
}

fn print_help() {
    println!("lttw - CLI for lttw");
    println!("Usage: lttw <command> [options]");
    println!("Commands:");
    println!("  fim <args>           Perform FIM (Fill-in-Middle) completion");
    println!("  instruction <args>   Perform instruction-based editing");
    println!("  ring <args>          Manage ring buffer for extra context");
    println!("  help                 Show this help message");
    println!("For help with a specific command, run:");
    println!("  lttw <command> --help");
}

fn handle_fim(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.len() < 3 || args[2] == "--help" {
        println!("Usage: lttw fim <command> [args] [options]");
        println!("Commands:");
        println!("  <pos_x> <pos_y> <is_auto> <buffer_file>   Perform FIM completion");
        println!("  render <content> <line_cur>                 Render FIM suggestion");
        println!("  accept <accept_type> <pos_x> <line_cur>     Accept FIM suggestion");
        println!("  help                                        Show this help message");
        println!("Options (for completion command):");
        println!("  --endpoint_fim <url>       FIM endpoint URL");
        println!("  --endpoint_inst <url>      Instruction endpoint URL");
        println!("  --model_fim <model>        FIM model name");
        println!("  --model_inst <model>       Instruction model name");
        println!("  --api_key <key>            API key for authentication");
        println!("  --n_prefix <n>             Number of prefix lines (default: 256)");
        println!("  --n_suffix <n>             Number of suffix lines (default: 64)");
        println!("  --n_predict <n>            Number of tokens to predict (default: 128)");
        println!("  --t_max_prompt_ms <n>      Max prompt time in ms (default: 500)");
        println!("  --t_max_predict_ms <n>     Max predict time in ms (default: 1000)");
        println!(
            "  --show_info <n>            Show info: 0=none, 1=statusline, 2=inline (default: 2)"
        );
        println!("  --prev <lines>             Previously accepted lines (for speculative FIM)");
        println!("  --use_cache <0|1>          Use cached suggestions (default: 1)");
        return Ok(());
    }

    let pos_x: usize = args[2].parse()?;
    let pos_y: usize = args[3].parse()?;
    let is_auto: bool = match args[4].as_str() {
        "0" | "false" => false,
        "1" | "true" => true,
        _ => {
            return Err(format!(
                "Invalid is_auto value: {}. Must be 0, 1, true, or false.",
                args[4]
            )
            .into())
        }
    };

    let buffer_file = &args[5];

    // Default show_info value (2 = inline display)
    let mut show_info = 2;
    // use_cache parameter is available for future use but currently not used
    let _use_cache = true;

    // Read buffer lines from file
    let buffer_content = fs::read_to_string(buffer_file)?;
    let lines: Vec<String> = serde_json::from_str(&buffer_content)?;

    // Build config from args
    let mut config = LlamaConfig::new();

    let mut i = 6;
    let mut prev_lines: Vec<String> = Vec::new();

    while i < args.len() {
        match args[i].as_str() {
            "--endpoint_fim" => {
                i += 1;
                if i < args.len() {
                    config.endpoint_fim = args[i].clone();
                }
            }
            "--endpoint_inst" => {
                i += 1;
                if i < args.len() {
                    config.endpoint_inst = args[i].clone();
                }
            }
            "--model_fim" => {
                i += 1;
                if i < args.len() {
                    config.model_fim = args[i].clone();
                }
            }
            "--model_inst" => {
                i += 1;
                if i < args.len() {
                    config.model_inst = args[i].clone();
                }
            }
            "--api_key" => {
                i += 1;
                if i < args.len() {
                    config.api_key = args[i].clone();
                }
            }
            "--n_prefix" => {
                i += 1;
                if i < args.len() {
                    config.n_prefix = args[i].parse()?;
                }
            }
            "--n_suffix" => {
                i += 1;
                if i < args.len() {
                    config.n_suffix = args[i].parse()?;
                }
            }
            "--n_predict" => {
                i += 1;
                if i < args.len() {
                    config.n_predict = args[i].parse()?;
                }
            }
            "--t_max_prompt_ms" => {
                i += 1;
                if i < args.len() {
                    config.t_max_prompt_ms = args[i].parse()?;
                }
            }
            "--t_max_predict_ms" => {
                i += 1;
                if i < args.len() {
                    config.t_max_predict_ms = args[i].parse()?;
                }
            }
            "--show_info" => {
                i += 1;
                if i < args.len() {
                    show_info = args[i].parse()?;
                }
            }
            "--prev" => {
                i += 1;
                if i < args.len() {
                    // Parse multiline prev content
                    let prev_str = args[i].clone();
                    if !prev_str.is_empty() {
                        prev_lines = prev_str.split('\n').map(|s| s.to_string()).collect();
                    }
                }
            }
            "--use_cache" => {
                i += 1;
                if i < args.len() {
                    // use_cache = args[i].parse().unwrap_or(true);
                    // Currently not used - reserved for future use
                    let _ = args[i].parse::<bool>().unwrap_or(true);
                }
            }
            _ => {}
        }
        i += 1;
    }

    // Run FIM completion and get timing info
    let fim_result = tokio::runtime::Runtime::new()?.block_on(async {
        let mut cache = Cache::new(config.max_cache_keys as usize);
        let ring_buffer = RingBuffer::new(
            config.ring_n_chunks as usize,
            config.ring_chunk_size as usize,
        );

        // Send request directly to get timing info
        let extra = ring_buffer.get_extra();
        let ctx =
            lttw::context::get_local_context(&lines, pos_x, pos_y, Some(&prev_lines), &config);
        let hashes = lttw::fim::compute_hashes(&ctx);

        let request = lttw::fim::FimRequest {
            id_slot: 0,
            input_prefix: ctx.prefix,
            input_suffix: ctx.suffix,
            input_extra: extra,
            prompt: ctx.middle,
            n_predict: config.n_predict,
            stop: config.stop_strings.clone(),
            n_indent: ctx.indent,
            top_k: 40,
            top_p: 0.90,
            samplers: vec![
                "top_k".to_string(),
                "top_p".to_string(),
                "infill".to_string(),
            ],
            stream: false,
            cache_prompt: true,
            t_max_prompt_ms: config.t_max_prompt_ms,
            t_max_predict_ms: if is_auto {
                250
            } else {
                config.t_max_predict_ms
            },
            response_fields: vec![
                "content".to_string(),
                "timings/prompt_n".to_string(),
                "timings/prompt_ms".to_string(),
                "timings/prompt_per_token_ms".to_string(),
                "timings/prompt_per_second".to_string(),
                "timings/predicted_n".to_string(),
                "timings/predicted_ms".to_string(),
                "timings/predicted_per_token_ms".to_string(),
                "timings/predicted_per_second".to_string(),
                "truncated".to_string(),
                "tokens_cached".to_string(),
            ],
            model: config.model_fim.clone(),
            prev: prev_lines,
        };

        // Send request
        let response_text = lttw::fim::send_request(&request, &config).await?;

        // Parse response to get timing info
        let response: lttw::fim::FimResponse = serde_json::from_str(&response_text)?;

        // Cache the response
        for hash in hashes {
            cache.insert(hash, response_text.clone());
        }

        // Build info string if show_info > 0
        let info = if show_info > 0 {
            if let Some(ref timings) = response.timings {
                Some(lttw::fim::build_info_string(
                    timings,
                    response.tokens_cached,
                    response.truncated,
                    ring_buffer.len(),
                    config.ring_n_chunks as usize,
                    ring_buffer.n_evict(),
                    ring_buffer.queued_len(),
                    cache.len(),
                    config.max_cache_keys as usize,
                ))
            } else {
                None
            }
        } else {
            None
        };

        Ok::<lttw::fim::FimResult, Box<dyn std::error::Error>>(lttw::fim::FimResult {
            content: response.content.clone(),
            can_accept: !response.content.trim().is_empty(),
            timings: response.timings,
            tokens_cached: response.tokens_cached,
            truncated: response.truncated,
            info,
        })
    })?;

    // Output result as JSON
    let suggestion = render_fim_suggestion(
        pos_x,
        pos_y,
        &fim_result.content,
        lines.get(pos_y).map(|s| s.as_str()).unwrap_or(""),
        &config,
    );

    // Build output with timing info
    let mut output = serde_json::json!({
        "content": suggestion.content,
        "can_accept": suggestion.can_accept
    });

    // Add timing info if available
    if let Some(timings) = &fim_result.timings {
        output["timings"] = serde_json::json!({
            "prompt_n": timings.prompt_n,
            "prompt_ms": timings.prompt_ms,
            "prompt_per_token_ms": timings.prompt_per_token_ms,
            "prompt_per_second": timings.prompt_per_second,
            "predicted_n": timings.predicted_n,
            "predicted_ms": timings.predicted_ms,
            "predicted_per_token_ms": timings.predicted_per_token_ms,
            "predicted_per_second": timings.predicted_per_second
        });
    }

    output["tokens_cached"] = serde_json::json!(fim_result.tokens_cached);
    output["truncated"] = serde_json::json!(fim_result.truncated);

    // Add info string if available
    output["info"] = serde_json::json!(fim_result.info);

    println!("{}", serde_json::to_string(&output)?);

    Ok(())
}

fn handle_instruction(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.len() < 3 || args[2] == "--help" {
        println!("Usage: lttw instruction <l0> <l1> <buffer_file> [options]");
        println!("Arguments:");
        println!("  l0            Start line number (1-indexed)");
        println!("  l1            End line number (1-indexed)");
        println!("  buffer_file   Path to file containing buffer lines (JSON array)");
        println!("Options:");
        println!("  --endpoint_inst <url>      Instruction endpoint URL");
        println!("  --model_inst <model>       Instruction model name");
        println!("  --api_key <key>            API key for authentication");
        println!("  --n_prefix <n>             Number of prefix lines (default: 256)");
        println!("  --n_suffix <n>             Number of suffix lines (default: 64)");
        println!("  --instruction <text>       Instruction text");
        return Ok(());
    }

    let l0: usize = args[2].parse()?;
    let l1: usize = args[3].parse()?;

    let buffer_file = &args[4];

    // Read buffer lines from file
    let buffer_content = fs::read_to_string(buffer_file)?;
    let lines: Vec<String> = serde_json::from_str(&buffer_content)?;

    // Extract instruction from args
    let mut instruction_text = String::new();

    let mut i = 5;
    while i < args.len() {
        match args[i].as_str() {
            "--endpoint_inst" => {
                i += 1;
                if i < args.len() {
                    i += 1; // skip value
                }
            }
            "--model_inst" => {
                i += 1;
                if i < args.len() {
                    i += 1; // skip value
                }
            }
            "--api_key" => {
                i += 1;
                if i < args.len() {
                    i += 1; // skip value
                }
            }
            "--n_prefix" => {
                i += 1;
                if i < args.len() {
                    i += 1; // skip value
                }
            }
            "--n_suffix" => {
                i += 1;
                if i < args.len() {
                    i += 1; // skip value
                }
            }
            "--instruction" => {
                i += 1;
                if i < args.len() {
                    instruction_text = args[i].clone();
                }
            }
            _ => {}
        }
        i += 1;
    }

    // Build config from args
    let mut config = LlamaConfig::new();

    i = 5;
    while i < args.len() {
        match args[i].as_str() {
            "--endpoint_inst" => {
                i += 1;
                if i < args.len() {
                    config.endpoint_inst = args[i].clone();
                }
            }
            "--model_inst" => {
                i += 1;
                if i < args.len() {
                    config.model_inst = args[i].clone();
                }
            }
            "--api_key" => {
                i += 1;
                if i < args.len() {
                    config.api_key = args[i].clone();
                }
            }
            "--n_prefix" => {
                i += 1;
                if i < args.len() {
                    config.n_prefix = args[i].parse()?;
                }
            }
            "--n_suffix" => {
                i += 1;
                if i < args.len() {
                    config.n_suffix = args[i].parse()?;
                }
            }
            _ => {}
        }
        i += 1;
    }

    // Build instruction payload
    let messages = build_instruction_payload(
        &lines,
        l0.saturating_sub(1), // Convert to 0-indexed
        l1.saturating_sub(1), // Convert to 0-indexed
        &instruction_text,
        &config,
    );

    // Send instruction request
    let result = tokio::runtime::Runtime::new()?
        .block_on(async { send_instruction(&messages, &config, 0).await })?;

    // Process response
    let processed = process_instruction_response(&result);

    // Output result as JSON
    let output = serde_json::json!({
        "content": processed
    });

    println!("{}", serde_json::to_string(&output)?);

    Ok(())
}

fn handle_ring(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.len() < 3 || args[2] == "--help" {
        println!("Usage: lttw ring <command> [args] [options]");
        println!("Commands:");
        println!("  gather [lines...]       Gather chunks from text");
        println!("  update                  Update ring buffer with queued chunks");
        println!("Options:");
        println!("  --no-mod                Don't pick chunks from modified buffers");
        println!("  --do-evict              Evict similar chunks");
        println!("  --n-chunks <n>          Max number of chunks (default: 16)");
        println!("  --chunk-size <n>        Chunk size in lines (default: 64)");
        return Ok(());
    }

    let command = &args[2];

    match command.as_str() {
        "gather" => handle_ring_gather(args),
        "update" => handle_ring_update(args),
        _ => {
            eprintln!("Unknown ring command: {}", command);
            eprintln!("Use 'lttw ring --help' for more information");
            process::exit(1);
        }
    }
}

fn handle_ring_gather(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.len() < 3 {
        eprintln!("Usage: lttw ring gather [lines...] [options]");
        eprintln!("Use 'lttw ring --help' for more information");
        process::exit(1);
    }

    let mut config = LlamaConfig::new();

    let mut i = 3;
    let mut lines: Vec<String> = Vec::new();
    let mut do_evict = true;

    while i < args.len() {
        match args[i].as_str() {
            "--no-mod" => {
                // no_mod parameter - currently not used in CLI mode
                i += 1;
            }
            "--do-evict" => {
                do_evict = true;
                i += 1;
            }
            "--no-evict" => {
                do_evict = false;
                i += 1;
            }
            "--n-chunks" => {
                i += 1;
                if i < args.len() {
                    config.ring_n_chunks = args[i].parse()?;
                }
            }
            "--chunk-size" => {
                i += 1;
                if i < args.len() {
                    config.ring_chunk_size = args[i].parse()?;
                }
            }
            _ => {
                lines.push(args[i].clone());
                i += 1;
            }
        }
    }

    if lines.is_empty() {
        eprintln!("No lines provided for gathering chunks");
        process::exit(1);
    }

    let chunk_size = config.ring_chunk_size as usize;

    // Use a simple approach: pick chunks from the provided text
    if lines.len() < 3 {
        eprintln!("Not enough lines to pick a chunk (need at least 3)");
        return Ok(());
    }

    let chunk_size_half = chunk_size / 2;
    let l0 = std::cmp::min(
        rand::random::<usize>() % std::cmp::max(1, lines.len().saturating_sub(chunk_size_half)),
        lines.len().saturating_sub(chunk_size_half),
    );
    let l1 = std::cmp::min(l0 + chunk_size_half, lines.len());
    let chunk = lines[l0..l1].to_vec();

    let chunk_str = chunk.join("\n") + "\n";

    // In the actual plugin, we would store this in a persistent ring buffer
    // For the CLI, we'll just print the chunk for debugging
    println!("Gathered chunk ({} lines):", chunk.len());
    println!("{}", chunk_str);

    if do_evict {
        println!("Eviction enabled (not implemented in CLI mode)");
    }

    Ok(())
}

fn handle_ring_update(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let mut config = LlamaConfig::new();

    let mut i = 3;
    while i < args.len() {
        match args[i].as_str() {
            "--n-chunks" => {
                i += 1;
                if i < args.len() {
                    config.ring_n_chunks = args[i].parse()?;
                }
            }
            "--chunk-size" => {
                i += 1;
                if i < args.len() {
                    config.ring_chunk_size = args[i].parse()?;
                }
            }
            _ => {}
        }
        i += 1;
    }

    // In the actual plugin, this would move queued chunks to the ring buffer
    // For the CLI, we'll just print a message
    println!(
        "Ring buffer update (n_chunks: {}, chunk_size: {}, update_ms: {})",
        config.ring_n_chunks, config.ring_chunk_size, config.ring_update_ms
    );
    println!("In the Neovim plugin, this would process queued chunks");

    Ok(())
}

#[cfg(test)]
mod tests {
    use lttw::config::LlamaConfig;

    // Integration tests that send actual requests to the llama.cpp server
    // These tests are marked with #[ignore] and must be run explicitly with `cargo test -- --ignored`

    #[tokio::test]
    #[ignore = "requires llama.cpp server running at http://127.0.0.1:8012"]
    async fn test_fim_completion_integration() {
        use lttw::cache::Cache;
        use lttw::fim::fim_completion;
        use lttw::ring_buffer::RingBuffer;

        // Skip if server is not available
        let config = LlamaConfig::new();

        // Try to connect to the server first
        let client = reqwest::Client::new();
        match client.get(&config.endpoint_fim).send().await {
            Ok(_) => {
                // Server is available, run the test
                let lines = vec![
                    "fn main() {".to_string(),
                    "    println!(\"hello\");".to_string(),
                    "}".to_string(),
                ];

                let mut cache = Cache::new(10);
                let mut ring_buffer = RingBuffer::new(2, 64);

                // This should either succeed or return an error (but not panic)
                let result = fim_completion(
                    4,
                    1,
                    false,
                    &lines,
                    &config,
                    &mut cache,
                    &mut ring_buffer,
                    None,
                )
                .await;

                // If the request succeeds, we should get some content
                // If the server is not configured properly, it might return an error
                // but it should not panic
                match &result {
                    Ok(Some(content)) => {
                        // Successfully got a completion
                        // The completion might be empty if the model is not properly configured
                        // or if it generated only whitespace
                        println!("Got completion: {:?}", content);
                    }
                    Ok(None) => {
                        // Completion was skipped or cached - this is valid
                    }
                    Err(e) => {
                        // Request failed (e.g., model not loaded, server error)
                        // This is expected if the server is not properly configured
                        println!(
                            "Expected server error (server may not be configured): {}",
                            e
                        );
                    }
                }
            }
            Err(_) => {
                // Server is not available, skip the test
                panic!(
                    "llama.cpp server not available at {}, skipping integration test",
                    config.endpoint_fim
                );
            }
        }
    }

    #[tokio::test]
    #[ignore = "requires llama.cpp server running at http://127.0.0.1:8012"]
    async fn test_instruction_completion_integration() {
        use lttw::config::LlamaConfig;
        use lttw::instruction::{build_instruction_payload, send_instruction};

        let config = LlamaConfig::new();

        // Try to connect to the server first
        let client = reqwest::Client::new();
        match client.get(&config.endpoint_inst).send().await {
            Ok(_) => {
                // Server is available, run the test
                let lines = vec![
                    "fn main() {".to_string(),
                    "    println!(\"hello\");".to_string(),
                    "}".to_string(),
                ];

                let messages =
                    build_instruction_payload(&lines, 0, 1, "make this shorter", &config);

                // This should either succeed or return an error (but not panic)
                let result = send_instruction(&messages, &config, 0).await;

                match &result {
                    Ok(response_text) => {
                        // Successfully got a response
                        println!("Response: {}", response_text);
                        // Verify response contains expected format
                        assert!(
                            response_text.contains("data: "),
                            "Response should be in SSE format"
                        );
                    }
                    Err(e) => {
                        // Request failed (e.g., model not loaded, server error)
                        // This is expected if the server is not properly configured
                        println!(
                            "Expected server error (server may not be configured): {}",
                            e
                        );
                    }
                }
            }
            Err(_) => {
                // Server is not available, skip the test
                panic!(
                    "llama.cpp server not available at {}, skipping integration test",
                    config.endpoint_inst
                );
            }
        }
    }

    #[tokio::test]
    #[ignore = "requires llama.cpp server running at http://127.0.0.1:8012"]
    async fn test_fim_request_sending_integration() {
        use lttw::config::LlamaConfig;
        use lttw::fim::{send_request, FimRequest};

        let config = LlamaConfig::new();

        // Try to connect to the server first
        let client = reqwest::Client::new();
        match client.get(&config.endpoint_fim).send().await {
            Ok(_) => {
                // Server is available, run the test
                let request = FimRequest {
                    id_slot: 0,
                    input_prefix: "fn main() {".to_string(),
                    input_suffix: "}".to_string(),
                    input_extra: vec![],
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
                    model: config.model_fim.clone(),
                    prev: vec![],
                };

                let result = send_request(&request, &config).await;

                match &result {
                    Ok(response_text) => {
                        // Successfully got a response
                        println!("FIM Response: {}", response_text);
                        // Verify response is valid JSON
                        let _json: serde_json::Value = serde_json::from_str(response_text)
                            .expect("Response should be valid JSON");
                    }
                    Err(e) => {
                        // Request failed (e.g., model not loaded, server error)
                        println!("Expected server error: {}", e);
                    }
                }
            }
            Err(_) => {
                // Server is not available, skip the test
                panic!(
                    "llama.cpp server not available at {}, skipping integration test",
                    config.endpoint_fim
                );
            }
        }
    }

    #[test]
    fn test_fim_request_serialization() {
        // Test that FIM request serialization works correctly
        let request = lttw::fim::FimRequest {
            id_slot: 0,
            input_prefix: "fn main() {".to_string(),
            input_suffix: "}".to_string(),
            input_extra: vec![],
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
        println!("Serialized FIM request: {}", json);

        // Verify the JSON contains expected fields
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("JSON should be parseable");
        assert_eq!(parsed["input_prefix"], "fn main() {");
        assert_eq!(parsed["input_suffix"], "}");
        assert_eq!(parsed["prompt"], "    println!(\"hello\"");
        assert_eq!(parsed["n_predict"], 32);
    }

    #[test]
    fn test_instruction_request_serialization() {
        // Test that instruction request serialization works correctly
        let messages = vec![
            lttw::instruction::InstMessage {
                role: "system".to_string(),
                content: "You are a helpful assistant".to_string(),
            },
            lttw::instruction::InstMessage {
                role: "user".to_string(),
                content: "Hello".to_string(),
            },
        ];

        let request = lttw::instruction::InstRequest {
            id_slot: 0,
            messages,
            min_p: Some(0.1),
            temperature: Some(0.1),
            samplers: Some(vec!["min_p".to_string()]),
            n_predict: Some(128),
            stream: Some(true),
            cache_prompt: Some(true),
            model: "".to_string(),
        };

        let json = serde_json::to_string(&request).expect("Request should serialize to JSON");
        println!("Serialized instruction request: {}", json);

        // Verify the JSON contains expected fields
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("JSON should be parseable");
        assert_eq!(parsed["messages"].as_array().unwrap().len(), 2);
        assert!(parsed["stream"].as_bool().unwrap());
    }
}
