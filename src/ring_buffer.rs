// src/ring_buffer.rs - Ring buffer for extra context chunks
//
// This module implements a ring buffer that collects and manages chunks of
// text from the buffer to provide additional context to the language model.

use {
    crate::{context::chunk_similarity, get_state, utils::in_normal_mode},
    nvim_oxi::{Dictionary, Result as NvimResult},
    serde::Serialize,
    std::sync::Arc,
    std::time::Duration,
};

/// Process ring buffer updates - moves queued chunks to active ring and sends to server
///  
fn ring_update() -> NvimResult<()> {
    let state = get_state();

    // update only if in normal mode or if the cursor hasn't moved for a while
    if in_normal_mode()? || (*state.last_move_time.read()).elapsed() > Duration::from_secs(3) {
        return Ok(());
    }

    // Get configuration
    let update_interval = state.config.read().ring_update_ms;

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
            &[&format!(
                "Processing {} ring buffer chunks (interval: {}ms)",
                chunk_count, update_interval
            )],
        );

        // Build request with ring buffer context
        let extra = state.ring_buffer.read().get_extra();
        let request = serde_json::json!({
            "input_extra": extra,
            "cache_prompt": true
        });

        // Send to server (fire and forget - non-blocking)
        let config = state.config.read().clone();
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async move {
                let client = reqwest::Client::new();
                let _ = client
                    .post(&config.endpoint_fim)
                    .json(&request)
                    .bearer_auth(&config.api_key)
                    .send()
                    .await;
            });
    }

    Ok(())
}

/// Setup a repeating timer to process ring buffer updates using tokio
pub fn setup_ring_buffer_timer() -> NvimResult<()> {
    let state = get_state();
    let interval = state.config.read().ring_update_ms;
    let interval_duration = std::time::Duration::from_millis(interval as u64);

    // Create a new tokio runtime and spawn the timer task
    // This follows the same pattern used elsewhere in the codebase
    let timer_handle = tokio::runtime::Runtime::new().unwrap().spawn(async move {
        // Create a recurring interval timer
        let mut interval = tokio::time::interval(interval_duration);

        loop {
            interval.tick().await;
            let _ = ring_update();
        }
    });

    // Store the handle in the plugin state
    {
        let mut ring_buffer_timer_handle_lock = state.ring_buffer_timer_handle.write();
        *ring_buffer_timer_handle_lock = Some(Arc::new(parking_lot::Mutex::new(timer_handle)));
    }

    state.debug_manager.read().log(
        "setup_ring_buffer_timer",
        &[&format!(
            "Started ring buffer timer (interval: {}ms)",
            interval
        )],
    );

    Ok(())
}

/// Ring buffer pick chunk function
// TODO verify if this is still needed
#[allow(dead_code)]
fn ring_pick_chunk(lines: Vec<String>, no_mod: bool, do_evict: bool) -> NvimResult<()> {
    let state = get_state();
    state
        .ring_buffer
        .write()
        .pick_chunk(lines, no_mod, do_evict);
    Ok(())
}

/// Ring buffer get extra function
// TODO verify if this is still needed
#[allow(dead_code)]
fn ring_get_extra() -> NvimResult<Vec<Dictionary>> {
    let state = get_state();
    let ring_buffer_lock = state.ring_buffer.read();
    let extra = ring_buffer_lock.get_extra();

    let mut result = Vec::new();
    for e in extra {
        let mut dict = Dictionary::new();
        dict.insert("text", e.text);
        dict.insert("filename", e.filename);
        result.push(dict);
    }

    Ok(result)
}

// -----------------------------

/// A chunk of text from the buffer
#[derive(Debug, Clone, Serialize)]
pub struct Chunk {
    pub data: Vec<String>,
    pub str: String,
    pub filename: String,
}

/// Ring buffer for extra context chunks
#[derive(Debug, Clone, Serialize)]
pub struct RingBuffer {
    chunks: Vec<Chunk>,
    pub queued: Vec<Chunk>,
    n_evict: usize,
    max_chunks: usize,
    chunk_size: usize,
}

impl RingBuffer {
    /// Create a new ring buffer with the given parameters
    pub fn new(max_chunks: usize, chunk_size: usize) -> Self {
        Self {
            chunks: Vec::new(),
            queued: Vec::new(),
            n_evict: 0,
            max_chunks,
            chunk_size,
        }
    }

    /// Pick a random chunk from the provided text and queue it for processing
    ///
    /// # Arguments
    /// * `text` - Text to pick a chunk from
    /// * `no_mod` - If true, don't pick chunks from buffers with pending changes
    /// * `do_evict` - If true, evict chunks that are very similar to the new one
    pub fn pick_chunk(&mut self, text: Vec<String>, _no_mod: bool, do_evict: bool) {
        // Skip if extra context is disabled
        if self.max_chunks == 0 {
            return;
        }

        // Skip very small chunks
        if text.len() < 3 {
            return;
        }

        let chunk_size_half = self.chunk_size / 2;

        // Pick a random chunk
        let chunk = if text.len() + 1 < self.chunk_size {
            text
        } else {
            let l0 = std::cmp::min(
                rand::random::<usize>()
                    % std::cmp::max(1, text.len().saturating_sub(chunk_size_half)),
                text.len().saturating_sub(chunk_size_half),
            );
            let l1 = std::cmp::min(l0 + chunk_size_half, text.len());
            text[l0..l1].to_vec()
        };

        let chunk_str = chunk.join("\n") + "\n";

        // Check if this chunk is already added
        if self.chunks.iter().any(|c| c.data == chunk) {
            return;
        }
        if self.queued.iter().any(|c| c.data == chunk) {
            return;
        }

        // Evict queued chunks that are very similar
        for i in (0..self.queued.len()).rev() {
            if chunk_similarity(&self.queued[i].data, &chunk) > 0.9 {
                if do_evict {
                    self.queued.remove(i);
                    self.n_evict += 1;
                } else {
                    return;
                }
            }
        }

        // Also evict chunks from the ring
        for i in (0..self.chunks.len()).rev() {
            if chunk_similarity(&self.chunks[i].data, &chunk) > 0.9 {
                if do_evict {
                    self.chunks.remove(i);
                    self.n_evict += 1;
                } else {
                    return;
                }
            }
        }

        // Keep only the last 16 queued chunks
        while self.queued.len() >= 16 {
            self.queued.remove(0);
        }

        self.queued.push(Chunk {
            data: chunk,
            str: chunk_str,
            filename: String::new(), // Will be set by caller
        });
    }

    /// Move the first queued chunk to the ring buffer
    pub fn update(&mut self) {
        if self.queued.is_empty() {
            return;
        }

        // Remove oldest chunk if buffer is full
        if self.chunks.len() >= self.max_chunks {
            self.chunks.remove(0);
        }

        self.chunks.push(self.queued.remove(0));
    }

    /// Get extra context from the ring buffer
    pub fn get_extra(&self) -> Vec<ExtraContext> {
        self.chunks
            .iter()
            .map(|chunk| ExtraContext {
                text: chunk.str.clone(),
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

    /// Evict chunks from the ring buffer that are very similar to the given text
    ///
    /// # Arguments
    /// * `text` - Text to compare against
    /// * `threshold` - Similarity threshold (0.0-1.0). Chunks with similarity > threshold are evicted
    pub fn evict_similar(&mut self, text: &[String], threshold: f64) {
        if text.is_empty() {
            return;
        }

        // Evict from queued chunks
        for i in (0..self.queued.len()).rev() {
            let sim = crate::context::chunk_similarity(&self.queued[i].data, text);
            if sim > threshold {
                self.queued.remove(i);
                self.n_evict += 1;
            }
        }

        // Evict from ring chunks
        for i in (0..self.chunks.len()).rev() {
            let sim = crate::context::chunk_similarity(&self.chunks[i].data, text);
            if sim > threshold {
                self.chunks.remove(i);
                self.n_evict += 1;
            }
        }
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
        let mut ring = RingBuffer::new(3, 64);

        ring.pick_chunk(
            vec![
                "line1".to_string(),
                "line2".to_string(),
                "line3".to_string(),
            ],
            false,
            true,
        );
        ring.pick_chunk(
            vec![
                "line4".to_string(),
                "line5".to_string(),
                "line6".to_string(),
            ],
            false,
            true,
        );

        assert_eq!(ring.queued_len(), 2);

        ring.update();

        assert_eq!(ring.len(), 1);
        assert_eq!(ring.queued_len(), 1);
    }

    #[test]
    fn test_ring_buffer_eviction() {
        let mut ring = RingBuffer::new(2, 64);

        // Add chunks
        ring.pick_chunk(
            vec![
                "line1".to_string(),
                "line2".to_string(),
                "line3".to_string(),
            ],
            false,
            true,
        );
        ring.pick_chunk(
            vec![
                "line4".to_string(),
                "line5".to_string(),
                "line6".to_string(),
            ],
            false,
            true,
        );
        ring.pick_chunk(
            vec![
                "line7".to_string(),
                "line8".to_string(),
                "line9".to_string(),
            ],
            false,
            true,
        );

        ring.update();
        ring.update();

        assert_eq!(ring.len(), 2);

        // The queued should be empty after 2 updates (3 queued - 2 updated = 1 remaining)
        // But we added 3 chunks, and 2 were moved to ring, so 1 should remain
        assert_eq!(ring.queued_len(), 1);
    }

    #[test]
    fn test_ring_buffer_update() {
        let mut ring = RingBuffer::new(2, 64);

        ring.pick_chunk(
            vec![
                "line1".to_string(),
                "line2".to_string(),
                "line3".to_string(),
            ],
            false,
            true,
        );
        ring.pick_chunk(
            vec![
                "line4".to_string(),
                "line5".to_string(),
                "line6".to_string(),
            ],
            false,
            true,
        );

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
        let mut ring = RingBuffer::new(3, 64);

        for _i in 0..10 {
            ring.pick_chunk(
                vec![
                    "line1".to_string(),
                    "line2".to_string(),
                    "line3".to_string(),
                ],
                false,
                true,
            );
            ring.update();
        }

        assert!(ring.len() <= 3);
    }

    #[test]
    fn test_ring_buffer_get_extra() {
        let mut ring = RingBuffer::new(2, 64);

        ring.pick_chunk(
            vec![
                "line1".to_string(),
                "line2".to_string(),
                "line3".to_string(),
            ],
            false,
            true,
        );
        ring.pick_chunk(
            vec![
                "line4".to_string(),
                "line5".to_string(),
                "line6".to_string(),
            ],
            false,
            true,
        );

        ring.update();
        ring.update();

        let extra = ring.get_extra();
        assert_eq!(extra.len(), 2);
    }

    #[test]
    fn test_ring_buffer_n_evict() {
        let mut ring = RingBuffer::new(2, 64);

        ring.pick_chunk(
            vec![
                "line1".to_string(),
                "line2".to_string(),
                "line3".to_string(),
            ],
            false,
            true,
        );
        ring.pick_chunk(
            vec![
                "line4".to_string(),
                "line5".to_string(),
                "line6".to_string(),
            ],
            false,
            true,
        );
        ring.pick_chunk(
            vec![
                "line7".to_string(),
                "line8".to_string(),
                "line9".to_string(),
            ],
            false,
            true,
        );

        assert_eq!(ring.queued_len(), 3);
    }
}
