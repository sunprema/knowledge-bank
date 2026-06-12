//! OpenAI `text-embedding-3-small` client (PRD §4 step 8).
//!
//! SECURITY: `OPENAI_API_KEY` must never appear in logs, error messages,
//! or debug output. The struct's Debug impl must not expose it.

use crate::KbError;
use serde::Deserialize;

/// Backoff schedule for 429/5xx responses (PRD §14): 1s, 2s, 4s — one
/// entry per retry, so max 3 retries (4 attempts total).
const DEFAULT_BACKOFF_MS: [u64; 3] = [1000, 2000, 4000];

pub struct OpenAiEmbedder {
    client: reqwest::Client,
    api_key: String,
    model: String,
    dimensions: usize,
    /// e.g. "https://api.openai.com/v1" — overridable for tests.
    base_url: String,
    /// Shrunk in tests so retry paths don't sleep for real.
    backoff_ms: [u64; 3],
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

#[derive(Deserialize)]
struct EmbeddingsResponse {
    data: Vec<EmbeddingItem>,
}

#[derive(Deserialize)]
struct EmbeddingItem {
    index: usize,
    embedding: Vec<f32>,
}

impl OpenAiEmbedder {
    /// Construct from `OPENAI_API_KEY`. Missing key ⇒ `Config` error
    /// (exit 10) telling the user to export it (without echoing anything).
    pub fn from_env(model: &str, dimensions: usize) -> Result<Self, KbError> {
        let api_key = std::env::var("OPENAI_API_KEY")
            .ok()
            .filter(|k| !k.trim().is_empty())
            .ok_or_else(|| {
                KbError::Config(
                    "OPENAI_API_KEY is not set; export it first, e.g. \
                     `export OPENAI_API_KEY=sk-...` (the key is never logged)"
                        .to_string(),
                )
            })?;
        Ok(Self::new(
            api_key,
            model,
            dimensions,
            "https://api.openai.com/v1".to_string(),
        ))
    }

    /// Test constructor with an explicit key and base URL (mock server).
    pub fn new(api_key: String, model: &str, dimensions: usize, base_url: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            model: model.to_string(),
            dimensions,
            base_url: base_url.trim_end_matches('/').to_string(),
            backoff_ms: DEFAULT_BACKOFF_MS,
        }
    }

    /// Test-only: same as [`Self::new`] but with a custom backoff schedule
    /// so retry tests don't sleep for seconds.
    #[cfg(test)]
    fn with_backoff_ms(
        api_key: String,
        model: &str,
        dimensions: usize,
        base_url: String,
        backoff_ms: [u64; 3],
    ) -> Self {
        let mut e = Self::new(api_key, model, dimensions, base_url);
        e.backoff_ms = backoff_ms;
        e
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
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let url = format!("{}/embeddings", self.base_url);
        let body = serde_json::json!({
            "model": self.model,
            "input": texts,
            "dimensions": self.dimensions,
        });

        let max_retries = self.backoff_ms.len();
        let mut last_status = None;
        for attempt in 0..=max_retries {
            let resp = self
                .client
                .post(&url)
                .bearer_auth(&self.api_key)
                .json(&body)
                .send()
                .await?; // transport error: Network via From<reqwest::Error>

            let status = resp.status();
            if status.is_success() {
                return self.parse_response(resp, texts.len()).await;
            }
            if status.as_u16() == 401 || status.as_u16() == 403 {
                // NEVER include the key (or any request/response content
                // that could echo it) in this message.
                return Err(KbError::Config(format!(
                    "invalid OPENAI_API_KEY (embedding API returned HTTP {}); \
                     check the key and re-export it",
                    status.as_u16()
                )));
            }
            if status.as_u16() == 429 || status.is_server_error() {
                last_status = Some(status.as_u16());
                if attempt < max_retries {
                    let delay = self.backoff_ms[attempt];
                    tracing::warn!(
                        status = status.as_u16(),
                        attempt = attempt + 1,
                        delay_ms = delay,
                        "embedding API transient failure, retrying"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                    continue;
                }
                break;
            }
            // Other 4xx: not retryable, not an auth problem.
            return Err(KbError::Network(format!(
                "embedding API returned HTTP {}",
                status.as_u16()
            )));
        }

        Err(KbError::Network(format!(
            "embedding API failed after {max_retries} retries (last HTTP status: {})",
            last_status.map_or_else(|| "unknown".to_string(), |s| s.to_string())
        )))
    }

    async fn parse_response(
        &self,
        resp: reqwest::Response,
        expected: usize,
    ) -> Result<Vec<Vec<f32>>, KbError> {
        let parsed: EmbeddingsResponse = resp.json().await.map_err(|e| {
            KbError::Network(format!("malformed embedding API response: {}", e.without_url()))
        })?;
        if parsed.data.len() != expected {
            return Err(KbError::Network(format!(
                "embedding API returned {} vectors for {} inputs",
                parsed.data.len(),
                expected
            )));
        }
        let mut items = parsed.data;
        items.sort_by_key(|i| i.index);
        // After sorting, indices must be exactly 0..expected.
        if let Some((pos, item)) = items.iter().enumerate().find(|(pos, i)| i.index != *pos) {
            return Err(KbError::Network(format!(
                "embedding API response has bad index {} at position {pos}",
                item.index
            )));
        }
        for (i, item) in items.iter().enumerate() {
            if item.embedding.len() != self.dimensions {
                return Err(KbError::Network(format!(
                    "embedding API returned a {}-dim vector for input {i}, expected {}",
                    item.embedding.len(),
                    self.dimensions
                )));
            }
        }
        Ok(items.into_iter().map(|i| i.embedding).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    const KEY: &str = "sk-TEST-SECRET-DO-NOT-LEAK";

    /// Tiny canned-response HTTP server on 127.0.0.1:0. Serves the given
    /// `(status, body)` responses one connection each, then exits. Returns
    /// the base_url and a handle yielding the captured request bodies.
    fn mock_server(
        responses: Vec<(u16, String)>,
    ) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let mut captured = Vec::new();
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                // Read headers.
                let mut buf = Vec::new();
                let mut byte = [0u8; 1];
                while !buf.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).unwrap();
                    buf.push(byte[0]);
                }
                let headers = String::from_utf8_lossy(&buf).to_string();
                // Read the body per Content-Length.
                let content_length: usize = headers
                    .lines()
                    .find_map(|l| {
                        let (name, value) = l.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().ok())?
                    })
                    .unwrap_or(0);
                let mut req_body = vec![0u8; content_length];
                stream.read_exact(&mut req_body).unwrap();
                captured.push(format!(
                    "{headers}{}",
                    String::from_utf8_lossy(&req_body)
                ));

                let reason = match status {
                    200 => "OK",
                    401 => "Unauthorized",
                    403 => "Forbidden",
                    429 => "Too Many Requests",
                    500 => "Internal Server Error",
                    _ => "Status",
                };
                let resp = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(resp.as_bytes()).unwrap();
                stream.flush().unwrap();
            }
            captured
        });
        (format!("http://127.0.0.1:{port}/v1"), handle)
    }

    fn ok_body(vectors: &[(usize, Vec<f32>)]) -> String {
        let data: Vec<serde_json::Value> = vectors
            .iter()
            .map(|(i, v)| serde_json::json!({"index": i, "embedding": v}))
            .collect();
        serde_json::json!({"data": data}).to_string()
    }

    fn embedder(base_url: String) -> OpenAiEmbedder {
        OpenAiEmbedder::with_backoff_ms(KEY.to_string(), "test-model", 3, base_url, [1, 1, 1])
    }

    #[tokio::test]
    async fn happy_path_returns_vectors_in_input_order() {
        // Response deliberately out of order: index 1 before index 0.
        let body = ok_body(&[(1, vec![4.0, 5.0, 6.0]), (0, vec![1.0, 2.0, 3.0])]);
        let (base, server) = mock_server(vec![(200, body)]);
        let e = embedder(base);
        let got = e.embed_batch(&["alpha", "beta"]).await.unwrap();
        assert_eq!(got, vec![vec![1.0, 2.0, 3.0], vec![4.0, 5.0, 6.0]]);

        // Request shape: model + input array + dimensions; bearer auth.
        let reqs = server.join().unwrap();
        assert_eq!(reqs.len(), 1);
        let req = &reqs[0];
        assert!(req.starts_with("POST /v1/embeddings HTTP/1.1\r\n"), "got: {req}");
        assert!(req.contains(&format!("Bearer {KEY}")));
        let json_start = req.find("\r\n\r\n").unwrap() + 4;
        let body: serde_json::Value = serde_json::from_str(&req[json_start..]).unwrap();
        assert_eq!(body["model"], "test-model");
        assert_eq!(body["input"], serde_json::json!(["alpha", "beta"]));
        assert_eq!(body["dimensions"], 3);
    }

    #[tokio::test]
    async fn empty_input_returns_empty_without_network() {
        // Point at a port with no listener: any HTTP attempt would error.
        let e = embedder("http://127.0.0.1:1/v1".to_string());
        let got = e.embed_batch(&[]).await.unwrap();
        assert!(got.is_empty());
    }

    #[tokio::test]
    async fn retries_on_429_then_succeeds() {
        let body = ok_body(&[(0, vec![1.0, 2.0, 3.0])]);
        let (base, server) = mock_server(vec![
            (429, r#"{"error":{"message":"rate limited"}}"#.to_string()),
            (200, body),
        ]);
        let e = embedder(base);
        let got = e.embed_batch(&["x"]).await.unwrap();
        assert_eq!(got, vec![vec![1.0, 2.0, 3.0]]);
        assert_eq!(server.join().unwrap().len(), 2, "exactly one retry");
    }

    #[tokio::test]
    async fn retries_on_5xx_then_succeeds() {
        let body = ok_body(&[(0, vec![1.0, 2.0, 3.0])]);
        let (base, server) = mock_server(vec![
            (500, "oops".to_string()),
            (500, "oops".to_string()),
            (200, body),
        ]);
        let e = embedder(base);
        let got = e.embed_batch(&["x"]).await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(server.join().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn gives_up_after_three_retries_with_network_error() {
        let responses = (0..4).map(|_| (500, "oops".to_string())).collect();
        let (base, server) = mock_server(responses);
        let e = embedder(base);
        let err = e.embed_batch(&["x"]).await.unwrap_err();
        match err {
            KbError::Network(msg) => {
                assert!(msg.contains("500"), "got: {msg}");
                assert!(!msg.contains(KEY));
            }
            other => panic!("expected Network, got {other:?}"),
        }
        assert_eq!(server.join().unwrap().len(), 4, "1 attempt + 3 retries");
    }

    #[tokio::test]
    async fn auth_failure_is_config_error_without_the_key() {
        for status in [401u16, 403] {
            let (base, server) =
                mock_server(vec![(status, r#"{"error":{"message":"bad key"}}"#.to_string())]);
            let e = embedder(base);
            let err = e.embed_batch(&["x"]).await.unwrap_err();
            match err {
                KbError::Config(msg) => {
                    assert!(msg.contains("OPENAI_API_KEY"), "got: {msg}");
                    assert!(!msg.contains(KEY), "key leaked: {msg}");
                    assert!(!msg.contains("sk-"), "key fragment leaked: {msg}");
                }
                other => panic!("expected Config, got {other:?}"),
            }
            server.join().unwrap();
        }
    }

    #[tokio::test]
    async fn auth_failure_is_not_retried() {
        // Only ONE canned response; a retry would hang on accept().
        let (base, server) = mock_server(vec![(401, "{}".to_string())]);
        let e = embedder(base);
        assert!(matches!(
            e.embed_batch(&["x"]).await,
            Err(KbError::Config(_))
        ));
        assert_eq!(server.join().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn vector_count_mismatch_is_an_error() {
        // 2 inputs, 1 vector back.
        let body = ok_body(&[(0, vec![1.0, 2.0, 3.0])]);
        let (base, server) = mock_server(vec![(200, body)]);
        let e = embedder(base);
        let err = e.embed_batch(&["a", "b"]).await.unwrap_err();
        match err {
            KbError::Network(msg) => {
                assert!(msg.contains('1') && msg.contains('2'), "got: {msg}")
            }
            other => panic!("expected Network, got {other:?}"),
        }
        server.join().unwrap();
    }

    #[tokio::test]
    async fn wrong_vector_length_is_an_error() {
        // dimensions = 3, server returns a 2-dim vector.
        let body = ok_body(&[(0, vec![1.0, 2.0])]);
        let (base, server) = mock_server(vec![(200, body)]);
        let e = embedder(base);
        let err = e.embed_batch(&["a"]).await.unwrap_err();
        assert!(matches!(err, KbError::Network(_)), "got {err:?}");
        server.join().unwrap();
    }

    #[tokio::test]
    async fn malformed_json_is_a_network_error() {
        let (base, server) = mock_server(vec![(200, "not json".to_string())]);
        let e = embedder(base);
        let err = e.embed_batch(&["a"]).await.unwrap_err();
        match err {
            KbError::Network(msg) => assert!(!msg.contains(KEY)),
            other => panic!("expected Network, got {other:?}"),
        }
        server.join().unwrap();
    }

    #[tokio::test]
    async fn duplicate_indices_are_rejected() {
        let body = ok_body(&[(0, vec![1.0, 2.0, 3.0]), (0, vec![4.0, 5.0, 6.0])]);
        let (base, server) = mock_server(vec![(200, body)]);
        let e = embedder(base);
        assert!(matches!(
            e.embed_batch(&["a", "b"]).await,
            Err(KbError::Network(_))
        ));
        server.join().unwrap();
    }

    #[test]
    fn debug_never_exposes_the_key() {
        let e = OpenAiEmbedder::new(
            KEY.to_string(),
            "test-model",
            3,
            "http://localhost/v1".to_string(),
        );
        let dbg = format!("{e:?}");
        assert!(!dbg.contains(KEY), "Debug leaked the key: {dbg}");
        assert!(dbg.contains("test-model"));
    }

    #[test]
    fn accessors() {
        let e = OpenAiEmbedder::new(KEY.to_string(), "m", 1536, "http://x/v1/".to_string());
        assert_eq!(e.model(), "m");
        assert_eq!(e.dimensions(), 1536);
        assert_eq!(e.base_url, "http://x/v1", "trailing slash trimmed");
    }

    // Env-var test: mutating the process environment is `unsafe` in Rust
    // 2024 and races with parallel tests, so this test never SETS the key —
    // it only exercises the missing-key path with a name that's absent.
    #[test]
    fn from_env_missing_key_is_config_error() {
        // Run in a scope where the var is guaranteed absent for this check.
        if std::env::var("OPENAI_API_KEY").is_ok() {
            // Key present in this environment — can't exercise the missing
            // path without unsafe env mutation; skip rather than race.
            return;
        }
        let err = OpenAiEmbedder::from_env("m", 8).unwrap_err();
        match err {
            KbError::Config(msg) => {
                assert!(msg.contains("OPENAI_API_KEY"), "got: {msg}");
                assert!(msg.contains("export"), "got: {msg}");
            }
            other => panic!("expected Config, got {other:?}"),
        }
    }
}
