//! OpenAI `text-embedding-3-small` client (PRD §4 step 8).
//!
//! SECURITY: `OPENAI_API_KEY` must never appear in logs, error messages,
//! or debug output. The struct's Debug impl must not expose it.

use crate::KbError;

pub struct OpenAiEmbedder {
    client: reqwest::Client,
    api_key: String,
    model: String,
    dimensions: usize,
    /// e.g. "https://api.openai.com/v1" — overridable for tests.
    base_url: String,
}

impl std::fmt::Debug for OpenAiEmbedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiEmbedder")
            .field("model", &self.model)
            .field("dimensions", &self.dimensions)
            .field("base_url", &self.base_url)
            .finish_non_exhaustive() // api_key intentionally omitted
    }
}

impl OpenAiEmbedder {
    /// Construct from `OPENAI_API_KEY`. Missing key ⇒ `Config` error
    /// (exit 10) telling the user to export it (without echoing anything).
    pub fn from_env(model: &str, dimensions: usize) -> Result<Self, KbError> {
        let _ = (model, dimensions);
        todo!("implemented in the storage slice")
    }

    /// Test constructor with an explicit key and base URL (mock server).
    pub fn new(api_key: String, model: &str, dimensions: usize, base_url: String) -> Self {
        let _ = (api_key, model, dimensions, base_url);
        todo!("implemented in the storage slice")
    }

    pub fn model(&self) -> &str {
        &self.model
    }
    pub fn dimensions(&self) -> usize {
        self.dimensions
    }

    /// Embed a batch of texts in one `POST {base_url}/embeddings` call
    /// (body: `{ "model", "input": [...], "dimensions" }`). Returns one
    /// vector per input, in input order, each of length `self.dimensions`.
    ///
    /// Retry policy (PRD §14): on 429/5xx, exponential backoff (1s, 2s, 4s),
    /// max 3 retries, then `Network` (exit 3). 401/403 ⇒ `Config` error
    /// ("invalid OPENAI_API_KEY" — never include the key). Empty input
    /// returns Ok(vec![]) without a network call.
    pub async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, KbError> {
        let _ = texts;
        todo!("implemented in the storage slice")
    }
}
