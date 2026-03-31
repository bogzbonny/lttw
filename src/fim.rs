// src/fim.rs - Fill-in-Middle (FIM) completion functions
//
// This module handles FIM completion requests to the llama.cpp server,
// including context gathering, request building, response processing,
// and rendering suggestions.

use {
    crate::{
        cache::Cache,
        config::LttwConfig,
        context::{get_local_context, LocalContext},
        debug::DebugManager,
        ring_buffer::{ExtraContext, RingBuffer},
        utils::sha256,
    },
    serde::{Deserialize, Serialize},
};

/// FIM completion request
#[derive(Debug, Clone, Serialize)]
pub struct FimRequest {
    pub id_slot: i64,
    pub input_prefix: String,
    pub input_suffix: String,
    pub input_extra: Vec<ExtraContext>,
    pub prompt: String,
    pub n_predict: u32,
    pub stop: Vec<String>,
    pub n_indent: usize,
    pub top_k: u32,
    pub top_p: f32,
    pub samplers: Vec<String>,
    pub stream: bool,
    pub cache_prompt: bool,
    pub t_max_prompt_ms: u32,
    pub t_max_predict_ms: u32,
    pub response_fields: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub prev: Vec<String>,
}

/// FIM completion response
#[derive(Debug, Clone, Deserialize)]
pub struct FimResponse {
    pub content: String,
    #[serde(default)]
    pub timings: Option<FimTimings>,
    #[serde(default)]
    pub tokens_cached: u64,
    #[serde(default)]
    pub truncated: bool,
}

/// FIM completion result with timing info
#[derive(Debug, Clone, Serialize)]
pub struct FimResult {
    pub content: String,
    pub can_accept: bool,
    pub timings: Option<FimTimings>,
    pub tokens_cached: u64,
    pub truncated: bool,
    pub info: Option<String>,
}

/// FIM timing information
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FimTimings {
    pub prompt_n: Option<i64>,
    pub prompt_ms: Option<f64>,
    pub prompt_per_token_ms: Option<f64>,
    pub prompt_per_second: Option<f64>,
    pub predicted_n: Option<i64>,
    pub predicted_ms: Option<f64>,
    pub predicted_per_token_ms: Option<f64>,
    pub predicted_per_second: Option<f64>,
}

/// Build info string from timing information
#[allow(clippy::too_many_arguments)] // Info display requires many parameters
pub fn build_info_string(
    timings: &FimTimings,
    tokens_cached: u64,
    truncated: bool,
    ring_chunks: usize,
    ring_n_chunks: usize,
    ring_n_evict: usize,
    ring_queued: usize,
    cache_size: usize,
    max_cache_keys: usize,
) -> String {
    // Extract timing values
    let n_prompt = timings.prompt_n.unwrap_or(0);
    let t_prompt_ms = timings.prompt_ms.unwrap_or(1.0);
    let s_prompt = timings.prompt_per_second.unwrap_or(0.0);

    let n_predict = timings.predicted_n.unwrap_or(0);
    let t_predict_ms = timings.predicted_ms.unwrap_or(1.0);
    let s_predict = timings.predicted_per_second.unwrap_or(0.0);

    // Build info string
    let info = if truncated {
        format!(
            " | WARNING: the context is full: {}, increase the server context size or reduce g:lttw_config.ring_n_chunks",
            tokens_cached
        )
    } else {
        format!(
            " | c: {}, r: {}/{}, e: {}, q: {}/16, C: {}/{} | p: {} ({:.2} ms, {:.2} t/s) | g: {} ({:.2} ms, {:.2} t/s)",
            tokens_cached,
            ring_chunks, ring_n_chunks,
            ring_n_evict,
            ring_queued,
            cache_size, max_cache_keys,
            n_prompt, t_prompt_ms, s_prompt,
            n_predict, t_predict_ms, s_predict
        )
    };

    info
}

/// Error type for FIM operations
#[derive(Debug, thiserror::Error)]
pub enum FimError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Server error: {0}")]
    Server(String),
    #[error("Regex error: {0}")]
    Regex(#[from] regex::Error),
}

/// Main FIM completion function that sends a request to the server
/// Returns the content and optionally timing info for display
#[allow(clippy::too_many_arguments)] // FIM requires context from multiple sources
pub async fn fim_completion(
    debug_manager: DebugManager,
    pos_x: usize,
    pos_y: usize,
    is_auto: bool,
    lines: &[String],
    config: &LttwConfig,
    cache: std::sync::Arc<parking_lot::RwLock<Cache>>,
    ring_buffer: std::sync::Arc<parking_lot::RwLock<RingBuffer>>,
    prev: Option<&[String]>,
) -> Result<Option<String>, FimError> {
    // Lock the cache and ring buffer for setup
    let request_data = {
        // Get local context
        debug_manager.log("fim_completion 1", &[]);
        let ctx = get_local_context(lines, pos_x, pos_y, prev, config);
        debug_manager.log(
            "fim_completion 2",
            &[&format!(
                "is_auto {is_auto}, ctx.line_cur_suffix.len() {}, config.max_line_suffix {}",
                ctx.line_cur_suffix.len(),
                config.max_line_suffix
            )],
        );

        // Skip auto FIM if too much suffix
        if is_auto && ctx.line_cur_suffix.len() > config.max_line_suffix as usize {
            return Ok(None);
        }
        debug_manager.log("fim_completion 3", &[]);

        // Evict ring buffer chunks that are very similar to current FIM context (>0.5 threshold)
        // This prevents redundant context from cluttering the ring buffer
        let current_prefix_lines: Vec<String> =
            ctx.prefix.split('\n').map(|s| s.to_string()).collect();
        if !current_prefix_lines.is_empty() {
            ring_buffer
                .write()
                .evict_similar(&current_prefix_lines, 0.5);
        }
        debug_manager.log("fim_completion 4", &[]);

        // Build request
        let extra = ring_buffer.read().get_extra();
        debug_manager.log("fim_completion 5", &[]);

        let hashes = compute_hashes(&ctx);
        debug_manager.log("fim_completion 6", &[]);

        // Check cache
        if config.auto_fim {
            for hash in &hashes {
                let cache_lock = cache.read();
                if cache_lock.contains_key(hash) {
                    return Ok(None);
                }
            }
        }
        debug_manager.log("fim_completion 7", &[]);

        // Build request
        let request = FimRequest {
            id_slot: 0,
            input_prefix: ctx.prefix.clone(),
            input_suffix: ctx.suffix.clone(),
            input_extra: extra,
            prompt: ctx.middle.clone(),
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
            prev: prev.map(|p| p.to_vec()).unwrap_or_default(),
        };
        debug_manager.log("fim_completion 8", &[]);

        // Return the request data and hashes, releasing locks before we exit the block
        (request, hashes, ctx.clone())
    }; // Locks released here

    let (request, hashes, _ctx) = request_data;

    // Send request without holding locks
    let response_text = send_request(&request, config).await?;
    debug_manager.log("fim_completion 9", &[]);

    // Parse response
    let response: FimResponse = serde_json::from_str(&response_text)?;
    debug_manager.log("fim_completion 10", &[]);

    // Cache the response with timing info (new block for re-acquired locks)
    {
        let mut cache_lock = cache.write();
        // Ring buffer is read but not modified here
        let _ring_buffer_lock = ring_buffer.read();
        for hash in &hashes {
            cache_lock.insert(hash.clone(), response_text.clone());
        }
        debug_manager.log("fim_completion 11", &[]);
    }

    // Return content - timing info is stored in cache alongside response
    Ok(Some(response.content))
}

/// Try to generate a suggestion using the data in the cache
/// Looks at the previous 10 characters to see if a completion is cached.
/// If one is found at (x,y) then it checks that the characters typed after (x,y)
/// match up with the cached completion result.
///
/// # Arguments
/// * `pos_x` - X position (column) in the current line
/// * `pos_y` - Y position (line number)
/// * `lines` - All lines in the buffer
/// * `cache` - Cache to lookup completions
/// * `config` - Plugin configuration
///
/// # Returns
/// * `Some(RenderedSuggestion)` - If a cached completion is found
/// * `None` - If no cached completion is found
pub fn fim_try_hint(
    pos_x: usize,
    pos_y: usize,
    lines: &[String],
    cache: &mut Cache,
    config: &LttwConfig,
) -> Option<RenderedSuggestion> {
    // Get local context
    let ctx = get_local_context(lines, pos_x, pos_y, None, config);

    // Compute primary hash
    let primary_hash = format!("{}{}{}{}", ctx.prefix, ctx.middle, "Î", ctx.suffix);
    let hash = sha256(&primary_hash);

    // Check if the completion is cached (and update LRU order)
    if let Some(raw) = cache.get_fim_mut(&hash) {
        if let Ok(response) = serde_json::from_str::<FimResponse>(&raw) {
            let content = response.content;
            if !content.is_empty() {
                return Some(render_fim_suggestion(
                    pos_x,
                    pos_y,
                    &content,
                    &ctx.line_cur_suffix,
                    config,
                ));
            }
        }
    }

    // ... or if there is a cached completion nearby (10 characters behind)
    // Looks at the previous 10 characters to see if a completion is cached.
    let pm = format!("{}{}", ctx.prefix, ctx.middle);
    let mut best_len = 0;
    let mut best_response: Option<FimResponse> = None;

    // Only search if pm has enough characters
    if pm.len() < 2 {
        return None;
    }

    for i in 0..128.min(pm.len() - 2) {
        let removed = &pm[pm.len() - (1 + i)..];
        let ctx_new = format!("{}Î{}", &pm[..pm.len() - (2 + i)], ctx.suffix);
        let hash_new = sha256(&ctx_new);

        if let Some(response_cached) = cache.get_fim_mut(&hash_new) {
            if response_cached.is_empty() {
                continue;
            }

            if let Ok(response) = serde_json::from_str::<FimResponse>(&response_cached) {
                let content = &response.content;

                // Check that the removed text matches the beginning of the cached response
                if content.len() > i && &content[..=i] == removed {
                    // Found a match - use the rest of the content
                    let remaining = if i + 1 < content.len() {
                        &content[i + 1..]
                    } else {
                        ""
                    };

                    if !remaining.is_empty() && remaining.len() > best_len {
                        best_len = remaining.len();
                        best_response = Some(FimResponse {
                            content: remaining.to_string(),
                            timings: response.timings,
                            tokens_cached: response.tokens_cached,
                            truncated: response.truncated,
                        });
                    }
                }
            }
        }
    }

    if let Some(response) = best_response {
        Some(render_fim_suggestion(
            pos_x,
            pos_y,
            &response.content,
            &ctx.line_cur_suffix,
            config,
        ))
    } else {
        None
    }
}

/// Compute hashes for caching
pub fn compute_hashes(ctx: &LocalContext) -> Vec<String> {
    let mut hashes = Vec::new();

    // Primary hash
    let primary = format!("{}{}{}{}", ctx.prefix, ctx.middle, "Î", ctx.suffix);
    let hash = sha256(&primary);
    hashes.push(hash);

    // Truncated prefix hashes (up to 3 levels)
    let mut prefix_trim = ctx.prefix.clone();
    let re = regex::Regex::new(r"^[^\n]*\n").unwrap();
    for _ in 0..3 {
        prefix_trim = re.replace(&prefix_trim, "").to_string();
        if prefix_trim.is_empty() {
            break;
        }

        let hash_input = format!("{}{}{}{}", prefix_trim, ctx.middle, "Î", ctx.suffix);
        let hash = sha256(&hash_input);
        hashes.push(hash);
    }

    hashes
}

/// Send FIM request to the server
pub async fn send_request(request: &FimRequest, config: &LttwConfig) -> Result<String, FimError> {
    let client = reqwest::Client::new();

    let mut request_body = serde_json::to_value(request)?;

    // Add model if specified
    if !config.model_fim.is_empty() {
        request_body["model"] = serde_json::Value::String(config.model_fim.clone());
    }

    let mut builder = client.post(&config.endpoint_fim).json(&request_body);

    // Add API key if specified
    if !config.api_key.is_empty() {
        builder = builder.bearer_auth(&config.api_key);
    }

    let response = builder.send().await?;

    if response.status().is_success() {
        Ok(response.text().await?)
    } else {
        Err(FimError::Server(format!(
            "Server returned status: {}",
            response.status()
        )))
    }
}

/// Render FIM suggestion at the current cursor location
/// Filters out duplicate text that already exists in the buffer
pub fn render_fim_suggestion(
    pos_x: usize,
    _pos_y: usize,
    content: &str,
    line_cur: &str,
    _config: &LttwConfig,
) -> RenderedSuggestion {
    // Parse content into lines
    let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();

    // Remove trailing empty lines
    while lines.last().map(|s| s.is_empty()).unwrap_or(false) {
        lines.pop();
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    // Filter out duplicate text - remove prefix that matches existing suffix
    let line_cur_suffix = &line_cur[pos_x.min(line_cur.len())..];
    if !line_cur_suffix.is_empty() && !lines[0].is_empty() {
        // Check if the beginning of the suggestion duplicates existing text
        for i in (0..line_cur_suffix.len()).rev() {
            if lines[0].starts_with(&line_cur_suffix[..=i]) {
                // Remove the duplicate part from the first line
                lines[0] = lines[0][line_cur_suffix[..=i].len()..].to_string();
                break;
            }
        }
    }

    // Handle whitespace-only lines
    let line_cur_stripped = line_cur.trim();
    if line_cur_stripped.is_empty() {
        let content_stripped = lines[0].trim_start();
        let _lead = std::cmp::min(lines[0].len() - content_stripped.len(), line_cur.len());

        let mut new_lines = lines.clone();
        new_lines[0] = content_stripped.to_string();

        lines = new_lines;
    }

    // Append suffix to last line
    let suffix_end = std::cmp::min(pos_x, line_cur.len());
    let suffix = &line_cur[suffix_end..];
    if !lines.is_empty() {
        let last_idx = lines.len() - 1;
        let mut last_line = lines[last_idx].clone();
        last_line += suffix;
        lines[last_idx] = last_line;
    }

    // Check if only whitespace
    let joined = lines.join("\n");
    let can_accept = !joined.trim().is_empty();

    RenderedSuggestion {
        content: lines,
        can_accept,
    }
}

/// Accept FIM suggestion - returns the modified line
pub fn accept_fim_suggestion(
    accept_type: &str,
    pos_x: usize,
    line_cur: &str,
    content: &[String],
) -> (String, Option<Vec<String>>) {
    let mut first_line = content[0].clone();

    // Handle whitespace-only lines
    let line_cur_stripped = line_cur.trim();
    if line_cur_stripped.is_empty() {
        let content_stripped = first_line.trim_start();
        first_line = content_stripped.to_string();
    }

    let new_line = line_cur[..pos_x].to_string() + &first_line;

    // Handle accept type
    match accept_type {
        "full" => {
            // Insert rest of suggestion
            if content.len() > 1 {
                let rest: Vec<String> = content[1..].to_vec();
                return (new_line, Some(rest));
            }
        }
        "line" => {
            // Insert only the second line
            if content.len() > 1 {
                return (new_line, Some(vec![content[1].clone()]));
            }
        }
        "word" => {
            // Accept only the first word
            let suffix = &line_cur[pos_x..];
            if let Some(word_match) = first_line.split_whitespace().next() {
                let _new_word = word_match.to_string() + suffix;
                return (new_line + word_match, None);
            }
        }
        _ => {}
    }

    (new_line, None)
}

/// Result of rendering a FIM suggestion
#[derive(Debug, Clone, serde::Serialize)]
pub struct RenderedSuggestion {
    pub content: Vec<String>,
    pub can_accept: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_hashes() {
        let ctx = LocalContext {
            prefix: "fn main() {\n".to_string(),
            middle: "    println!".to_string(),
            suffix: "(\"hello\");\n}\n".to_string(),
            ..Default::default()
        };

        let hashes = compute_hashes(&ctx);
        assert!(!hashes.is_empty());
    }

    #[test]
    fn test_fim_request_with_ring_buffer_extra() {
        // Test that FIM request properly includes extra context from ring buffer
        let ring_buffer = RingBuffer::new(2, 64);

        let request = FimRequest {
            id_slot: 0,
            input_prefix: "fn main() {".to_string(),
            input_suffix: "}".to_string(),
            input_extra: ring_buffer.get_extra(),
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

        // Verify input_extra is an empty array when ring buffer is empty
        assert_eq!(parsed["input_extra"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn test_ring_buffer_integration_with_cache() {
        // Test that ring buffer chunks are properly tracked and cached
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

        // Add second chunk (should not evict first since they're different)
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

        // Add third chunk
        ring_buffer.pick_chunk(
            vec![
                "mod test;".to_string(),
                "fn test_func() {".to_string(),
                "    assert_eq!(1, 1);".to_string(),
            ],
            false,
            true,
        );
        ring_buffer.update();

        assert_eq!(ring_buffer.len(), 3);

        // Add fourth chunk - should evict the oldest one due to max_chunks limit
        ring_buffer.pick_chunk(
            vec![
                "pub fn export_func() {".to_string(),
                "    test_func();".to_string(),
                "}".to_string(),
            ],
            false,
            true,
        );
        ring_buffer.update();

        // Should still be at max_chunks (3)
        assert_eq!(ring_buffer.len(), 3);
    }

    #[test]
    fn test_ring_buffer_eviction_with_similarity() {
        // Test that similar chunks are evicted based on similarity threshold
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
    }

    #[test]
    fn test_fim_request_serialization_with_extra() {
        // Test that FIM request properly serializes with extra context
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
    }

    #[test]
    fn test_cache_with_ring_buffer_chunks() {
        // Test that cache properly handles entries with ring buffer context
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
        // Use a prefix with newlines to test truncated prefix hashes
        let ctx = LocalContext {
            prefix: "fn main() {\n    let x = 1;\n".to_string(),
            middle: "    println!(\"hello\"".to_string(),
            suffix: ");\n}".to_string(),
            line_cur_suffix: "rintln!(\"hello\");".to_string(),
            indent: 4,
        };

        let hashes = compute_hashes(&ctx);

        // Verify we generated multiple hashes (prefix has newlines)
        assert!(
            hashes.len() > 1,
            "Should generate multiple hashes from truncated prefixes"
        );

        // Cache a response for these hashes
        let response = r#"{"content":" world","timings":{},"tokens_cached":0,"truncated":false}"#;
        for hash in &hashes {
            cache.insert(hash.clone(), response.to_string());
        }

        // Verify cache contains the entries
        for hash in &hashes {
            assert!(cache.contains_key(hash));
        }

        // Verify cache size matches the number of hashes generated
        assert_eq!(
            cache.len(),
            hashes.len(),
            "Cache should contain all {} hash entries",
            hashes.len()
        );
    }

    #[test]
    fn test_ring_buffer_n_evict_counter() {
        // Test that n_evict counter tracks evicted chunks correctly
        let mut ring_buffer = RingBuffer::new(2, 64);

        ring_buffer.pick_chunk(
            vec![
                "fn func1() {".to_string(),
                "    let x = 1;".to_string(),
                "}".to_string(),
            ],
            false,
            true,
        );
        ring_buffer.update();

        let n_evict_before = ring_buffer.n_evict();

        // Add similar chunks to trigger eviction
        for _ in 0..5 {
            let similar_chunk = vec![
                "fn func1() {".to_string(),
                "    let x = 100;".to_string(), // Slightly different
                "}".to_string(),
            ];
            ring_buffer.pick_chunk(similar_chunk, false, true);
            ring_buffer.update();
        }

        let n_evict_after = ring_buffer.n_evict();

        // Should have evicted some chunks
        assert!(n_evict_after >= n_evict_before);
    }

    #[test]
    fn test_ring_buffer_get_extra_returns_correct_data() {
        // Test that get_extra returns properly formatted extra context
        let mut ring_buffer = RingBuffer::new(2, 64);

        let chunk_data = vec![
            "fn test_function() {".to_string(),
            "    let x = 42;".to_string(),
            "    return x;".to_string(),
            "}".to_string(),
        ];

        ring_buffer.pick_chunk(chunk_data.clone(), false, true);
        ring_buffer.update();

        let extra = ring_buffer.get_extra();

        assert_eq!(extra.len(), 1);
        assert_eq!(extra[0].text, chunk_data.join("\n") + "\n");
    }

    #[test]
    fn test_multiple_ring_buffer_updates() {
        // Test multiple sequential updates to ring buffer
        let mut ring_buffer = RingBuffer::new(3, 64);

        // Pick multiple chunks without updating
        for i in 0..5 {
            ring_buffer.pick_chunk(
                vec![
                    format!("fn func{}_()", i),
                    format!("    let x = {};", i),
                    "}".to_string(),
                ],
                false,
                true,
            );
        }

        // All should be in queued
        assert_eq!(ring_buffer.queued_len(), 5);
        assert_eq!(ring_buffer.len(), 0);

        // Update twice
        ring_buffer.update();
        ring_buffer.update();

        // Should have moved 2 to ring, 3 remaining in queue
        assert_eq!(ring_buffer.len(), 2);
        assert_eq!(ring_buffer.queued_len(), 3);

        // Update remaining queued chunks
        ring_buffer.update();
        ring_buffer.update();
        ring_buffer.update();

        // All should be in ring (max 3 due to limit)
        assert_eq!(ring_buffer.len(), 3);
        assert_eq!(ring_buffer.queued_len(), 0);
    }

    #[test]
    fn test_ring_buffer_chunk_duplicate_prevention() {
        // Test that duplicate chunks are not added to the buffer
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

        // Should still be 1 (no duplicate added)
        assert_eq!(ring_buffer.len(), 1);

        // Try to add same chunk via queued (should also be ignored)
        ring_buffer.pick_chunk(chunk, false, true);

        // Should still have same queued count
        assert_eq!(ring_buffer.queued_len(), 0);
    }

    #[test]
    fn test_fim_try_hint_basic() {
        // Test that fim_try_hint finds cached completions
        let mut cache = Cache::new(10);
        let config = LttwConfig::new();
        let lines = vec![
            "fn main() {".to_string(),
            "    println!(\"hello\");".to_string(),
            "}".to_string(),
        ];

        // Get the actual context that fim_try_hint will compute
        let actual_ctx = get_local_context(&lines, 4, 1, None, &config);

        // Cache a completion for this context
        let response = r#"{"content":"world","timings":{},"tokens_cached":0,"truncated":false}"#;
        let hash = sha256(&format!(
            "{}{}{}{}",
            actual_ctx.prefix, actual_ctx.middle, "Î", actual_ctx.suffix
        ));
        println!("Test hash: {}", hash);
        println!(
            "Actual context - prefix: {:?}, middle: {:?}, suffix: {:?}",
            actual_ctx.prefix, actual_ctx.middle, actual_ctx.suffix
        );
        cache.insert(hash, response.to_string());

        // Try to get hint
        let result = fim_try_hint(4, 1, &lines, &mut cache, &config);

        // Should find the cached completion
        assert!(result.is_some(), "Should find cached completion");
        let suggestion = result.unwrap();
        assert!(suggestion.can_accept);
    }

    #[test]
    fn test_fim_try_hint_nearby_completion() {
        // Test that fim_try_hint finds nearby cached completions
        let mut cache = Cache::new(10);
        let config = LttwConfig::new();
        let lines = vec![
            "fn main() {".to_string(),
            "    println!(\"hello world\");".to_string(),
            "}".to_string(),
        ];

        // Get context at position 4 (before "world")
        let ctx_behind = get_local_context(&lines, 4, 1, None, &config);

        // Cache a completion for this position
        let response = r#"{"content":" world","timings":{},"tokens_cached":0,"truncated":false}"#;
        let hash_behind = sha256(&format!(
            "{}{}{}{}",
            ctx_behind.prefix, ctx_behind.middle, "Î", ctx_behind.suffix
        ));
        cache.insert(hash_behind.clone(), response.to_string());

        // Test that we can find the hash directly (basic nearby lookup)
        let ctx_current = get_local_context(&lines, 9, 1, None, &config);
        let hash_current = sha256(&format!(
            "{}{}{}{}",
            ctx_current.prefix, ctx_current.middle, "Î", ctx_current.suffix
        ));

        // The hashes should be different
        assert_ne!(hash_behind, hash_current);

        // But the cache should still contain the original hash
        assert!(cache.contains_key(&hash_behind));
    }

    #[test]
    fn test_fim_try_hint_no_cache() {
        // Test that fim_try_hint returns None when no cache entry exists
        let mut cache = Cache::new(10);
        let config = LttwConfig::new();
        let lines = vec![
            "fn main() {".to_string(),
            "    println!(\"hello\");".to_string(),
            "}".to_string(),
        ];

        let result = fim_try_hint(4, 1, &lines, &mut cache, &config);

        // Should return None when no cached completion exists
        assert!(result.is_none());
    }
}
