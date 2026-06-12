//! Implementations behind each CLI subcommand (PRD §6). main.rs stays a
//! thin clap dispatcher; everything testable lives here.

use crate::config::{Config, KbPaths};
use crate::search::{SearchFilters, SearchMode};
use crate::KbError;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Pretty,
    Json,
}

/// Shared context every command opens first.
pub struct Kb {
    pub paths: KbPaths,
    pub config: Config,
    pub format: OutputFormat,
}

impl Kb {
    pub fn open(root: Option<PathBuf>, format: OutputFormat) -> Result<Self, KbError> {
        let paths = KbPaths::resolve(root)?;
        let config = Config::load_or_create(&paths)?;
        Ok(Kb {
            paths,
            config,
            format,
        })
    }
}

fn not_yet(cmd: &str) -> KbError {
    KbError::Usage(format!("`kb {cmd}` is not implemented yet (v0.1 build in progress)"))
}

fn planned(cmd: &str, version: &str) -> KbError {
    KbError::Usage(format!("`kb {cmd}` is planned for {version}"))
}

pub fn init(kb: &Kb) -> Result<(), KbError> {
    // Config::load_or_create already created .arxiv-kb/ + config.toml.
    println!("initialized KB at {}", kb.paths.root.display());
    println!("config: {}", kb.paths.config_path().display());
    Ok(())
}

pub async fn add(kb: &Kb, id_or_url: Option<String>, pdf: Option<PathBuf>) -> Result<(), KbError> {
    let _ = (kb, id_or_url, pdf);
    Err(not_yet("add"))
}

pub async fn update(kb: &Kb, arxiv_id: String) -> Result<(), KbError> {
    let _ = (kb, arxiv_id);
    Err(not_yet("update"))
}

pub async fn remove(kb: &Kb, arxiv_id: String, yes: bool) -> Result<(), KbError> {
    let _ = (kb, arxiv_id, yes);
    Err(not_yet("remove"))
}

pub fn note(kb: &Kb, arxiv_id: String) -> Result<(), KbError> {
    let _ = (kb, arxiv_id);
    Err(not_yet("note"))
}

pub fn tag(kb: &Kb, arxiv_id: String, tags: Vec<String>) -> Result<(), KbError> {
    let _ = (kb, arxiv_id, tags);
    Err(not_yet("tag"))
}

#[allow(clippy::too_many_arguments)]
pub async fn search(
    kb: &Kb,
    query: String,
    mode: SearchMode,
    k: Option<usize>,
    filters: SearchFilters,
) -> Result<(), KbError> {
    let _ = (kb, query, mode, k, filters);
    Err(not_yet("search"))
}

pub fn list(kb: &Kb, tag: Option<String>) -> Result<(), KbError> {
    let _ = (kb, tag);
    Err(not_yet("list"))
}

pub fn show(kb: &Kb, arxiv_id: String) -> Result<(), KbError> {
    let _ = (kb, arxiv_id);
    Err(not_yet("show"))
}

pub async fn similar(kb: &Kb, arxiv_id: String) -> Result<(), KbError> {
    let _ = (kb, arxiv_id);
    Err(planned("similar", "v0.2"))
}

pub fn open_target(kb: &Kb, target: String, section: Option<String>) -> Result<(), KbError> {
    let _ = (kb, target, section);
    Err(not_yet("open"))
}

pub fn excerpt(kb: &Kb, chunk_ids: Vec<String>, out: PathBuf) -> Result<(), KbError> {
    let _ = (kb, chunk_ids, out);
    Err(planned("excerpt", "v0.2"))
}

pub fn stats(kb: &Kb) -> Result<(), KbError> {
    let _ = kb;
    Err(not_yet("stats"))
}

pub fn status(kb: &Kb) -> Result<(), KbError> {
    let _ = kb;
    Err(not_yet("status"))
}

pub fn verify(kb: &Kb, deep: bool) -> Result<(), KbError> {
    let _ = (kb, deep);
    Err(not_yet("verify"))
}

pub async fn reindex(kb: &Kb, yes: bool) -> Result<(), KbError> {
    let _ = (kb, yes);
    Err(not_yet("reindex"))
}

pub fn gc(kb: &Kb) -> Result<(), KbError> {
    let _ = kb;
    Err(not_yet("gc"))
}

pub fn cache_clear(kb: &Kb) -> Result<(), KbError> {
    let _ = kb;
    Err(not_yet("cache clear"))
}

pub fn cache_gc(kb: &Kb) -> Result<(), KbError> {
    let _ = kb;
    Err(not_yet("cache gc"))
}

pub async fn watch(kb: Kb, daemon: bool) -> Result<(), KbError> {
    if daemon {
        return Err(planned("watch --daemon", "v0.2"));
    }
    crate::watcher::fs_watcher::run(kb.paths, kb.config).await
}

pub async fn mcp(kb: Kb) -> Result<(), KbError> {
    crate::server::mcp::run(kb.paths, kb.config).await
}

pub async fn serve(kb: &Kb, port: u16) -> Result<(), KbError> {
    let _ = (kb, port);
    Err(planned("serve", "v0.2"))
}
