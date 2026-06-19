//! Ingest orchestration (PRD §4 data flow + addendum §5 write sequence).
//!
//! All network work happens before the two-store commit; the commit order
//! is: meta.db BEGIN → inserts → index add (memory) → index.tv atomic
//! write → COMMIT. A crash anywhere leaves a state the startup consistency
//! check (addendum §7) can recover from.

use crate::config::{Config, KbPaths};
use crate::embed::OpenAiEmbedder;
use crate::index::{MetaDb, NewChunk, VectorIndex};
use crate::ingest::latex::EprintContent;
use crate::ingest::{arxiv, html, latex, ocr, pdf, sections};
use crate::{
    content_hash, make_snippet, now_rfc3339, DocKind, KbError, PaperMetadata, RawChunk,
    SectionType, SourceFormat, EMBEDDING_VERSION,
};
use std::path::Path;
use std::time::Instant;

/// Outcome summary for logging and CLI display.
#[derive(Debug)]
pub struct IngestReport {
    pub paper_id: String,
    pub title: String,
    pub chunks: usize,
    pub cache_hits: usize,
    pub source_format: SourceFormat,
    pub elapsed_secs: f64,
}

fn http_client() -> Result<reqwest::Client, KbError> {
    reqwest::Client::builder()
        .user_agent(concat!("arxiv-kb/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| KbError::Network(format!("http client: {e}")))
}

fn io_err(context: &str, e: std::io::Error) -> KbError {
    KbError::Index(format!("{context}: {e}"))
}

/// Full `kb add` / `kb update` flow for one arXiv id or URL.
/// `refetch: false` refuses ids that already exist (PRD §14: racing adds
/// no-op); `refetch: true` re-downloads everything and re-embeds
/// (preserving tags, which are canonical in metadata.json).
pub async fn ingest_paper(
    paths: &KbPaths,
    config: &Config,
    input: &str,
    refetch: bool,
    progress: &dyn Fn(&str),
) -> Result<IngestReport, KbError> {
    let t0 = Instant::now();
    let (id, _requested_version) = arxiv::parse_arxiv_id(input)?;

    if paths.metadata_path(&id).exists() && !refetch {
        return Err(KbError::Usage(format!(
            "{id} is already in the KB (use `kb update {id}` to re-fetch)"
        )));
    }

    let client = http_client()?;

    progress("fetching metadata from arXiv");
    let mut meta = arxiv::fetch_metadata(&client, &id).await?;
    if refetch
        && let Ok(old) = PaperMetadata::load(&paths.metadata_path(&id))
    {
        meta.tags = old.tags; // tags are user-owned; survive re-fetch
    }

    let paper_dir = paths.paper_dir(&id);
    std::fs::create_dir_all(&paper_dir).map_err(|e| io_err("create paper folder", e))?;

    // LaTeX source path (PRD §4 steps 2-3); failures degrade to PDF.
    let mut source_format = SourceFormat::Pdf;
    let mut main_tex: Option<String> = None;
    if config.ingest.prefer_latex {
        progress("downloading LaTeX source");
        match latex::download_and_extract_source(&client, &id, &paths.source_dir(&id)).await {
            Ok(EprintContent::Latex { main_tex: mt }) => {
                // Pandoc missing is a hard Config error (exit 10, PRD §14),
                // not a silent degradation — the user should install it.
                latex::check_pandoc(&config.ingest.pandoc_path)?;
                progress("converting LaTeX → markdown via pandoc");
                let main_abs = paths.source_dir(&id).join(&mt);
                match latex::run_pandoc(
                    &config.ingest.pandoc_path,
                    &main_abs,
                    &paths.sections_path(&id),
                )
                .await
                {
                    Ok(()) => {
                        source_format = SourceFormat::Latex;
                        main_tex = Some(mt.to_string_lossy().into_owned());
                    }
                    Err(e) => {
                        tracing::warn!("pandoc failed ({e}); falling back to PDF extraction");
                    }
                }
            }
            Ok(EprintContent::PdfOnly) => {
                tracing::info!("e-print for {id} has no LaTeX; using PDF path");
            }
            Err(e) => {
                tracing::warn!("e-print download failed ({e}); using PDF path");
            }
        }
    }

    progress("downloading PDF");
    pdf::download_pdf(&client, &id, &paths.pdf_path(&id)).await?;

    if source_format == SourceFormat::Pdf {
        progress("extracting text from PDF (fallback path)");
        write_sections_from_pdf(&paths.pdf_path(&id), &paths.sections_path(&id))?;
    }

    meta.source_format = source_format;
    meta.main_tex = main_tex;
    meta.save(&paths.metadata_path(&id))?;

    ensure_notes_template(paths, &id, &meta.title)?;

    index_and_report(paths, config, &id, &meta, t0, progress).await
}

/// `kb add --pdf` flow: ingest a local PDF with a filename-derived slug as
/// its paper id. No arXiv round-trips — metadata is whatever the PDF's Info
/// dictionary offers (title), with the filename as fallback.
pub async fn ingest_local_pdf(
    paths: &KbPaths,
    config: &Config,
    pdf_file: &Path,
    progress: &dyn Fn(&str),
) -> Result<IngestReport, KbError> {
    let t0 = Instant::now();
    let id = materialize_local_pdf(paths, pdf_file, progress)?;
    let meta = PaperMetadata::load(&paths.metadata_path(&id))?;
    index_and_report(paths, config, &id, &meta, t0, progress).await
}

/// Write a local PDF's canonical folder to disk (paper.pdf, sections.md,
/// metadata.json, notes template) and return its id — WITHOUT touching the
/// vector index. Split out so the folder-watcher's inbox can materialize a
/// paper and let its own long-lived index handle the indexing (a second
/// index handle would diverge from the watcher's; see `fs_watcher`).
pub fn materialize_local_pdf(
    paths: &KbPaths,
    pdf_file: &Path,
    progress: &dyn Fn(&str),
) -> Result<String, KbError> {
    let stem = pdf_file
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| {
            KbError::Usage(format!("cannot derive an id from {}", pdf_file.display()))
        })?;
    let id = slug_from_filename(stem)?;

    if paths.metadata_path(&id).exists() {
        return Err(KbError::Usage(format!(
            "{id} is already in the KB (run `kb remove {id}` first to re-ingest)"
        )));
    }

    // Validate the source before touching the KB so a bad path leaves
    // no half-created paper folder behind.
    let bytes = pdf::read_local_pdf(pdf_file)?;

    let paper_dir = paths.paper_dir(&id);
    std::fs::create_dir_all(&paper_dir).map_err(|e| io_err("create paper folder", e))?;

    progress("copying PDF into the KB");
    pdf::write_pdf_atomic(&bytes, &paths.pdf_path(&id))?;

    progress("extracting text from PDF");
    write_sections_from_pdf(&paths.pdf_path(&id), &paths.sections_path(&id))?;

    let title = pdf::extract_info_title(&paths.pdf_path(&id))
        .unwrap_or_else(|| stem.replace(['-', '_'], " ").trim().to_string());

    let meta = PaperMetadata {
        arxiv_id: id.clone(),
        kind: DocKind::Paper,
        project: None,
        links: Vec::new(),
        version: None,
        title,
        authors: Vec::new(),
        abstract_text: String::new(),
        categories: Vec::new(),
        published_at: String::new(),
        updated_at: String::new(),
        ingested_at: now_rfc3339(),
        source_format: SourceFormat::Pdf,
        source_url: None,
        main_tex: None,
        tags: Vec::new(),
        schema_version: crate::SCHEMA_VERSION,
    };
    meta.save(&paths.metadata_path(&id))?;

    ensure_notes_template(paths, &id, &meta.title)?;
    Ok(id)
}

/// `kb add --url` flow: fetch a web page, extract its main article with a
/// readability port, and ingest the markdown like a paper's `sections.md`.
/// The on-disk id is a slug of the URL; the URL itself is recorded in
/// metadata as the document's canonical identity. `refetch: false` refuses
/// a URL already in the KB; `refetch: true` re-downloads and re-embeds
/// (preserving user-owned tags and notes.md).
pub async fn ingest_url(
    paths: &KbPaths,
    config: &Config,
    input: &str,
    refetch: bool,
    progress: &dyn Fn(&str),
) -> Result<IngestReport, KbError> {
    let t0 = Instant::now();
    let id = materialize_url(paths, input, refetch, progress).await?;

    progress("embedding and indexing");
    let db = MetaDb::open(&paths.meta_db_path())?;
    let mut index = VectorIndex::open_or_create(
        &paths.index_path(),
        config.embedding.dimensions,
        config.turbovec.bit_width,
    )?;
    let (n_chunks, cache_hits) = index_paper_from_disk(paths, config, &db, &mut index, &id).await?;
    connect_cortex(paths, config, &db, &index, &id);
    let meta = PaperMetadata::load(&paths.metadata_path(&id))?;

    let elapsed = t0.elapsed().as_secs_f64();
    log_line(
        paths,
        &format!("ingested {id} from {input}: {n_chunks} chunks ({elapsed:.1}s)"),
    );

    Ok(IngestReport {
        paper_id: id,
        title: meta.title,
        chunks: n_chunks,
        cache_hits,
        source_format: SourceFormat::Html,
        elapsed_secs: elapsed,
    })
}

/// Fetch a page and write its canonical folder (sections.md, metadata.json,
/// notes template) to disk, returning the id — WITHOUT touching the vector
/// index. Counterpart to [`materialize_local_pdf`] for the inbox watcher.
pub async fn materialize_url(
    paths: &KbPaths,
    input: &str,
    refetch: bool,
    progress: &dyn Fn(&str),
) -> Result<String, KbError> {
    let url = html::parse_url(input)?;
    let id = html::slug_from_url(&url);

    let existing = PaperMetadata::load(&paths.metadata_path(&id)).ok();
    if existing.is_some() && !refetch {
        return Err(KbError::Usage(format!(
            "{url} is already in the KB as {id} (use `kb update {id}` to re-fetch)"
        )));
    }

    let client = http_client()?;
    progress("fetching page");
    let body = html::fetch_html(&client, &url).await?;
    progress("extracting article (readability)");
    let (title, markdown) = html::extract_article(&body, &url)?;

    let paper_dir = paths.paper_dir(&id);
    std::fs::create_dir_all(&paper_dir).map_err(|e| io_err("create paper folder", e))?;
    std::fs::write(paths.sections_path(&id), markdown + "\n")
        .map_err(|e| io_err("write sections.md", e))?;

    let now = now_rfc3339();
    let meta = PaperMetadata {
        arxiv_id: id.clone(),
        kind: DocKind::Paper,
        project: None,
        links: Vec::new(),
        version: None,
        title,
        authors: Vec::new(),
        abstract_text: String::new(),
        categories: Vec::new(),
        published_at: String::new(),
        updated_at: now.clone(),
        ingested_at: existing.as_ref().map_or(now, |old| old.ingested_at.clone()),
        source_format: SourceFormat::Html,
        source_url: Some(url.to_string()),
        main_tex: None,
        // Tags are user-owned and canonical here; survive a re-fetch.
        tags: existing.map(|old| old.tags).unwrap_or_default(),
        schema_version: crate::SCHEMA_VERSION,
    };
    meta.save(&paths.metadata_path(&id))?;

    ensure_notes_template(paths, &id, &meta.title)?;
    Ok(id)
}

/// Parse a `*.url`/`*.txt` inbox file into a list of URLs: one per line,
/// skipping blanks and `#` comments. (Both extensions use the same rule —
/// a single-line `.url` is just the one-URL case.)
pub fn parse_url_lines(content: &str) -> Vec<String> {
    content
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// One `kb idea add` / MCP `kb_capture_idea` payload.
#[derive(Debug, Clone)]
pub struct IdeaSpec {
    /// Explicit id (MCP `upsert_key`); derived from the title when absent.
    pub slug: Option<String>,
    pub project: String,
    pub title: String,
    pub body: String,
    /// Empty = keep existing tags on an upsert.
    pub tags: Vec<String>,
    /// Empty = keep existing links on an upsert.
    pub links: Vec<String>,
}

/// Capture (or upsert) a standalone idea: write the canonical folder
/// (metadata.json + idea.md), then chunk + embed + index it. Re-capturing
/// an existing id updates in place — no duplicate.
pub async fn ingest_idea(
    paths: &KbPaths,
    config: &Config,
    spec: &IdeaSpec,
    progress: &dyn Fn(&str),
) -> Result<IngestReport, KbError> {
    let t0 = Instant::now();
    let id = match &spec.slug {
        Some(s) => {
            let normalized = slug_from_filename(s)?;
            if normalized != *s {
                return Err(KbError::Usage(format!(
                    "'{s}' is not a valid idea id (did you mean '{normalized}'?)"
                )));
            }
            normalized
        }
        None => slug_from_filename(&spec.title)?,
    };
    if spec.project.trim().is_empty() {
        return Err(KbError::Usage("project must not be empty".to_string()));
    }
    if spec.body.trim().is_empty() {
        return Err(KbError::Usage("idea body must not be empty".to_string()));
    }

    // Upsert: an existing note with this id is updated; an existing PAPER
    // with this id is a collision, not an update target.
    let existing = PaperMetadata::load(&paths.metadata_path(&id)).ok();
    if let Some(old) = &existing
        && old.kind != DocKind::Note
    {
        return Err(KbError::Usage(format!(
            "{id} already names a paper in the KB; pick a different idea title or id"
        )));
    }

    let now = now_rfc3339();
    let meta = PaperMetadata {
        arxiv_id: id.clone(),
        kind: DocKind::Note,
        project: Some(spec.project.clone()),
        links: match (&existing, spec.links.is_empty()) {
            (Some(old), true) => old.links.clone(),
            _ => spec.links.clone(),
        },
        version: None,
        title: spec.title.clone(),
        authors: Vec::new(),
        abstract_text: String::new(),
        categories: Vec::new(),
        published_at: String::new(),
        updated_at: now.clone(),
        ingested_at: existing.as_ref().map_or(now, |old| old.ingested_at.clone()),
        source_format: SourceFormat::Markdown,
        source_url: None,
        main_tex: None,
        tags: match (&existing, spec.tags.is_empty()) {
            (Some(old), true) => old.tags.clone(),
            _ => spec.tags.clone(),
        },
        schema_version: crate::SCHEMA_VERSION,
    };

    progress("writing idea");
    std::fs::create_dir_all(paths.paper_dir(&id)).map_err(|e| io_err("create idea folder", e))?;
    std::fs::write(paths.idea_path(&id), spec.body.trim_end().to_string() + "\n")
        .map_err(|e| io_err("write idea.md", e))?;
    meta.save(&paths.metadata_path(&id))?;

    progress("embedding and indexing");
    let db = MetaDb::open(&paths.meta_db_path())?;
    let mut index = VectorIndex::open_or_create(
        &paths.index_path(),
        config.embedding.dimensions,
        config.turbovec.bit_width,
    )?;
    let (n_chunks, cache_hits) = index_paper_from_disk(paths, config, &db, &mut index, &id).await?;
    connect_cortex(paths, config, &db, &index, &id);

    let elapsed = t0.elapsed().as_secs_f64();
    log_line(
        paths,
        &format!("captured idea {id}: {n_chunks} chunks ({elapsed:.1}s)"),
    );

    Ok(IngestReport {
        paper_id: id,
        title: meta.title,
        chunks: n_chunks,
        cache_hits,
        source_format: SourceFormat::Markdown,
        elapsed_secs: elapsed,
    })
}

/// One `kb reflect` / MCP `kb_create_reflection` payload.
#[derive(Debug, Clone)]
pub struct ReflectionSpec {
    /// Explicit id; derived from title when absent (prefixed with "reflection-").
    pub slug: Option<String>,
    pub title: String,
    pub body: String,
    /// paper_ids this reflection draws from (stored as `links` in metadata).
    pub scope: Vec<String>,
    pub tags: Vec<String>,
}

/// Create (or update) a cross-paper synthesis reflection: write the canonical
/// folder (metadata.json + reflection.md), then embed and index it.
/// Re-calling with the same id updates in place.
pub async fn ingest_reflection(
    paths: &KbPaths,
    config: &Config,
    spec: &ReflectionSpec,
    progress: &dyn Fn(&str),
) -> Result<IngestReport, KbError> {
    let t0 = Instant::now();
    let raw_slug = slug_from_filename(&spec.title)?;
    let id = match &spec.slug {
        Some(s) => s.clone(),
        None => {
            if raw_slug.starts_with("reflection-") {
                raw_slug
            } else {
                format!("reflection-{raw_slug}")
            }
        }
    };

    if spec.body.trim().is_empty() {
        return Err(KbError::Usage("reflection body must not be empty".to_string()));
    }

    let existing = PaperMetadata::load(&paths.metadata_path(&id)).ok();
    if let Some(old) = &existing
        && old.kind != DocKind::Reflection
    {
        return Err(KbError::Usage(format!(
            "{id} already names a non-reflection document; choose a different title"
        )));
    }

    let now = now_rfc3339();
    let meta = PaperMetadata {
        arxiv_id: id.clone(),
        kind: DocKind::Reflection,
        project: None,
        links: spec.scope.clone(),
        version: None,
        title: spec.title.clone(),
        authors: Vec::new(),
        abstract_text: String::new(),
        categories: Vec::new(),
        published_at: String::new(),
        updated_at: now.clone(),
        ingested_at: existing.as_ref().map_or(now, |old| old.ingested_at.clone()),
        source_format: SourceFormat::Markdown,
        source_url: None,
        main_tex: None,
        tags: match (&existing, spec.tags.is_empty()) {
            (Some(old), true) => old.tags.clone(),
            _ => spec.tags.clone(),
        },
        schema_version: crate::SCHEMA_VERSION,
    };

    progress("writing reflection");
    std::fs::create_dir_all(paths.paper_dir(&id))
        .map_err(|e| io_err("create reflection folder", e))?;
    std::fs::write(paths.reflection_path(&id), spec.body.trim_end().to_string() + "\n")
        .map_err(|e| io_err("write reflection.md", e))?;
    meta.save(&paths.metadata_path(&id))?;

    progress("embedding and indexing");
    let db = MetaDb::open(&paths.meta_db_path())?;
    let mut index = VectorIndex::open_or_create(
        &paths.index_path(),
        config.embedding.dimensions,
        config.turbovec.bit_width,
    )?;
    let (n_chunks, cache_hits) = index_paper_from_disk(paths, config, &db, &mut index, &id).await?;
    connect_cortex(paths, config, &db, &index, &id);

    let elapsed = t0.elapsed().as_secs_f64();
    log_line(
        paths,
        &format!("created reflection {id}: {n_chunks} chunks ({elapsed:.1}s)"),
    );
    Ok(IngestReport {
        paper_id: id,
        title: meta.title,
        chunks: n_chunks,
        cache_hits,
        source_format: SourceFormat::Markdown,
        elapsed_secs: elapsed,
    })
}

/// Filename stem → paper id for local PDFs: lowercase ASCII alphanumerics,
/// every other run of characters collapsed to a single hyphen.
/// "Attention Is All You Need.pdf" → "attention-is-all-you-need". Dots
/// become hyphens, so a slug can never collide with the arXiv id namespace.
pub fn slug_from_filename(stem: &str) -> Result<String, KbError> {
    let mut slug = String::with_capacity(stem.len());
    for c in stem.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        return Err(KbError::Usage(format!(
            "cannot derive an id from '{stem}' (no ASCII letters or digits)"
        )));
    }
    Ok(slug)
}

/// Shared tail of every full ingest: TOC → section chunks → embed →
/// two-store commit → log + report. Assumes metadata.json, sections.md,
/// notes.md and paper.pdf are already in place.
async fn index_and_report(
    paths: &KbPaths,
    config: &Config,
    id: &str,
    meta: &PaperMetadata,
    t0: Instant,
    progress: &dyn Fn(&str),
) -> Result<IngestReport, KbError> {
    progress("extracting PDF table of contents");
    let toc = pdf::extract_toc(&paths.pdf_path(id)).unwrap_or_else(|e| {
        tracing::warn!("TOC extraction failed ({e}); deep links will be page-less");
        Vec::new()
    });

    progress("classifying sections");
    let sections_md = std::fs::read_to_string(paths.sections_path(id)).ok();
    let notes_md = std::fs::read_to_string(paths.notes_path(id)).ok();
    let overrides = match sections_md.as_deref() {
        Some(md) => {
            let others = sections::other_headings(md);
            if !others.is_empty() {
                progress("classifying ambiguous headings (LLM)");
            }
            sections::llm_heading_overrides(config, &others).await
        }
        None => std::collections::HashMap::new(),
    };
    let chunks = sections::build_chunks_with_overrides(
        sections_md.as_deref(),
        &meta.abstract_text,
        notes_md.as_deref(),
        None,
        config.ingest.chunk_max_tokens,
        &overrides,
    )?;
    let pages = assign_pages(&chunks, &toc);

    progress("embedding and indexing");
    let db = MetaDb::open(&paths.meta_db_path())?;
    let mut index = VectorIndex::open_or_create(
        &paths.index_path(),
        config.embedding.dimensions,
        config.turbovec.bit_width,
    )?;
    let (n_chunks, cache_hits) = embed_and_commit(
        paths, config, &db, &mut index, id, &chunks, &pages, &toc, meta,
    )
    .await?;
    connect_cortex(paths, config, &db, &index, id);

    let elapsed = t0.elapsed().as_secs_f64();
    log_line(
        paths,
        &format!("ingested {id}: {n_chunks} chunks ({elapsed:.1}s)"),
    );

    Ok(IngestReport {
        paper_id: id.to_string(),
        title: meta.title.clone(),
        chunks: n_chunks,
        cache_hits,
        source_format: meta.source_format,
        elapsed_secs: elapsed,
    })
}

/// Rebuild one paper's chunks from its canonical files on disk (no network
/// except embeddings, and those mostly hit the cache). Shared by reindex,
/// the watcher (new folder / notes.md / sections.md changes), and
/// `kb note`'s post-edit re-embed.
pub async fn index_paper_from_disk(
    paths: &KbPaths,
    config: &Config,
    db: &MetaDb,
    index: &mut VectorIndex,
    paper_id: &str,
) -> Result<(usize, usize), KbError> {
    let meta = PaperMetadata::load(&paths.metadata_path(paper_id))?;

    let (sections_md, reflection_md) = match meta.kind {
        DocKind::Note => (std::fs::read_to_string(paths.idea_path(paper_id)).ok(), None),
        DocKind::Reflection => {
            (None, std::fs::read_to_string(paths.reflection_path(paper_id)).ok())
        }
        DocKind::Paper => {
            let sections_path = paths.sections_path(paper_id);
            if !sections_path.exists() && paths.pdf_path(paper_id).exists() {
                write_sections_from_pdf(&paths.pdf_path(paper_id), &sections_path)?;
            }
            (std::fs::read_to_string(&sections_path).ok(), None)
        }
    };
    let notes_md = std::fs::read_to_string(paths.notes_path(paper_id)).ok();

    let overrides = match sections_md.as_deref() {
        Some(md) => sections::llm_heading_overrides(config, &sections::other_headings(md)).await,
        None => std::collections::HashMap::new(),
    };
    let chunks = sections::build_chunks_with_overrides(
        sections_md.as_deref(),
        &meta.abstract_text,
        notes_md.as_deref(),
        reflection_md.as_deref(),
        config.ingest.chunk_max_tokens,
        &overrides,
    )?;

    // Pages come from a PDF outline; notes and reflections have none.
    let has_pdf = meta.kind == DocKind::Paper && paths.pdf_path(paper_id).exists();
    let toc = if has_pdf {
        pdf::extract_toc(&paths.pdf_path(paper_id)).unwrap_or_default()
    } else {
        Vec::new()
    };
    let pages = if has_pdf {
        assign_pages(&chunks, &toc)
    } else {
        vec![None; chunks.len()]
    };

    embed_and_commit(
        paths, config, db, index, paper_id, &chunks, &pages, &toc, &meta,
    )
    .await
}

/// Re-embed after a notes.md change. Thanks to the embedding cache, the
/// unchanged section chunks cost zero API calls — only the notes text is
/// actually re-embedded.
pub async fn reembed_notes(
    paths: &KbPaths,
    config: &Config,
    paper_id: &str,
) -> Result<(), KbError> {
    let db = MetaDb::open(&paths.meta_db_path())?;
    let mut index = VectorIndex::open_or_create(
        &paths.index_path(),
        config.embedding.dimensions,
        config.turbovec.bit_width,
    )?;
    index_paper_from_disk(paths, config, &db, &mut index, paper_id).await?;
    // Re-embedding reassigns this paper's chunk ids, so its Cortex edges must
    // be recomputed or they'd dangle.
    connect_cortex(paths, config, &db, &index, paper_id);
    Ok(())
}

/// Remove a paper from both stores (addendum §5 remove sequence). Does NOT
/// touch the canonical folder — that's the caller's decision (`kb remove`
/// deletes it, the watcher reacts to it already being gone).
pub fn remove_paper_from_stores(
    paths: &KbPaths,
    config: &Config,
    paper_id: &str,
) -> Result<usize, KbError> {
    let db = MetaDb::open(&paths.meta_db_path())?;
    let mut index = VectorIndex::open_or_create(
        &paths.index_path(),
        config.embedding.dimensions,
        config.turbovec.bit_width,
    )?;
    db.begin_immediate()?;
    let result = (|| -> Result<usize, KbError> {
        let removed = db.remove_paper(paper_id)?;
        for vid in &removed {
            index.remove(*vid as u64);
        }
        index.save_atomic(&paths.index_path())?;
        Ok(removed.len())
    })();
    match result {
        Ok(n) => {
            db.commit()?;
            log_line(paths, &format!("removed {paper_id}: {n} chunks"));
            Ok(n)
        }
        Err(e) => {
            let _ = db.rollback();
            Err(e)
        }
    }
}

/// Rebuild both derived stores from canonical files (addendum §8).
/// Returns (papers, chunks).
pub async fn reindex_all(
    paths: &KbPaths,
    config: &Config,
    progress: &dyn Fn(&str),
) -> Result<(usize, usize), KbError> {
    let index_path = paths.index_path();
    let backup = index_path.with_extension("tv.backup");
    if index_path.exists() {
        std::fs::rename(&index_path, &backup).map_err(|e| io_err("backup index.tv", e))?;
    }

    let db = MetaDb::open(&paths.meta_db_path())?;
    db.reset_derived_tables()?;
    let mut index = VectorIndex::create(config.embedding.dimensions, config.turbovec.bit_width)?;

    let ids = paths.list_paper_ids()?;
    let mut total_chunks = 0usize;
    let mut papers = 0usize;
    for id in &ids {
        progress(&format!("reindexing {id}"));
        match index_paper_from_disk(paths, config, &db, &mut index, id).await {
            Ok((n, _)) => {
                papers += 1;
                total_chunks += n;
            }
            Err(e) => {
                // One broken folder must not sink the rebuild; report and go on.
                eprintln!("warning: skipped {id}: {e}");
                log_line(paths, &format!("reindex skipped {id}: {e}"));
            }
        }
    }

    db.meta_set("vector_fingerprint", &config.vector_fingerprint())?;
    db.meta_set("chunking_fingerprint", &config.chunking_fingerprint())?;
    index.save_atomic(&index_path)?;

    // Rebuild the Cortex layer over the freshly indexed corpus (every paper is
    // present now, so cross-paper edges are complete). Derived state, like the
    // index itself — best-effort so a connect hiccup never fails the reindex.
    match crate::cortex::rebuild_all_with(paths, config, &db, &index) {
        Ok(n) => log_line(paths, &format!("cortex: rebuilt {n} connections")),
        Err(e) => {
            eprintln!("warning: cortex rebuild failed: {e}");
            log_line(paths, &format!("cortex rebuild failed: {e}"));
        }
    }

    if backup.exists() {
        let _ = std::fs::remove_file(&backup);
    }
    log_line(
        paths,
        &format!("reindex complete: {papers} papers, {total_chunks} chunks"),
    );
    Ok((papers, total_chunks))
}

/// The addendum §5 add sequence for one paper's chunks. Embeddings are
/// gathered (cache-aware, batched) BEFORE the transaction; replaces any
/// existing chunks for the paper, so it is idempotent.
#[allow(clippy::too_many_arguments)]
async fn embed_and_commit(
    paths: &KbPaths,
    config: &Config,
    db: &MetaDb,
    index: &mut VectorIndex,
    paper_id: &str,
    chunks: &[RawChunk],
    pages: &[Option<u32>],
    toc: &[crate::TocEntry],
    meta: &PaperMetadata,
) -> Result<(usize, usize), KbError> {
    let model = &config.embedding.model;

    // Step 1-2 (network, before any store mutation): collect vectors.
    let hashes: Vec<String> = chunks.iter().map(|c| content_hash(&c.text)).collect();
    let mut vectors: Vec<Option<Vec<f32>>> = Vec::with_capacity(chunks.len());
    let mut cache_hits = 0usize;
    let mut missing: Vec<usize> = Vec::new();
    for (i, h) in hashes.iter().enumerate() {
        match db.cache_get(h, model, EMBEDDING_VERSION)? {
            Some(v) => {
                cache_hits += 1;
                vectors.push(Some(v));
            }
            None => {
                vectors.push(None);
                missing.push(i);
            }
        }
    }
    if !missing.is_empty() {
        let embedder = OpenAiEmbedder::from_env(model, config.embedding.dimensions)?;
        let texts: Vec<&str> = missing.iter().map(|&i| chunks[i].text.as_str()).collect();
        let embedded = embedder.embed_batch(&texts).await?;
        if embedded.len() != missing.len() {
            return Err(KbError::Network(format!(
                "embedding API returned {} vectors for {} inputs",
                embedded.len(),
                missing.len()
            )));
        }
        for (j, &i) in missing.iter().enumerate() {
            db.cache_put(&hashes[i], model, EMBEDDING_VERSION, &embedded[j])?;
            vectors[i] = Some(embedded[j].clone());
        }
    }

    // Steps 3-9: the two-store commit.
    db.begin_immediate()?;
    let result = (|| -> Result<(), KbError> {
        for vid in db.delete_chunks_for_paper(paper_id)? {
            index.remove(vid as u64);
        }
        let mut ids: Vec<u64> = Vec::with_capacity(chunks.len());
        let mut flat: Vec<f32> = Vec::with_capacity(chunks.len() * config.embedding.dimensions);
        for (i, chunk) in chunks.iter().enumerate() {
            let vid = db.insert_chunk(&NewChunk {
                chunk_id: chunk.chunk_id(paper_id),
                paper_id: paper_id.to_string(),
                section_type: chunk.section_type,
                ordinal: chunk.ordinal,
                content_hash: hashes[i].clone(),
                text: chunk.text.clone(),
                page: pages.get(i).copied().flatten(),
                snippet: make_snippet(&chunk.text),
                embedded_at: now_rfc3339(),
                embedding_model: model.clone(),
                embedding_version: EMBEDDING_VERSION,
            })?;
            ids.push(vid as u64);
            flat.extend_from_slice(vectors[i].as_ref().expect("vector gathered above"));
        }
        if !ids.is_empty() {
            index.add(&ids, &flat)?;
        }
        db.replace_toc(paper_id, toc)?;
        db.set_tags(paper_id, &meta.tags)?;
        db.set_document(paper_id, meta.kind, meta.project.as_deref())?;
        db.meta_set("vector_fingerprint", &config.vector_fingerprint())?;
        db.meta_set("chunking_fingerprint", &config.chunking_fingerprint())?;
        index.save_atomic(&paths.index_path())?;
        Ok(())
    })();
    match result {
        Ok(()) => db.commit()?,
        Err(e) => {
            let _ = db.rollback();
            return Err(e);
        }
    }
    Ok((chunks.len(), cache_hits))
}

/// Page mapping (PRD §4 step 9): TOC best-match per heading, else the
/// nearest preceding chunk's page. user_notes chunks are not in the PDF
/// and get no page. Everything starts at page 1 (the abstract).
fn assign_pages(chunks: &[RawChunk], toc: &[crate::TocEntry]) -> Vec<Option<u32>> {
    let mut out = Vec::with_capacity(chunks.len());
    let mut last: Option<u32> = Some(1);
    for chunk in chunks {
        if matches!(chunk.section_type, SectionType::UserNotes | SectionType::Reflection) {
            out.push(None);
            continue;
        }
        let matched = sections::page_for_heading(chunk.heading.as_deref(), toc).map(|(p, _)| p);
        out.push(matched.or(last));
        if matched.is_some() {
            last = matched;
        }
    }
    out
}

/// A page whose extracted text is shorter than this (after trimming) is
/// treated as having no real text layer — a scanned/figure-only page that's a
/// candidate for OCR. Tuned to ignore stray headers/page numbers `pdf_extract`
/// sometimes recovers from otherwise-image pages.
const MIN_TEXT_CHARS: usize = 24;

fn write_sections_from_pdf(pdf_path: &Path, sections_path: &Path) -> Result<(), KbError> {
    // An image-only PDF may have no text layer at all — `extract_text_per_page`
    // can then error. Before giving up, fall through to OCR with blank pages.
    let mut pages = match pdf::extract_text_per_page(pdf_path) {
        Ok(pages) => pages,
        Err(e) => {
            let n = pdf::page_count(pdf_path).map_err(|_| e)?;
            tracing::warn!("no PDF text layer; attempting OCR on all {n} page(s)");
            vec![String::new(); n]
        }
    };

    ocr_blank_pages(pdf_path, &mut pages);

    let mut md = String::new();
    for (i, text) in pages.iter().enumerate() {
        md.push_str(&format!("## Page {}\n\n{}\n\n", i + 1, text.trim()));
    }
    std::fs::write(sections_path, md).map_err(|e| io_err("write sections.md", e))
}

/// Recover text for pages with no text layer via the `kb-ocr` sidecar, in
/// place. Best-effort: with no helper (non-macOS, or a build without it) or on
/// OCR failure, the blank pages stay blank — never fatal to ingest.
fn ocr_blank_pages(pdf_path: &Path, pages: &mut [String]) {
    let blanks: Vec<usize> = pages
        .iter()
        .enumerate()
        .filter(|(_, t)| t.trim().chars().count() < MIN_TEXT_CHARS)
        .map(|(i, _)| i)
        .collect();
    if blanks.is_empty() {
        return;
    }

    let Some(helper) = ocr::locate_helper() else {
        tracing::info!(
            "{} page(s) lack a text layer; no kb-ocr helper available, leaving them blank",
            blanks.len()
        );
        return;
    };

    let pages_1indexed: Vec<usize> = blanks.iter().map(|i| i + 1).collect();
    match ocr::run(&helper, pdf_path, &pages_1indexed) {
        Ok(recovered) => {
            let mut filled = 0usize;
            for (page_1, text) in recovered {
                if page_1 >= 1 && page_1 <= pages.len() && !text.trim().is_empty() {
                    pages[page_1 - 1] = text;
                    filled += 1;
                }
            }
            tracing::info!("OCR recovered text for {filled}/{} page(s)", blanks.len());
        }
        Err(e) => tracing::warn!("OCR failed ({e}); leaving {} page(s) blank", blanks.len()),
    }
}

/// Create the notes template if absent (PRD §4 step 7; prompts live in
/// HTML comments — resolved decision — so they're stripped before embedding).
pub fn ensure_notes_template(paths: &KbPaths, paper_id: &str, title: &str) -> Result<(), KbError> {
    let path = paths.notes_path(paper_id);
    if path.exists() {
        return Ok(());
    }
    let body = format!(
        "# Notes on {title}\n\n\
         <!-- Why is this interesting to me? -->\n\n\n\
         <!-- What would I build with this? -->\n\n\n\
         <!-- Connections to other things I've saved -->\n\n\n"
    );
    std::fs::write(&path, body).map_err(|e| io_err("write notes.md", e))
}

/// (Re)establish a paper's Cortex connections after it has been (re)indexed.
/// Best-effort: the paper is already committed to both stores, so a failure to
/// grow the associative layer must never fail the ingest — it's logged and the
/// next `kb cortex rebuild` / `kb reindex` will catch up. No-op when Cortex is
/// disabled (the call returns 0 immediately).
fn connect_cortex(
    paths: &KbPaths,
    config: &Config,
    db: &MetaDb,
    index: &VectorIndex,
    paper_id: &str,
) {
    match crate::cortex::connect_paper_with(paths, config, db, index, paper_id) {
        Ok(n) if n > 0 => log_line(paths, &format!("cortex: {paper_id} formed {n} connections")),
        Ok(_) => {}
        Err(e) => {
            tracing::warn!("cortex connect for {paper_id} failed: {e}");
            log_line(paths, &format!("cortex connect for {paper_id} failed: {e}"));
        }
    }
}

/// Append one timestamped line to .arxiv-kb/kb.log (PRD §9 format).
pub fn log_line(paths: &KbPaths, msg: &str) {
    use std::io::Write;
    let line = format!("{} INFO  {msg}\n", now_rfc3339());
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths.log_path())
    {
        let _ = f.write_all(line.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_url_lines_skips_blanks_and_comments() {
        let content = "\n  https://a.example/x  \n# a comment\n\nhttps://b.example/y\n   \n";
        assert_eq!(
            parse_url_lines(content),
            vec!["https://a.example/x", "https://b.example/y"]
        );
    }

    #[test]
    fn parse_url_lines_single_url_file() {
        assert_eq!(
            parse_url_lines("https://only.example/page\n"),
            vec!["https://only.example/page"]
        );
    }

    #[test]
    fn materialize_local_pdf_writes_canonical_folder() {
        use lopdf::content::{Content, Operation};
        use lopdf::{dictionary, Document, Object, Stream};

        // Build a minimal one-page PDF with extractable text.
        let dir = tempfile::tempdir().unwrap();
        let pdf_path = dir.path().join("Hello World.pdf");
        {
            let mut doc = Document::with_version("1.5");
            let pages_id = doc.new_object_id();
            let font_id = doc.add_object(dictionary! {
                "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
            });
            let resources_id =
                doc.add_object(dictionary! { "Font" => dictionary! { "F1" => font_id } });
            let content = Content {
                operations: vec![
                    Operation::new("BT", vec![]),
                    Operation::new("Tf", vec!["F1".into(), 24.into()]),
                    Operation::new("Td", vec![72.into(), 700.into()]),
                    Operation::new("Tj", vec![Object::string_literal("Body text on the page")]),
                    Operation::new("ET", vec![]),
                ],
            };
            let content_id =
                doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
            let page_id = doc.add_object(dictionary! {
                "Type" => "Page", "Parent" => pages_id, "Contents" => content_id,
                "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
                "Resources" => resources_id,
            });
            doc.objects.insert(
                pages_id,
                Object::Dictionary(dictionary! {
                    "Type" => "Pages", "Kids" => vec![Object::Reference(page_id)], "Count" => 1,
                }),
            );
            let catalog_id =
                doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
            doc.trailer.set("Root", catalog_id);
            doc.save(&pdf_path).unwrap();
        }

        let root = tempfile::tempdir().unwrap();
        let paths = KbPaths { root: root.path().to_path_buf() };
        let id = materialize_local_pdf(&paths, &pdf_path, &|_| {}).unwrap();

        assert_eq!(id, "hello-world");
        assert!(paths.pdf_path(&id).exists(), "paper.pdf copied in");
        assert!(paths.sections_path(&id).exists(), "sections.md written");
        assert!(paths.notes_path(&id).exists(), "notes template written");
        let meta = PaperMetadata::load(&paths.metadata_path(&id)).unwrap();
        assert_eq!(meta.source_format, SourceFormat::Pdf);
        assert!(meta.source_url.is_none());

        // The index was NOT touched (materialize is disk-only).
        assert!(!paths.index_path().exists());
    }

    #[test]
    fn slug_basic() {
        assert_eq!(
            slug_from_filename("Attention Is All You Need").unwrap(),
            "attention-is-all-you-need"
        );
    }

    #[test]
    fn slug_collapses_runs_and_trims() {
        assert_eq!(
            slug_from_filename("  my -- paper (v2)!.final ").unwrap(),
            "my-paper-v2-final"
        );
        assert_eq!(slug_from_filename("_leading_underscore").unwrap(), "leading-underscore");
    }

    #[test]
    fn slug_of_arxiv_like_name_leaves_arxiv_namespace() {
        // A file named 2504.19874.pdf must not produce an id that parses
        // as an arXiv id (the dot becomes a hyphen).
        let slug = slug_from_filename("2504.19874").unwrap();
        assert_eq!(slug, "2504-19874");
        assert!(crate::ingest::arxiv::parse_arxiv_id(&slug).is_err());
    }

    #[test]
    fn slug_with_no_alphanumerics_is_usage_error() {
        for bad in ["", "---", "日本語", "(!)"] {
            assert!(matches!(
                slug_from_filename(bad).unwrap_err(),
                KbError::Usage(_)
            ));
        }
    }

    #[test]
    fn slug_is_idempotent() {
        let s = slug_from_filename("Attention Is All You Need").unwrap();
        assert_eq!(slug_from_filename(&s).unwrap(), s);
    }
}
