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
use crate::ingest::{arxiv, latex, pdf, sections};
use crate::{
    content_hash, make_snippet, now_rfc3339, KbError, PaperMetadata, RawChunk, SectionType,
    SourceFormat, EMBEDDING_VERSION,
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

    progress("extracting PDF table of contents");
    let toc = pdf::extract_toc(&paths.pdf_path(&id)).unwrap_or_else(|e| {
        tracing::warn!("TOC extraction failed ({e}); deep links will be page-less");
        Vec::new()
    });

    progress("classifying sections");
    let sections_md = std::fs::read_to_string(paths.sections_path(&id)).ok();
    let notes_md = std::fs::read_to_string(paths.notes_path(&id)).ok();
    let chunks = sections::build_chunks(
        sections_md.as_deref(),
        &meta.abstract_text,
        notes_md.as_deref(),
        config.ingest.chunk_max_tokens,
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
        paths, config, &db, &mut index, &id, &chunks, &pages, &toc, &meta.tags,
    )
    .await?;

    let elapsed = t0.elapsed().as_secs_f64();
    log_line(
        paths,
        &format!("ingested {id}: {n_chunks} chunks ({elapsed:.1}s)"),
    );

    Ok(IngestReport {
        paper_id: id,
        title: meta.title,
        chunks: n_chunks,
        cache_hits,
        source_format,
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

    // sections.md is derived; if it's missing but the PDF survives,
    // regenerate it (addendum §8 step 5b).
    let sections_path = paths.sections_path(paper_id);
    if !sections_path.exists() && paths.pdf_path(paper_id).exists() {
        write_sections_from_pdf(&paths.pdf_path(paper_id), &sections_path)?;
    }
    let sections_md = std::fs::read_to_string(&sections_path).ok();
    let notes_md = std::fs::read_to_string(paths.notes_path(paper_id)).ok();

    let chunks = sections::build_chunks(
        sections_md.as_deref(),
        &meta.abstract_text,
        notes_md.as_deref(),
        config.ingest.chunk_max_tokens,
    )?;

    let toc = if paths.pdf_path(paper_id).exists() {
        pdf::extract_toc(&paths.pdf_path(paper_id)).unwrap_or_default()
    } else {
        Vec::new()
    };
    let pages = assign_pages(&chunks, &toc);

    embed_and_commit(
        paths, config, db, index, paper_id, &chunks, &pages, &toc, &meta.tags,
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
    tags: &[String],
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
        db.set_tags(paper_id, tags)?;
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
        if chunk.section_type == SectionType::UserNotes {
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

fn write_sections_from_pdf(pdf_path: &Path, sections_path: &Path) -> Result<(), KbError> {
    let pages = pdf::extract_text_per_page(pdf_path)?;
    let mut md = String::new();
    for (i, text) in pages.iter().enumerate() {
        md.push_str(&format!("## Page {}\n\n{}\n\n", i + 1, text.trim()));
    }
    std::fs::write(sections_path, md).map_err(|e| io_err("write sections.md", e))
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
