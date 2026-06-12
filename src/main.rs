//! `kb` — thin clap dispatcher over `kb::commands` (PRD §6).

use clap::{Parser, Subcommand, ValueEnum};
use kb::commands::{self, Kb, OutputFormat};
use kb::search::{SearchFilters, SearchMode};
use kb::{DocKind, KbError, SectionType};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "kb",
    version,
    about = "arxiv-kb — a personal knowledge base for arXiv papers with semantic search"
)]
struct Cli {
    /// KB root folder (default: $KB_ROOT or ~/arxiv-kb)
    #[arg(long, global = true)]
    root: Option<PathBuf>,

    /// Output format
    #[arg(long, global = true, value_enum, default_value_t = FormatArg::Pretty)]
    format: FormatArg,

    /// Debug logging to stderr
    #[arg(long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    cmd: Command,
}

#[derive(Copy, Clone, ValueEnum)]
enum FormatArg {
    Pretty,
    Json,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize the KB root folder and default config
    Init,
    /// Ingest a paper by arXiv id or URL, a local PDF via --pdf, or a web
    /// page via --url
    Add {
        /// arXiv id or URL (e.g. 2504.19874, https://arxiv.org/abs/2504.19874)
        id_or_url: Option<String>,
        /// Ingest a local PDF instead; its id is the slugified filename
        /// (My Paper.pdf → my-paper)
        #[arg(long)]
        pdf: Option<PathBuf>,
        /// Ingest a web page instead; readability-extracted, id is a slug of
        /// the URL (https://example.com/post → example-com-post-a3f9c2)
        #[arg(long)]
        url: Option<String>,
    },
    /// Re-fetch and re-ingest a paper (e.g. after a new arXiv version)
    Update { arxiv_id: String },
    /// Remove a paper from the index AND delete its folder
    Remove {
        arxiv_id: String,
        /// Skip the confirmation prompt
        #[arg(long)]
        yes: bool,
    },
    /// Open the paper's notes.md in $EDITOR
    Note { arxiv_id: String },
    /// Capture standalone ideas, keyed by project
    Idea {
        #[command(subcommand)]
        cmd: IdeaCmd,
    },
    /// Add (+tag) or remove (-tag) tags on a paper
    Tag {
        arxiv_id: String,
        /// e.g. +consumer +quantization -stale
        #[arg(required = true, allow_hyphen_values = true)]
        tags: Vec<String>,
    },
    /// Semantic search across the corpus
    Search {
        query: String,
        /// Wide mode: more results, no score floor (for synthesis)
        #[arg(long)]
        wide: bool,
        /// Number of results (default: 10 narrow / 40 wide)
        #[arg(short)]
        k: Option<usize>,
        /// Restrict to section types (comma-separated, e.g. method,applications)
        #[arg(long, value_delimiter = ',')]
        section: Vec<String>,
        /// Restrict to papers with this tag (repeatable)
        #[arg(long)]
        tag: Vec<String>,
        /// Restrict to these paper ids (repeatable)
        #[arg(long)]
        paper: Vec<String>,
        /// Restrict to one document kind
        #[arg(long, value_enum, default_value_t = KindArg::All)]
        kind: KindArg,
        /// Restrict ideas to these projects (repeatable; e.g. --project kitgig --project global)
        #[arg(long)]
        project: Vec<String>,
    },
    /// List all papers and ideas
    List {
        #[arg(long)]
        tag: Option<String>,
        /// Restrict to one document kind
        #[arg(long, value_enum, default_value_t = KindArg::All)]
        kind: KindArg,
        /// Restrict ideas to this project
        #[arg(long)]
        project: Option<String>,
    },
    /// Paper details: metadata + notes + sections summary
    Show { arxiv_id: String },
    /// Papers semantically near this one (v0.2)
    Similar { arxiv_id: String },
    /// Open the PDF in the default viewer, optionally at a section/chunk
    Open {
        /// arXiv id or chunk id (e.g. 2504.19874 or 2504.19874_method_0)
        target: String,
        /// Open at this section's page (e.g. method)
        #[arg(long)]
        section: Option<String>,
    },
    /// Assemble selected chunks' page ranges into one PDF (v0.2)
    Excerpt {
        #[arg(required = true)]
        chunk_ids: Vec<String>,
        #[arg(long)]
        out: PathBuf,
    },
    /// Corpus summary: papers, chunks, section breakdown, top tags
    Stats,
    /// Watcher/index health
    Status,
    /// On-demand index ↔ meta.db consistency check
    Verify {
        /// Check every chunk, not a sample
        #[arg(long)]
        deep: bool,
    },
    /// Rebuild index + meta.db from canonical files
    Reindex {
        /// Skip the confirmation prompt
        #[arg(long)]
        yes: bool,
    },
    /// Remove orphaned chunks and stale derived state
    Gc,
    /// Embedding cache maintenance
    Cache {
        #[command(subcommand)]
        cmd: CacheCmd,
    },
    /// Watch the KB folder and re-index on changes (foreground)
    Watch {
        /// Run as a background daemon (v0.2)
        #[arg(long)]
        daemon: bool,
    },
    /// MCP server on stdio (for Claude Code)
    Mcp,
    /// HTTP server (v0.2)
    Serve {
        #[arg(long, default_value_t = 4321)]
        port: u16,
    },
}

#[derive(Subcommand)]
enum CacheCmd {
    /// Drop all cached embeddings
    Clear,
    /// Remove cache entries no longer referenced by any chunk
    Gc,
}

#[derive(Subcommand)]
enum IdeaCmd {
    /// Capture a new idea (same title or id again = update in place)
    Add {
        /// Project this idea is keyed to ('global' = applies everywhere)
        #[arg(long)]
        project: String,
        /// Idea title; also derives the id (slugified)
        #[arg(long)]
        title: String,
        /// Body text, '-' for stdin; omit to compose in $EDITOR
        #[arg(long)]
        body: Option<String>,
        /// Tags (comma-separated)
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        /// Related paper/idea ids (repeatable)
        #[arg(long = "link")]
        links: Vec<String>,
    },
}

/// `--kind` filter: a concrete kind, or `all` (no filter).
#[derive(Copy, Clone, ValueEnum)]
enum KindArg {
    Paper,
    Note,
    All,
}

impl KindArg {
    fn to_filter(self) -> Option<DocKind> {
        match self {
            KindArg::Paper => Some(DocKind::Paper),
            KindArg::Note => Some(DocKind::Note),
            KindArg::All => None,
        }
    }
}

fn init_tracing(verbose: bool) {
    use tracing_subscriber::EnvFilter;
    let default = if verbose { "debug" } else { "warn" };
    let filter = EnvFilter::try_from_env("KB_LOG_LEVEL")
        .unwrap_or_else(|_| EnvFilter::new(default));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}

async fn run(cli: Cli) -> Result<(), KbError> {
    let format = match cli.format {
        FormatArg::Pretty => OutputFormat::Pretty,
        FormatArg::Json => OutputFormat::Json,
    };
    let kb = Kb::open(cli.root, format)?;

    match cli.cmd {
        Command::Init => commands::init(&kb),
        Command::Add { id_or_url, pdf, url } => commands::add(&kb, id_or_url, pdf, url).await,
        Command::Update { arxiv_id } => commands::update(&kb, arxiv_id).await,
        Command::Remove { arxiv_id, yes } => commands::remove(&kb, arxiv_id, yes).await,
        Command::Note { arxiv_id } => commands::note(&kb, arxiv_id).await,
        Command::Idea { cmd } => match cmd {
            IdeaCmd::Add {
                project,
                title,
                body,
                tags,
                links,
            } => commands::idea_add(&kb, project, title, body, tags, links).await,
        },
        Command::Tag { arxiv_id, tags } => commands::tag(&kb, arxiv_id, tags),
        Command::Search {
            query,
            wide,
            k,
            section,
            tag,
            paper,
            kind,
            project,
        } => {
            let mode = if wide { SearchMode::Wide } else { SearchMode::Narrow };
            let section_types = if section.is_empty() {
                None
            } else {
                let mut types = Vec::new();
                for s in &section {
                    match SectionType::parse(s) {
                        Some(t) => types.push(t),
                        None => {
                            return Err(KbError::Usage(format!(
                                "unknown section type '{s}' (expected one of: {})",
                                SectionType::ALL.map(|t| t.as_str()).join(", ")
                            )))
                        }
                    }
                }
                Some(types)
            };
            let filters = SearchFilters {
                section_types,
                paper_ids: if paper.is_empty() { None } else { Some(paper) },
                tags: if tag.is_empty() { None } else { Some(tag) },
                kind: kind.to_filter(),
                projects: if project.is_empty() { None } else { Some(project) },
            };
            commands::search(&kb, query, mode, k, filters).await
        }
        Command::List { tag, kind, project } => {
            commands::list(&kb, tag, kind.to_filter(), project)
        }
        Command::Show { arxiv_id } => commands::show(&kb, arxiv_id),
        Command::Similar { arxiv_id } => commands::similar(&kb, arxiv_id).await,
        Command::Open { target, section } => commands::open_target(&kb, target, section),
        Command::Excerpt { chunk_ids, out } => commands::excerpt(&kb, chunk_ids, out),
        Command::Stats => commands::stats(&kb),
        Command::Status => commands::status(&kb),
        Command::Verify { deep } => commands::verify(&kb, deep),
        Command::Reindex { yes } => commands::reindex(&kb, yes).await,
        Command::Gc => commands::gc(&kb),
        Command::Cache { cmd } => match cmd {
            CacheCmd::Clear => commands::cache_clear(&kb),
            CacheCmd::Gc => commands::cache_gc(&kb),
        },
        Command::Watch { daemon } => commands::watch(kb, daemon).await,
        Command::Mcp => commands::mcp(kb).await,
        Command::Serve { port } => commands::serve(&kb, port).await,
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    init_tracing(cli.verbose);
    if let Err(e) = run(cli).await {
        eprintln!("error: {e}");
        std::process::exit(e.exit_code());
    }
}
