//! Config loading (`.arxiv-kb/config.toml`), env overrides, KB folder paths.
//!
//! Resolution order for the KB root: `--root` flag > `KB_ROOT` env >
//! `~/arxiv-kb`. Env vars override config values (PRD §10).

use crate::{KbError, SCHEMA_VERSION};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub schema_version: u32,
    pub embedding: EmbeddingConfig,
    pub turbovec: TurbovecConfig,
    pub search: SearchConfig,
    pub ingest: IngestConfig,
    pub server: ServerConfig,
    pub watcher: WatcherConfig,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            schema_version: SCHEMA_VERSION,
            embedding: EmbeddingConfig::default(),
            turbovec: TurbovecConfig::default(),
            search: SearchConfig::default(),
            ingest: IngestConfig::default(),
            server: ServerConfig::default(),
            watcher: WatcherConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbeddingConfig {
    pub provider: String,
    pub model: String,
    pub dimensions: usize,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        EmbeddingConfig {
            provider: "openai".into(),
            model: "text-embedding-3-small".into(),
            dimensions: 1536,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TurbovecConfig {
    pub bit_width: usize,
}

impl Default for TurbovecConfig {
    fn default() -> Self {
        TurbovecConfig { bit_width: 4 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchConfig {
    pub default_k_narrow: usize,
    pub default_k_wide: usize,
    pub default_min_score_narrow: f32,
    pub default_min_score_wide: f32,
}

impl Default for SearchConfig {
    fn default() -> Self {
        SearchConfig {
            default_k_narrow: 10,
            default_k_wide: 40,
            // Calibrated for text-embedding-3-small + 4-bit turbovec, whose
            // scores for clearly-relevant matches sit around 0.45-0.60 (the
            // PRD's original 0.72 hid everything — see smoke-test finding).
            default_min_score_narrow: 0.30,
            default_min_score_wide: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IngestConfig {
    pub chunk_max_tokens: usize,
    pub prefer_latex: bool,
    pub pandoc_path: String,
}

impl Default for IngestConfig {
    fn default() -> Self {
        IngestConfig {
            chunk_max_tokens: 2000,
            prefer_latex: true,
            pandoc_path: "pandoc".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub http_port: u16,
    pub http_bind: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            http_port: 4321,
            http_bind: "127.0.0.1".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WatcherConfig {
    pub debounce_ms: u64,
}

impl Default for WatcherConfig {
    fn default() -> Self {
        WatcherConfig { debounce_ms: 2000 }
    }
}

impl Config {
    /// Load `.arxiv-kb/config.toml`, creating the directory and a default
    /// config on first run. Refuses configs with a newer schema_version.
    pub fn load_or_create(paths: &KbPaths) -> Result<Self, KbError> {
        let dot = paths.dot_dir();
        std::fs::create_dir_all(&dot)
            .map_err(|e| KbError::Config(format!("cannot create {}: {e}", dot.display())))?;
        let path = paths.config_path();
        if !path.exists() {
            let cfg = Config::default();
            cfg.save(&path)?;
            return Ok(cfg);
        }
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| KbError::Config(format!("cannot read {}: {e}", path.display())))?;
        let cfg: Config = toml::from_str(&raw)
            .map_err(|e| KbError::Config(format!("malformed {}: {e}", path.display())))?;
        if cfg.schema_version > SCHEMA_VERSION {
            return Err(KbError::Config(format!(
                "config schema_version is {} but this binary supports {}; \
                 upgrade kb, or run `kb reindex` to rebuild at this version",
                cfg.schema_version, SCHEMA_VERSION
            )));
        }
        Ok(cfg)
    }

    pub fn save(&self, path: &Path) -> Result<(), KbError> {
        let body = toml::to_string_pretty(self)
            .map_err(|e| KbError::Config(format!("serialize config: {e}")))?;
        std::fs::write(path, body)
            .map_err(|e| KbError::Config(format!("write {}: {e}", path.display())))?;
        Ok(())
    }

    /// Fingerprint of settings that make existing *vectors* unusable when
    /// changed (resolved decision: mismatch ⇒ refuse queries until reindex).
    pub fn vector_fingerprint(&self) -> String {
        format!(
            "{}:{}:{}",
            self.embedding.model, self.embedding.dimensions, self.turbovec.bit_width
        )
    }

    /// Fingerprint of chunking-only settings (mismatch ⇒ warn but serve).
    pub fn chunking_fingerprint(&self) -> String {
        format!("chunk_max_tokens={}", self.ingest.chunk_max_tokens)
    }
}

/// All filesystem locations derived from the KB root (PRD §3 folder layout).
#[derive(Debug, Clone)]
pub struct KbPaths {
    pub root: PathBuf,
}

impl KbPaths {
    /// `--root` flag > `KB_ROOT` env > `~/arxiv-kb`.
    pub fn resolve(cli_root: Option<PathBuf>) -> Result<Self, KbError> {
        let root = match cli_root {
            Some(r) => r,
            None => match std::env::var_os("KB_ROOT") {
                Some(r) => PathBuf::from(r),
                None => dirs::home_dir()
                    .ok_or_else(|| KbError::Config("cannot determine home directory".into()))?
                    .join("arxiv-kb"),
            },
        };
        Ok(KbPaths { root })
    }

    pub fn dot_dir(&self) -> PathBuf {
        self.root.join(".arxiv-kb")
    }
    pub fn config_path(&self) -> PathBuf {
        self.dot_dir().join("config.toml")
    }
    pub fn index_path(&self) -> PathBuf {
        self.dot_dir().join("index.tv")
    }
    pub fn meta_db_path(&self) -> PathBuf {
        self.dot_dir().join("meta.db")
    }
    pub fn log_path(&self) -> PathBuf {
        self.dot_dir().join("kb.log")
    }
    pub fn pid_path(&self) -> PathBuf {
        self.dot_dir().join("kb.pid")
    }
    pub fn api_key_path(&self) -> PathBuf {
        self.dot_dir().join("api_key")
    }

    pub fn paper_dir(&self, arxiv_id: &str) -> PathBuf {
        self.root.join(arxiv_id)
    }
    pub fn metadata_path(&self, arxiv_id: &str) -> PathBuf {
        self.paper_dir(arxiv_id).join("metadata.json")
    }
    pub fn source_dir(&self, arxiv_id: &str) -> PathBuf {
        self.paper_dir(arxiv_id).join("source")
    }
    pub fn pdf_path(&self, arxiv_id: &str) -> PathBuf {
        self.paper_dir(arxiv_id).join("paper.pdf")
    }
    pub fn sections_path(&self, arxiv_id: &str) -> PathBuf {
        self.paper_dir(arxiv_id).join("sections.md")
    }
    pub fn notes_path(&self, arxiv_id: &str) -> PathBuf {
        self.paper_dir(arxiv_id).join("notes.md")
    }

    /// Paper folders = direct children of root containing a metadata.json
    /// (skips `.arxiv-kb` and anything else). Sorted for stable output.
    pub fn list_paper_ids(&self) -> Result<Vec<String>, KbError> {
        let mut ids = Vec::new();
        let entries = match std::fs::read_dir(&self.root) {
            Ok(e) => e,
            Err(_) => return Ok(ids), // no root yet = empty corpus
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir()
                && path.join("metadata.json").exists()
                && let Some(name) = path.file_name().and_then(|n| n.to_str())
            {
                ids.push(name.to_string());
            }
        }
        ids.sort();
        Ok(ids)
    }
}
