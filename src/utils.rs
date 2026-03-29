// src/utils.rs - Utility functions
//
// This module provides various utility functions used throughout the plugin.

use rand::Rng;

/// Generate a random number in the range [i0, i1]
pub fn random_range(i0: usize, i1: usize) -> usize {
    let mut rng = rand::thread_rng();
    rng.gen_range(i0..=i1)
}

/// Compute SHA256 hash of a string
pub fn sha256(input: &str) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(input.as_bytes());
    format!("{:x}", hash)
}

/// Split a string into lines, preserving empty lines
pub fn split_lines(s: &str) -> Vec<String> {
    s.split('\n').map(|s| s.to_string()).collect()
}

/// Join lines with newlines
pub fn join_lines(lines: &[String]) -> String {
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_random_range() {
        let value = random_range(1, 10);
        assert!((1..=10).contains(&value));
    }

    #[test]
    fn test_sha256() {
        let hash = sha256("hello");
        assert_eq!(hash.len(), 64); // SHA256 produces 64 hex characters
        assert_eq!(
            hash,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn test_split_lines() {
        let lines = split_lines("line1\nline2\nline3");
        assert_eq!(lines, vec!["line1", "line2", "line3"]);
    }

    #[test]
    fn test_join_lines() {
        let lines = vec![
            "line1".to_string(),
            "line2".to_string(),
            "line3".to_string(),
        ];
        assert_eq!(join_lines(&lines), "line1\nline2\nline3");
    }
}
