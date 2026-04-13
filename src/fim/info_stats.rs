use {
    crate::fim::FimModel,
    serde::{Deserialize, Serialize},
};

/// FIM timing information (matches server response format with flat keys)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FimTimings {
    #[serde(rename = "timings/prompt_n")]
    pub prompt_n: Option<i64>,
    #[serde(rename = "timings/prompt_ms")]
    pub prompt_ms: Option<f64>,
    #[serde(rename = "timings/prompt_per_token_ms")]
    pub prompt_per_token_ms: Option<f64>,
    #[serde(rename = "timings/prompt_per_second")]
    pub prompt_per_second: Option<f64>,
    #[serde(rename = "timings/predicted_n")]
    pub predicted_n: Option<i64>,
    #[serde(rename = "timings/predicted_ms")]
    pub predicted_ms: Option<f64>,
    #[serde(rename = "timings/predicted_per_token_ms")]
    pub predicted_per_token_ms: Option<f64>,
    #[serde(rename = "timings/predicted_per_second")]
    pub predicted_per_second: Option<f64>,
}

/// Build info string from timing information
#[allow(clippy::too_many_arguments)] // Info display requires many parameters
#[tracing::instrument]
pub fn build_info_string(
    timings: &FimTimings,
    cached: bool,
    model: FimModel,
    tokens_cached: u64,
    truncated: bool,
    ring_chunks: usize,
    ring_n_chunks: usize,
    ring_n_evict: usize,
    ring_queued: usize,
    ring_queue_length: usize,
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
    let cached_str = if cached { "[CACHED] " } else { "" };

    // Build info string
    if truncated {
        format!(
            " | WARNING: the context is full: {}, increase the server context size or reduce g:lttw_config.ring_n_chunks",
            tokens_cached
        )
    } else {
        format!(
            "{}{}\n\
            tokens cached: {}\n\
            ring chunks: {}/{}\n\
            evicted from ring: {}\n\
            ring queue: {}/{}\n\
            cache size: {}/{}\n\
            new prompt tokens: {}\n({:.1} ms, {:.1} tok/s)\n\
            generated tokens: {}\n({:.1} ms, {:.1} tok/s)",
            cached_str,
            model,
            tokens_cached,
            ring_chunks,
            ring_n_chunks,
            ring_n_evict,
            ring_queued,
            ring_queue_length,
            cache_size,
            max_cache_keys,
            n_prompt,
            t_prompt_ms,
            s_prompt,
            n_predict,
            t_predict_ms,
            s_predict
        )
    }
}
