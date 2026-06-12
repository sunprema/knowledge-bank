//! Error type with the PRD §6 exit-code contract.

use thiserror::Error;

/// Every error maps to one of the documented exit codes:
///
/// | code | variant     | meaning                                  |
/// |------|-------------|------------------------------------------|
/// | 1    | Usage       | bad arguments                            |
/// | 2    | NotFound    | paper, chunk, etc. not found             |
/// | 3    | Network     | arXiv API or embedding API failure       |
/// | 4    | Extraction  | pandoc failed, PDF malformed             |
/// | 5    | Index       | turbovec / meta.db / persistence failure |
/// | 10   | Config      | configuration error                      |
///
/// Never include `OPENAI_API_KEY` (or any secret) in error messages.
#[derive(Debug, Error)]
pub enum KbError {
    #[error("{0}")]
    Usage(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Network(String),
    #[error("{0}")]
    Extraction(String),
    #[error("{0}")]
    Index(String),
    #[error("{0}")]
    Config(String),
}

impl KbError {
    pub fn exit_code(&self) -> i32 {
        match self {
            KbError::Usage(_) => 1,
            KbError::NotFound(_) => 2,
            KbError::Network(_) => 3,
            KbError::Extraction(_) => 4,
            KbError::Index(_) => 5,
            KbError::Config(_) => 10,
        }
    }
}

impl From<reqwest::Error> for KbError {
    fn from(e: reqwest::Error) -> Self {
        // reqwest errors redact URLs' userinfo but be defensive anyway:
        // no header/body content is ever embedded in these messages.
        KbError::Network(e.without_url().to_string())
    }
}

impl From<rusqlite::Error> for KbError {
    fn from(e: rusqlite::Error) -> Self {
        KbError::Index(format!("meta.db: {e}"))
    }
}
