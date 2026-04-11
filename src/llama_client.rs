use {
    crate::{ring_buffer::ExtraContext, FimTimings},
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
    pub stop: Vec<String>,
    pub n_predict: u32,
    pub n_indent: usize,
    pub top_k: u32,
    pub top_p: f32,
    pub samplers: Vec<String>,
    pub t_max_prompt_ms: u32,
    pub t_max_predict_ms: u32,
    pub response_fields: Vec<String>,
}

//
// FIM completion channel types for async communication between worker and main thread
/// Timing information from FIM completion
#[derive(Debug, Clone, Default)]
pub struct FimTimingsData {
    pub n_prompt: i64,
    pub t_prompt_ms: f64,
    pub s_prompt: f64,
    pub n_predict: i64,
    pub t_predict_ms: f64,
    pub s_predict: f64,
    pub tokens_cached: u64,
    pub truncated: bool,
}

impl FimTimingsData {
    pub fn new(t: FimTimings, tokens_cached: u64, truncated: bool) -> Self {
        Self {
            n_prompt: t.prompt_n.unwrap_or(0),
            t_prompt_ms: t.prompt_ms.unwrap_or(0.0),
            s_prompt: t.prompt_per_second.unwrap_or(0.0),
            n_predict: t.predicted_n.unwrap_or(0),
            t_predict_ms: t.predicted_ms.unwrap_or(0.0),
            s_predict: t.predicted_per_second.unwrap_or(0.0),
            tokens_cached,
            truncated,
        }
    }
}

/// FIM completion response (uses flat keys from server)
#[derive(Debug, Clone, Deserialize)]
pub struct FimResponse {
    pub content: String,
    #[serde(flatten)]
    pub timings: Option<FimTimings>,
    #[serde(default)]
    pub tokens_cached: u64,
    #[serde(default)]
    pub truncated: bool,
}

// XXX to delete
///// FIM completion result with timing info
//#[derive(Debug, Clone, Serialize)]
//pub struct FimResult {
//    pub content: String,
//    pub can_accept: bool,
//    pub timings: Option<FimTimings>,
//    pub tokens_cached: u64,
//    pub truncated: bool,
//    pub info: Option<String>,
//}
