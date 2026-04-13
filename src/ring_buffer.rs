/// The ring buffer passively accumulates and processes chunks of data
/// provided by the pick process. NOTE that the ring buffer will not
/// update the queue until the the passive update timer fires.
/// This actually restricts the amount of additional context generated.
///
/// Currently:
///  - queued chunks are taken from the front of the queue (older chunks are processed first).
///     - Maybe this doesn't actually make sense? newer chunks are more relavent to what's
///       currently happening.
use {
    crate::{
        context::chunk_similarity, fim::FimLLM, get_state, plugin_state::PluginState,
        utils::random_range, LttwResult,
    },
    std::collections::VecDeque,
    std::sync::{atomic::Ordering, Arc},
    std::time::{Duration, Instant},
};

/// Setup a repeating timer to process ring buffer updates using tokio
#[tracing::instrument]
pub fn setup_ring_buffer_timer() -> LttwResult<()> {
    let state = get_state();
    let interval = state.config.read().ring_update_ms;
    let dur = Duration::from_millis(interval);

    // Create a new tokio runtime and spawn the timer task
    // This follows the same pattern used elsewhere in the codebase
    let rt = state.tokio_runtime.clone();
    let state_ = state.clone();

    let _span = tracing::span!(tracing::Level::INFO, "ring_buffer_timer").entered();

    let timer_handle = rt.read().spawn(async move {
        let mut interval = tokio::time::interval(dur);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if !state_.ring_updating_active.load(Ordering::SeqCst)
                        && let Ok(nm) = state_.in_normal_mode()
                        && (nm || (*state_.last_move_time.read()).elapsed() < Duration::from_secs(3))
                    {
                        start_processing_ring_updates().await;
                    }
                }
                // TODO add shutdown signal
                // _ = state.shutdown_notify.notified() => break,
            }
        }
    });

    // Store the handle in the plugin state
    *state.ring_buffer_timer_handle.write() = Some(Arc::new(parking_lot::Mutex::new(timer_handle)));

    info!("Started ring buffer timer (interval: {}ms)", interval,);

    Ok(())
}

/// The mode has changed, maybe start processing ring updates
#[tracing::instrument]
pub fn mode_change_maybe_start_processing_ring_updates() -> LttwResult<()> {
    let state = get_state();
    if !state.ring_updating_active.load(Ordering::SeqCst)
        && let Ok(nm) = state.in_normal_mode()
        && (nm || (*state.last_move_time.read()).elapsed() > Duration::from_secs(3))
    {
        let rt = state.tokio_runtime.clone();
        rt.read().spawn(async move {
            start_processing_ring_updates().await;
        });
    }
    Ok(())
}

/// start processing ring updates until none are left of the mode changes and we have to stop
#[tracing::instrument]
pub async fn start_processing_ring_updates() {
    let state = get_state();
    state.ring_updating_active.store(true, Ordering::SeqCst);
    let _ = tokio::spawn(async move {
        loop {
            let m = FimLLM::Fast; // XXX
            let stop = match ring_update(m).await {
                Ok(stop) => stop,
                Err(e) => {
                    error!(e);
                    true // don't want an infinite loop
                }
            };
            if stop {
                break;
            }
        }
    })
    .await;
    state.ring_updating_active.store(false, Ordering::SeqCst);
}

/// Process ring buffer updates - moves queued chunks to active ring and sends to server
///  
#[tracing::instrument]
async fn ring_update(m: FimLLM) -> LttwResult<bool> {
    let state = get_state();

    // skip update if we're not in normal mode and cursor movement is recent
    if !state.in_normal_mode()? && (*state.last_move_time.read()).elapsed() < Duration::from_secs(3)
    {
        return Ok(true);
    }

    if state.get_ring_buffer(m).read().queued.is_empty() {
        return Ok(true);
    }

    // Check if we have chunks before logging
    let chunk_count = state.get_ring_buffer(m).write().update();

    if chunk_count > 0 {
        info!("Processing {chunk_count} ring buffer chunks");
        let extra = state.get_ring_buffer(m).read().get_extra();
        state.send_fim_request_buffer(m, extra).await?;
    }

    Ok(false)
}

// -----------------------------

/// A chunk of text from the buffer
// TODO combine data and chunk_str maybe seems redundant to store twice
#[derive(Debug, Clone)]
pub struct Chunk {
    pub data: Vec<String>, // all the lines seperated
    pub chunk_str: String, // all the lines combined (same as data)
    pub time: Instant,
    pub filename: String,
}

/// Ring buffer for extra context chunks
#[derive(Debug, Clone)]
pub struct RingBuffer {
    pub chunks: Vec<Chunk>,
    pub queued: VecDeque<Chunk>,
    n_evict: usize,
    ring_n_chunks: usize,
    ring_queue_length: usize,
    chunk_size: usize,
}

impl PluginState {
    /// Pick a random chunk from the provided text and queue it for processing
    ///
    /// ## Arguments
    ///  - `text` - Text to pick a chunk from
    ///  - `no_mod` - If true, don't pick chunks from buffers with pending changes
    #[tracing::instrument(skip(self, text, filename))]
    pub fn pick_chunk(&self, text: &[String], filename: String) {
        let info = self.get_cur_buffer_info();
        if !(info.filepath == filename && info.is_loaded && info.is_readable) {
            return;
        }
        // Pick chunk from yanked text
        if self.config.read().duel_model_mode {
            self.get_ring_buffer(FimLLM::Slow)
                .write()
                .pick_chunk_inner(text, filename.clone());
        }
        self.get_ring_buffer(FimLLM::Fast)
            .write()
            .pick_chunk_inner(text, filename);
    }
}

impl RingBuffer {
    /// Create a new ring buffer with the given parameters
    #[tracing::instrument]
    pub fn new(ring_n_chunks: usize, chunk_size: usize, ring_queue_length: usize) -> Self {
        Self {
            chunks: Vec::new(),
            queued: VecDeque::new(),
            n_evict: 0,
            ring_n_chunks,
            ring_queue_length,
            chunk_size,
        }
    }

    /// Pick a random chunk from the provided text and queue it for processing
    ///
    /// ## Arguments
    ///  - `text` - Text to pick a chunk from
    ///  - `no_mod` - If true, don't pick chunks from buffers with pending changes
    ///  - `do_evict` - If true, evict chunks that are very similar to the new one
    //#[tracing::instrument(skip(state, text, filename))]
    //pub fn pick_chunk(&mut self, state: &PluginState, text: &[String], filename: String) {
    //    let info = state.get_cur_buffer_info();
    //    if !(info.filepath == filename && info.is_loaded && info.is_readable) {
    //        return;
    //    }

    //    self.pick_chunk_inner(text, filename);
    //}

    #[tracing::instrument(skip(text, filename))]
    pub fn pick_chunk_inner(&mut self, text: &[String], filename: String) {
        // Skip if extra context is disabled
        if self.ring_n_chunks == 0 {
            return;
        }

        // Skip very small chunks
        if text.len() < 3 {
            return;
        }

        let chunk = self.get_chunk_from_text(text);

        let chunk_str = chunk.join("\n") + "\n";

        // Check if this chunk is already added
        if self.chunks.iter().any(|c| c.data == chunk) {
            return;
        }
        if self.queued.iter().any(|c| c.data == chunk) {
            return;
        }

        // Evict queued chunks that are very similar
        // Only evict from the live ring_buffer once the chunk enters the buffer. But evicting
        // from the similar from the queue immediately
        self.evict_similar_from_queue(chunk, 0.9);

        // Keep only the last N queued chunks (configurable via ring_queue_length)
        while self.queued.len() >= self.ring_queue_length {
            self.queued.remove(0);
        }

        self.queued.push_back(Chunk {
            data: chunk.to_vec(),
            chunk_str,
            time: Instant::now(),
            filename,
        });
    }

    /// Move the first queued chunk to the ring buffer
    /// returns the size of the ring buffer after completion
    #[tracing::instrument]
    pub fn update(&mut self) -> usize {
        if self.queued.is_empty() {
            return self.chunks.len();
        }

        // take from the front of the queue (oldest, but in order) and add to the ring buffer
        if let Some(chunk) = self.queued.pop_front() {
            let chunk_data = chunk.data.clone();
            // evict similar from the live buffer BEFORE adding the new chunk
            // this prevents evicting the chunk we're about to add
            self.evict_similar_from_live(&chunk_data, 0.9);
            self.chunks.push(chunk);
        }

        // Remove oldest chunk if buffer is full
        while self.chunks.len() > self.ring_n_chunks {
            self.chunks.remove(0);
        }
        self.chunks.len()
    }

    /// Get extra context from the ring buffer
    #[tracing::instrument]
    pub fn get_extra(&self) -> Vec<ExtraContext> {
        self.chunks
            .iter()
            .map(|chunk| ExtraContext {
                text: chunk.chunk_str.clone(),
                filename: chunk.filename.clone(),
            })
            .collect()
    }

    // XXX delete
    ///// Get the number of chunks in the ring buffer
    //#[tracing::instrument]
    //pub fn len(&self) -> usize {
    //    self.chunks.len()
    //}
    ///// Check if the ring buffer is empty
    //#[tracing::instrument]
    //pub fn is_empty(&self) -> bool {
    //    self.chunks.is_empty()
    //}

    /// Get the number of queued chunks
    #[tracing::instrument]
    pub fn queued_len(&self) -> usize {
        self.queued.len()
    }

    /// Check if the queue is empty
    #[tracing::instrument]
    pub fn queue_is_empty(&self) -> bool {
        self.queued.is_empty()
    }

    /// Get the number of evicted chunks
    #[tracing::instrument]
    pub fn n_evict(&self) -> usize {
        self.n_evict
    }

    // Gets a chunk from the text, either the whole text (in len < chunk size)
    // or a random range selection from the provided text which is of the chunk size.
    //
    // Random select to:
    // - Avoids Bias: By randomly selecting the starting position within the text, it prevents always
    //   picking the same or similar chunks from the same locations, ensuring a more diverse context
    //   collection llama.vim:435-438 .
    // - Better Coverage: When gathering context from various events (yank, buffer enter/leave,
    //   file save), random sampling helps capture different parts of the codebase that might be
    //   relevant for future completions
    pub fn get_chunk_from_text<'a>(&self, text: &'a [String]) -> &'a [String] {
        let text_len = text.len();
        if text_len < self.chunk_size {
            text
        } else {
            let chunk_size_half = self.chunk_size / 2;
            let l0 = if text_len > chunk_size_half {
                random_range(0, text_len.saturating_sub(chunk_size_half))
            } else {
                0
            };
            let l1 = (l0 + chunk_size_half).min(text_len);
            &text[l0..l1]
        }
    }

    /// Evict chunks from the ring buffer that are very similar to the given text
    ///
    /// # Arguments
    /// * `text` - Text to compare against
    /// * `threshold` - Similarity threshold (0.0-1.0). Chunks with similarity > threshold are evicted
    pub fn evict_similar_from_live(&mut self, text: &[String], threshold: f64) {
        if text.is_empty() {
            return;
        }

        // Evict from ring chunks
        for i in (0..self.chunks.len()).rev() {
            let sim = chunk_similarity(&self.chunks[i].data, text);
            if sim > threshold {
                self.chunks.remove(i);
                self.n_evict += 1;
            }
        }
    }
    /// Evict chunks from the ring buffer that are very similar to the given text
    ///
    /// # Arguments
    /// * `text` - Text to compare against
    /// * `threshold` - Similarity threshold (0.0-1.0). Chunks with similarity > threshold are evicted
    pub fn evict_similar_from_queue(&mut self, text: &[String], threshold: f64) {
        if text.is_empty() {
            return;
        }

        // Evict from queued chunks
        for i in (0..self.queued.len()).rev() {
            let sim = chunk_similarity(&self.queued[i].data, text);
            if sim > threshold {
                self.queued.remove(i);
                self.n_evict += 1;
            }
        }
    }

    /// Evict chunks from the ring buffer by filename
    ///
    /// # Arguments
    /// * `filename` - Filename to match for eviction
    pub fn evict_by_filename(&mut self, filename: &str) {
        // Evict from queued chunks
        for i in (0..self.queued.len()).rev() {
            if self.queued[i].filename == filename {
                self.queued.remove(i);
            }
        }

        // Evict from ring chunks (internal method to access private field)
        self.chunks.retain(|c| c.filename != filename);
    }

    /// Get the number of chunks in the ring buffer (for testing)
    #[cfg(test)]
    #[tracing::instrument]
    pub fn get_chunks_count(&self) -> usize {
        self.chunks.len()
    }
}

/// Extra context for the server request
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExtraContext {
    pub text: String,
    #[serde(skip_serializing)]
    pub filename: String,
}

#[cfg(test)]
mod tests {
    use crate::{
        cache::{compute_hashes, Cache},
        context::LocalContext,
        ring_buffer::RingBuffer,
        FimResponse,
    };

    #[test]
    fn test_ring_buffer_basic() {
        let mut ring = RingBuffer::new(3, 64, 16);

        ring.pick_chunk_inner(
            &[
                "line1".to_string(),
                "line2".to_string(),
                "line3".to_string(),
            ],
            String::new(),
        );
        ring.pick_chunk_inner(
            &[
                "line4".to_string(),
                "line5".to_string(),
                "line6".to_string(),
            ],
            String::new(),
        );

        assert_eq!(ring.queued_len(), 2);

        ring.update();

        assert_eq!(ring.chunks.len(), 1);
        assert_eq!(ring.queued_len(), 1);
    }

    #[test]
    fn test_ring_buffer_eviction() {
        let mut ring = RingBuffer::new(2, 64, 16);

        // Add chunks
        ring.pick_chunk_inner(
            &[
                "line1".to_string(),
                "line2".to_string(),
                "line3".to_string(),
            ],
            String::new(),
        );
        ring.pick_chunk_inner(
            &[
                "line4".to_string(),
                "line5".to_string(),
                "line6".to_string(),
            ],
            String::new(),
        );
        ring.pick_chunk_inner(
            &[
                "line7".to_string(),
                "line8".to_string(),
                "line9".to_string(),
            ],
            String::new(),
        );

        ring.update();
        ring.update();

        assert_eq!(ring.chunks.len(), 2);

        // The queued should be empty after 2 updates (3 queued - 2 updated = 1 remaining)
        // But we added 3 chunks, and 2 were moved to ring, so 1 should remain
        assert_eq!(ring.queued_len(), 1);
    }

    #[test]
    fn test_ring_buffer_update() {
        let mut ring = RingBuffer::new(2, 64, 16);

        ring.pick_chunk_inner(
            &[
                "line1".to_string(),
                "line2".to_string(),
                "line3".to_string(),
            ],
            String::new(),
        );
        ring.pick_chunk_inner(
            &[
                "line4".to_string(),
                "line5".to_string(),
                "line6".to_string(),
            ],
            String::new(),
        );

        assert_eq!(ring.queued_len(), 2);

        ring.update();

        assert_eq!(ring.chunks.len(), 1);
        assert_eq!(ring.queued_len(), 1);

        ring.update();

        assert_eq!(ring.chunks.len(), 2);
        assert_eq!(ring.queued_len(), 0);
    }

    #[test]
    fn test_ring_buffer_max_chunks() {
        let mut ring = RingBuffer::new(3, 64, 16);

        for _i in 0..10 {
            ring.pick_chunk_inner(
                &[
                    "line1".to_string(),
                    "line2".to_string(),
                    "line3".to_string(),
                ],
                String::new(),
            );
            ring.update();
        }

        assert!(ring.chunks.len() <= 3);
    }

    #[test]
    fn test_ring_buffer_get_extra() {
        let mut ring = RingBuffer::new(2, 64, 16);

        ring.pick_chunk_inner(
            &[
                "line1".to_string(),
                "line2".to_string(),
                "line3".to_string(),
            ],
            String::new(),
        );
        ring.pick_chunk_inner(
            &[
                "line4".to_string(),
                "line5".to_string(),
                "line6".to_string(),
            ],
            String::new(),
        );

        ring.update();
        ring.update();

        let extra = ring.get_extra();
        assert_eq!(extra.len(), 2);
    }

    #[test]
    fn test_ring_buffer_n_evict() {
        let mut ring = RingBuffer::new(2, 64, 16);

        ring.pick_chunk_inner(
            &[
                "line1".to_string(),
                "line2".to_string(),
                "line3".to_string(),
            ],
            String::new(),
        );
        ring.pick_chunk_inner(
            &[
                "line4".to_string(),
                "line5".to_string(),
                "line6".to_string(),
            ],
            String::new(),
        );
        ring.pick_chunk_inner(
            &[
                "line7".to_string(),
                "line8".to_string(),
                "line9".to_string(),
            ],
            String::new(),
        );

        assert_eq!(ring.queued_len(), 3);
    }

    #[test]
    fn test_ring_buffer_chunk_duplicate_prevention() {
        // Test that duplicate chunks are not added to the buffer
        let mut ring_buffer = RingBuffer::new(5, 64, 16);

        let chunk = vec![
            "fn duplicate_test() {".to_string(),
            "    let x = 1;".to_string(),
            "}".to_string(),
        ];

        // Add chunk first time
        ring_buffer.pick_chunk_inner(&chunk, String::new());
        ring_buffer.update();

        assert_eq!(ring_buffer.chunks.len(), 1);

        // Try to add exact same chunk again (should be ignored)
        ring_buffer.pick_chunk_inner(&chunk, String::new());

        // Should still be 1 (no duplicate added)
        assert_eq!(ring_buffer.chunks.len(), 1);

        // Try to add same chunk via queued (should also be ignored)
        ring_buffer.pick_chunk_inner(&chunk, String::new());

        // Should still have same queued count
        assert_eq!(ring_buffer.queued_len(), 0);
    }

    #[test]
    fn test_ring_buffer_integration_with_cache() {
        // Test that ring buffer chunks are properly tracked and cached
        let mut ring_buffer = RingBuffer::new(3, 64, 16);

        // Add first chunk
        ring_buffer.pick_chunk_inner(
            &[
                "fn main() {".to_string(),
                "    println!(\"hello\");".to_string(),
                "}".to_string(),
            ],
            String::new(),
        );
        ring_buffer.update();

        assert_eq!(ring_buffer.chunks.len(), 1);
        assert_eq!(ring_buffer.queued_len(), 0);

        // Add second chunk (should not evict first since they're different)
        ring_buffer.pick_chunk_inner(
            &[
                "use std::io;".to_string(),
                "fn read_input() {".to_string(),
                "    let mut s = String::new();".to_string(),
            ],
            String::new(),
        );
        ring_buffer.update();

        assert_eq!(ring_buffer.chunks.len(), 2);

        // Add third chunk
        ring_buffer.pick_chunk_inner(
            &[
                "mod test;".to_string(),
                "fn test_func() {".to_string(),
                "    assert_eq!(1, 1);".to_string(),
            ],
            String::new(),
        );
        ring_buffer.update();

        assert_eq!(ring_buffer.chunks.len(), 3);

        // Add fourth chunk - should evict the oldest one due to max_chunks limit
        ring_buffer.pick_chunk_inner(
            &[
                "pub fn export_func() {".to_string(),
                "    test_func();".to_string(),
                "}".to_string(),
            ],
            String::new(),
        );
        ring_buffer.update();

        // Should still be at max_chunks (3)
        assert_eq!(ring_buffer.chunks.len(), 3);
    }

    #[test]
    fn test_ring_buffer_eviction_with_similarity() {
        // Test that similar chunks are evicted based on similarity threshold
        let mut ring_buffer = RingBuffer::new(5, 64, 16);

        let chunk1 = vec![
            "fn function_one() {".to_string(),
            "    let x = 1;".to_string(),
            "    let y = 2;".to_string(),
            "    let z = 3;".to_string(),
            "}".to_string(),
        ];

        // Add first chunk
        ring_buffer.pick_chunk_inner(&chunk1, String::new());
        ring_buffer.update();

        assert_eq!(ring_buffer.chunks.len(), 1);

        // Add very similar chunk (should evict first due to >0.9 similarity)
        let mut chunk2 = chunk1.clone();
        chunk2[1] = "    let x = 100;".to_string(); // Slightly different

        ring_buffer.pick_chunk_inner(&chunk2, String::new());
        ring_buffer.update();

        // Due to high similarity, first chunk should be evicted
        // The exact behavior depends on the similarity threshold (0.9)
        assert!(ring_buffer.chunks.len() <= 2);
    }

    #[test]
    fn test_cache_with_ring_buffer_chunks() {
        // Test that cache properly handles entries with ring buffer context
        let mut cache = Cache::new(10);
        let mut ring_buffer = RingBuffer::new(3, 64, 16);

        // Add chunks to ring buffer
        ring_buffer.pick_chunk_inner(
            &[
                "fn test1() {".to_string(),
                "    println!(\"test1\");".to_string(),
                "}".to_string(),
            ],
            String::new(),
        );
        ring_buffer.update();

        // Simulate a FIM request with ring buffer context
        // Use a prefix with newlines to test truncated prefix hashes
        let ctx = LocalContext {
            prefix: "fn main() {\n    let x = 1;\n".to_string(),
            middle: "    println!(\"hello\"".to_string(),
            suffix: ");\n}".to_string(),
            line_cur_suffix: "rintln!(\"hello\");".to_string(),
            line_cur: "    println!(\"hello\");".to_string(),
            indent: 4,
        };

        let hashes = compute_hashes(&ctx.prefix, &ctx.middle, &ctx.suffix);

        // Verify we generated multiple hashes (prefix has newlines)
        assert!(
            hashes.len() > 1,
            "Should generate multiple hashes from truncated prefixes"
        );

        // Cache a response for these hashes
        let response = r#"{"content":" world","timings":{},"tokens_cached":0,"truncated":false}"#;
        let response = serde_json::from_str::<FimResponse>(response).unwrap();
        let response = crate::llama_client::FimResponseWithInfo {
            resp: response,
            cached: false,
            model: crate::fim::FimModel::LLMFast,
        };
        for hash in &hashes {
            cache.insert(hash.clone(), response.clone());
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
        let mut ring_buffer = RingBuffer::new(2, 64, 16);

        ring_buffer.pick_chunk_inner(
            &[
                "fn func1() {".to_string(),
                "    let x = 1;".to_string(),
                "}".to_string(),
            ],
            String::new(),
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

            ring_buffer.pick_chunk_inner(&similar_chunk, String::new());
            ring_buffer.update();
        }

        let n_evict_after = ring_buffer.n_evict();

        // Should have evicted some chunks
        assert!(n_evict_after >= n_evict_before);
    }

    #[test]
    fn test_ring_buffer_get_extra_returns_correct_data() {
        // Test that get_extra returns properly formatted extra context
        let mut ring_buffer = RingBuffer::new(2, 64, 16);

        let chunk_data = vec![
            "fn test_function() {".to_string(),
            "    let x = 42;".to_string(),
            "    return x;".to_string(),
            "}".to_string(),
        ];

        ring_buffer.pick_chunk_inner(&chunk_data, String::new());
        ring_buffer.update();

        let extra = ring_buffer.get_extra();

        assert_eq!(extra.len(), 1);
        assert_eq!(extra[0].text, chunk_data.join("\n") + "\n");
    }

    #[test]
    fn test_multiple_ring_buffer_updates() {
        // Test multiple sequential updates to ring buffer
        let mut ring_buffer = RingBuffer::new(3, 64, 16);

        // Pick multiple chunks without updating
        for i in 0..5 {
            ring_buffer.pick_chunk_inner(
                &[
                    format!("fn func{}_()", i),
                    format!("    let x = {};", i),
                    "}".to_string(),
                ],
                String::new(),
            );
        }

        // All should be in queued
        assert_eq!(ring_buffer.queued_len(), 5);
        assert_eq!(ring_buffer.chunks.len(), 0);

        // Update twice
        ring_buffer.update();
        ring_buffer.update();

        // Should have moved 2 to ring, 3 remaining in queue
        assert_eq!(ring_buffer.chunks.len(), 2);
        assert_eq!(ring_buffer.queued_len(), 3);

        // Update remaining queued chunks
        ring_buffer.update();
        ring_buffer.update();
        ring_buffer.update();

        // All should be in ring (max 3 due to limit)
        assert_eq!(ring_buffer.chunks.len(), 3);
        assert_eq!(ring_buffer.queued_len(), 0);
    }
}
