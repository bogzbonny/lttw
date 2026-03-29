// src/ring_buffer.rs - Ring buffer for extra context chunks
//
// This module implements a ring buffer that collects and manages chunks of
// text from the buffer to provide additional context to the language model.

use crate::context::chunk_similarity;
use serde::Serialize;

/// A chunk of text from the buffer
#[derive(Debug, Clone, Serialize)]
#[allow(dead_code)]
pub struct Chunk {
    pub data: Vec<String>,
    pub str: String,
    pub filename: String,
}

/// Ring buffer for extra context chunks
#[derive(Debug, Clone, Serialize)]
#[allow(dead_code)]
pub struct RingBuffer {
    chunks: Vec<Chunk>,
    pub queued: Vec<Chunk>,
    n_evict: usize,
    max_chunks: usize,
    chunk_size: usize,
}

impl RingBuffer {
    /// Create a new ring buffer with the given parameters
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
    pub fn queued_len(&self) -> usize {
        self.queued.len()
    }

    /// Check if the queue is empty
    #[allow(dead_code)]
    pub fn queue_is_empty(&self) -> bool {
        self.queued.is_empty()
    }

    /// Get the number of evicted chunks
    #[allow(dead_code)]
    pub fn n_evict(&self) -> usize {
        self.n_evict
    }

    /// Evict chunks from the ring buffer that are very similar to the given text
    ///
    /// # Arguments
    /// * `text` - Text to compare against
    /// * `threshold` - Similarity threshold (0.0-1.0). Chunks with similarity > threshold are evicted
    #[allow(dead_code)]
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

    /// Get context string for the ring buffer
    #[allow(dead_code)]
    pub fn get_context(&self, _buffer: u64, _position: usize) -> String {
        self.chunks
            .iter()
            .map(|c| c.str.clone())
            .collect::<Vec<String>>()
            .join("\n")
    }
}

/// Extra context for the server request
#[derive(Debug, Clone, serde::Serialize)]
#[allow(dead_code)]
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
