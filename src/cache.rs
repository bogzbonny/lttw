// src/cache.rs - Cache management with LRU eviction
//
// This module implements a cache for FIM completions with LRU (Least Recently Used)
// eviction policy to manage memory usage and improve performance.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Cache entry for FIM completions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    pub hash: String,
    pub data: String,
}

/// Cache with LRU eviction
#[derive(Debug, Clone)]
pub struct Cache {
    data: std::collections::HashMap<String, String>,
    lru_order: VecDeque<String>,
    max_keys: usize,
}

impl Cache {
    /// Create a new cache with the given maximum number of keys
    pub fn new(max_keys: usize) -> Self {
        Self {
            data: std::collections::HashMap::new(),
            lru_order: VecDeque::new(),
            max_keys,
        }
    }

    /// Insert a value into the cache
    /// Evicts the least recently used entry if the cache is full
    pub fn insert(&mut self, key: String, value: String) {
        // Check if we need to evict an entry
        if self.data.len() >= self.max_keys {
            if let Some(lru_key) = self.lru_order.pop_front() {
                self.data.remove(&lru_key);
            }
        }

        // Update the cache
        self.data.insert(key.clone(), value);

        // Update LRU order - remove key if it exists and add to end (most recent)
        self.lru_order.retain(|k| k != &key);
        self.lru_order.push_back(key);
    }

    /// Get a value from the cache and update LRU order
    pub fn get(&mut self, key: &str) -> Option<String> {
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
    pub fn get_fim(&self, key: &str) -> Option<String> {
        self.data.get(key).cloned()
    }

    /// Get a FIM response from the cache and update LRU order
    pub fn get_fim_mut(&mut self, key: &str) -> Option<String> {
        self.get(key)
    }

    /// Cache a FIM response
    pub fn cache_fim(&mut self, key: &str, response: &str) {
        self.insert(key.to_string(), response.to_string());
    }

    /// Get the maximum number of keys
    #[cfg(test)]
    pub fn max_keys(&self) -> usize {
        self.max_keys
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_basic() {
        let mut cache = Cache::new(3);

        cache.insert("key1".to_string(), "value1".to_string());
        cache.insert("key2".to_string(), "value2".to_string());

        assert_eq!(cache.get("key1"), Some("value1".to_string()));
        assert_eq!(cache.get("key2"), Some("value2".to_string()));
        assert_eq!(cache.get("key3"), None);

        #[cfg(test)]
        {
            assert_eq!(cache.len(), 2);
            assert!(!cache.is_empty());
            assert_eq!(cache.max_keys(), 3);
        }
    }

    #[test]
    fn test_cache_lru_eviction() {
        let mut cache = Cache::new(3);

        cache.insert("key1".to_string(), "value1".to_string());
        cache.insert("key2".to_string(), "value2".to_string());
        cache.insert("key3".to_string(), "value3".to_string());

        // Access key1 to make it most recently used
        cache.get("key1");

        // Insert key4, should evict key2 (least recently used)
        cache.insert("key4".to_string(), "value4".to_string());

        assert_eq!(cache.get("key1"), Some("value1".to_string()));
        assert_eq!(cache.get("key2"), None); // Evicted
        assert_eq!(cache.get("key3"), Some("value3".to_string()));
        assert_eq!(cache.get("key4"), Some("value4".to_string()));
    }

    #[test]
    fn test_cache_update_existing() {
        let mut cache = Cache::new(3);

        cache.insert("key1".to_string(), "value1".to_string());
        cache.insert("key1".to_string(), "value1_updated".to_string());

        assert_eq!(cache.get("key1"), Some("value1_updated".to_string()));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn test_cache_insert_and_get() {
        let mut cache = Cache::new(10);

        cache.insert("key1".to_string(), "value1".to_string());
        cache.insert("key2".to_string(), "value2".to_string());

        assert_eq!(cache.get("key1"), Some("value1".to_string()));
        assert_eq!(cache.get("key2"), Some("value2".to_string()));
        assert_eq!(cache.get("key3"), None);
    }

    #[test]
    fn test_cache_max_keys() {
        let mut cache = Cache::new(5);

        for _i in 0..10 {
            cache.insert(format!("key{}", _i), format!("value{}", _i));
        }

        assert!(cache.len() <= 5);
    }

    #[test]
    fn test_cache_contains_key() {
        let mut cache = Cache::new(10);

        cache.insert("key1".to_string(), "value1".to_string());

        assert!(cache.contains_key("key1"));
        assert!(!cache.contains_key("key2"));
    }

    #[test]
    fn test_cache_len() {
        let mut cache = Cache::new(10);

        assert_eq!(cache.len(), 0);

        cache.insert("key1".to_string(), "value1".to_string());
        assert_eq!(cache.len(), 1);

        cache.insert("key2".to_string(), "value2".to_string());
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn test_cache_is_empty() {
        let mut cache = Cache::new(10);

        assert!(cache.is_empty());

        cache.insert("key1".to_string(), "value1".to_string());
        assert!(!cache.is_empty());
    }

    #[test]
    fn test_cache_update_existing_key() {
        let mut cache = Cache::new(10);

        cache.insert("key1".to_string(), "value1".to_string());
        cache.insert("key1".to_string(), "value1_updated".to_string());

        assert_eq!(cache.get("key1"), Some("value1_updated".to_string()));
        assert_eq!(cache.len(), 1);
    }
}
