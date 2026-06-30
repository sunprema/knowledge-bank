//! HTTP server on `127.0.0.1` (PRD §8): a minimal REST surface for tools
//! that don't speak MCP — browser extensions, curl scripts, alternative AI
//! clients. Launched via `kb serve [--port 4321]`.
//!
//! Auth: every request (except `/health`, a pure liveness probe) must carry
//! an `X-KB-Key: <key>` header matching the key in `.arxiv-kb/api_key`
//! (generated on first run, overridable via `KB_API_KEY`). The socket binds
//! to loopback only — never `0.0.0.0`.

use std::net::IpAddr;
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, Query, Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{
        Html, IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_stream::{StreamExt, wrappers::ReceiverStream, wrappers::UnboundedReceiverStream};
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::agents::roundtable::{self, BrainstormRequest};
use crate::chat::ChatMessage;
use crate::config::{Config, KbPaths};
use crate::index::MetaDb;
use crate::ingest::pipeline;
use crate::search::{SearchFilters, SearchMode, retrieval};
use crate::{DocKind, KbError, PaperMetadata, SectionType, deep_link};

struct AppState {
    paths: KbPaths,
    config: Config,
    api_key: String,
    /// Live roundtables, keyed by client session id → interjection sender. Lets
    /// `POST /brainstorm/{id}/interject` push guidance into a running debate.
    roundtables: std::sync::Mutex<std::collections::HashMap<String, tokio::sync::mpsc::Sender<String>>>,
}

type Shared = Arc<AppState>;

/// Single-file browser app (paper browser + analytics), served at `/`. It
/// holds the API key in `localStorage` and sends it as `X-KB-Key` on the
/// fetch calls to the JSON endpoints below.
const WEB_UI: &str = include_str!("webui.html");

/// Entry point for `kb serve`. Validates the index up front (HTTP is a query
/// mode — addendum §7: refuse to start on an out-of-sync index), resolves the
/// API key, then serves until Ctrl-C.
pub async fn run(paths: KbPaths, config: Config, port: u16) -> Result<(), KbError> {
    // Fail fast if the stores are unusable, rather than 500ing every request.
    let _ = retrieval::open_stores_for_query(&paths, &config)?;

    let api_key = load_or_create_api_key(&paths)?;

    let bind: IpAddr = config.server.http_bind.parse().map_err(|_| {
        KbError::Config(format!(
            "invalid server.http_bind '{}' (expected an IP like 127.0.0.1)",
            config.server.http_bind
        ))
    })?;
    if !bind.is_loopback() {
        return Err(KbError::Config(format!(
            "refusing to bind {bind}: the HTTP server is loopback-only (PRD §8); set server.http_bind to 127.0.0.1"
        )));
    }

    let state: Shared = Arc::new(AppState {
        paths,
        config,
        api_key,
        roundtables: std::sync::Mutex::new(std::collections::HashMap::new()),
    });

    let app = router(state.clone());

    let listener = tokio::net::TcpListener::bind((bind, port))
        .await
        .map_err(|e| KbError::Config(format!("cannot bind {bind}:{port}: {e}")))?;

    eprintln!("kb serve listening on http://{bind}:{port}");
    eprintln!("  web UI:  http://{bind}:{port}/?key={}", state.api_key);
    eprintln!("  API key: {}", state.api_key);
    eprintln!("  clients send header  X-KB-Key: {}", state.api_key);
    if is_browser_blocked_port(port) {
        eprintln!(
            "  ⚠ port {port} is on browsers' unsafe-port blocklist — Chrome/Firefox will refuse\n    \
             to load the web UI (ERR_UNSAFE_PORT). curl still works; for the browser, restart\n    \
             with a different port, e.g. `kb serve --port 4321`."
        );
    }

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|e| KbError::Index(format!("http server: {e}")))?;
    Ok(())
}

fn router(state: Shared) -> Router {
    // `/health` stays unauthenticated so liveness probes don't need the key;
    // every other route sits behind the X-KB-Key check.
    let protected = Router::new()
        .route("/stats", get(stats))
        .route("/papers", get(list_papers))
        .route("/papers/{paper_id}", get(get_paper))
        .route("/papers/{paper_id}/notes", post(add_note).put(put_notes))
        .route("/papers/{paper_id}/reader", post(generate_reader_stream).get(get_reader))
        .route("/papers/{paper_id}/similar", get(similar))
        .route("/graph", get(graph))
        .route("/sparks", get(sparks))
        .route("/brief", get(brief))
        .route("/watches", get(list_watches).post(add_watch))
        .route("/watches/{watch_id}", axum::routing::delete(remove_watch))
        .route("/watch/refresh", post(watch_refresh))
        .route("/watch/candidates/{arxiv_id}/status", post(set_candidate_status))
        .route("/bookmarks", get(list_bookmarks).post(add_bookmark))
        .route("/bookmarks/{paper_id}", axum::routing::delete(remove_bookmark))
        .route("/search", post(search))
        .route("/problems", post(problems))
        .route("/chat", post(chat))
        .route("/chat/stream", post(chat_stream))
        .route("/brainstorm", post(brainstorm))
        .route("/brainstorm/{session_id}/interject", post(interject))
        .route("/ingest", post(ingest))
        .route("/ideas", post(create_idea))
        .route("/notes", get(list_notes).post(create_note))
        .route("/notes/{note_id}", get(get_note).put(update_note).delete(delete_note))
        .route("/reflections", post(create_reflection))
        .route("/compose/assist", post(compose_assist))
        .route("/chunks/{chunk_id}", get(get_chunk))
        .route("/pdf/{paper_id}", get(get_pdf))
        .route("/open/{chunk_id}", get(open_chunk))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth));

    Router::new()
        .route("/", get(web_ui))
        .route("/health", get(health))
        .merge(protected)
        .layer(cors_layer())
        .with_state(state)
}

async fn web_ui() -> Html<&'static str> {
    Html(WEB_UI)
}

/// Default-deny CORS that allows loopback origins (any port) and Chrome
/// extensions, plus the `X-KB-Key` request header (PRD §8 CORS).
fn cors_layer() -> CorsLayer {
    let allow = AllowOrigin::predicate(|origin: &HeaderValue, _req| {
        let Ok(o) = origin.to_str() else {
            return false;
        };
        o.starts_with("http://localhost")
            || o.starts_with("http://127.0.0.1")
            || o.starts_with("chrome-extension://")
    });
    CorsLayer::new()
        .allow_origin(allow)
        .allow_methods(tower_http::cors::Any)
        .allow_headers([header::CONTENT_TYPE, header::HeaderName::from_static("x-kb-key")])
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutdown signal received");
}

// ---- auth ------------------------------------------------------------------

async fn auth(
    State(state): State<Shared>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    // Header is the norm for API clients. Top-level navigations and embeds
    // (opening a PDF in a new tab, an <iframe>) can't set custom headers, so we
    // also accept `?key=` — safe here because the server is loopback-only.
    let presented = req
        .headers()
        .get("x-kb-key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .or_else(|| {
            req.uri().query().and_then(|q| {
                url::form_urlencoded::parse(q.as_bytes())
                    .find(|(k, _)| k == "key")
                    .map(|(_, v)| v.into_owned())
            })
        })
        .unwrap_or_default();
    if constant_time_eq(presented.as_bytes(), state.api_key.as_bytes()) {
        Ok(next.run(req).await)
    } else {
        Err(ApiError {
            status: StatusCode::UNAUTHORIZED,
            message: "missing or invalid API key (X-KB-Key header or ?key=)".into(),
        })
    }
}

/// Length-independent, branch-free byte comparison so auth failures don't leak
/// the key through timing.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Ports Chromium- and Firefox-based browsers refuse to connect to (they fail
/// with `ERR_UNSAFE_PORT` before reaching the server). Binding still succeeds —
/// curl and other clients work — so this only drives a startup warning, not a
/// hard error. List mirrors Chromium's `kRestrictedPorts`.
fn is_browser_blocked_port(port: u16) -> bool {
    const BLOCKED: &[u16] = &[
        1, 7, 9, 11, 13, 15, 17, 19, 20, 21, 22, 23, 25, 37, 42, 43, 53, 69, 77, 79, 87, 95, 101,
        102, 103, 104, 109, 110, 111, 113, 115, 117, 119, 123, 135, 137, 139, 143, 161, 179, 389,
        427, 465, 512, 513, 514, 515, 526, 530, 531, 532, 540, 548, 554, 556, 563, 587, 601, 636,
        989, 990, 993, 995, 1719, 1720, 1723, 2049, 3659, 4045, 4190, 5060, 5061, 6000, 6566, 6665,
        6666, 6667, 6668, 6669, 6679, 6697, 10080,
    ];
    BLOCKED.contains(&port)
}

/// `KB_API_KEY` env var (wins, never persisted) > `.arxiv-kb/api_key` >
/// freshly generated 32-byte key, stored mode-0600.
pub fn load_or_create_api_key(paths: &KbPaths) -> Result<String, KbError> {
    if let Some(k) = std::env::var_os("KB_API_KEY") {
        let k = k.to_string_lossy().trim().to_string();
        if !k.is_empty() {
            return Ok(k);
        }
    }

    let path = paths.api_key_path();
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let existing = existing.trim().to_string();
        if !existing.is_empty() {
            return Ok(existing);
        }
    }

    let key = generate_api_key()?;
    write_api_key(paths, &key)?;
    Ok(key)
}

/// Generate and persist a fresh key, replacing any existing one (`kb
/// rotate-key`). Returns the new key.
pub fn rotate_api_key(paths: &KbPaths) -> Result<String, KbError> {
    let key = generate_api_key()?;
    write_api_key(paths, &key)?;
    Ok(key)
}

fn generate_api_key() -> Result<String, KbError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|e| KbError::Config(format!("cannot generate API key: {e}")))?;
    Ok(hex::encode(bytes))
}

fn write_api_key(paths: &KbPaths, key: &str) -> Result<(), KbError> {
    let dir = paths.dot_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| KbError::Config(format!("create {}: {e}", dir.display())))?;
    let path = paths.api_key_path();
    std::fs::write(&path, key)
        .map_err(|e| KbError::Config(format!("write {}: {e}", path.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| KbError::Config(format!("chmod {}: {e}", path.display())))?;
    }
    Ok(())
}

// ---- handlers --------------------------------------------------------------

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok", "version": env!("CARGO_PKG_VERSION") }))
}

async fn stats(State(state): State<Shared>) -> Result<Json<Value>, ApiError> {
    let db = MetaDb::open(&state.paths.meta_db_path())?;
    let s = db.stats()?;
    let ids = state.paths.list_paper_ids()?;
    let mut tag_counts: std::collections::BTreeMap<String, usize> = Default::default();
    for id in &ids {
        if let Ok(meta) = PaperMetadata::load(&state.paths.metadata_path(id)) {
            for t in meta.tags {
                *tag_counts.entry(t).or_default() += 1;
            }
        }
    }
    Ok(Json(json!({
        "papers": ids.len(),
        "db": s,
        "tags": tag_counts,
    })))
}

#[derive(Deserialize)]
struct PapersQuery {
    tag: Option<String>,
    category: Option<String>,
}

async fn list_papers(
    State(state): State<Shared>,
    Query(q): Query<PapersQuery>,
) -> Result<Json<Vec<PaperMetadata>>, ApiError> {
    let mut rows = Vec::new();
    for id in state.paths.list_paper_ids()? {
        let meta = match PaperMetadata::load(&state.paths.metadata_path(&id)) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("skipping {id}: {e}");
                continue;
            }
        };
        if let Some(t) = &q.tag
            && !meta.tags.iter().any(|x| x == t)
        {
            continue;
        }
        if let Some(c) = &q.category
            && !meta.categories.iter().any(|x| x == c)
        {
            continue;
        }
        rows.push(meta);
    }
    Ok(Json(rows))
}

async fn get_paper(
    State(state): State<Shared>,
    Path(paper_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let meta_path = state.paths.metadata_path(&paper_id);
    if !meta_path.exists() {
        return Err(KbError::NotFound(format!("{paper_id} is not in the KB")).into());
    }
    let meta = PaperMetadata::load(&meta_path)?;
    let notes = std::fs::read_to_string(state.paths.notes_path(&paper_id)).unwrap_or_default();
    // Only hand the UI a `pdf_path` when the PDF actually exists. Markdown/HTML
    // docs (ideas, reflections, web pages) have no `paper.pdf`; returning a
    // bogus path made the app try to render a PDF and fail with "Couldn't open
    // the PDF" instead of showing the document's markdown content.
    let pdf = state.paths.pdf_path(&paper_id);
    let pdf_path = if pdf.exists() { Some(pdf) } else { None };
    let bookmarked = MetaDb::open(&state.paths.meta_db_path())?.is_bookmarked(&paper_id)?;
    Ok(Json(json!({
        "metadata": meta,
        "notes": notes,
        "pdf_path": pdf_path,
        "has_reader": state.paths.reader_path(&paper_id).exists(),
        "bookmarked": bookmarked,
    })))
}

#[derive(Deserialize)]
struct SearchRequest {
    query: String,
    mode: Option<String>,
    k: Option<usize>,
    section_types: Option<Vec<String>>,
    tags: Option<Vec<String>>,
    paper_ids: Option<Vec<String>>,
    kind: Option<String>,
    project: Option<Vec<String>>,
}

async fn search(
    State(state): State<Shared>,
    Json(req): Json<SearchRequest>,
) -> Result<Json<retrieval::SearchResponse>, ApiError> {
    let mode = match req.mode.as_deref().unwrap_or("narrow") {
        "wide" => SearchMode::Wide,
        _ => SearchMode::Narrow,
    };
    let k = req.k.map(|k| k.clamp(1, 100));

    let section_types = match req.section_types {
        Some(names) => {
            let mut types = Vec::new();
            for n in &names {
                types.push(SectionType::parse(n).ok_or_else(|| {
                    KbError::Usage(format!(
                        "unknown section type '{n}' (expected one of: {})",
                        SectionType::ALL.map(|t| t.as_str()).join(", ")
                    ))
                })?);
            }
            Some(types)
        }
        None => None,
    };

    let kind = match req.kind.as_deref() {
        None | Some("all") => None,
        Some(other) => Some(DocKind::parse(other).ok_or_else(|| {
            KbError::Usage(format!("unknown kind '{other}' (expected paper, note, or all)"))
        })?),
    };

    let filters = SearchFilters {
        section_types,
        paper_ids: non_empty(req.paper_ids),
        tags: non_empty(req.tags),
        kind,
        projects: non_empty(req.project),
    };

    let paths = state.paths.clone();
    let config = state.config.clone();
    let query = req.query;
    let response = run_blocking(move || async move {
        retrieval::search(&paths, &config, &query, mode, k, filters).await
    })
    .await?;
    Ok(Json(response))
}

#[derive(Deserialize)]
struct ProblemsRequest {
    /// Optional topic to focus the hunt; omitted = scan broadly.
    domain: Option<String>,
    k: Option<usize>,
}

/// Problem hunting (ResearchAgent): surface unsolved problems (papers'
/// limitations/future_work) paired with the nearest method/applications work
/// elsewhere in the corpus. Embeds across `.await` while holding the stores, so
/// it runs on a blocking thread (same as `/search`).
async fn problems(
    State(state): State<Shared>,
    Json(req): Json<ProblemsRequest>,
) -> Result<Json<retrieval::ProblemsResponse>, ApiError> {
    let k = req.k.unwrap_or(8).clamp(1, 30);
    let paths = state.paths.clone();
    let config = state.config.clone();
    let domain = req.domain;
    let resp = run_blocking(move || async move {
        retrieval::find_problems(&paths, &config, domain.as_deref(), k).await
    })
    .await?;
    Ok(Json(resp))
}

#[derive(Deserialize)]
struct SimilarQuery {
    limit: Option<usize>,
}

/// Documents most similar to `{paper_id}` (the "Related" panel). Embeds across
/// `.await` while holding the stores, so it runs on a blocking thread.
async fn similar(
    State(state): State<Shared>,
    Path(paper_id): Path<String>,
    Query(q): Query<SimilarQuery>,
) -> Result<Json<retrieval::SimilarResponse>, ApiError> {
    let limit = q.limit.unwrap_or(8).clamp(1, 50);
    let paths = state.paths.clone();
    let config = state.config.clone();
    let resp = run_blocking(move || async move {
        retrieval::similar_papers(&paths, &config, &paper_id, limit).await
    })
    .await?;
    Ok(Json(resp))
}

#[derive(Deserialize)]
struct GraphQuery {
    /// Nearest-neighbor "similar" edges per node (0 = explicit links only).
    neighbors: Option<usize>,
}

async fn graph(
    State(state): State<Shared>,
    Query(q): Query<GraphQuery>,
) -> Result<Json<retrieval::GraphResponse>, ApiError> {
    let neighbors = q.neighbors.unwrap_or(3).min(10);
    let paths = state.paths.clone();
    let config = state.config.clone();
    // Heavy (a centroid + index search per node) and touches the !Send MetaDb;
    // run it off the async worker.
    let resp = run_blocking(move || async move {
        retrieval::knowledge_graph(&paths, &config, neighbors)
    })
    .await?;
    Ok(Json(resp))
}

#[derive(Deserialize)]
struct SparksQuery {
    limit: Option<usize>,
    kind: Option<String>,
}

/// The Cortex associative layer: the most surprising cross-document
/// connections, most surprising first. Read-only over `meta.db` (no API calls).
async fn sparks(
    State(state): State<Shared>,
    Query(q): Query<SparksQuery>,
) -> Result<Json<Value>, ApiError> {
    let limit = q.limit.unwrap_or(0).min(500);
    let kind = crate::cortex::parse_kind_filter(q.kind.as_deref())?.map(str::to_string);
    let paths = state.paths.clone();
    let config = state.config.clone();
    let list = run_blocking(move || async move {
        crate::cortex::list_sparks(&paths, &config, limit, kind.as_deref())
    })
    .await?;
    Ok(Json(json!({ "sparks": list })))
}

// ---- ArXiv Watch + Daily Brief ---------------------------------------------

/// The daily brief: new candidate papers, fresh Sparks, one resurfaced
/// reflection, and headline stats. Read-only over `meta.db` (no API calls)
/// except for advancing the resurfacing rotation cursor.
async fn brief(State(state): State<Shared>) -> Result<Json<crate::watch::Brief>, ApiError> {
    let paths = state.paths.clone();
    let b = run_blocking(move || async move {
        crate::watch::brief(&paths, crate::watch::DEFAULT_BRIEF_PAPERS)
    })
    .await?;
    Ok(Json(b))
}

async fn list_watches(
    State(state): State<Shared>,
) -> Result<Json<Vec<crate::index::Watch>>, ApiError> {
    let db = MetaDb::open(&state.paths.meta_db_path())?;
    Ok(Json(db.list_watches()?))
}

#[derive(Deserialize)]
struct AddWatchRequest {
    kind: String,
    value: String,
}

async fn add_watch(
    State(state): State<Shared>,
    Json(req): Json<AddWatchRequest>,
) -> Result<Json<Value>, ApiError> {
    crate::watch::validate_kind(&req.kind)?;
    let value = req.value.trim().to_string();
    if value.is_empty() {
        return Err(KbError::Usage("watch value must not be empty".into()).into());
    }
    let db = MetaDb::open(&state.paths.meta_db_path())?;
    let id = db.add_watch(&req.kind, &value)?;
    Ok(Json(json!({
        "id": id,
        "kind": req.kind,
        "value": value,
        "search_query": crate::watch::search_query_for(&req.kind, &value),
    })))
}

async fn remove_watch(
    State(state): State<Shared>,
    Path(watch_id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let db = MetaDb::open(&state.paths.meta_db_path())?;
    if !db.remove_watch(watch_id)? {
        return Err(KbError::NotFound(format!("no watch #{watch_id}")).into());
    }
    Ok(Json(json!({ "removed": watch_id })))
}

// ---- bookmarks -------------------------------------------------------------

/// Bookmarked documents, most recently bookmarked first. Returns full metadata
/// (like `/papers`) so the UI can render covers without a second round-trip.
/// Bookmarks whose document no longer exists are silently skipped.
async fn list_bookmarks(
    State(state): State<Shared>,
) -> Result<Json<Vec<PaperMetadata>>, ApiError> {
    let db = MetaDb::open(&state.paths.meta_db_path())?;
    let mut rows = Vec::new();
    for id in db.list_bookmark_ids()? {
        match PaperMetadata::load(&state.paths.metadata_path(&id)) {
            Ok(m) => rows.push(m),
            Err(e) => tracing::warn!("skipping bookmark {id}: {e}"),
        }
    }
    Ok(Json(rows))
}

#[derive(Deserialize)]
struct AddBookmarkRequest {
    paper_id: String,
}

async fn add_bookmark(
    State(state): State<Shared>,
    Json(req): Json<AddBookmarkRequest>,
) -> Result<Json<Value>, ApiError> {
    let paper_id = req.paper_id.trim().to_string();
    if paper_id.is_empty() {
        return Err(KbError::Usage("paper_id must not be empty".into()).into());
    }
    if !state.paths.metadata_path(&paper_id).exists() {
        return Err(KbError::NotFound(format!("{paper_id} is not in the KB")).into());
    }
    let db = MetaDb::open(&state.paths.meta_db_path())?;
    let created = db.add_bookmark(&paper_id)?;
    Ok(Json(json!({ "paper_id": paper_id, "bookmarked": true, "created": created })))
}

async fn remove_bookmark(
    State(state): State<Shared>,
    Path(paper_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let db = MetaDb::open(&state.paths.meta_db_path())?;
    if !db.remove_bookmark(&paper_id)? {
        return Err(KbError::NotFound(format!("{paper_id} is not bookmarked")).into());
    }
    Ok(Json(json!({ "paper_id": paper_id, "bookmarked": false })))
}

/// Poll every enabled watch and score new papers against the corpus. Network +
/// embedding work, so it can take a few seconds; the macOS app calls it on
/// launch and on a timer.
async fn watch_refresh(
    State(state): State<Shared>,
) -> Result<Json<crate::watch::RefreshSummary>, ApiError> {
    let paths = state.paths.clone();
    let config = state.config.clone();
    let summary = run_blocking(move || async move {
        crate::watch::refresh(&paths, &config).await
    })
    .await?;
    Ok(Json(summary))
}

#[derive(Deserialize)]
struct CandidateStatusRequest {
    /// 'ingested' | 'dismissed' | 'new'
    status: String,
}

/// Mark a candidate paper ingested or dismissed so it leaves the brief's
/// "new" list. The app calls this after a successful ingest, or on dismiss.
async fn set_candidate_status(
    State(state): State<Shared>,
    Path(arxiv_id): Path<String>,
    Json(req): Json<CandidateStatusRequest>,
) -> Result<Json<Value>, ApiError> {
    const VALID: [&str; 3] = ["new", "ingested", "dismissed"];
    if !VALID.contains(&req.status.as_str()) {
        return Err(KbError::Usage(format!(
            "unknown status {:?}; expected one of: {}",
            req.status,
            VALID.join(", ")
        ))
        .into());
    }
    let db = MetaDb::open(&state.paths.meta_db_path())?;
    if !db.set_candidate_status(&arxiv_id, &req.status)? {
        return Err(KbError::NotFound(format!("no candidate {arxiv_id}")).into());
    }
    Ok(Json(json!({ "arxiv_id": arxiv_id, "status": req.status })))
}

#[derive(Deserialize)]
struct ChatRequest {
    query: String,
    #[serde(default)]
    history: Vec<ChatMessage>,
    /// Optional persona to answer in (the `@persona` chat mode). Absent ⇒ the
    /// default research-assistant chat on the configured model.
    #[serde(default)]
    persona: Option<retrieval::PersonaChat>,
}

/// Chat-over-corpus: wide-retrieve context, answer with the chat model, return
/// the answer plus cited sources. Requires `OPENAI_API_KEY` (same key the rest
/// of the KB uses).
async fn chat(
    State(state): State<Shared>,
    Json(req): Json<ChatRequest>,
) -> Result<Json<retrieval::ChatResponse>, ApiError> {
    let paths = state.paths.clone();
    let config = state.config.clone();
    let resp = run_blocking(move || async move {
        retrieval::chat(&paths, &config, &req.query, &req.history, req.persona.as_ref()).await
    })
    .await?;
    Ok(Json(resp))
}

/// One SSE event of a streamed chat answer (`POST /chat/stream`). `type`-tagged
/// so the client switches on it: `searching` (retrieval started) → `sources`
/// (citations, once) → `delta` (answer tokens, in order) → `done` (final answer)
/// — or `error`.
#[derive(serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ChatStreamWire {
    Searching,
    Sources { sources: Vec<retrieval::ChatSource> },
    Delta { text: String },
    Done { answer: String },
    Error { message: String },
}

/// Streaming chat-over-corpus (SSE). Same body as `POST /chat`, but the answer
/// arrives token-by-token: the engine wide-retrieves, emits the cited `sources`,
/// then streams `delta`s as the model writes. The work runs in a spawned task
/// pushing events over a channel; this handler just streams the channel (a
/// missing/invalid provider key surfaces as an `error` event, not a 500).
async fn chat_stream(
    State(state): State<Shared>,
    Json(req): Json<ChatRequest>,
) -> impl IntoResponse {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<ChatStreamWire>();
    let st = state.clone();
    tokio::spawn(async move {
        let _ = tx.send(ChatStreamWire::Searching);
        let sink = tx.clone();
        let result = retrieval::chat_stream(
            &st.paths,
            &st.config,
            &req.query,
            &req.history,
            req.persona.as_ref(),
            |ev| {
                let wire = match ev {
                    retrieval::ChatStreamEvent::Sources(s) => ChatStreamWire::Sources { sources: s },
                    retrieval::ChatStreamEvent::Delta(t) => ChatStreamWire::Delta { text: t },
                };
                let _ = sink.send(wire);
            },
        )
        .await;
        let _ = match result {
            Ok(resp) => tx.send(ChatStreamWire::Done { answer: resp.answer }),
            Err(e) => tx.send(ChatStreamWire::Error { message: e.to_string() }),
        };
    });

    let stream = UnboundedReceiverStream::new(rx).map(|ev| {
        Event::default()
            .json_data(&ev)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// Return the cached Clean Read (`reader.md`) for a paper, or 404 if it hasn't
/// been generated yet. `GET /papers/{id}` already exposes `has_reader` so the
/// client knows whether to fetch this or offer to generate.
async fn get_reader(
    State(state): State<Shared>,
    Path(paper_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    if !state.paths.metadata_path(&paper_id).exists() {
        return Err(KbError::NotFound(format!("{paper_id} is not in the KB")).into());
    }
    let reader = std::fs::read_to_string(state.paths.reader_path(&paper_id))
        .map_err(|_| KbError::NotFound(format!("{paper_id} has no clean read yet")))?;
    Ok(Json(json!({ "reader": reader })))
}

#[derive(Deserialize)]
struct ReaderRequest {
    /// Which model generates the rewrite (e.g. `claude-opus-4-8`). Falls back to
    /// `config.chat.model` when omitted or blank.
    #[serde(default)]
    model: Option<String>,
}

/// One SSE event of a streamed Clean Read generation (`POST /papers/{id}/reader`).
/// `type`-tagged like [`ChatStreamWire`]: `generating` (started) → `delta`
/// (rewrite tokens, in order) → `done` (full markdown, also now on disk) — or
/// `error`.
#[derive(serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ReaderStreamWire {
    Generating,
    Delta { text: String },
    Done { reader: String },
    Error { message: String },
}

/// Generate (and cache) the Clean Read for a paper, streaming the rewrite live
/// (SSE). The work runs in a spawned task pushing events over a channel; the
/// file is written atomically inside `reader::generate_reader`, so by the time
/// `done` fires `reader.md` exists on disk. A missing provider key or missing
/// `sections.md` surfaces as an `error` event, not a 500 (mirrors `chat_stream`).
async fn generate_reader_stream(
    State(state): State<Shared>,
    Path(paper_id): Path<String>,
    Json(req): Json<ReaderRequest>,
) -> impl IntoResponse {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<ReaderStreamWire>();
    let st = state.clone();
    let model = req
        .model
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| state.config.chat.model.clone());
    tokio::spawn(async move {
        let _ = tx.send(ReaderStreamWire::Generating);
        let sink = tx.clone();
        let result = crate::reader::generate_reader(&st.paths, &st.config, &paper_id, &model, |d| {
            let _ = sink.send(ReaderStreamWire::Delta { text: d.to_string() });
        })
        .await;
        let _ = match result {
            Ok(reader) => tx.send(ReaderStreamWire::Done { reader }),
            Err(e) => tx.send(ReaderStreamWire::Error { message: e.to_string() }),
        };
    });

    let stream = UnboundedReceiverStream::new(rx).map(|ev| {
        Event::default()
            .json_data(&ev)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// Brainstorming roundtable (multi-agent): a panel of specialist agents debates
/// the objective across rounds — grounded in the corpus — streamed live as SSE.
/// Each SSE event's data is one JSON `RoundtableEvent` (`type`-tagged), so the
/// client renders turns as they arrive. The orchestrator runs in a spawned task
/// and pushes events over a channel; this handler just streams the channel.
///
/// Requires the API key for whichever providers the panel's models use
/// (`OPENAI_API_KEY` and/or `ANTHROPIC_API_KEY`); a missing key surfaces as an
/// `error` event, not a failed request.
async fn brainstorm(
    State(state): State<Shared>,
    Json(req): Json<BrainstormRequest>,
) -> impl IntoResponse {
    let (tx, rx) = tokio::sync::mpsc::channel::<roundtable::RoundtableEvent>(32);
    // Side-channel for live interjections; registered under the client's session
    // id so `POST /brainstorm/{id}/interject` can reach this run.
    let (itx, irx) = tokio::sync::mpsc::channel::<String>(16);
    let session_id = req.session_id.clone();
    if let Some(ref sid) = session_id {
        if let Ok(mut map) = state.roundtables.lock() {
            map.insert(sid.clone(), itx);
        }
    }

    let st = state.clone();
    tokio::spawn(async move {
        roundtable::run(&st.paths, &st.config, req, irx, tx).await;
        // The run is over — drop the interjection sender so a stale id can't
        // accumulate. (Steering after this point is a no-op.)
        if let Some(sid) = session_id {
            if let Ok(mut map) = st.roundtables.lock() {
                map.remove(&sid);
            }
        }
    });

    let stream = ReceiverStream::new(rx).map(|ev| {
        Event::default()
            .json_data(&ev)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

#[derive(Deserialize)]
struct InterjectRequest {
    text: String,
}

/// Push a guiding idea into a running roundtable. The orchestrator drains it
/// before the next agent speaks. 404 if the session isn't live (finished or
/// never started); 202 once queued.
async fn interject(
    State(state): State<Shared>,
    Path(session_id): Path<String>,
    Json(req): Json<InterjectRequest>,
) -> StatusCode {
    let sender = state
        .roundtables
        .lock()
        .ok()
        .and_then(|map| map.get(&session_id).cloned());
    match sender {
        Some(tx) => {
            // Clone the sender out before awaiting so we never hold the lock
            // across the send.
            let _ = tx.send(req.text).await;
            StatusCode::ACCEPTED
        }
        None => StatusCode::NOT_FOUND,
    }
}

#[derive(Deserialize)]
struct CreateIdeaRequest {
    title: String,
    body: String,
    project: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    links: Vec<String>,
}

/// `kb add` over HTTP. Exactly one source: an arXiv id/URL, a web page `url`,
/// or a local `pdf_path` (the engine and app share the machine, so a path the
/// app picked is readable here).
#[derive(Deserialize)]
struct IngestRequest {
    #[serde(default)]
    arxiv: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    pdf_path: Option<String>,
}

/// SSE frames for a running ingest. `progress` mirrors the CLI spinner's status
/// lines as they happen; the run ends with exactly one `done` or `error`.
#[derive(serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum IngestEvent {
    Progress { message: String },
    Done {
        id: String,
        title: String,
        chunks: usize,
        source_format: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        source_url: Option<String>,
    },
    Error { message: String },
}

/// Streams ingestion as Server-Sent Events so the UI can show live progress
/// (downloading → chunking → embedding). A failure is an `error` event, not a
/// failed request — the stream still opens 200, matching `/brainstorm`.
async fn ingest(
    State(state): State<Shared>,
    Json(req): Json<IngestRequest>,
) -> impl IntoResponse {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<IngestEvent>();
    let paths = state.paths.clone();
    let config = state.config.clone();

    // Ingestion mixes async network calls with blocking sqlite/index work, so
    // run it on a blocking thread with its own runtime (as the prior sync path
    // did). The `progress` closure forwards each status line into the SSE
    // channel via the non-blocking `UnboundedSender::send`.
    tokio::task::spawn_blocking(move || {
        let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(rt) => rt,
            Err(e) => {
                let _ = tx.send(IngestEvent::Error { message: format!("build runtime: {e}") });
                return;
            }
        };
        rt.block_on(async move {
            let progress = {
                let tx = tx.clone();
                move |msg: &str| {
                    let _ = tx.send(IngestEvent::Progress { message: msg.to_string() });
                }
            };
            let arxiv = req.arxiv.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            let url = req.url.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            let pdf = req.pdf_path.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            let result = match (arxiv, url, pdf) {
                (Some(id), None, None) => {
                    pipeline::ingest_paper(&paths, &config, &id, false, &progress).await
                }
                (None, Some(url), None) => {
                    pipeline::ingest_url(&paths, &config, &url, false, &progress).await
                }
                (None, None, Some(path)) => {
                    pipeline::ingest_local_pdf(&paths, &config, std::path::Path::new(&path), &progress).await
                }
                (None, None, None) => Err(KbError::Usage(
                    "provide one of: arxiv, url, or pdf_path".into(),
                )),
                _ => Err(KbError::Usage(
                    "provide exactly one of: arxiv, url, or pdf_path".into(),
                )),
            };
            match result {
                Ok(report) => {
                    let source_format = match report.source_format {
                        crate::SourceFormat::Latex => "latex",
                        crate::SourceFormat::Pdf => "pdf",
                        crate::SourceFormat::Markdown => "markdown",
                        crate::SourceFormat::Html => "html",
                    };
                    let _ = tx.send(IngestEvent::Done {
                        id: report.paper_id,
                        title: report.title,
                        chunks: report.chunks,
                        source_format: source_format.to_string(),
                        source_url: report.source_url,
                    });
                }
                Err(e) => {
                    let _ = tx.send(IngestEvent::Error { message: e.to_string() });
                }
            }
        });
    });

    let stream = UnboundedReceiverStream::new(rx).map(|ev| {
        Event::default()
            .json_data(&ev)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn create_idea(
    State(state): State<Shared>,
    Json(req): Json<CreateIdeaRequest>,
) -> Result<Json<Value>, ApiError> {
    let paths = state.paths.clone();
    let config = state.config.clone();
    let report = run_blocking(move || async move {
        let spec = pipeline::IdeaSpec {
            slug: None,
            project: req.project.unwrap_or_else(|| "global".into()),
            title: req.title,
            body: req.body,
            tags: req.tags,
            links: req.links,
        };
        pipeline::ingest_idea(&paths, &config, &spec, &|_| {}).await
    })
    .await?;
    Ok(Json(json!({ "ok": true, "id": report.paper_id, "chunks": report.chunks })))
}

/// One row in the Notes list: enough to render the master list without reading
/// every note's full body.
#[derive(serde::Serialize)]
struct NoteSummary {
    id: String,
    title: String,
    project: String,
    updated_at: String,
    /// First ~200 chars of the body, for the list's secondary line.
    preview: String,
}

/// List every standalone note (`DocKind::Note`), newest first. Backs the
/// Notes sidebar section. Notes are the same `idea.md` documents created by
/// `POST /ideas` / Roundtable syntheses, so they show up here too.
async fn list_notes(State(state): State<Shared>) -> Result<Json<Vec<NoteSummary>>, ApiError> {
    let db = MetaDb::open(&state.paths.meta_db_path())?;
    let mut rows = Vec::new();
    for id in db.paper_ids_by_kind(DocKind::Note)? {
        let Ok(meta) = PaperMetadata::load(&state.paths.metadata_path(&id)) else {
            continue;
        };
        let body = std::fs::read_to_string(state.paths.idea_path(&id)).unwrap_or_default();
        let preview: String = body.trim().chars().take(200).collect();
        rows.push(NoteSummary {
            id,
            title: meta.title,
            project: meta.project.unwrap_or_else(|| "global".into()),
            updated_at: meta.updated_at,
            preview,
        });
    }
    // Newest first by ISO-8601 `updated_at` (lexical sort == chronological).
    rows.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(Json(rows))
}

/// Fetch one note's full markdown body plus its metadata (for the editor).
async fn get_note(
    State(state): State<Shared>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let meta_path = state.paths.metadata_path(&id);
    if !meta_path.exists() {
        return Err(KbError::NotFound(format!("{id} is not in the KB")).into());
    }
    let meta = PaperMetadata::load(&meta_path)?;
    if meta.kind != DocKind::Note {
        return Err(KbError::Usage(format!("{id} is not a note")).into());
    }
    let body = std::fs::read_to_string(state.paths.idea_path(&id)).unwrap_or_default();
    Ok(Json(json!({
        "id": id,
        "title": meta.title,
        "project": meta.project.unwrap_or_else(|| "global".into()),
        "body": body,
        "tags": meta.tags,
        "updated_at": meta.updated_at,
    })))
}

/// Create (POST `/notes`) or update (PUT `/notes/{id}`) payload. Body and
/// title are required by the pipeline; empty tags on an update keep existing
/// ones.
#[derive(Deserialize)]
struct NotePayload {
    title: String,
    #[serde(default)]
    body: String,
    project: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
}

/// Create a new note. The id (slug) is derived from the title; re-using a
/// title upserts the same note (matching `ingest_idea`'s semantics).
async fn create_note(
    State(state): State<Shared>,
    Json(req): Json<NotePayload>,
) -> Result<Json<Value>, ApiError> {
    let paths = state.paths.clone();
    let config = state.config.clone();
    let report = run_blocking(move || async move {
        let spec = pipeline::IdeaSpec {
            slug: None,
            project: req.project.unwrap_or_else(|| "global".into()),
            title: req.title,
            body: req.body,
            tags: req.tags,
            links: Vec::new(),
        };
        pipeline::ingest_idea(&paths, &config, &spec, &|_| {}).await
    })
    .await?;
    Ok(Json(json!({ "ok": true, "id": report.paper_id })))
}

/// Update a note in place. Pinning the slug to the path id keeps the id stable
/// even when the title changes. Refuses to clobber a non-note document.
async fn update_note(
    State(state): State<Shared>,
    Path(id): Path<String>,
    Json(req): Json<NotePayload>,
) -> Result<Json<Value>, ApiError> {
    let meta_path = state.paths.metadata_path(&id);
    if meta_path.exists() {
        let meta = PaperMetadata::load(&meta_path)?;
        if meta.kind != DocKind::Note {
            return Err(KbError::Usage(format!("{id} is not a note")).into());
        }
    }
    let paths = state.paths.clone();
    let config = state.config.clone();
    let report = run_blocking(move || async move {
        let spec = pipeline::IdeaSpec {
            slug: Some(id),
            project: req.project.unwrap_or_else(|| "global".into()),
            title: req.title,
            body: req.body,
            tags: req.tags,
            links: Vec::new(),
        };
        pipeline::ingest_idea(&paths, &config, &spec, &|_| {}).await
    })
    .await?;
    Ok(Json(json!({ "ok": true, "id": report.paper_id })))
}

/// Delete a note: wipe its index/db traces, then remove its folder. Guarded so
/// it can never delete an ingested paper.
async fn delete_note(
    State(state): State<Shared>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let meta_path = state.paths.metadata_path(&id);
    if !meta_path.exists() {
        return Err(KbError::NotFound(format!("{id} is not in the KB")).into());
    }
    let meta = PaperMetadata::load(&meta_path)?;
    if meta.kind != DocKind::Note {
        return Err(KbError::Usage(format!("{id} is not a note; refusing to delete")).into());
    }
    let paths = state.paths.clone();
    let config = state.config.clone();
    run_blocking(move || async move {
        pipeline::remove_paper_from_stores(&paths, &config, &id)?;
        std::fs::remove_dir_all(paths.paper_dir(&id))
            .map_err(|e| KbError::Index(format!("folder delete failed (index already updated): {e}")))?;
        Ok(())
    })
    .await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct CreateReflectionRequest {
    title: String,
    body: String,
    #[serde(default)]
    scope: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
}

async fn create_reflection(
    State(state): State<Shared>,
    Json(req): Json<CreateReflectionRequest>,
) -> Result<Json<Value>, ApiError> {
    let paths = state.paths.clone();
    let config = state.config.clone();
    let report = run_blocking(move || async move {
        let spec = pipeline::ReflectionSpec {
            slug: None,
            title: req.title,
            body: req.body,
            scope: req.scope,
            tags: req.tags,
        };
        pipeline::ingest_reflection(&paths, &config, &spec, &|_| {}).await
    })
    .await?;
    Ok(Json(json!({ "ok": true, "id": report.paper_id, "chunks": report.chunks })))
}

#[derive(Deserialize)]
struct ComposeAssistRequest {
    draft: String,
    message: String,
    #[serde(default)]
    history: Vec<ChatMessage>,
}

async fn compose_assist(
    State(state): State<Shared>,
    Json(req): Json<ComposeAssistRequest>,
) -> Result<Json<Value>, ApiError> {
    let paths = state.paths.clone();
    let config = state.config.clone();
    let answer = run_blocking(move || async move {
        retrieval::compose_assist(&paths, &config, &req.draft, &req.message, &req.history).await
    })
    .await?;
    Ok(Json(json!({ "answer": answer })))
}

#[derive(Deserialize)]
struct NoteRequest {
    note: String,
}

async fn add_note(
    State(state): State<Shared>,
    Path(paper_id): Path<String>,
    Json(req): Json<NoteRequest>,
) -> Result<Json<Value>, ApiError> {
    let paths = state.paths.clone();
    let config = state.config.clone();
    let note = req.note;
    let message = run_blocking(move || async move {
        let meta_path = paths.metadata_path(&paper_id);
        if !meta_path.exists() {
            return Err(KbError::NotFound(format!("{paper_id} is not in the KB")));
        }
        let meta = PaperMetadata::load(&meta_path)?;
        pipeline::ensure_notes_template(&paths, &paper_id, &meta.title)?;

        use std::io::Write;
        let stamp = crate::now_rfc3339();
        let block = format!("\n---\n_{stamp} (added via HTTP)_\n\n{note}\n");
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(paths.notes_path(&paper_id))
            .map_err(|e| KbError::Index(format!("open notes.md: {e}")))?;
        f.write_all(block.as_bytes())
            .map_err(|e| KbError::Index(format!("append notes.md: {e}")))?;

        Ok(match pipeline::reembed_notes(&paths, &config, &paper_id).await {
            Ok(()) => format!("note appended to {paper_id} and re-embedded"),
            Err(e) => format!(
                "note appended to {paper_id}; re-embedding deferred ({e}) — a running `kb watch` or `kb reindex` will pick it up"
            ),
        })
    })
    .await?;
    Ok(Json(json!({ "ok": true, "message": message })))
}

/// Overwrite a paper's `notes.md` wholesale and re-embed (the editable-notes
/// editor). Unlike `add_note` (append-only), this replaces the file with the
/// supplied content — the canonical, hand-editable note body.
async fn put_notes(
    State(state): State<Shared>,
    Path(paper_id): Path<String>,
    Json(req): Json<NoteRequest>,
) -> Result<Json<Value>, ApiError> {
    let paths = state.paths.clone();
    let config = state.config.clone();
    let body = req.note;
    let message = run_blocking(move || async move {
        let meta_path = paths.metadata_path(&paper_id);
        if !meta_path.exists() {
            return Err(KbError::NotFound(format!("{paper_id} is not in the KB")));
        }
        std::fs::write(paths.notes_path(&paper_id), body.as_bytes())
            .map_err(|e| KbError::Index(format!("write notes.md: {e}")))?;

        Ok(match pipeline::reembed_notes(&paths, &config, &paper_id).await {
            Ok(()) => format!("notes saved to {paper_id} and re-embedded"),
            Err(e) => format!(
                "notes saved to {paper_id}; re-embedding deferred ({e}) — a running `kb watch` or `kb reindex` will pick it up"
            ),
        })
    })
    .await?;
    Ok(Json(json!({ "ok": true, "message": message })))
}

/// Run a `!Send` async operation (rusqlite `MetaDb` lives across `.await`) to
/// completion on a dedicated blocking thread with its own current-thread
/// runtime. Only the closure (owning `Send` clones) crosses the thread
/// boundary; the `MetaDb` is born and dies on that thread, so the calling
/// axum handler future stays `Send`.
async fn run_blocking<F, Fut, T>(f: F) -> Result<T, ApiError>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<T, KbError>>,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| KbError::Index(format!("build runtime: {e}")))?;
        rt.block_on(f())
    })
    .await
    .map_err(|e| KbError::Index(format!("task join: {e}")))?
    .map_err(ApiError::from)
}

async fn get_chunk(
    State(state): State<Shared>,
    Path(chunk_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let db = MetaDb::open(&state.paths.meta_db_path())?;
    let rec = db
        .chunk_by_chunk_id(&chunk_id)?
        .ok_or_else(|| KbError::NotFound(format!("chunk {chunk_id} not found")))?;
    let link = deep_link(
        &state.paths.link_target(&rec.paper_id, rec.section_type),
        rec.page,
        None,
    );
    Ok(Json(json!({
        "chunk_id": rec.chunk_id,
        "paper_id": rec.paper_id,
        "section_type": rec.section_type.as_str(),
        "ordinal": rec.ordinal,
        "page": rec.page,
        "text": rec.text,
        "deep_link": link,
    })))
}

/// Stream a paper's `paper.pdf` over HTTP so browsers can open it — a page
/// served over `http://` is forbidden by the browser from loading `file://`
/// URLs, so the `deep_link` fields (correct for the CLI/MCP) don't work in the
/// web app. Served inline with `#page=N` honored by the built-in PDF viewer.
async fn get_pdf(
    State(state): State<Shared>,
    Path(paper_id): Path<String>,
) -> Result<Response, ApiError> {
    if paper_id.contains('/') || paper_id.contains('\\') || paper_id.contains("..") {
        return Err(KbError::Usage(format!("invalid paper id '{paper_id}'")).into());
    }
    let path = state.paths.pdf_path(&paper_id);
    if !path.exists() {
        return Err(KbError::NotFound(format!("{paper_id} has no PDF")).into());
    }
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| KbError::Index(format!("read pdf: {e}")))?;
    let headers = [
        (header::CONTENT_TYPE, HeaderValue::from_static("application/pdf")),
        (
            header::CONTENT_DISPOSITION,
            HeaderValue::from_static("inline"),
        ),
    ];
    Ok((headers, bytes).into_response())
}

async fn open_chunk(
    State(state): State<Shared>,
    Path(chunk_id): Path<String>,
) -> Result<Response, ApiError> {
    let db = MetaDb::open(&state.paths.meta_db_path())?;
    let rec = db
        .chunk_by_chunk_id(&chunk_id)?
        .ok_or_else(|| KbError::NotFound(format!("chunk {chunk_id} not found")))?;
    // Prefer the same-origin served PDF (browser-openable). Fall back to the
    // `file://` deep link for non-PDF docs (notes/reflections/web pages) — that
    // path is for the CLI/`kb open`, not browsers.
    let location = if state.paths.pdf_path(&rec.paper_id).exists() {
        let mut url = format!("/pdf/{}?key={}", rec.paper_id, state.api_key);
        if let Some(p) = rec.page {
            url.push_str(&format!("#page={p}"));
        }
        url
    } else {
        deep_link(
            &state.paths.link_target(&rec.paper_id, rec.section_type),
            rec.page,
            None,
        )
    };
    let location = HeaderValue::from_str(&location)
        .map_err(|e| KbError::Index(format!("bad redirect target: {e}")))?;
    Ok((StatusCode::FOUND, [(header::LOCATION, location)]).into_response())
}

fn non_empty(v: Option<Vec<String>>) -> Option<Vec<String>> {
    v.filter(|list| !list.is_empty())
}

// ---- errors ----------------------------------------------------------------

struct ApiError {
    status: StatusCode,
    message: String,
}

impl From<KbError> for ApiError {
    fn from(e: KbError) -> Self {
        let status = match e {
            KbError::NotFound(_) => StatusCode::NOT_FOUND,
            KbError::Usage(_) => StatusCode::BAD_REQUEST,
            KbError::Network(_) => StatusCode::BAD_GATEWAY,
            KbError::Extraction(_) | KbError::Index(_) | KbError::Config(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };
        ApiError {
            status,
            message: e.to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}
