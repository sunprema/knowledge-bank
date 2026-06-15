//! notify-based folder watcher (PRD §9), foreground mode.
//!
//! Event model: any change to a paper folder's `notes.md`, `sections.md`,
//! or `metadata.json` (plus any removal event) schedules that paper for a
//! debounced sync. The sync itself is state-driven: if the folder still has
//! a metadata.json, the paper is (re)indexed from disk — the embedding
//! cache makes unchanged chunks free; if the folder is gone, the paper is
//! removed from both stores. This one rule covers all five PRD §9 cases.

use crate::config::{Config, KbPaths};
use crate::index::{MetaDb, VectorIndex};
use crate::ingest::pipeline::{self, index_paper_from_disk, log_line};
use crate::KbError;
use notify::{EventKind, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub async fn run(paths: KbPaths, config: Config) -> Result<(), KbError> {
    // Single-instance guard (PRD §14 "two watchers race"): pid file +
    // liveness probe instead of flock (keeps us dependency-free).
    if let Ok(pid_raw) = std::fs::read_to_string(paths.pid_path()) {
        let pid = pid_raw.trim();
        let alive = std::process::Command::new("ps")
            .args(["-p", pid])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if alive {
            return Err(KbError::Usage(format!(
                "another watcher is already running (pid {pid})"
            )));
        }
        let _ = std::fs::remove_file(paths.pid_path()); // stale
    }
    std::fs::write(paths.pid_path(), std::process::id().to_string())
        .map_err(|e| KbError::Index(format!("write kb.pid: {e}")))?;
    // Ensure pid file removal even on early error paths below.
    let _pid_guard = PidGuard(paths.pid_path());

    let db = MetaDb::open(&paths.meta_db_path())?;
    let mut index = VectorIndex::open_or_create(
        &paths.index_path(),
        config.embedding.dimensions,
        config.turbovec.bit_width,
    )?;

    // A long-running writer must not mix incompatible vectors into the
    // index (resolved config-drift decision).
    if let Some(stored) = db.meta_get("vector_fingerprint")?
        && stored != config.vector_fingerprint()
    {
        return Err(KbError::Index(format!(
            "embedding/index config changed ({} → {}) — run `kb reindex` before watching",
            stored,
            config.vector_fingerprint()
        )));
    }

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<notify::Event>();
    let mut watcher = notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
        if let Ok(ev) = res {
            let _ = tx.send(ev);
        }
    })
    .map_err(|e| KbError::Index(format!("create watcher: {e}")))?;
    watcher
        .watch(&paths.root, RecursiveMode::Recursive)
        .map_err(|e| KbError::Index(format!("watch {}: {e}", paths.root.display())))?;

    // Event paths arrive fully resolved (on macOS, FSEvents canonicalizes
    // `/var` → `/private/var`), so compare them against the canonical root,
    // not the possibly-symlinked configured path.
    let canon_root = std::fs::canonicalize(&paths.root).unwrap_or_else(|_| paths.root.clone());

    log_line(
        &paths,
        &format!("watcher started, monitoring {}", paths.root.display()),
    );
    eprintln!("watching {} (ctrl-c to stop)", paths.root.display());

    // Catch-up: folders that appeared while no watcher was running.
    let mut pending: HashSet<String> = HashSet::new();
    for id in paths.list_paper_ids()? {
        if db.chunks_for_paper(&id)?.is_empty() {
            pending.insert(id);
        }
    }

    // Inbox: a drop-folder for raw files. Create it (so it's discoverable)
    // and catch up on anything dropped while no watcher was running.
    let mut inbox_pending: HashSet<PathBuf> = HashSet::new();
    let canon_inbox = std::fs::canonicalize(paths.inbox_dir()).unwrap_or_else(|_| canon_root.join("inbox"));
    let canon_failed = canon_inbox.join("failed");
    if config.watcher.inbox_enabled {
        std::fs::create_dir_all(paths.inbox_dir())
            .map_err(|e| KbError::Index(format!("create inbox: {e}")))?;
        for entry in std::fs::read_dir(paths.inbox_dir()).into_iter().flatten().flatten() {
            let p = entry.path();
            if p.is_file() {
                inbox_pending.insert(p);
            }
        }
    }

    let debounce = Duration::from_millis(config.watcher.debounce_ms);
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|e| KbError::Index(format!("signal handler: {e}")))?;

    loop {
        tokio::select! {
            ev = rx.recv() => {
                match ev {
                    Some(ev) => {
                        if config.watcher.inbox_enabled {
                            for file in inbox_files_affected(&canon_inbox, &canon_failed, &ev) {
                                inbox_pending.insert(file);
                            }
                        }
                        for paper_id in papers_affected(&canon_root, &ev) {
                            pending.insert(paper_id);
                        }
                    }
                    None => break, // watcher thread gone
                }
            }
            // Recreated each iteration ⇒ fires only after `debounce` of
            // quiet (every new event restarts the wait). PRD §9: a save
            // followed by another within 2s triggers one re-embed. A file
            // still being copied into the inbox keeps the timer resetting,
            // so we only ingest once the copy settles.
            _ = tokio::time::sleep(debounce), if !pending.is_empty() || !inbox_pending.is_empty() => {
                // Inbox first: materializing a dropped file enqueues its new
                // paper id into `pending`, so it indexes in this same pass.
                for file in inbox_pending.drain().collect::<Vec<_>>() {
                    process_inbox_file(&paths, &mut pending, &file).await;
                }
                for paper_id in pending.drain().collect::<Vec<_>>() {
                    sync_paper(&paths, &config, &db, &mut index, &paper_id).await;
                }
            }
            _ = tokio::signal::ctrl_c() => break,
            _ = sigterm.recv() => break,
        }
    }

    // Flush in-flight work, save, exit (PRD §14 shutdown row).
    for file in inbox_pending.drain().collect::<Vec<_>>() {
        process_inbox_file(&paths, &mut pending, &file).await;
    }
    if !pending.is_empty() {
        let batch: Vec<String> = pending.drain().collect();
        for paper_id in batch {
            sync_paper(&paths, &config, &db, &mut index, &paper_id).await;
        }
    }
    log_line(&paths, "watcher stopped");
    eprintln!("watcher stopped");
    Ok(())
}

struct PidGuard(std::path::PathBuf);
impl Drop for PidGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Map one notify event to the paper folders it affects. Only content
/// files matter (notes.md / sections.md / metadata.json / idea.md) — PDF
/// and source/ churn is ignored (PRD §14: PDFs aren't the source of truth
/// for content). Removal events always count (folder deletion).
fn papers_affected(root: &Path, ev: &notify::Event) -> Vec<String> {
    let interesting_file = |p: &Path| {
        matches!(
            p.file_name().and_then(|n| n.to_str()),
            Some("notes.md") | Some("sections.md") | Some("metadata.json") | Some("idea.md")
        )
    };
    let is_removal = matches!(ev.kind, EventKind::Remove(_));

    let mut out = Vec::new();
    for path in &ev.paths {
        let Ok(rel) = path.strip_prefix(root) else { continue };
        let Some(first) = rel.components().next() else { continue };
        let folder = first.as_os_str().to_string_lossy();
        // `inbox` is the drop-folder, not a paper (handled separately).
        if folder == ".arxiv-kb" || folder == "inbox" || folder.starts_with('.') {
            continue;
        }
        // rel == just the folder (dir-level event) or a file inside it.
        let dir_level = rel.components().count() == 1;
        if is_removal || (dir_level && path.is_dir()) || interesting_file(path) {
            out.push(folder.into_owned());
        }
    }
    out
}

/// Files dropped directly into `inbox/` (not `inbox/failed/`). Removal
/// events are ignored — we react to files that exist, not ones that left.
/// `inbox`/`failed` must be canonical so they match resolved event paths.
fn inbox_files_affected(inbox: &Path, failed: &Path, ev: &notify::Event) -> Vec<PathBuf> {
    if matches!(ev.kind, EventKind::Remove(_)) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for path in &ev.paths {
        // Direct child of inbox/, a regular file, not under failed/.
        if path.parent() == Some(inbox) && !path.starts_with(failed) && path.is_file() {
            out.push(path.clone());
        }
    }
    out
}

/// Ingest one dropped inbox file, then delete it (success) or move it to
/// `inbox/failed/` (error). Only the canonical folder is written here; the
/// new paper id is queued in `pending` so the caller's long-lived index
/// embeds it. Dispatch is by extension: `.pdf` → local-PDF ingest;
/// `.url`/`.txt` → one URL per line. Other extensions are left untouched
/// (likely a temp or partial file). All failures are logged, never fatal.
async fn process_inbox_file(paths: &KbPaths, pending: &mut HashSet<String>, path: &Path) {
    if !path.is_file() {
        return; // already consumed by an earlier event in this batch
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());

    match ext.as_deref() {
        Some("pdf") => {
            match pipeline::materialize_local_pdf(paths, path, &|_| {}) {
                Ok(id) => {
                    log_line(paths, &format!("inbox: ingested PDF {} as {id}", display(path)));
                    pending.insert(id);
                    consume_inbox_file(paths, path);
                }
                Err(e) => {
                    log_line(paths, &format!("inbox: PDF {} failed: {e}", display(path)));
                    fail_inbox_file(paths, path);
                }
            }
        }
        Some("url") | Some("txt") => {
            let Ok(content) = std::fs::read_to_string(path) else {
                log_line(paths, &format!("inbox: cannot read {}", display(path)));
                fail_inbox_file(paths, path);
                return;
            };
            let urls = pipeline::parse_url_lines(&content);
            if urls.is_empty() {
                log_line(paths, &format!("inbox: no URLs in {}", display(path)));
                fail_inbox_file(paths, path);
                return;
            }
            let mut all_ok = true;
            for url in urls {
                match pipeline::materialize_url(paths, &url, false, &|_| {}).await {
                    Ok(id) => {
                        log_line(paths, &format!("inbox: ingested {url} as {id}"));
                        pending.insert(id);
                    }
                    Err(e) => {
                        all_ok = false;
                        log_line(paths, &format!("inbox: URL {url} failed: {e}"));
                    }
                }
            }
            if all_ok {
                consume_inbox_file(paths, path);
            } else {
                fail_inbox_file(paths, path);
            }
        }
        _ => {} // unknown extension: leave it alone
    }
}

fn display(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Delete a successfully-ingested source file.
fn consume_inbox_file(paths: &KbPaths, path: &Path) {
    if let Err(e) = std::fs::remove_file(path) {
        log_line(paths, &format!("inbox: could not delete {}: {e}", display(path)));
    }
}

/// Park a file that failed to ingest under `inbox/failed/` so it isn't
/// retried on every event and the user can inspect it.
fn fail_inbox_file(paths: &KbPaths, path: &Path) {
    let failed_dir = paths.inbox_failed_dir();
    if std::fs::create_dir_all(&failed_dir).is_err() {
        return; // best-effort; the file simply stays in inbox/
    }
    let dest = failed_dir.join(path.file_name().unwrap_or_default());
    if let Err(e) = std::fs::rename(path, &dest) {
        log_line(paths, &format!("inbox: could not move {} to failed/: {e}", display(path)));
    }
}

/// State-driven sync: folder has metadata.json ⇒ (re)index from disk;
/// folder gone ⇒ remove from both stores. Errors are logged, never fatal —
/// the watcher must outlive a bad folder or a flaky embedding call.
async fn sync_paper(
    paths: &KbPaths,
    config: &Config,
    db: &MetaDb,
    index: &mut VectorIndex,
    paper_id: &str,
) {
    if paths.metadata_path(paper_id).exists() {
        log_line(paths, &format!("detected change in {paper_id}, re-embedding"));
        let t0 = std::time::Instant::now();
        match index_paper_from_disk(paths, config, db, index, paper_id).await {
            Ok((chunks, cache_hits)) => {
                log_line(
                    paths,
                    &format!(
                        "re-embedded {paper_id}: {chunks} chunks, {cache_hits} from cache ({:.1}s)",
                        t0.elapsed().as_secs_f64()
                    ),
                );
                // Grow/refresh the associative layer for this paper (its chunk
                // ids just changed). Best-effort — never let it crash the watcher.
                match crate::cortex::connect_paper_with(paths, config, db, index, paper_id) {
                    Ok(n) if n > 0 => {
                        log_line(paths, &format!("cortex: {paper_id} formed {n} connections"))
                    }
                    Ok(_) => {}
                    Err(e) => log_line(paths, &format!("cortex connect for {paper_id} failed: {e}")),
                }
            }
            Err(e) => log_line(paths, &format!("re-embed of {paper_id} failed: {e}")),
        }
    } else {
        match remove_inline(paths, db, index, paper_id) {
            Ok(0) => {} // never indexed; nothing to do
            Ok(n) => log_line(
                paths,
                &format!("folder {paper_id} deleted, removed {n} chunks"),
            ),
            Err(e) => log_line(paths, &format!("removal of {paper_id} failed: {e}")),
        }
    }
}

/// Same sequence as [`pipeline::remove_paper_from_stores`] but against the
/// watcher's long-lived handles (a second in-memory index would diverge).
fn remove_inline(
    paths: &KbPaths,
    db: &MetaDb,
    index: &mut VectorIndex,
    paper_id: &str,
) -> Result<usize, KbError> {
    db.begin_immediate()?;
    let result = (|| -> Result<usize, KbError> {
        let removed = db.remove_paper(paper_id)?;
        if removed.is_empty() {
            return Ok(0);
        }
        for vid in &removed {
            index.remove(*vid as u64);
        }
        index.save_atomic(&paths.index_path())?;
        Ok(removed.len())
    })();
    match result {
        Ok(n) => {
            db.commit()?;
            Ok(n)
        }
        Err(e) => {
            let _ = db.rollback();
            Err(e)
        }
    }
}

// Quiet the unused-import warning until pipeline::remove_paper_from_stores
// gains another caller; keeping the reference documents the relationship.
#[allow(unused)]
fn _doc_ref() {
    let _ = pipeline::remove_paper_from_stores;
}
