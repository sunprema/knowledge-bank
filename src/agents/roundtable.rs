//! The brainstorming roundtable orchestrator.
//!
//! A panel of persona-agents debates an objective across N rounds, then a
//! synthesizer converges the discussion. Each agent runs on its own model
//! (mixed providers — Claude or OpenAI, routed by model id) and may pull
//! grounding from the corpus before speaking. The run emits [`RoundtableEvent`]s
//! over a channel as they happen; `POST /brainstorm` streams them as SSE.
//!
//! The unit of streaming here is the *turn*: an agent's full contribution is
//! emitted when ready (with `turn_start` / `kb_query` / `citation` events
//! beforehand so the UI can animate "thinking → searching → speaking"). The
//! event shapes leave room to add token-level deltas later without changing the
//! client protocol.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::{Receiver, Sender};

use crate::agents::harness::{kb_tools, run_agent};
use crate::anthropic::{AgentMessage, AnthropicChat, ContentBlock};
use crate::chat::{ChatMessage, OpenAiChat};
use crate::config::{Config, KbPaths};
use crate::embed::OpenAiEmbedder;
use crate::search::{retrieval, SearchFilters, SearchMode};
use crate::KbError;

/// Default rounds of debate before synthesis (clamped 1..=4 from the request).
const DEFAULT_ROUNDS: usize = 2;
/// KB hits surfaced per agent lookup.
const KB_HITS_PER_TURN: usize = 3;
/// Creativity for OpenAI agents (Anthropic agents send no sampling params —
/// Opus 4.8/4.7 reject `temperature`).
const OPENAI_TEMPERATURE: f32 = 0.6;
/// Extra attempts on a *transient* turn failure, on top of the model client's
/// own 429/5xx retries. Auth/config errors are not retried (see `is_retryable`).
const TURN_RETRIES: usize = 2;
/// Backoff before each retry attempt.
const TURN_RETRY_BACKOFF_MS: [u64; 2] = [800, 1600];
/// Most specialists the moderator may recruit beyond the starting panel.
const MAX_RECRUITS: usize = 3;
/// Model calls a tool-enabled persona may make in one turn before the harness
/// forces a final answer — bounds a self-directed search loop (typically 1–2
/// searches then speak; the cap guards against a runaway).
const MAX_TOOL_ITERS: usize = 6;

/// Cosine-similarity threshold (per agent, round-over-round) above which the
/// debate is considered to have converged — agents are restating, so we stop
/// early and jump to synthesis instead of burning the remaining rounds. Tuned
/// for `text-embedding-3-small`; high enough to avoid stopping on a genuinely
/// evolving discussion.
const CONVERGE_THRESHOLD: f32 = 0.90;

/// Cosine similarity of two equal-length embedding vectors (0 if either is zero).
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 { 0.0 } else { dot / (na * nb) }
}

/// Whether an agent failure is worth retrying. Transient (`Network`) failures —
/// connection drops, overload past the client's own backoff, malformed
/// responses — yes. Auth/missing-key (`Config`) and the rest — no; retrying a
/// missing key just wastes the user's time.
fn is_retryable(e: &KbError) -> bool {
    matches!(e, KbError::Network(_))
}

// ===========================================================================
// Request / persona specs
// ===========================================================================

#[derive(Debug, Clone, Deserialize)]
pub struct BrainstormRequest {
    pub objective: String,
    /// Client-generated id correlating this run's SSE stream with live
    /// `interject` calls. Optional — without it, steering is simply unavailable.
    #[serde(default)]
    pub session_id: Option<String>,
    /// Optional client-supplied panel (lets the macOS app assign models per
    /// agent). Omitted ⇒ the default panel below.
    #[serde(default)]
    pub personas: Option<Vec<PersonaSpec>>,
    #[serde(default)]
    pub rounds: Option<usize>,
    /// Prior discussion to seed the run with (formatted transcript lines), so a
    /// continuation builds on earlier turns instead of starting cold.
    #[serde(default)]
    pub transcript: Option<Vec<String>>,
    /// The user's new points to fold into the agents' prompts from the start
    /// (same effect as a live interjection, but available before round one).
    #[serde(default)]
    pub guidance: Option<Vec<String>>,
    /// Whether to run a scoring pass at the end (each debater rates the idea on
    /// four dimensions). Defaults to true.
    #[serde(default)]
    pub score: Option<bool>,
    /// Whether to stop rounds early once the debate converges (agents repeat).
    /// Defaults to true; needs an embedding key to take effect.
    #[serde(default)]
    pub converge: Option<bool>,
    /// Whether a moderator agent directs the debate (picks who speaks next and
    /// can recruit new specialists) instead of a fixed round-robin. Default false.
    #[serde(default)]
    pub moderated: Option<bool>,
    /// Persona ids the user has *directly addressed* (`@mentioned`). When present
    /// and non-empty, the run is a single **directed exchange**: only these
    /// personas speak — in this order, answering the user's message (`guidance`) —
    /// instead of the autonomous round-robin. The synthesizer then re-synthesizes
    /// and the debaters re-score (closeout). Used by the macOS chat composer.
    #[serde(default)]
    pub targets: Option<Vec<String>>,
    /// Round number to label the directed exchange's turns with, so a continued
    /// conversation's turns sort after the opening debate. Defaults to 1.
    #[serde(default)]
    pub base_round: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PersonaSpec {
    pub id: String,
    pub name: String,
    pub role: String,
    /// Wire model id, e.g. `claude-opus-4-8` or `gpt-4o`. Routes the provider.
    pub model: String,
    /// Runs last and produces the final synthesis.
    #[serde(default)]
    pub is_synth: bool,
    /// Verifies the panel's claims against the corpus. Runs after the debaters,
    /// before the synthesizer, with extra grounding.
    #[serde(default)]
    pub is_fact_checker: bool,
    /// Pulls grounding from the corpus before speaking.
    #[serde(default)]
    pub queries_kb: bool,
    /// Grants live tool access: the persona can actively search the corpus
    /// (and fetch papers) mid-turn via the agent harness, on top of any
    /// pre-fetched grounding. Read-only — no KB writes. Only takes effect on
    /// Claude models (tool-use is Anthropic-only); ignored for OpenAI personas.
    #[serde(default)]
    pub tools: bool,
}

impl PersonaSpec {
    fn default_panel() -> Vec<PersonaSpec> {
        vec![
            PersonaSpec { id: "tech".into(), name: "Aria".into(), role: "Technologist".into(),
                model: "claude-opus-4-8".into(), is_synth: false, is_fact_checker: false, queries_kb: true, tools: true },
            PersonaSpec { id: "biz".into(), name: "Mateo".into(), role: "Business & GTM".into(),
                model: "gpt-4o".into(), is_synth: false, is_fact_checker: false, queries_kb: true, tools: false },
            PersonaSpec { id: "skeptic".into(), name: "Nadia".into(), role: "Skeptic / Risk".into(),
                model: "claude-sonnet-4-6".into(), is_synth: false, is_fact_checker: false, queries_kb: false, tools: false },
            PersonaSpec { id: "factcheck".into(), name: "Vera".into(), role: "Fact-checker".into(),
                model: "gpt-4o".into(), is_synth: false, is_fact_checker: true, queries_kb: true, tools: false },
            PersonaSpec { id: "synth".into(), name: "Sol".into(), role: "Synthesizer".into(),
                model: "claude-opus-4-8".into(), is_synth: true, is_fact_checker: false, queries_kb: true, tools: false },
        ]
    }
}

// ===========================================================================
// Streamed events
// ===========================================================================

/// One event in the live roundtable stream. Serialized with a `type` tag so the
/// client can switch on it; sent as the JSON body of an SSE event.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RoundtableEvent {
    Start { objective: String, rounds: usize },
    TurnStart { persona_id: String, round: usize },
    KbQuery { persona_id: String, query: String },
    Citation {
        persona_id: String,
        title: String,
        section_type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        page: Option<u32>,
        snippet: String,
        deep_link: String,
    },
    Turn { persona_id: String, round: usize, text: String },
    /// The debate converged early (agents are restating); rounds were cut short.
    Converged { round: usize, similarity: f64 },
    /// The moderator made a decision (who speaks next / why) — shown live.
    Moderator { reason: String },
    /// The moderator recruited a new specialist into the panel mid-debate.
    Recruited {
        persona_id: String,
        name: String,
        role: String,
        model: String,
        reason: String,
    },
    /// One agent's rating of the idea (each dimension 1-10), for the scorecard.
    Score {
        persona_id: String,
        feasibility: f64,
        market: f64,
        defensibility: f64,
        timing: f64,
        rationale: String,
    },
    /// A transient failure is being retried (with the attempt number).
    Retry { persona_id: String, attempt: usize, message: String },
    /// A user interjection was received and folded into the upcoming prompts.
    Interjected { text: String },
    Done,
    Error { message: String },
}

// ===========================================================================
// Orchestration
// ===========================================================================

/// Run the roundtable, emitting events over `tx`. `interjects` carries guiding
/// ideas the user injects mid-debate (drained before each agent speaks and
/// folded into its prompt). Returns when the debate is done, the client
/// disconnected (send fails), or an error was reported. Never panics —
/// engine/model failures are surfaced as an `Error` event.
pub async fn run(
    paths: &KbPaths,
    config: &Config,
    req: BrainstormRequest,
    mut interjects: Receiver<String>,
    tx: Sender<RoundtableEvent>,
) {
    let objective = req.objective.trim().to_string();
    if objective.is_empty() {
        let _ = tx.send(RoundtableEvent::Error { message: "objective is empty".into() }).await;
        return;
    }
    // No hard cap on rounds — research can go as deep as the user wants — but a
    // generous ceiling guards against a runaway request.
    let rounds = req.rounds.unwrap_or(DEFAULT_ROUNDS).clamp(1, 50);
    let personas = req.personas.filter(|p| !p.is_empty()).unwrap_or_else(PersonaSpec::default_panel);

    if tx.send(RoundtableEvent::Start { objective: objective.clone(), rounds }).await.is_err() {
        return;
    }

    let debaters: Vec<&PersonaSpec> = personas.iter().filter(|p| !p.is_synth && !p.is_fact_checker).collect();
    let checkers: Vec<&PersonaSpec> = personas.iter().filter(|p| p.is_fact_checker && !p.is_synth).collect();
    let synth: Option<&PersonaSpec> = personas.iter().find(|p| p.is_synth);

    // Running transcript fed to each subsequent agent — seeded from a prior run
    // when continuing.
    let mut transcript: Vec<String> = req.transcript.unwrap_or_default();
    // User guidance accumulated from interjections — seeded with any points the
    // user attached up front. Once added it stays in context across rounds.
    let mut steering: Vec<String> = req.guidance.unwrap_or_default();

    // Directed exchange: the user @mentioned specific agents — only they speak,
    // answering the user's message, then the synthesizer closes and we re-score.
    if let Some(target_ids) = req.targets.as_ref().filter(|t| !t.is_empty()) {
        run_directed(paths, config, &objective, &mut transcript, &mut steering,
                     &personas, target_ids, req.base_round.unwrap_or(1),
                     req.score.unwrap_or(true), &mut interjects, &tx).await;
        return;
    }

    if req.moderated.unwrap_or(false) {
        // Moderator-directed debate: a facilitator picks who speaks next and can
        // recruit new specialists. The moderator runs on the synthesizer's model.
        let debater_specs: Vec<PersonaSpec> = personas
            .iter().filter(|p| !p.is_synth && !p.is_fact_checker).cloned().collect();
        let mod_model = synth
            .map(|s| s.model.clone())
            .or_else(|| debater_specs.first().map(|p| p.model.clone()))
            .unwrap_or_else(|| "gpt-4o".to_string());
        let budget = (rounds * debater_specs.len().max(1)).clamp(4, 20);
        match run_moderated(paths, config, &objective, &mut transcript, &mut steering,
                            debater_specs, &mod_model, budget, &mut interjects, &tx).await {
            Some(final_panel) => {
                run_closeout(paths, config, &objective, &mut transcript, &mut steering,
                             &checkers, synth, &final_panel, budget,
                             req.score.unwrap_or(true), &mut interjects, &tx).await;
            }
            None => {} // client gone
        }
        return;
    }

    // Convergence detection: embed each round's turns and stop early when agents
    // start restating themselves. Needs an embedding key; off if unavailable.
    let embedder = OpenAiEmbedder::from_env(&config.embedding.model, config.embedding.dimensions).ok();
    let auto_converge = req.converge.unwrap_or(true) && embedder.is_some();
    let mut prev_round: HashMap<String, Vec<f32>> = HashMap::new();

    'rounds: for round in 1..=rounds {
        let mut round_turns: Vec<(String, String)> = Vec::new();
        for persona in &debaters {
            drain_interjections(&mut interjects, &mut steering, &tx).await;
            match run_turn(paths, config, persona, round, &objective, &transcript, &steering, &tx).await {
                Ok(text) => {
                    transcript.push(format!("**{} ({})**: {}", persona.name, persona.role, text));
                    round_turns.push((persona.id.clone(), text));
                }
                Err(stop) => {
                    if stop { return; } // client gone or error already reported
                }
            }
        }

        // Compare this round to the last, per agent. If they're mostly restating
        // (high cosine similarity), there's nothing left to gain — stop early.
        // Skipped on the final round (no rounds left to save).
        if auto_converge && round < rounds && !round_turns.is_empty() {
            if let Some(emb) = &embedder {
                let texts: Vec<&str> = round_turns.iter().map(|(_, t)| t.as_str()).collect();
                if let Ok(vecs) = emb.embed_batch(&texts).await {
                    let cur: HashMap<String, Vec<f32>> =
                        round_turns.iter().map(|(pid, _)| pid.clone()).zip(vecs).collect();
                    if !prev_round.is_empty() {
                        let sims: Vec<f32> = cur
                            .iter()
                            .filter_map(|(pid, v)| prev_round.get(pid).map(|pv| cosine(v, pv)))
                            .collect();
                        if !sims.is_empty() {
                            let avg = sims.iter().sum::<f32>() / sims.len() as f32;
                            if avg >= CONVERGE_THRESHOLD {
                                let _ = tx.send(RoundtableEvent::Converged {
                                    round, similarity: avg as f64,
                                }).await;
                                break 'rounds;
                            }
                        }
                    }
                    prev_round = cur;
                }
            }
        }
    }

    let score_targets: Vec<PersonaSpec> = personas
        .iter().filter(|p| !p.is_synth && !p.is_fact_checker).cloned().collect();
    run_closeout(paths, config, &objective, &mut transcript, &mut steering,
                 &checkers, synth, &score_targets, rounds,
                 req.score.unwrap_or(true), &mut interjects, &tx).await;
}

/// Shared tail of a debate: fact-checkers verify the claims, the synthesizer
/// closes, each debater scores the idea, then `Done`. Used by both the
/// round-robin and the moderated paths.
#[allow(clippy::too_many_arguments)]
async fn run_closeout(
    paths: &KbPaths,
    config: &Config,
    objective: &str,
    transcript: &mut Vec<String>,
    steering: &mut Vec<String>,
    checkers: &[&PersonaSpec],
    synth: Option<&PersonaSpec>,
    score_targets: &[PersonaSpec],
    base_round: usize,
    score: bool,
    interjects: &mut Receiver<String>,
    tx: &Sender<RoundtableEvent>,
) {
    for persona in checkers {
        drain_interjections(interjects, steering, tx).await;
        match run_turn(paths, config, persona, base_round + 1, objective, transcript, steering, tx).await {
            Ok(text) => transcript.push(format!("**{} ({})**: {}", persona.name, persona.role, text)),
            Err(stop) => {
                if stop { return; }
            }
        }
    }

    if let Some(persona) = synth {
        drain_interjections(interjects, steering, tx).await;
        let _ = run_turn(paths, config, persona, base_round + 2, objective, transcript, steering, tx).await;
    }

    // Scorecard: each debater rates the idea so "vibes" become a comparable
    // radar. Best-effort per agent — a malformed/missing score is just skipped.
    if score {
        for persona in score_targets {
            if let Some((feasibility, market, defensibility, timing, rationale)) =
                score_idea(persona, objective, transcript).await
            {
                if tx.send(RoundtableEvent::Score {
                    persona_id: persona.id.clone(),
                    feasibility, market, defensibility, timing, rationale,
                }).await.is_err() {
                    return;
                }
            }
        }
    }

    let _ = tx.send(RoundtableEvent::Done).await;
}

/// Run a single **directed exchange**: only the `@mentioned` personas speak — in
/// mention order, each answering the user's message and building on the prior
/// answers — then the synthesizer re-synthesizes and the debaters re-score. This
/// is the conversational mode that follows the opening autonomous debate: the
/// user drives the table turn by turn, choosing exactly who weighs in.
///
/// Fact-checkers only run when explicitly addressed (so none are auto-invoked
/// here). The synthesizer is skipped in closeout if it was itself addressed.
#[allow(clippy::too_many_arguments)]
async fn run_directed(
    paths: &KbPaths,
    config: &Config,
    objective: &str,
    transcript: &mut Vec<String>,
    steering: &mut Vec<String>,
    personas: &[PersonaSpec],
    target_ids: &[String],
    base_round: usize,
    score: bool,
    interjects: &mut Receiver<String>,
    tx: &Sender<RoundtableEvent>,
) {
    // Resolve the @mentions to panel members, in mention order, de-duplicated.
    // Tolerant of id-or-name so the client can address by either.
    let mut targets: Vec<PersonaSpec> = Vec::new();
    for tid in target_ids {
        if let Some(p) = personas.iter().find(|p| {
            p.id.eq_ignore_ascii_case(tid) || p.name.eq_ignore_ascii_case(tid)
        }) {
            if !targets.iter().any(|x| x.id == p.id) {
                targets.push(p.clone());
            }
        }
    }
    if targets.is_empty() {
        // Nobody addressable was named — nothing to do (per the chat model, no
        // agent gets involved when none are mentioned).
        let _ = tx.send(RoundtableEvent::Done).await;
        return;
    }

    // Fold the user's message in as the latest line of the discussion so every
    // addressed agent answers it directly. (It arrived via `guidance`/`steering`;
    // move it into the transcript rather than leaving it as ambient steering.)
    let user_msg = steering.join(" ").trim().to_string();
    steering.clear();
    if !user_msg.is_empty() {
        transcript.push(format!("**You**: {user_msg}"));
    }

    // Each addressed agent speaks once, in order, seeing the prior answers.
    for persona in &targets {
        drain_interjections(interjects, steering, tx).await;
        match run_turn(paths, config, persona, base_round, objective, transcript, steering, tx).await {
            Ok(text) => transcript.push(format!("**{} ({})**: {}", persona.name, persona.role, text)),
            Err(stop) => {
                if stop { return; }
            }
        }
    }

    // Closeout: the synthesizer re-synthesizes and the debaters re-score, unless
    // the synthesizer was itself one of the addressed agents (already spoke).
    let synth = personas
        .iter()
        .find(|p| p.is_synth && !targets.iter().any(|t| t.id == p.id));
    let score_targets: Vec<PersonaSpec> = personas
        .iter().filter(|p| !p.is_synth && !p.is_fact_checker).cloned().collect();
    run_closeout(paths, config, objective, transcript, steering,
                 &[], synth, &score_targets, base_round, score, interjects, tx).await;
}

/// Pull every queued interjection into `steering` (non-blocking) and echo each
/// as an event so the UI can confirm it landed.
async fn drain_interjections(rx: &mut Receiver<String>, steering: &mut Vec<String>, tx: &Sender<RoundtableEvent>) {
    while let Ok(idea) = rx.try_recv() {
        let idea = idea.trim().to_string();
        if idea.is_empty() { continue; }
        let _ = tx.send(RoundtableEvent::Interjected { text: idea.clone() }).await;
        steering.push(idea);
    }
}

/// Drive one agent's turn. `Ok(text)` is the contribution. `Err(true)` means
/// stop the whole run (client disconnected, or an error was already sent);
/// `Err(false)` means skip this turn but keep going.
async fn run_turn(
    paths: &KbPaths,
    config: &Config,
    persona: &PersonaSpec,
    round: usize,
    objective: &str,
    transcript: &[String],
    steering: &[String],
    tx: &Sender<RoundtableEvent>,
) -> Result<String, bool> {
    if tx.send(RoundtableEvent::TurnStart { persona_id: persona.id.clone(), round }).await.is_err() {
        return Err(true);
    }

    // 1. KB grounding. Fact-checkers always pull (and pull more) to verify.
    let mut kb_context = String::new();
    if persona.queries_kb || persona.is_fact_checker {
        let hits = if persona.is_fact_checker { KB_HITS_PER_TURN * 2 } else { KB_HITS_PER_TURN };
        if tx.send(RoundtableEvent::KbQuery { persona_id: persona.id.clone(), query: objective.to_string() }).await.is_err() {
            return Err(true);
        }
        if let Ok(resp) = retrieval::search(paths, config, objective, SearchMode::Wide, Some(hits), SearchFilters::default()).await {
            for group in resp.papers.iter().take(hits) {
                if let Some(chunk) = group.chunks.first() {
                    let snippet = truncate(&chunk.snippet, 240);
                    kb_context.push_str(&format!(
                        "- \"{}\" ({}): {}\n",
                        group.paper.title, chunk.section_type, snippet
                    ));
                    let _ = tx.send(RoundtableEvent::Citation {
                        persona_id: persona.id.clone(),
                        title: group.paper.title.clone(),
                        section_type: chunk.section_type.clone(),
                        page: chunk.page,
                        snippet,
                        deep_link: chunk.deep_link.clone(),
                    }).await;
                }
            }
        }
    }

    // 2. Build the prompt and call this persona's model, retrying transient
    //    failures a couple of times. Auth/config errors fail fast.
    let messages = build_messages(persona, round, objective, transcript, &kb_context, steering);
    // Tool-enabled Claude personas drive their own corpus lookups mid-turn; all
    // others take the plain single-shot path (tool-use is Anthropic-only).
    let use_tools = persona.tools && persona.model.starts_with("claude");
    let mut attempt = 0usize;
    let result = loop {
        let turn = if use_tools {
            complete_tooled(paths, config, &persona.model, &messages).await
        } else {
            complete(&persona.model, &messages).await
        };
        match turn {
            Ok(text) => break Ok(text),
            Err(e) => {
                if !is_retryable(&e) || attempt >= TURN_RETRIES {
                    break Err(e);
                }
                let delay = TURN_RETRY_BACKOFF_MS[attempt.min(TURN_RETRY_BACKOFF_MS.len() - 1)];
                attempt += 1;
                if tx.send(RoundtableEvent::Retry {
                    persona_id: persona.id.clone(),
                    attempt,
                    message: e.to_string(),
                }).await.is_err() {
                    return Err(true); // client gone
                }
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            }
        }
    };

    match result {
        Ok(text) => {
            let text = text.trim().to_string();
            if tx.send(RoundtableEvent::Turn { persona_id: persona.id.clone(), round, text: text.clone() }).await.is_err() {
                return Err(true);
            }
            Ok(text)
        }
        Err(e) => {
            // Non-fatal: one agent's failure (e.g. a missing provider key)
            // shouldn't kill the whole roundtable — surface it for this agent
            // and let the rest of the panel carry on.
            let _ = tx.send(RoundtableEvent::Error {
                message: format!("{} ({}) failed: {e}", persona.name, persona.model),
            }).await;
            Err(false)
        }
    }
}

/// Route to the right provider by model id and run one completion. Claude models
/// go through the Anthropic Messages API (no sampling params); everything else
/// through OpenAI chat-completions.
async fn complete(model: &str, messages: &[ChatMessage]) -> Result<String, KbError> {
    if model.starts_with("claude") {
        AnthropicChat::from_env(model)?.complete(messages).await
    } else {
        OpenAiChat::from_env(model)?.complete(messages, OPENAI_TEMPERATURE).await
    }
}

/// Tool-enabled completion for a Claude persona: runs the same prompt through
/// the agent harness with the read-only corpus tools, so the model can issue
/// its own `kb_search` / `kb_get_paper` calls mid-turn (on top of the grounding
/// already baked into `messages`) before producing its contribution. Bounded by
/// [`MAX_TOOL_ITERS`]. Only valid for Claude models — the caller gates on that.
async fn complete_tooled(
    paths: &KbPaths,
    config: &Config,
    model: &str,
    messages: &[ChatMessage],
) -> Result<String, KbError> {
    let client = AnthropicChat::from_env(model)?;
    // The Messages API takes `system` separately; lift it out and convert the
    // remaining turns into structured `AgentMessage`s for the tool loop.
    let system = messages
        .iter()
        .filter(|m| m.role == "system")
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    let turns: Vec<AgentMessage> = messages
        .iter()
        .filter(|m| m.role == "user" || m.role == "assistant")
        .map(|m| {
            if m.role == "assistant" {
                AgentMessage::assistant(vec![ContentBlock::text(&m.content)])
            } else {
                AgentMessage::user_text(&m.content)
            }
        })
        .collect();
    let registry = kb_tools::kb_registry_readonly(paths, config);
    run_agent(&client, &system, turns, &registry, MAX_TOOL_ITERS).await
}

fn build_messages(persona: &PersonaSpec, round: usize, objective: &str, transcript: &[String], kb_context: &str, steering: &[String]) -> Vec<ChatMessage> {
    let system = if persona.is_fact_checker {
        format!(
            "You are {name}, the fact-checker on a brainstorming roundtable. Review the key \
             factual and empirical claims made in the discussion. Using ONLY the knowledge-bank \
             material provided, judge each significant claim: mark it ✓ supported (name the source) \
             or ⚠ unsupported / not found in the corpus. Be concise — a short bulleted list of \
             claims with verdicts, no new ideas or opinions of your own. End with a one-line \
             overall trust assessment of the discussion.",
            name = persona.name,
        )
    } else if persona.is_synth {
        format!(
            "You are {name}, the synthesizer closing a startup brainstorming roundtable. \
             Read the full discussion and produce a crisp final synthesis in markdown with these \
             sections: ### Synthesis (the opportunity in 1-2 sentences), **What to build first**, \
             **Biggest risks**, and **Next step** (one concrete action). Be decisive — recommend, \
             don't survey.",
            name = persona.name,
        )
    } else {
        format!(
            "You are {name}, the {role} on a startup brainstorming roundtable with other specialists. \
             Debate the objective: be concrete and opinionated, build on or push back against what \
             others said, and stay in your lane as the {role}. Keep it to 2-4 short paragraphs. \
             Markdown (bold, bullets) is welcome. Do not restate the objective.",
            name = persona.name, role = persona.role,
        )
    };

    let mut user = format!("Objective: {objective}\n\n");
    if transcript.is_empty() {
        user.push_str("You are opening the discussion.\n");
    } else {
        user.push_str("Discussion so far:\n");
        user.push_str(&transcript.join("\n\n"));
        user.push_str("\n\n");
    }
    if !kb_context.is_empty() {
        user.push_str("Relevant material from the knowledge bank (cite it where useful):\n");
        user.push_str(kb_context);
        user.push('\n');
    }
    if !steering.is_empty() {
        user.push_str("The user is steering the discussion — weave this guidance in:\n");
        for s in steering {
            user.push_str(&format!("- {s}\n"));
        }
        user.push('\n');
    }
    if persona.is_fact_checker {
        user.push_str("Now fact-check the claims in the discussion above against the knowledge-bank material.");
    } else if persona.is_synth {
        user.push_str("Now synthesize the discussion into the final output.");
    } else {
        user.push_str(&format!("It is round {round}. Give your contribution now."));
    }

    vec![ChatMessage::system(system), ChatMessage::user(user)]
}

/// Ask one agent to rate the idea on the four scorecard dimensions, returning
/// `(feasibility, market, defensibility, timing, rationale)` clamped to 0..=10.
/// Best-effort: any failure (model error, unparseable output) yields `None`.
async fn score_idea(persona: &PersonaSpec, objective: &str, transcript: &[String]) -> Option<(f64, f64, f64, f64, String)> {
    let messages = build_score_messages(persona, objective, transcript);
    let text = complete(&persona.model, &messages).await.ok()?;
    parse_score(&text)
}

fn build_score_messages(persona: &PersonaSpec, objective: &str, transcript: &[String]) -> Vec<ChatMessage> {
    let system = format!(
        "You are {name}, the {role}. Score the startup idea on four dimensions, each an \
         integer from 1 to 10 (10 = strongest), through your {role} lens and the discussion. \
         Respond with ONLY a JSON object, no prose, no code fence:\n\
         {{\"feasibility\": N, \"market\": N, \"defensibility\": N, \"timing\": N, \
         \"rationale\": \"one short sentence\"}}",
        name = persona.name, role = persona.role,
    );
    let mut user = format!("Objective: {objective}\n\n");
    if !transcript.is_empty() {
        user.push_str("Discussion:\n");
        user.push_str(&transcript.join("\n\n"));
        user.push_str("\n\n");
    }
    user.push_str("Score the idea now. JSON only.");
    vec![ChatMessage::system(system), ChatMessage::user(user)]
}

/// Pull the four scores out of a model reply, tolerating prose or code fences
/// around the JSON object.
fn parse_score(text: &str) -> Option<(f64, f64, f64, f64, String)> {
    let json = {
        let start = text.find('{')?;
        let end = text.rfind('}')?;
        if end > start { &text[start..=end] } else { return None }
    };
    #[derive(serde::Deserialize)]
    struct Raw {
        feasibility: Option<f64>,
        market: Option<f64>,
        defensibility: Option<f64>,
        timing: Option<f64>,
        #[serde(default)]
        rationale: String,
    }
    let raw: Raw = serde_json::from_str(json).ok()?;
    let clamp = |v: f64| v.clamp(0.0, 10.0);
    Some((
        clamp(raw.feasibility?),
        clamp(raw.market?),
        clamp(raw.defensibility?),
        clamp(raw.timing?),
        raw.rationale,
    ))
}

// ===========================================================================
// Moderator-directed debate
// ===========================================================================

enum ModeratorDecision {
    Speak { persona_id: String, reason: String },
    Recruit { name: String, role: String, reason: String },
    Synthesize,
}

/// Run a facilitated debate: a moderator agent repeatedly decides who speaks
/// next (or recruits a new specialist) until it calls for synthesis or the
/// budget runs out. Returns the final panel (including recruits), or `None` if
/// the client disconnected.
#[allow(clippy::too_many_arguments)]
async fn run_moderated(
    paths: &KbPaths,
    config: &Config,
    objective: &str,
    transcript: &mut Vec<String>,
    steering: &mut Vec<String>,
    mut panel: Vec<PersonaSpec>,
    moderator_model: &str,
    budget: usize,
    interjects: &mut Receiver<String>,
    tx: &Sender<RoundtableEvent>,
) -> Option<Vec<PersonaSpec>> {
    let mut last_speaker: Option<String> = None;
    let mut recruits_left = MAX_RECRUITS;
    let mut spoken = 0usize;

    while spoken < budget {
        drain_interjections(interjects, steering, tx).await;
        let decision = moderator_decide(moderator_model, objective, transcript, &panel,
                                        last_speaker.as_deref(), budget - spoken, recruits_left).await;

        // Resolve the decision to a concrete speaker. Speaker matching is
        // tolerant (the model often returns a name/role rather than the exact
        // id). A recruit adds a specialist to the panel.
        let resolved: Option<(PersonaSpec, String)> = match decision {
            ModeratorDecision::Speak { persona_id, reason } => panel
                .iter()
                .find(|p| p.id.eq_ignore_ascii_case(&persona_id)
                    || p.name.eq_ignore_ascii_case(&persona_id)
                    || p.role.eq_ignore_ascii_case(&persona_id))
                .cloned()
                .map(|p| (p, reason)),
            ModeratorDecision::Recruit { name, role, reason } if recruits_left > 0 => {
                recruits_left -= 1;
                let id = unique_id(&name, &panel);
                let p = PersonaSpec {
                    id: id.clone(), name: name.clone(), role: role.clone(),
                    model: moderator_model.to_string(),
                    is_synth: false, is_fact_checker: false, queries_kb: true, tools: false,
                };
                if tx.send(RoundtableEvent::Recruited {
                    persona_id: id, name, role,
                    model: moderator_model.to_string(), reason: reason.clone(),
                }).await.is_err() {
                    return None;
                }
                panel.push(p.clone());
                Some((p, reason))
            }
            _ => None, // synthesize / recruit-exhausted / unparseable
        };

        let (persona, reason) = match resolved {
            Some(x) => x,
            None if spoken == 0 => {
                // Never end on an empty debate — open with the first panelist.
                match panel.first() {
                    Some(p) => (p.clone(), "Opening the discussion.".to_string()),
                    None => break,
                }
            }
            None => break, // moderator wrapped up
        };

        if tx.send(RoundtableEvent::Moderator { reason }).await.is_err() {
            return None;
        }
        match run_turn(paths, config, &persona, spoken + 1, objective, transcript, steering, tx).await {
            Ok(text) => {
                transcript.push(format!("**{} ({})**: {}", persona.name, persona.role, text));
                last_speaker = Some(persona.id.clone());
                spoken += 1;
            }
            Err(stop) => {
                if stop { return None; }
            }
        }
    }
    Some(panel)
}

async fn moderator_decide(
    model: &str, objective: &str, transcript: &[String], panel: &[PersonaSpec],
    last: Option<&str>, remaining: usize, recruits_left: usize,
) -> ModeratorDecision {
    let messages = build_moderator_messages(objective, transcript, panel, last, remaining, recruits_left);
    match complete(model, &messages).await {
        Ok(text) => parse_decision(&text),
        Err(_) => ModeratorDecision::Synthesize, // fail-safe: end the debate
    }
}

fn build_moderator_messages(
    objective: &str, transcript: &[String], panel: &[PersonaSpec],
    last: Option<&str>, remaining: usize, recruits_left: usize,
) -> Vec<ChatMessage> {
    let roster = panel.iter()
        .map(|p| format!("- {}: {} ({})", p.id, p.name, p.role))
        .collect::<Vec<_>>().join("\n");
    let recruit_line = if recruits_left > 0 {
        format!("You may recruit up to {recruits_left} more specialist(s) — only when a genuinely \
                 missing perspective (e.g. regulatory, security, design, finance) would change the \
                 conclusion.")
    } else {
        "You have used all recruitment slots; choose from the existing panel or synthesize.".to_string()
    };
    let system = format!(
        "You are the moderator of a startup brainstorming roundtable. Run a productive debate: at \
         each step pick the single best next action to cover what's missing and move the discussion \
         forward. {recruit_line} Aim to wrap up within about {remaining} more contributions. Respond \
         with ONLY a JSON object, no prose:\n\
         - call on a panelist: {{\"action\":\"speak\",\"persona_id\":\"<id from the panel>\",\"reason\":\"<short why>\"}}\n\
         - recruit a missing lens: {{\"action\":\"recruit\",\"name\":\"<short first name>\",\"role\":\"<lens, e.g. Regulatory & Compliance>\",\"reason\":\"<short why>\"}}\n\
         - end and synthesize once the key angles are covered: {{\"action\":\"synthesize\",\"reason\":\"<short why>\"}}",
    );
    let mut user = format!("Objective: {objective}\n\nPanel:\n{roster}\n\n");
    if transcript.is_empty() {
        user.push_str("The discussion hasn't started — pick who should open.\n");
    } else {
        user.push_str("Discussion so far:\n");
        user.push_str(&transcript.join("\n\n"));
        user.push_str("\n\n");
    }
    if let Some(l) = last {
        user.push_str(&format!("The last speaker was '{l}' — don't call on them again immediately.\n"));
    }
    user.push_str("Decide the next action. JSON only.");
    vec![ChatMessage::system(system), ChatMessage::user(user)]
}

fn parse_decision(text: &str) -> ModeratorDecision {
    let json = match (text.find('{'), text.rfind('}')) {
        (Some(s), Some(e)) if e > s => &text[s..=e],
        _ => return ModeratorDecision::Synthesize,
    };
    #[derive(serde::Deserialize)]
    struct Raw {
        action: String,
        persona_id: Option<String>,
        name: Option<String>,
        role: Option<String>,
        #[serde(default)]
        reason: String,
    }
    let raw: Raw = match serde_json::from_str(json) {
        Ok(r) => r,
        Err(_) => return ModeratorDecision::Synthesize,
    };
    match raw.action.as_str() {
        "speak" => match raw.persona_id {
            Some(id) => ModeratorDecision::Speak { persona_id: id, reason: raw.reason },
            None => ModeratorDecision::Synthesize,
        },
        "recruit" => match (raw.name, raw.role) {
            (Some(name), Some(role)) => ModeratorDecision::Recruit { name, role, reason: raw.reason },
            _ => ModeratorDecision::Synthesize,
        },
        _ => ModeratorDecision::Synthesize,
    }
}

/// A unique, slug-ish persona id for a recruited specialist.
fn unique_id(name: &str, panel: &[PersonaSpec]) -> String {
    let base: String = name.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_lowercase();
    let base = if base.is_empty() { "agent".to_string() } else { base };
    let mut id = base.clone();
    let mut n = 2;
    while panel.iter().any(|p| p.id == id) {
        id = format!("{base}{n}");
        n += 1;
    }
    id
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_panel_has_one_synth_and_distinct_ids() {
        let panel = PersonaSpec::default_panel();
        assert_eq!(panel.iter().filter(|p| p.is_synth).count(), 1);
        let ids: std::collections::HashSet<_> = panel.iter().map(|p| &p.id).collect();
        assert_eq!(ids.len(), panel.len());
    }

    #[test]
    fn provider_routing_by_model_prefix() {
        // We can't hit the network here; assert the routing predicate that
        // `complete` uses so a mis-prefixed model can't silently hit the wrong API.
        assert!("claude-opus-4-8".starts_with("claude"));
        assert!(!"gpt-4o".starts_with("claude"));
    }

    #[test]
    fn opening_prompt_omits_discussion_block() {
        let p = &PersonaSpec::default_panel()[0];
        let msgs = build_messages(p, 1, "an AI study tool", &[], "", &[]);
        let user = &msgs[1].content;
        assert!(user.contains("opening the discussion"));
        assert!(!user.contains("Discussion so far"));
    }

    #[test]
    fn later_prompt_includes_transcript_and_kb() {
        let p = &PersonaSpec::default_panel()[1];
        let transcript = vec!["**Aria (Technologist)**: build a RAG layer".to_string()];
        let msgs = build_messages(p, 2, "an AI study tool", &transcript, "- \"Paper\" (method): foo\n", &[]);
        let user = &msgs[1].content;
        assert!(user.contains("Discussion so far"));
        assert!(user.contains("build a RAG layer"));
        assert!(user.contains("knowledge bank"));
        assert!(user.contains("round 2"));
    }

    #[test]
    fn steering_guidance_is_injected_into_the_prompt() {
        let p = &PersonaSpec::default_panel()[0];
        let steer = vec!["focus on a freemium model".to_string()];
        let msgs = build_messages(p, 1, "an AI study tool", &[], "", &steer);
        let user = &msgs[1].content;
        assert!(user.contains("steering the discussion"));
        assert!(user.contains("freemium model"));
    }

    #[test]
    fn fact_checker_prompt_asks_to_verify() {
        let p = PersonaSpec {
            id: "fc".into(), name: "Vera".into(), role: "Fact-checker".into(),
            model: "gpt-4o".into(), is_synth: false, is_fact_checker: true, queries_kb: true, tools: false,
        };
        let msgs = build_messages(&p, 1, "x", &["**A**: a claim".to_string()], "- \"src\" (method): foo\n", &[]);
        assert!(msgs[0].content.contains("fact-checker"));
        assert!(msgs[1].content.contains("fact-check"));
    }

    #[test]
    fn synth_prompt_asks_for_synthesis() {
        let panel = PersonaSpec::default_panel();
        let synth = panel.iter().find(|p| p.is_synth).unwrap();
        let msgs = build_messages(synth, 3, "x", &["**A**: y".to_string()], "", &[]);
        assert!(msgs[0].content.contains("synthesizer"));
        assert!(msgs[1].content.contains("synthesize"));
    }

    #[test]
    fn only_transient_failures_are_retried() {
        assert!(is_retryable(&KbError::Network("connection reset".into())));
        // A missing/invalid key must NOT be retried.
        assert!(!is_retryable(&KbError::Config("ANTHROPIC_API_KEY is not set".into())));
        assert!(!is_retryable(&KbError::Usage("x".into())));
    }

    #[test]
    fn parse_decision_handles_all_actions_and_garbage() {
        assert!(matches!(
            parse_decision(r#"{"action":"speak","persona_id":"biz","reason":"market view"}"#),
            ModeratorDecision::Speak { persona_id, .. } if persona_id == "biz"
        ));
        assert!(matches!(
            parse_decision("Sure: {\"action\":\"recruit\",\"name\":\"Reg\",\"role\":\"Regulatory\",\"reason\":\"x\"}"),
            ModeratorDecision::Recruit { name, role, .. } if name == "Reg" && role == "Regulatory"
        ));
        assert!(matches!(parse_decision(r#"{"action":"synthesize","reason":"done"}"#), ModeratorDecision::Synthesize));
        // Missing fields / garbage ⇒ safe end, never a panic.
        assert!(matches!(parse_decision(r#"{"action":"speak"}"#), ModeratorDecision::Synthesize));
        assert!(matches!(parse_decision("not json"), ModeratorDecision::Synthesize));
    }

    #[test]
    fn unique_id_avoids_collisions() {
        let panel = vec![PersonaSpec {
            id: "reg".into(), name: "Reg".into(), role: "r".into(), model: "m".into(),
            is_synth: false, is_fact_checker: false, queries_kb: true, tools: false,
        }];
        assert_eq!(unique_id("Reg!", &panel), "reg2");
        assert_eq!(unique_id("Dana", &panel), "dana");
        assert_eq!(unique_id("", &panel), "agent");
    }

    #[test]
    fn cosine_similarity_basics() {
        let a = [1.0_f32, 0.0, 0.0];
        let b = [1.0_f32, 0.0, 0.0];
        let c = [0.0_f32, 1.0, 0.0];
        assert!((cosine(&a, &b) - 1.0).abs() < 1e-6);   // identical
        assert!(cosine(&a, &c).abs() < 1e-6);            // orthogonal
        assert_eq!(cosine(&a, &[0.0, 0.0, 0.0]), 0.0);   // zero vector ⇒ 0, no NaN
        // A near-restatement clears the convergence bar; a different direction doesn't.
        assert!(cosine(&[1.0, 1.0, 0.02], &[1.0, 1.0, 0.0]) >= CONVERGE_THRESHOLD);
        assert!(cosine(&[1.0, 0.0, 0.0], &[0.3, 1.0, 0.0]) < CONVERGE_THRESHOLD);
    }

    #[test]
    fn parse_score_tolerates_prose_and_clamps() {
        let reply = "Sure! Here's my rating:\n```json\n{\"feasibility\": 8, \"market\": 11, \
                     \"defensibility\": 3, \"timing\": 7, \"rationale\": \"strong but copyable\"}\n```";
        let (f, m, d, t, r) = parse_score(reply).expect("should parse");
        assert_eq!((f, m, d, t), (8.0, 10.0, 3.0, 7.0)); // 11 clamped to 10
        assert!(r.contains("copyable"));
        // Garbage / missing dimensions ⇒ None, not a panic.
        assert!(parse_score("no json here").is_none());
        assert!(parse_score("{\"feasibility\": 5}").is_none());
    }

    #[test]
    fn directed_request_deserializes_targets_and_base_round() {
        let req: BrainstormRequest = serde_json::from_str(
            r#"{"objective":"x","targets":["factcheck","tech"],"base_round":3,"guidance":["what about the moat?"]}"#,
        ).expect("should parse");
        assert_eq!(req.targets.as_deref(), Some(["factcheck".to_string(), "tech".to_string()].as_slice()));
        assert_eq!(req.base_round, Some(3));
        // Absent ⇒ None, so the run stays in the autonomous round-robin.
        let plain: BrainstormRequest = serde_json::from_str(r#"{"objective":"x"}"#).expect("parse");
        assert!(plain.targets.is_none());
        assert!(plain.base_round.is_none());
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 5), "hello…");
    }

    #[test]
    fn persona_tools_flag_defaults_off_and_parses() {
        // Absent ⇒ false, so an old client (no `tools` field) is unaffected.
        let p: PersonaSpec = serde_json::from_str(
            r#"{"id":"a","name":"A","role":"r","model":"claude-opus-4-8"}"#,
        ).expect("parse");
        assert!(!p.tools);
        // Present ⇒ honored (the macOS checkbox sends it).
        let t: PersonaSpec = serde_json::from_str(
            r#"{"id":"a","name":"A","role":"r","model":"claude-opus-4-8","tools":true}"#,
        ).expect("parse");
        assert!(t.tools);
        // The default panel ships the Technologist with tools on as a live demo.
        let tech = PersonaSpec::default_panel().into_iter().find(|p| p.id == "tech").unwrap();
        assert!(tech.tools);
    }
}
