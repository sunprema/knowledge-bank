//! Implementations behind each CLI subcommand (PRD §6). main.rs stays a
//! thin clap dispatcher; everything testable lives here.

use crate::config::{Config, KbPaths};
use crate::index::{consistency_check, MetaDb, VectorIndex};
use crate::ingest::{arxiv, pipeline};
use crate::search::{retrieval, SearchFilters, SearchMode};
use crate::{deep_link, DocKind, KbError, PaperMetadata, SectionType};
use serde_json::json;
use std::collections::BTreeMap;
use std::io::Write;
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

    fn open_stores(&self) -> Result<(MetaDb, VectorIndex), KbError> {
        let db = MetaDb::open(&self.paths.meta_db_path())?;
        let index = VectorIndex::open_or_create(
            &self.paths.index_path(),
            self.config.embedding.dimensions,
            self.config.turbovec.bit_width,
        )?;
        Ok((db, index))
    }
}

fn planned(cmd: &str, version: &str) -> KbError {
    KbError::Usage(format!("`kb {cmd}` is planned for {version}"))
}

fn confirm(prompt: &str) -> Result<bool, KbError> {
    eprint!("{prompt} [y/N] ");
    std::io::stderr().flush().ok();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| KbError::Usage(format!("cannot read confirmation: {e}")))?;
    Ok(matches!(line.trim(), "y" | "Y" | "yes"))
}

/// Validate-and-normalize an id argument: an arXiv id or URL (like
/// `kb add`), or a local-PDF slug id (e.g. attention-is-all-you-need).
fn canonical_id(input: &str) -> Result<String, KbError> {
    if let Ok((id, _)) = arxiv::parse_arxiv_id(input) {
        return Ok(id);
    }
    match pipeline::slug_from_filename(input) {
        // Already-canonical slugs only; "My Paper" should not silently
        // resolve to my-paper.
        Ok(slug) if slug == input => Ok(slug),
        _ => Err(KbError::Usage(format!(
            "unrecognized paper id or URL: {input}"
        ))),
    }
}

fn require_paper(kb: &Kb, paper_id: &str) -> Result<PaperMetadata, KbError> {
    let path = kb.paths.metadata_path(paper_id);
    if !path.exists() {
        return Err(KbError::NotFound(format!(
            "{paper_id} is not in the KB (try `kb add {paper_id}`)"
        )));
    }
    PaperMetadata::load(&path)
}

pub fn init(kb: &Kb) -> Result<(), KbError> {
    // Config::load_or_create already created .arxiv-kb/ + config.toml.
    println!("initialized KB at {}", kb.paths.root.display());
    println!("config: {}", kb.paths.config_path().display());
    Ok(())
}

pub async fn add(
    kb: &Kb,
    id_or_url: Option<String>,
    pdf: Option<PathBuf>,
    url: Option<String>,
) -> Result<(), KbError> {
    match (id_or_url, pdf, url) {
        (None, Some(path), None) => run_ingest(kb, IngestInput::LocalPdf(path)).await,
        (None, None, Some(url)) => {
            run_ingest(kb, IngestInput::Url { input: url, refetch: false }).await
        }
        (Some(input), None, None) => {
            run_ingest(kb, IngestInput::Arxiv { input, refetch: false }).await
        }
        (None, None, None) => Err(KbError::Usage(
            "usage: kb add <arxiv-id-or-url> | kb add --pdf <file.pdf> | kb add --url <page-url>"
                .to_string(),
        )),
        _ => Err(KbError::Usage(
            "pass exactly one of: an arXiv id/URL, --pdf <file>, or --url <page-url>".to_string(),
        )),
    }
}

pub async fn update(kb: &Kb, arxiv_id: String) -> Result<(), KbError> {
    let id = canonical_id(&arxiv_id)?;
    let meta = require_paper(kb, &id)?;
    if meta.kind == DocKind::Note {
        return Err(KbError::Usage(format!(
            "{id} is an idea; re-capture it with `kb idea add` (same title or id updates in place)"
        )));
    }
    // A web page re-fetches from its recorded URL, not the arXiv API.
    if let Some(url) = meta.source_url {
        return run_ingest(kb, IngestInput::Url { input: url, refetch: true }).await;
    }
    if arxiv::parse_arxiv_id(&id).is_err() {
        return Err(KbError::Usage(format!(
            "{id} was ingested from a local PDF and has nothing to re-fetch; \
             `kb remove {id}` and `kb add --pdf <file>` again"
        )));
    }
    run_ingest(kb, IngestInput::Arxiv { input: id, refetch: true }).await
}

/// `kb idea add`: capture (or upsert) a standalone idea. `body` is literal
/// text, `-` for stdin, or absent to compose in $EDITOR.
pub async fn idea_add(
    kb: &Kb,
    project: String,
    title: String,
    body: Option<String>,
    tags: Vec<String>,
    links: Vec<String>,
) -> Result<(), KbError> {
    let body = match body.as_deref() {
        Some("-") => {
            let mut buf = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)
                .map_err(|e| KbError::Usage(format!("cannot read body from stdin: {e}")))?;
            buf
        }
        Some(text) => text.to_string(),
        None => compose_in_editor(&title)?,
    };
    let spec = pipeline::IdeaSpec {
        slug: None,
        project,
        title,
        body,
        tags,
        links,
    };
    run_ingest(kb, IngestInput::Idea(spec)).await
}

/// `kb reflect` — write a cross-paper synthesis reflection, then index it.
/// Opens `$EDITOR` with a template pre-seeded with guiding questions and
/// (when `--scope` paper ids are given) the titles of those papers so the
/// user has context while writing.
pub async fn reflect(
    kb: &Kb,
    title: String,
    body: Option<String>,
    scope: Vec<String>,
    tags: Vec<String>,
) -> Result<(), KbError> {
    let body = match body.as_deref() {
        Some("-") => {
            let mut buf = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)
                .map_err(|e| KbError::Usage(format!("cannot read body from stdin: {e}")))?;
            buf
        }
        Some(text) => text.to_string(),
        None => compose_reflection_in_editor(&title, &scope, &kb.paths)?,
    };
    let spec = pipeline::ReflectionSpec {
        slug: None,
        title,
        body,
        scope,
        tags,
    };
    run_ingest(kb, IngestInput::Reflection(spec)).await
}

fn compose_reflection_in_editor(
    title: &str,
    scope: &[String],
    paths: &crate::config::KbPaths,
) -> Result<String, KbError> {
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let tmp = tempfile::Builder::new()
        .prefix("kb-reflect-")
        .suffix(".md")
        .tempfile()
        .map_err(|e| KbError::Usage(format!("cannot create temp file: {e}")))?;

    let mut seed = String::new();
    if !scope.is_empty() {
        seed.push_str("<!-- Papers in scope:\n");
        for id in scope {
            let meta_path = paths.metadata_path(id);
            if let Ok(meta) = PaperMetadata::load(&meta_path) {
                seed.push_str(&format!("  [{id}] {}\n", meta.title));
            } else {
                seed.push_str(&format!("  {id}\n"));
            }
        }
        seed.push_str("-->\n\n");
    }
    seed.push_str(&format!("<!-- Reflection: {title} -->\n\n"));
    seed.push_str("## Themes\n\n\n## Contradictions\n\n\n## Combined ideas\n\n\n");

    std::fs::write(tmp.path(), &seed)
        .map_err(|e| KbError::Usage(format!("cannot seed temp file: {e}")))?;

    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$1\""))
        .arg("kb-reflect")
        .arg(tmp.path())
        .status()
        .map_err(|e| KbError::Usage(format!("cannot launch $EDITOR ({editor}): {e}")))?;
    if !status.success() {
        return Err(KbError::Usage(format!("$EDITOR exited with {status}")));
    }
    let raw = std::fs::read_to_string(tmp.path())
        .map_err(|e| KbError::Usage(format!("cannot read back temp file: {e}")))?;
    let body: String = raw
        .lines()
        .filter(|l| !l.trim_start().starts_with("<!--"))
        .collect::<Vec<_>>()
        .join("\n");
    if body.trim().is_empty() {
        return Err(KbError::Usage(
            "reflection body is empty; nothing to save".to_string(),
        ));
    }
    Ok(body)
}

/// Open $EDITOR on a temp file pre-seeded with the idea title; returns the
/// composed body (the seed heading stripped if left untouched).
fn compose_in_editor(title: &str) -> Result<String, KbError> {
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let tmp = tempfile::Builder::new()
        .prefix("kb-idea-")
        .suffix(".md")
        .tempfile()
        .map_err(|e| KbError::Usage(format!("cannot create temp file: {e}")))?;
    std::fs::write(tmp.path(), format!("<!-- {title} — write the idea below -->\n\n"))
        .map_err(|e| KbError::Usage(format!("cannot seed temp file: {e}")))?;
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$1\""))
        .arg("kb-idea")
        .arg(tmp.path())
        .status()
        .map_err(|e| KbError::Usage(format!("cannot launch $EDITOR ({editor}): {e}")))?;
    if !status.success() {
        return Err(KbError::Usage(format!("$EDITOR exited with {status}")));
    }
    let raw = std::fs::read_to_string(tmp.path())
        .map_err(|e| KbError::Usage(format!("cannot read back temp file: {e}")))?;
    let body: String = raw
        .lines()
        .filter(|l| !l.trim_start().starts_with("<!--"))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(body)
}

enum IngestInput {
    Arxiv { input: String, refetch: bool },
    Url { input: String, refetch: bool },
    LocalPdf(PathBuf),
    Idea(pipeline::IdeaSpec),
    Reflection(pipeline::ReflectionSpec),
}

async fn run_ingest(kb: &Kb, job: IngestInput) -> Result<(), KbError> {
    let spinner = if kb.format == OutputFormat::Pretty {
        let s = indicatif::ProgressBar::new_spinner();
        s.enable_steady_tick(std::time::Duration::from_millis(120));
        Some(s)
    } else {
        None
    };
    let progress = |msg: &str| {
        if let Some(s) = &spinner {
            s.set_message(msg.to_string());
        } else {
            eprintln!("… {msg}");
        }
    };
    let report = match &job {
        IngestInput::Arxiv { input, refetch } => {
            pipeline::ingest_paper(&kb.paths, &kb.config, input, *refetch, &progress).await
        }
        IngestInput::Url { input, refetch } => {
            pipeline::ingest_url(&kb.paths, &kb.config, input, *refetch, &progress).await
        }
        IngestInput::LocalPdf(path) => {
            pipeline::ingest_local_pdf(&kb.paths, &kb.config, path, &progress).await
        }
        IngestInput::Idea(spec) => {
            pipeline::ingest_idea(&kb.paths, &kb.config, spec, &progress).await
        }
        IngestInput::Reflection(spec) => {
            pipeline::ingest_reflection(&kb.paths, &kb.config, spec, &progress).await
        }
    };
    if let Some(s) = &spinner {
        s.finish_and_clear();
    }
    let report = report?;

    match kb.format {
        OutputFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "paper_id": report.paper_id,
                "title": report.title,
                "chunks": report.chunks,
                "cache_hits": report.cache_hits,
                "source_format": report.source_format,
                "elapsed_secs": report.elapsed_secs,
            }))
            .unwrap()
        ),
        OutputFormat::Pretty => {
            println!("✓ {} — {}", report.paper_id, report.title);
            println!(
                "  {} chunks indexed ({} from cache), {} source, {:.1}s",
                report.chunks,
                report.cache_hits,
                match report.source_format {
                    crate::SourceFormat::Latex => "LaTeX",
                    crate::SourceFormat::Pdf => "PDF",
                    crate::SourceFormat::Markdown => "markdown",
                    crate::SourceFormat::Html => "HTML",
                },
                report.elapsed_secs
            );
            if report.source_format == crate::SourceFormat::Markdown {
                println!("  view: kb show {}", report.paper_id);
            } else {
                println!("  notes: kb note {}", report.paper_id);
            }
        }
    }
    Ok(())
}

pub async fn remove(kb: &Kb, arxiv_id: String, yes: bool) -> Result<(), KbError> {
    let id = canonical_id(&arxiv_id)?;
    let meta = require_paper(kb, &id)?;
    if !yes
        && !confirm(&format!(
            "remove \"{}\" ({id}) from the index AND delete its folder?",
            meta.title
        ))?
    {
        println!("aborted");
        return Ok(());
    }
    let n = pipeline::remove_paper_from_stores(&kb.paths, &kb.config, &id)?;
    std::fs::remove_dir_all(kb.paths.paper_dir(&id))
        .map_err(|e| KbError::Index(format!("folder delete failed (index already updated): {e}")))?;
    println!("removed {id} ({n} chunks)");
    Ok(())
}

pub async fn note(kb: &Kb, arxiv_id: String) -> Result<(), KbError> {
    let id = canonical_id(&arxiv_id)?;
    let meta = require_paper(kb, &id)?;
    pipeline::ensure_notes_template(&kb.paths, &id, &meta.title)?;

    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let notes_path = kb.paths.notes_path(&id);
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$1\""))
        .arg("kb-note")
        .arg(&notes_path)
        .status()
        .map_err(|e| KbError::Usage(format!("cannot launch $EDITOR ({editor}): {e}")))?;
    if !status.success() {
        return Err(KbError::Usage(format!("$EDITOR exited with {status}")));
    }

    // Re-embed immediately so the next search sees the notes even without
    // a running watcher. Failure here must not lose the notes (they're
    // canonical, already on disk) — warn and move on.
    match pipeline::reembed_notes(&kb.paths, &kb.config, &id).await {
        Ok(()) => println!("notes saved and re-embedded"),
        Err(e) => eprintln!(
            "notes saved; re-embedding failed ({e}) — a running `kb watch` or the next `kb reindex` will pick them up"
        ),
    }
    Ok(())
}

pub fn tag(kb: &Kb, arxiv_id: String, tags: Vec<String>) -> Result<(), KbError> {
    let id = canonical_id(&arxiv_id)?;
    let mut meta = require_paper(kb, &id)?;

    for spec in &tags {
        match spec.split_at_checked(1) {
            Some(("+", name)) if !name.is_empty() => {
                if !meta.tags.iter().any(|t| t == name) {
                    meta.tags.push(name.to_string());
                }
            }
            Some(("-", name)) if !name.is_empty() => {
                meta.tags.retain(|t| t != name);
            }
            _ => {
                return Err(KbError::Usage(format!(
                    "tag '{spec}' must start with + (add) or - (remove), e.g. +consumer"
                )))
            }
        }
    }
    meta.tags.sort();
    meta.save(&kb.paths.metadata_path(&id))?; // canonical
    let db = MetaDb::open(&kb.paths.meta_db_path())?;
    db.set_tags(&id, &meta.tags)?; // derived mirror
    println!(
        "{id} tags: {}",
        if meta.tags.is_empty() {
            "(none)".to_string()
        } else {
            meta.tags.join(", ")
        }
    );
    Ok(())
}

pub async fn search(
    kb: &Kb,
    query: String,
    mode: SearchMode,
    k: Option<usize>,
    filters: SearchFilters,
) -> Result<(), KbError> {
    let response = retrieval::search(&kb.paths, &kb.config, &query, mode, k, filters).await?;
    match kb.format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&response).unwrap());
        }
        OutputFormat::Pretty => {
            if response.papers.is_empty() {
                println!("no results for \"{query}\"");
                if mode == SearchMode::Narrow {
                    println!("(narrow mode filters scores < {:.2}; try --wide)",
                        kb.config.search.default_min_score_narrow);
                }
                return Ok(());
            }
            for (i, paper) in response.papers.iter().enumerate() {
                println!(
                    "{}. {}  ({})  best {:.2}",
                    i + 1,
                    paper.paper.title,
                    paper.paper_id,
                    paper.best_score
                );
                if !paper.tags.is_empty() {
                    println!("   tags: {}", paper.tags.join(", "));
                }
                for chunk in &paper.chunks {
                    let page = chunk
                        .page
                        .map(|p| format!(" p.{p}"))
                        .unwrap_or_default();
                    println!(
                        "   • [{:.2}] {}{} — {}",
                        chunk.score, chunk.section_type, page, chunk.snippet
                    );
                    println!("     {}", chunk.deep_link);
                }
                println!();
            }
            println!(
                "{} chunks across {} papers ({} mode)",
                response.total_chunks,
                response.papers.len(),
                response.mode
            );
        }
    }
    Ok(())
}

pub fn list(
    kb: &Kb,
    tag: Option<String>,
    kind: Option<DocKind>,
    project: Option<String>,
) -> Result<(), KbError> {
    let ids = kb.paths.list_paper_ids()?;
    let mut rows = Vec::new();
    for id in ids {
        let meta = match PaperMetadata::load(&kb.paths.metadata_path(&id)) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("warning: skipping {id}: {e}");
                continue;
            }
        };
        if let Some(t) = &tag
            && !meta.tags.iter().any(|x| x == t)
        {
            continue;
        }
        if let Some(k) = kind
            && meta.kind != k
        {
            continue;
        }
        if let Some(p) = &project
            && meta.project.as_deref() != Some(p.as_str())
        {
            continue;
        }
        rows.push(meta);
    }

    match kb.format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&rows).unwrap());
        }
        OutputFormat::Pretty => {
            if rows.is_empty() {
                println!("no documents{}", tag.map(|t| format!(" with tag '{t}'")).unwrap_or_default());
                return Ok(());
            }
            for meta in &rows {
                let marker = match (&meta.kind, &meta.project) {
                    (DocKind::Note, Some(p)) => format!("  (idea: {p})"),
                    (DocKind::Note, None) => "  (idea)".to_string(),
                    _ => String::new(),
                };
                let tags = if meta.tags.is_empty() {
                    String::new()
                } else {
                    format!("  [{}]", meta.tags.join(", "))
                };
                println!("{}  {}{}{}", meta.arxiv_id, meta.title, marker, tags);
            }
            println!("\n{} documents", rows.len());
        }
    }
    Ok(())
}

pub fn show(kb: &Kb, arxiv_id: String) -> Result<(), KbError> {
    let id = canonical_id(&arxiv_id)?;
    let meta = require_paper(kb, &id)?;
    let notes = std::fs::read_to_string(kb.paths.notes_path(&id)).unwrap_or_default();
    let body = if meta.kind == DocKind::Note {
        std::fs::read_to_string(kb.paths.idea_path(&id)).unwrap_or_default()
    } else {
        String::new()
    };
    let db = MetaDb::open(&kb.paths.meta_db_path())?;
    let chunks = db.chunks_for_paper(&id)?;

    match kb.format {
        OutputFormat::Json => {
            let sections: Vec<_> = chunks
                .iter()
                .map(|c| {
                    json!({
                        "chunk_id": c.chunk_id,
                        "section_type": c.section_type.as_str(),
                        "page": c.page,
                        "snippet": c.snippet,
                    })
                })
                .collect();
            let mut payload = json!({
                "metadata": &meta,
                "notes": notes,
                "chunks": sections,
            });
            if meta.kind == DocKind::Note {
                payload["body"] = json!(body);
            }
            println!("{}", serde_json::to_string_pretty(&payload).unwrap());
        }
        OutputFormat::Pretty if meta.kind == DocKind::Note => {
            println!("{}  ({})", meta.title, meta.arxiv_id);
            println!("project:    {}", meta.project.as_deref().unwrap_or("(none)"));
            if !meta.tags.is_empty() {
                println!("tags:       {}", meta.tags.join(", "));
            }
            if !meta.links.is_empty() {
                println!("links:      {}", meta.links.join(", "));
            }
            println!(
                "captured:   {}, updated {}",
                meta.ingested_at, meta.updated_at
            );
            println!("file:       {}", kb.paths.idea_path(&id).display());
            println!("\n{}", body.trim_end());
            if chunks.is_empty() {
                println!("\n(no indexed chunks — run `kb reindex`)");
            }
        }
        OutputFormat::Pretty => {
            println!("{}  ({})", meta.title, meta.arxiv_id);
            println!("authors:    {}", meta.authors.join(", "));
            println!("categories: {}", meta.categories.join(", "));
            println!("published:  {}", meta.published_at);
            if !meta.tags.is_empty() {
                println!("tags:       {}", meta.tags.join(", "));
            }
            println!("source:     {:?}, ingested {}", meta.source_format, meta.ingested_at);
            if let Some(url) = &meta.source_url {
                println!("url:        {url}");
            }
            let pdf_path = kb.paths.pdf_path(&id);
            if pdf_path.exists() {
                println!("pdf:        {}", pdf_path.display());
            }
            if !meta.abstract_text.is_empty() {
                println!("\nabstract:\n{}\n", meta.abstract_text);
            }
            if chunks.is_empty() {
                println!("(no indexed chunks — run `kb reindex`)");
            } else {
                println!("indexed sections:");
                for c in &chunks {
                    let page = c.page.map(|p| format!(" p.{p}")).unwrap_or_default();
                    println!("  {}{}  {}", c.chunk_id, page, c.snippet);
                }
            }
            let notes_body = notes.trim();
            if !notes_body.is_empty() {
                println!("\nnotes.md:\n{notes_body}");
            }
        }
    }
    Ok(())
}

pub async fn similar(kb: &Kb, arxiv_id: String) -> Result<(), KbError> {
    let _ = (kb, arxiv_id);
    Err(planned("similar", "v0.2"))
}

pub fn open_target(kb: &Kb, target: String, section: Option<String>) -> Result<(), KbError> {
    let db = MetaDb::open(&kb.paths.meta_db_path())?;

    // A chunk id (2504.19874_method_0) opens at that chunk's page.
    if let Some(chunk) = db.chunk_by_chunk_id(&target)? {
        return open_pdf(kb, &chunk.paper_id, chunk.page);
    }

    let id = canonical_id(&target)?;
    require_paper(kb, &id)?;
    let page = match &section {
        Some(s) => {
            let stype = SectionType::parse(s).ok_or_else(|| {
                KbError::Usage(format!(
                    "unknown section type '{s}' (expected one of: {})",
                    SectionType::ALL.map(|t| t.as_str()).join(", ")
                ))
            })?;
            db.chunks_for_paper(&id)?
                .iter()
                .find(|c| c.section_type == stype)
                .and_then(|c| c.page)
        }
        None => None,
    };
    if section.is_some() && page.is_none() {
        eprintln!("warning: no page known for that section; opening the PDF at the start");
    }
    open_pdf(kb, &id, page)
}

fn open_pdf(kb: &Kb, paper_id: &str, page: Option<u32>) -> Result<(), KbError> {
    let pdf = kb.paths.pdf_path(paper_id);
    if !pdf.exists() {
        return Err(KbError::NotFound(format!("no PDF at {}", pdf.display())));
    }
    let link = deep_link(&pdf, page, None);
    // Page-anchored file URLs depend on the viewer; fall back to the plain
    // file if the URL form is refused.
    if open::that(&link).is_err() {
        open::that(&pdf).map_err(|e| KbError::Usage(format!("cannot open PDF: {e}")))?;
    }
    println!("{link}");
    Ok(())
}

pub fn stats(kb: &Kb) -> Result<(), KbError> {
    let db = MetaDb::open(&kb.paths.meta_db_path())?;
    let s = db.stats()?;
    let ids = kb.paths.list_paper_ids()?;
    let mut tag_counts: BTreeMap<String, usize> = BTreeMap::new();
    for id in &ids {
        if let Ok(meta) = PaperMetadata::load(&kb.paths.metadata_path(id)) {
            for t in meta.tags {
                *tag_counts.entry(t).or_default() += 1;
            }
        }
    }

    match kb.format {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "papers": ids.len(),
                    "db": s,
                    "tags": tag_counts,
                }))
                .unwrap()
            );
        }
        OutputFormat::Pretty => {
            println!("papers:  {}", ids.len());
            println!("chunks:  {}", s.chunks);
            println!("cache:   {} embeddings", s.cache_entries);
            if !s.chunks_per_section.is_empty() {
                println!("sections:");
                for (name, n) in &s.chunks_per_section {
                    println!("  {name:<14} {n}");
                }
            }
            println!("other ratio: {:.0}%{}", s.other_ratio * 100.0,
                if s.other_ratio > 0.25 { "  ⚠ classifier may need attention (>25%)" } else { "" });
            if !tag_counts.is_empty() {
                let mut tags: Vec<_> = tag_counts.into_iter().collect();
                tags.sort_by(|a, b| b.1.cmp(&a.1));
                let top: Vec<String> = tags.iter().take(10).map(|(t, n)| format!("{t} ({n})")).collect();
                println!("top tags: {}", top.join(", "));
            }
        }
    }
    Ok(())
}

pub fn status(kb: &Kb) -> Result<(), KbError> {
    let papers = kb.paths.list_paper_ids()?.len();
    let index_exists = kb.paths.index_path().exists();

    let (consistency, db_chunks, vectors) = match kb.open_stores() {
        Ok((db, index)) => match consistency_check(&db, &index, false) {
            Ok(r) => (
                if r.ok { "ok".to_string() } else { "OUT OF SYNC — run `kb reindex`".to_string() },
                Some(r.db_chunks),
                Some(r.index_vectors),
            ),
            Err(e) => (format!("check failed: {e}"), None, None),
        },
        Err(e) => (format!("stores unavailable: {e}"), None, None),
    };

    // Watcher liveness via pid file + kill -0 semantics (PRD §9).
    let watcher = match std::fs::read_to_string(kb.paths.pid_path()) {
        Ok(pid_raw) => {
            let pid = pid_raw.trim().to_string();
            let alive = std::process::Command::new("ps")
                .args(["-p", &pid])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if alive {
                format!("running (pid {pid})")
            } else {
                "not running (stale pid file)".to_string()
            }
        }
        Err(_) => "not running".to_string(),
    };

    match kb.format {
        OutputFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "root": kb.paths.root,
                "papers": papers,
                "index_exists": index_exists,
                "db_chunks": db_chunks,
                "index_vectors": vectors,
                "consistency": consistency,
                "watcher": watcher,
            }))
            .unwrap()
        ),
        OutputFormat::Pretty => {
            println!("root:        {}", kb.paths.root.display());
            println!("papers:      {papers}");
            println!(
                "index:       {}{}",
                if index_exists { "present" } else { "absent" },
                match (db_chunks, vectors) {
                    (Some(c), Some(v)) => format!(" ({c} chunks, {v} vectors)"),
                    _ => String::new(),
                }
            );
            println!("consistency: {consistency}");
            println!("watcher:     {watcher}");
        }
    }
    Ok(())
}

pub fn verify(kb: &Kb, deep: bool) -> Result<(), KbError> {
    let (db, index) = kb.open_stores()?;
    let report = consistency_check(&db, &index, deep)?;
    match kb.format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&report).unwrap()),
        OutputFormat::Pretty => {
            println!("meta.db chunks: {}", report.db_chunks);
            println!("index vectors:  {}", report.index_vectors);
            println!("ids checked:    {}{}", report.checked, if deep { " (deep)" } else { " (sample)" });
            if !report.missing_in_index.is_empty() {
                println!("missing in index: {:?}", report.missing_in_index);
            }
            println!("status: {}", if report.ok { "ok" } else { "OUT OF SYNC" });
        }
    }
    if !report.ok {
        return Err(KbError::Index(
            "index out of sync — run `kb reindex` to rebuild".to_string(),
        ));
    }
    Ok(())
}

pub async fn reindex(kb: &Kb, yes: bool) -> Result<(), KbError> {
    let papers = kb.paths.list_paper_ids()?.len();
    if papers == 0 {
        println!("nothing to reindex (no papers in {})", kb.paths.root.display());
        return Ok(());
    }
    if !yes
        && !confirm(&format!(
            "rebuild the index from {papers} papers? (embedding cache keeps API costs near zero)"
        ))?
    {
        println!("aborted");
        return Ok(());
    }
    let t0 = std::time::Instant::now();
    let progress = |msg: &str| eprintln!("… {msg}");
    let (n_papers, n_chunks) = pipeline::reindex_all(&kb.paths, &kb.config, &progress).await?;
    println!(
        "reindexed {n_papers} papers, {n_chunks} chunks in {:.1}s",
        t0.elapsed().as_secs_f64()
    );
    Ok(())
}

pub fn gc(kb: &Kb) -> Result<(), KbError> {
    let (db, mut index) = kb.open_stores()?;

    // Orphans: chunks whose paper folder no longer exists on disk.
    let folders: std::collections::HashSet<String> =
        kb.paths.list_paper_ids()?.into_iter().collect();
    let all_ids = db.all_vector_ids()?;
    let orphan_papers: std::collections::HashSet<String> = db
        .chunks_by_vector_ids(&all_ids)?
        .into_iter()
        .map(|c| c.paper_id)
        .filter(|p| !folders.contains(p))
        .collect();

    let mut removed_chunks = 0usize;
    if !orphan_papers.is_empty() {
        db.begin_immediate()?;
        let result = (|| -> Result<usize, KbError> {
            let mut n = 0;
            for paper in &orphan_papers {
                for vid in db.remove_paper(paper)? {
                    index.remove(vid as u64);
                    n += 1;
                }
            }
            index.save_atomic(&kb.paths.index_path())?;
            Ok(n)
        })();
        match result {
            Ok(n) => {
                db.commit()?;
                removed_chunks = n;
            }
            Err(e) => {
                let _ = db.rollback();
                return Err(e);
            }
        }
    }

    let cache_removed = db.cache_gc()?;
    println!(
        "gc: removed {} orphaned chunks from {} papers, {} stale cache entries",
        removed_chunks,
        orphan_papers.len(),
        cache_removed
    );
    Ok(())
}

pub fn cache_clear(kb: &Kb) -> Result<(), KbError> {
    let db = MetaDb::open(&kb.paths.meta_db_path())?;
    let n = db.cache_clear()?;
    println!("cleared {n} cached embeddings");
    Ok(())
}

pub fn cache_gc(kb: &Kb) -> Result<(), KbError> {
    let db = MetaDb::open(&kb.paths.meta_db_path())?;
    let n = db.cache_gc()?;
    println!("removed {n} stale cache entries");
    Ok(())
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

pub fn excerpt(kb: &Kb, chunk_ids: Vec<String>, out: PathBuf) -> Result<(), KbError> {
    let _ = (kb, chunk_ids, out);
    Err(planned("excerpt", "v0.2"))
}
