// src/cache.rs - Cache management with LRU eviction
//
// This module implements a cache for FIM completions with LRU (Least Recently Used)
// eviction policy to manage memory usage and improve performance.

use {
    crate::{context::LocalContext, utils::hash_input, FimResponse, FimResponseWithInfo},
    ahash::{HashMap, HashMapExt, HashSet},
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
    data: HashMap<String, HashSet<FimResponseWithInfo>>,
    lru_order: VecDeque<String>,
    max_keys: usize,
}

impl Cache {
    /// Create a new cache with the given maximum number of keys
    #[tracing::instrument]
    pub fn new(max_keys: usize) -> Self {
        Self {
            data: HashMap::new(),
            lru_order: VecDeque::new(),
            max_keys,
        }
    }

    /// Insert a value into the cache
    /// Evicts the least recently used entry if the cache is full
    #[tracing::instrument]
    pub fn insert(&mut self, key: String, mut value: FimResponseWithInfo) {
        // Check if we need to evict an entry
        if self.data.len() >= self.max_keys
            && let Some(lru_key) = self.lru_order.pop_front()
        {
            self.data.remove(&lru_key);
        }

        // don't cache if empty response
        if value.resp.content.is_empty() {
            return;
        }

        value.cached = true;

        // Update the cache
        if let Some(v) = self.data.get_mut(&key) {
            // cached value exists, push
            v.insert(value);
        } else {
            // first time getting the value
            let mut set = HashSet::default();
            set.insert(value);

            self.data.insert(key.clone(), set);
        }

        // Update LRU order - remove key if it exists and add to end (most recent)
        self.lru_order.retain(|k| k != &key);
        self.lru_order.push_back(key);
    }

    /// Get a value from the cache and update LRU order
    #[tracing::instrument]
    pub fn get(&mut self, key: &str) -> Option<HashSet<FimResponseWithInfo>> {
        if !self.data.contains_key(key) {
            return None;
        }

        // Update LRU order - remove key if it exists and add to end (most recent)
        self.lru_order.retain(|k| k != key);
        self.lru_order.push_back(key.to_string());

        self.data.get(key).cloned()
    }

    /// Check if a key exists in the cache
    #[tracing::instrument]
    pub fn contains_key(&self, key: &str) -> bool {
        self.data.contains_key(key)
    }

    /// Get the number of entries in the cache
    #[tracing::instrument]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    #[tracing::instrument]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Get the maximum number of keys
    #[cfg(test)]
    pub fn max_keys(&self) -> usize {
        self.max_keys
    }

    // returns all the cache completions for the provided context, recaching them to a closer
    // position if found
    #[tracing::instrument]
    pub fn get_cached_completion(
        &mut self,
        ctx: &LocalContext,
    ) -> (Vec<FimResponseWithInfo>, usize) {
        // Compute primary hash
        let primary_hash_inp = format!("{}{}Î{}", ctx.prefix, ctx.middle, ctx.suffix);
        let hash = hash_input(&primary_hash_inp);

        // Check if the completion is cached (and update LRU order)
        let response = self.get(&hash);

        // the bool in all_completions is "recache"
        let mut completions_idx = 0;
        let mut all_completions: Vec<(FimResponseWithInfo, bool)> = Vec::new();
        let find_better_completion = if let Some(resp) = response {
            let comps: Vec<(FimResponseWithInfo, bool)> =
                resp.into_iter().map(|c| (c, false)).collect();
            all_completions.extend(comps);
            false
        } else {
            true
        };

        // ... or if there is a cached completion nearby (128 characters behind)
        // Looks at the previous 128 characters to see if a completion is cached.
        let pm = format!("{}{}", ctx.prefix, ctx.middle);
        let mut best_len = 0;

        // iterate through the prefix+midde string while removing characters from the tail
        //
        let mut char_indices = pm.char_indices().collect::<Vec<_>>();
        char_indices.push((pm.len(), '\0')); // needed for simplifying the loop logic, can be any char,
                                             // its never used
        let char_len = char_indices.len() - 1;

        let max_iters = 128; // TODO parameterize this
        for i in 1..=(max_iters.min(char_len.saturating_sub(1))) {
            let split_byte_idx = char_indices[char_len - i].0;
            let (pm_with_less_tail, removed) = pm.split_at(split_byte_idx);

            let new_prefix_middle = format!("{}Î{}", pm_with_less_tail, ctx.suffix);
            let hash_new = hash_input(&new_prefix_middle);

            if let Some(responses) = self.get(&hash_new) {
                for response_ in responses {
                    let content = &response_.resp.content;

                    // Check that the removed text matches the beginning of the cached response
                    // NOTE 'i' always is == removed.len()
                    // don't bother if i == content.len() because then there isn't any additional
                    // predicted text
                    if content.starts_with(removed) {
                        // Found a match - use the rest of the content
                        let Some(remaining) = content.strip_prefix(removed) else {
                            continue;
                        };

                        all_completions.push((
                            FimResponseWithInfo {
                                resp: FimResponse {
                                    content: remaining.to_string(),
                                    timings: response_.resp.timings,
                                    tokens_cached: response_.resp.tokens_cached,
                                    truncated: response_.resp.truncated,
                                },
                                cached: response_.cached,
                                model: response_.model,
                            },
                            true,
                        )); // recache = true

                        // could use chars().count() but it's not to important
                        if find_better_completion
                            && !remaining.is_empty()
                            && remaining.len() > best_len
                        {
                            best_len = remaining.len();
                            completions_idx = all_completions.len() - 1;
                        }
                    }
                }
            }
        }

        for all_completion in all_completions.iter() {
            let (resp, recache) = all_completion;
            // recache the re-found response at the new position - this way the response can still be found
            // if it was longer than 128 characters and the user is accepting this line by line.
            if *recache {
                // use the original ctx to compute the hashes
                let hashes = compute_hashes(&ctx.prefix, &ctx.middle, &ctx.suffix);
                for hash in &hashes {
                    self.insert(hash.clone(), resp.clone());
                }
            }
        }

        let completions = all_completions.into_iter().map(|(c, _)| c).collect();
        (completions, completions_idx)
    }
}

/// Compute hashes for caching
pub fn compute_hashes(prefix: &str, middle: &str, suffix: &str) -> Vec<String> {
    let max_hashes = 3; // TODO parameterize this
    let mut hashes = Vec::with_capacity(max_hashes + 1);

    // Primary hash
    let primary = format!("{}{}Î{}", prefix, middle, suffix);
    let hash = hash_input(&primary);
    hashes.push(hash);

    // Truncated prefix hashes (up to 3 levels)
    let mut prefix_trim = prefix.to_string();
    let re = match regex::Regex::new(r"^[^\n]*\n") {
        Ok(r) => r,
        Err(_) => return hashes, // Return partial hashes on regex error
    };
    for _ in 0..max_hashes {
        prefix_trim = re.replace(&prefix_trim, "").to_string();
        if prefix_trim.is_empty() {
            break;
        }

        let inp = format!("{}{}Î{}", prefix_trim, middle, suffix);
        let hash = hash_input(&inp);
        hashes.push(hash);
    }

    hashes
}

#[cfg(test)]
mod tests {
    use {super::*, crate::context::LocalContext};

    #[test]
    fn test_compute_hashes() {
        let ctx = LocalContext {
            prefix: "fn main() {\n".to_string(),
            middle: "    println!".to_string(),
            suffix: "(\"hello\");\n}\n".to_string(),
            ..Default::default()
        };

        let hashes = compute_hashes(&ctx.prefix, &ctx.middle, &ctx.suffix);

        assert!(!hashes.is_empty());
    }
}
