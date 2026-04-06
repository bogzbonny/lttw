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
        LttwResult, context::chunk_similarity, get_state, plugin_state::PluginState,
        utils::random_range,
    },
    std::sync::{Arc, atomic::Ordering},
    std::time::{Duration, Instant},
};

/// Setup a repeating timer to process ring buffer updates using tokio
pub fn setup_ring_buffer_timer() -> LttwResult<()> {
    let state = get_state();
    let interval = state.config.read().ring_update_ms;
    let dur = Duration::from_millis(interval);

    // Create a new tokio runtime and spawn the timer task
    // This follows the same pattern used elsewhere in the codebase
    let rt = state.tokio_runtime.clone();
    let state_ = state.clone();

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

    state.debug_manager.read().log(
        "setup_ring_buffer_timer",
        format!("Started ring buffer timer (interval: {}ms)", interval),
    );

    Ok(())
}

/// The mode has changed, maybe start processing ring updates
pub fn mode_change_maybe_start_processing_ring_updates() -> LttwResult<()> {
    let state = get_state();
    if !state.ring_updating_active.load(Ordering::SeqCst)
        && let Ok(nm) = state.in_normal_mode()
        && (nm || (*state.last_move_time.read()).elapsed() < Duration::from_secs(3))
    {
        let rt = state.tokio_runtime.clone();
        rt.read().spawn(async move {
            start_processing_ring_updates().await;
        });
    }
    Ok(())
}

/// start processing ring updates until none are left of the mode changes and we have to stop
pub async fn start_processing_ring_updates() {
    let state = get_state();
    state.ring_updating_active.store(true, Ordering::SeqCst);
    let _ = tokio::spawn(async move {
        loop {
            let Ok(stop) = ring_update().await else {
                break;
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
async fn ring_update() -> LttwResult<bool> {
    let state = get_state();

    // skip update if we're not in normal mode and cursor movement is recent
    if !state.in_normal_mode()? && (*state.last_move_time.read()).elapsed() < Duration::from_secs(3)
    {
        return Ok(true);
    }

    if state.ring_buffer.read().queued.is_empty() {
        return Ok(true);
    }

    // Check if we have chunks before logging
    let chunk_count = {
        // Move first queued chunk to ring
        let mut ring_buffer_lock = state.ring_buffer.write();
        ring_buffer_lock.update();
        ring_buffer_lock.len()
    };

    if chunk_count > 0 {
        state.debug_manager.read().log(
            "ring_update",
            format!("Processing {chunk_count} ring buffer chunks "),
        );

        // TODO should n-predict be 0 here?? test
        // Build request with ring buffer context
        let extra = state.ring_buffer.read().get_extra();
        let request = serde_json::json!({
            "input_extra": extra,
            "cache_prompt": true
        });

        // Send to server (fire and forget - non-blocking)
        let config = state.config.read().clone();
        let client = reqwest::Client::new();
        let _ = client
            .post(&config.endpoint_fim)
            .json(&request)
            .bearer_auth(&config.api_key)
            .send()
            .await;
    }

    Ok(false)
}

// -----------------------------

/// A chunk of text from the buffer
#[derive(Debug, Clone)]
pub struct Chunk {
    pub data: Vec<String>,
    pub chunk_str: String,
    pub time: Instant,
    pub filename: String,
}

/// Ring buffer for extra context chunks
#[derive(Debug, Clone)]
pub struct RingBuffer {
    chunks: Vec<Chunk>,
    pub queued: Vec<Chunk>,
    n_evict: usize,
    ring_n_chunks: usize,
    ring_queue_length: usize,
    chunk_size: usize,
}

impl RingBuffer {
    /// Create a new ring buffer with the given parameters
    pub fn new(ring_n_chunks: usize, chunk_size: usize, ring_queue_length: usize) -> Self {
        Self {
            chunks: Vec::new(),
            queued: Vec::new(),
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
    pub fn pick_chunk(
        &mut self,
        state: &PluginState,
        text: &[String],
        filename: String,
    ) -> LttwResult<()> {
        let info = state.get_cur_buffer_info();
        if !(info.filepath == filename && info.is_loaded && info.is_readable) {
            return Ok(());
        }

        self.pick_chunk_inner(text, filename)
    }

    pub fn pick_chunk_inner(&mut self, text: &[String], filename: String) -> LttwResult<()> {
        // Skip if extra context is disabled
        if self.ring_n_chunks == 0 {
            return Ok(());
        }

        // Skip very small chunks
        if text.len() < 3 {
            return Ok(());
        }

        let chunk = self.get_chunk_from_text(text);

        let chunk_str = chunk.join("\n") + "\n";

        // Check if this chunk is already added
        if self.chunks.iter().any(|c| c.data == chunk) {
            return Ok(());
        }
        if self.queued.iter().any(|c| c.data == chunk) {
            return Ok(());
        }

        // Evict queued chunks that are very similar
        // Only evict from the live ring_buffer once the chunk enters the buffer. But evicting
        // from the similar from the queue immediately
        self.evict_similar_from_queue(chunk, 0.9);

        // Keep only the last N queued chunks (configurable via ring_queue_length)
        while self.queued.len() >= self.ring_queue_length {
            self.queued.remove(0);
        }

        self.queued.push(Chunk {
            data: chunk.to_vec(),
            chunk_str,
            time: Instant::now(),
            filename,
        });
        Ok(())
    }

    /// Move the first queued chunk to the ring buffer
    pub fn update(&mut self) {
        if self.queued.is_empty() {
            return;
        }

        // take from the tail of the queue (most recently added / relevant) and add to the ring buffer
        // NOTE it may make sense to actually take it from the front.. less relevant things will be
        // added first however the more relavent things (added last) will be in the ring buffer for
        // longer (get evicted last). I DONT KNOW - should do trial and error TODO
        if let Some(chunk) = self.queued.pop() {
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
    }

    /// Get extra context from the ring buffer
    pub fn get_extra(&self) -> Vec<ExtraContext> {
        self.chunks
            .iter()
            .map(|chunk| ExtraContext {
                text: chunk.chunk_str.clone(),
                filename: chunk.filename.clone(),
            })
            .collect()
    }

    /// Get the number of chunks in the ring buffer
    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    /// Check if the ring buffer is empty
    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    /// Get the number of queued chunks
    pub fn queued_len(&self) -> usize {
        self.queued.len()
    }

    /// Check if the queue is empty
    pub fn queue_is_empty(&self) -> bool {
        self.queued.is_empty()
    }

    /// Get the number of evicted chunks
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
    use super::*;

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
        )
        .unwrap();
        ring.pick_chunk_inner(
            &[
                "line4".to_string(),
                "line5".to_string(),
                "line6".to_string(),
            ],
            String::new(),
        )
        .unwrap();

        assert_eq!(ring.queued_len(), 2);

        ring.update();

        assert_eq!(ring.len(), 1);
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
        )
        .unwrap();
        ring.pick_chunk_inner(
            &[
                "line4".to_string(),
                "line5".to_string(),
                "line6".to_string(),
            ],
            String::new(),
        )
        .unwrap();
        ring.pick_chunk_inner(
            &[
                "line7".to_string(),
                "line8".to_string(),
                "line9".to_string(),
            ],
            String::new(),
        )
        .unwrap();

        ring.update();
        ring.update();

        assert_eq!(ring.len(), 2);

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
        )
        .unwrap();
        ring.pick_chunk_inner(
            &[
                "line4".to_string(),
                "line5".to_string(),
                "line6".to_string(),
            ],
            String::new(),
        )
        .unwrap();

        assert_eq!(ring.queued_len(), 2);

        ring.update();

        assert_eq!(ring.len(), 1);
        assert_eq!(ring.queued_len(), 1);

        ring.update();

        assert_eq!(ring.len(), 2);
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
            )
            .unwrap();
            ring.update();
        }

        assert!(ring.len() <= 3);
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
        )
        .unwrap();
        ring.pick_chunk_inner(
            &[
                "line4".to_string(),
                "line5".to_string(),
                "line6".to_string(),
            ],
            String::new(),
        )
        .unwrap();

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
        )
        .unwrap();
        ring.pick_chunk_inner(
            &[
                "line4".to_string(),
                "line5".to_string(),
                "line6".to_string(),
            ],
            String::new(),
        )
        .unwrap();
        ring.pick_chunk_inner(
            &[
                "line7".to_string(),
                "line8".to_string(),
                "line9".to_string(),
            ],
            String::new(),
        )
        .unwrap();

        assert_eq!(ring.queued_len(), 3);
    }
}
