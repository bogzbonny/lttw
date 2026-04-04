// src/cache.rs - Cache management with LRU eviction
//
// This module implements a cache for FIM completions with LRU (Least Recently Used)
// eviction policy to manage memory usage and improve performance.

use {
    crate::{context::LocalContext, fim::FimResponse, utils::hash_input},
    ahash::{HashMap, HashMapExt},
    serde::{Deserialize, Serialize},
    std::collections::VecDeque,
};

/// Cache entry for FIM completions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    pub hash: String,
    pub data: String,
}

/// Cache with LRU eviction
#[derive(Debug, Clone)]
pub struct Cache {
    data: HashMap<String, FimResponse>,
    lru_order: VecDeque<String>,
    max_keys: usize,
}

impl Cache {
    /// Create a new cache with the given maximum number of keys
    pub fn new(max_keys: usize) -> Self {
        Self {
            data: HashMap::new(),
            lru_order: VecDeque::new(),
            max_keys,
        }
    }

    /// Insert a value into the cache
    /// Evicts the least recently used entry if the cache is full
    pub fn insert(&mut self, key: String, value: FimResponse) {
        // Check if we need to evict an entry
        if self.data.len() >= self.max_keys
            && let Some(lru_key) = self.lru_order.pop_front()
        {
            self.data.remove(&lru_key);
        }

        // Update the cache
        self.data.insert(key.clone(), value);

        // Update LRU order - remove key if it exists and add to end (most recent)
        self.lru_order.retain(|k| k != &key);
        self.lru_order.push_back(key);
    }

    /// Get a value from the cache and update LRU order
    pub fn get(&mut self, key: &str) -> Option<FimResponse> {
        if !self.data.contains_key(key) {
            return None;
        }

        // Update LRU order - remove key if it exists and add to end (most recent)
        self.lru_order.retain(|k| k != key);
        self.lru_order.push_back(key.to_string());

        self.data.get(key).cloned()
    }

    /// Check if a key exists in the cache
    pub fn contains_key(&self, key: &str) -> bool {
        self.data.contains_key(key)
    }

    /// Get the number of entries in the cache
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Check if the cache is empty
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Get a FIM response from the cache (without updating LRU order)
    pub fn get_fim(&self, key: &str) -> Option<FimResponse> {
        self.data.get(key).cloned()
    }

    /// Get the maximum number of keys
    #[cfg(test)]
    pub fn max_keys(&self) -> usize {
        self.max_keys
    }
}

/// Compute hashes for caching
pub fn compute_hashes(ctx: &LocalContext) -> Vec<String> {
    let max_hashes = 3; // TODO parameterize this
    let mut hashes = Vec::with_capacity(max_hashes + 1);

    // Primary hash
    let primary = format!("{}{}Î{}", ctx.prefix, ctx.middle, ctx.suffix);
    let hash = hash_input(&primary);
    hashes.push(hash);

    // Truncated prefix hashes (up to 3 levels)
    let mut prefix_trim = ctx.prefix.clone();
    let re = match regex::Regex::new(r"^[^\n]*\n") {
        Ok(r) => r,
        Err(_) => return hashes, // Return partial hashes on regex error
    };
    for _ in 0..max_hashes {
        prefix_trim = re.replace(&prefix_trim, "").to_string();
        if prefix_trim.is_empty() {
            break;
        }

        let inp = format!("{}{}Î{}", prefix_trim, ctx.middle, ctx.suffix);
        let hash = hash_input(&inp);
        hashes.push(hash);
    }

    hashes
}
