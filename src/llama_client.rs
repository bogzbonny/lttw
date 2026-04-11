use {
    crate::{ring_buffer::ExtraContext, Error, FimTimings, LttwResult},
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

/// Send FIM request to the server
#[tracing::instrument]
pub async fn send_fim_request(
    request: &FimRequest,
    endpoint_fim: String,
    model_fim: String,
    api_key: String,
) -> LttwResult<String> {
    let client = reqwest::Client::new();

    let mut request_body = serde_json::to_value(request)?;

    // Add model if specified
    if !model_fim.is_empty() {
        request_body["model"] = serde_json::Value::String(model_fim.clone());
    }

    let mut builder = client.post(&endpoint_fim).json(&request_body);

    // Add API key if specified
    if !api_key.is_empty() {
        builder = builder.bearer_auth(&api_key);
    }

    let response = builder.send().await?;

    if response.status().is_success() {
        Ok(response.text().await?)
    } else {
        Err(Error::Server(format!(
            "Server returned status: {}",
            response.status()
        )))
    }
}

#[cfg(test)]
mod tests {
    use {super::*, crate::ring_buffer::RingBuffer};
    #[test]
    fn test_fim_request_serialization_with_extra() {
        // Test that FIM request properly serializes with extra context
        let mut ring_buffer = RingBuffer::new(2, 64, 16);

        // Add some chunks to the ring buffer
        ring_buffer.pick_chunk_inner(
            &[
                "mod module1;".to_string(),
                "mod module2;".to_string(),
                "mod module3;".to_string(),
            ],
            String::new(),
        );
        ring_buffer.update();

        let extra = ring_buffer.get_extra();

        let request = FimRequest {
            id_slot: 0,
            input_prefix: "fn main() {".to_string(),
            input_suffix: "}".to_string(),
            input_extra: extra,
            prompt: "    println!(\"hello\"".to_string(),
            n_predict: 32,
            stop: vec![],
            n_indent: 4,
            top_k: 40,
            top_p: 0.90,
            samplers: vec!["top_k".to_string(), "top_p".to_string()],
            t_max_prompt_ms: 500,
            t_max_predict_ms: 1000,
            response_fields: vec!["content".to_string()],
        };

        let json = serde_json::to_string(&request).expect("Request should serialize to JSON");
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("JSON should be parseable");

        // Verify input_extra contains the chunk data
        let extra_array = parsed["input_extra"].as_array().unwrap();
        assert_eq!(extra_array.len(), 1);
        assert!(extra_array[0].get("text").is_some());
    }

    #[test]
    fn test_fim_request_with_ring_buffer_extra() {
        // Test that FIM request properly includes extra context from ring buffer
        let ring_buffer = RingBuffer::new(2, 64, 16);

        let request = FimRequest {
            id_slot: 0,
            input_prefix: "fn main() {".to_string(),
            input_suffix: "}".to_string(),
            input_extra: ring_buffer.get_extra(),
            prompt: "    println!(\"hello\"".to_string(),
            n_predict: 32,
            stop: vec![],
            n_indent: 4,
            top_k: 40,
            top_p: 0.90,
            samplers: vec!["top_k".to_string(), "top_p".to_string()],
            t_max_prompt_ms: 500,
            t_max_predict_ms: 1000,
            response_fields: vec!["content".to_string()],
        };

        let json = serde_json::to_string(&request).expect("Request should serialize to JSON");
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("JSON should be parseable");

        // Verify input_extra is an empty array when ring buffer is empty
        assert_eq!(parsed["input_extra"].as_array().unwrap().len(), 0);
    }
}
