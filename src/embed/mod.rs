//! Embedding clients. v0.1: OpenAI only. v0.3 adds `local` (fastembed).

pub mod openai;

pub use openai::OpenAiEmbedder;
