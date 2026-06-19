import SwiftUI
import Observation

// Drives a live roundtable by consuming the engine's `/brainstorm` SSE stream.
// The engine runs the panel of agents (mixed Claude/OpenAI models), grounds them
// in the corpus, and pushes turn-level events; this session maps those onto the
// observable state the canvas and transcript render. Completed turns are
// revealed word-by-word client-side so the debate still *streams* on screen.
@MainActor
@Observable
final class RoundtableSession {
    enum Phase { case idle, running, done, replaying }

    var objective: String
    var personas: [Persona]
    /// Rounds for the *next* run (the setup stepper, or a continuation amount).
    var rounds: Int

    /// Identity of the persisted record this session reads/writes. A fresh start
    /// mints a new one; continuing or resuming reuses it so history updates in place.
    private(set) var recordId = UUID()
    private var createdAt = Date()

    private(set) var turns: [AgentTurn] = []
    /// Per-agent ratings of the idea (the scorecard / radar).
    private(set) var scores: [AgentScore] = []
    /// Whether the engine runs a scoring pass at the end of a run.
    var scoreEnabled = true
    /// Whether the engine stops rounds early when the debate converges.
    var convergeEnabled = true
    /// Whether a moderator agent directs the debate (vs fixed round-robin).
    var moderatedEnabled = false
    /// The moderator's latest decision rationale (shown while running).
    private(set) var moderatorNote: String?

    // Replay: step through a finished debate on a timeline, re-animating the
    // canvas. `playhead` is how many turns are currently revealed.
    private var replayTurns: [AgentTurn] = []
    private var replayScores: [AgentScore] = []
    private(set) var playhead: Int = 0
    private(set) var isReplayPlaying = false
    var replaySpeed: Double = 1.0
    var replayTotal: Int { replayTurns.count }
    var isReplaying: Bool { phase == .replaying }
    private(set) var interjections: [Interjection] = []
    /// Live activity log — a running console of what the engine is doing, so the
    /// app visibly shows it's functioning (and surfaces errors plainly).
    private(set) var log: [LogEntry] = []
    private(set) var phase: Phase = .idle
    /// The persona currently holding the floor (glows on the canvas).
    private(set) var activePersona: String? = nil
    private(set) var currentRound: Int = 0
    /// The personas the user addressed in the in-flight directed exchange (the
    /// `@mention` chat). Empty during an autonomous run; drives the status label.
    private(set) var directedTargets: [String] = []

    private var driver: Task<Void, Never>?
    /// Live connection + session id, retained so mid-debate interjections can be
    /// pushed to the running roundtable.
    private var client: KBClient?
    private var sessionId = UUID().uuidString

    init(objective: String = "",
         personas: [Persona] = Persona.defaultPanel,
         rounds: Int = 2) {
        self.objective = objective
        self.personas = personas
        self.rounds = rounds
    }

    var debaters: [Persona] { personas.filter { !$0.isSynth } }
    var synthesizer: Persona? { personas.first { $0.isSynth } }
    var isRunning: Bool { phase == .running }

    func persona(_ id: String) -> Persona? {
        if id == Persona.you.id { return .you }
        return personas.first { $0.id == id }
    }

    /// Comma-joined names of the agents addressed in the current directed
    /// exchange (for the status label, e.g. "Asking Vera, Aria").
    var directedNames: String {
        directedTargets.compactMap { persona($0)?.name }.joined(separator: ", ")
    }
    func turns(forRound round: Int) -> [AgentTurn] { turns.filter { $0.round == round } }
    var latestTurn: AgentTurn? { turns.last }

    /// The synthesizer's latest real contribution (if the debate produced one).
    var synthesisText: String? {
        guard let synthId = personas.first(where: { $0.isSynth })?.id else { return nil }
        return turns.last { $0.personaId == synthId && !$0.text.isEmpty && !$0.text.hasPrefix("⚠︎") }?.text
    }

    /// Capture this research back into the knowledge bank as a searchable idea —
    /// the synthesis up top, the full discussion below. Closes the loop: future
    /// roundtables can retrieve your own past conclusions.
    func saveAsIdea(client: KBClient) {
        guard synthesisText != nil else {
            note(.warn, "No synthesis yet — finish a debate first")
            return
        }
        let title = String(objective.trimmingCharacters(in: .whitespacesAndNewlines).prefix(120))
        let body = ideaBody()
        note(.active, "Saving synthesis to the knowledge bank…")
        Task {
            do {
                _ = try await client.createIdea(title: title, body: body, tags: ["roundtable", "brainstorm"])
                note(.ok, "Saved to the knowledge bank as an idea ✓ — it's now searchable")
            } catch {
                note(.error, "Couldn't save idea: \(error.localizedDescription)")
            }
        }
    }

    private func ideaBody() -> String {
        var b = ""
        if let s = synthesisText { b += s + "\n\n" }
        let panel = personas.filter { !$0.isSynth }
            .map { "\($0.name) (\($0.role))" }.joined(separator: ", ")
        b += "---\n_Captured from a Roundtable debate among \(panel)._\n\n## Discussion\n\n"
        for turn in turns where !turn.text.isEmpty && !turn.text.hasPrefix("⚠︎") {
            if let p = persona(turn.personaId) {
                b += "**\(p.name) (\(p.role))**\n\n\(turn.text)\n\n"
            }
        }
        return b
    }

    private func name(_ personaId: String?) -> String {
        guard let id = personaId else { return "Agent" }
        return persona(id)?.name ?? id
    }

    /// Build a persona for a moderator-recruited specialist, cycling color/icon
    /// so it's visually distinct as it joins the table.
    private func recruitedPersona(id: String, name: String, role: String, modelId: String) -> Persona {
        let i = personas.count
        let colorName = PersonaPalette.options[i % PersonaPalette.options.count].name
        let icon = PersonaIcons.options[i % PersonaIcons.options.count]
        return Persona(id: id, name: name, role: role, icon: icon,
                       colorName: colorName, modelId: modelId, queriesKB: true)
    }

    private func note(_ level: LogEntry.Level, _ text: String) {
        log.append(LogEntry(level: level, text: text))
        if log.count > 300 { log.removeFirst(log.count - 300) }
    }

    // MARK: Control

    /// Kick off a live roundtable against the engine.
    func start(client: KBClient) {
        guard phase != .running else { return }
        let goal = objective.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !goal.isEmpty else { return }
        turns.removeAll()
        scores.removeAll()
        interjections.removeAll()
        log.removeAll()
        moderatorNote = nil
        currentRound = 0
        directedTargets = []
        recordId = UUID()
        createdAt = Date()
        phase = .running
        self.client = client
        self.sessionId = UUID().uuidString
        note(.info, "Starting roundtable · \(personas.count) agents · \(rounds) round\(rounds == 1 ? "" : "s")")

        let payload = personas.map(\.wirePayload)
        driver = Task { await self.consume(client: client, sessionId: sessionId, personas: payload,
                                           transcript: [], guidance: []) }
    }

    /// Continue a finished debate: keep everything on screen, seed the agents
    /// with the discussion so far plus the user's new points, and run more
    /// rounds. Reuses the same record so the research grows in place.
    func continueDiscussion(client: KBClient, guidance: [String], rounds: Int) {
        guard phase == .done else { return }
        self.rounds = max(1, rounds)
        self.client = client
        self.sessionId = UUID().uuidString
        phase = .running
        scores.removeAll()   // re-score after the new rounds
        let seed = seedTranscript()
        if !guidance.isEmpty {
            note(.warn, "Continuing with your points: \(guidance.joined(separator: "; "))")
        } else {
            note(.info, "Continuing the discussion · \(self.rounds) more round\(self.rounds == 1 ? "" : "s")")
        }
        let payload = personas.map(\.wirePayload)
        driver = Task { await self.consume(client: client, sessionId: sessionId, personas: payload,
                                           transcript: seed, guidance: guidance) }
    }

    /// Drive the table conversationally: the user addresses specific agents with
    /// `@mentions` and only those speak (in mention order), answering `message`;
    /// the synthesizer then re-synthesizes and the debaters re-score. Reuses the
    /// same record so the conversation grows in place. No-op without targets — per
    /// the chat model, no agent gets involved when none are addressed.
    func directedExchange(client: KBClient, message: String, targetIds: [String]) {
        guard phase == .done, !targetIds.isEmpty else { return }
        let msg = message.trimmingCharacters(in: .whitespacesAndNewlines)
        self.client = client
        self.sessionId = UUID().uuidString
        phase = .running
        scores.removeAll()                 // re-score after this exchange
        directedTargets = targetIds
        // Capture the prior discussion *before* showing the new user message, so
        // the engine adds the user's line itself (no duplication in the seed).
        let seed = seedTranscript()
        // Number this exchange's turns after everything on screen so they sort last.
        let exchangeRound = (turns.map(\.round).max() ?? rounds) + 1
        var userTurn = AgentTurn(personaId: Persona.you.id, round: exchangeRound, status: .done)
        userTurn.text = msg
        turns.append(userTurn)
        let names = targetIds.compactMap { persona($0)?.name }.joined(separator: ", ")
        note(.info, "You → \(names): \(msg)")
        let payload = personas.map(\.wirePayload)
        driver = Task {
            await self.consume(client: client, sessionId: sessionId, personas: payload,
                               transcript: seed, guidance: msg.isEmpty ? [] : [msg],
                               targets: targetIds, baseRound: exchangeRound)
        }
    }

    /// Persona ids the text addresses via `@name` tokens (e.g. "@vera @aria"),
    /// resolved against `personas`, in order, de-duplicated. Tolerates trailing
    /// punctuation on a mention ("@vera,") and is case-insensitive.
    static func parseMentions(_ text: String, personas: [Persona]) -> [String] {
        let byName: [String: String] = personas.reduce(into: [:]) { acc, p in
            acc[p.name.lowercased()] = p.id
        }
        var ids: [String] = []
        for token in text.split(whereSeparator: { $0 == " " || $0 == "\n" || $0 == "\t" }) {
            guard token.hasPrefix("@") else { continue }
            let name = token.dropFirst().prefix { $0.isLetter || $0.isNumber }.lowercased()
            if let id = byName[String(name)], !ids.contains(id) { ids.append(id) }
        }
        return ids
    }

    /// Reopen a saved roundtable in a read-only "done" state, ready to continue.
    func loadRecord(_ record: RoundtableRecord) {
        driver?.cancel()
        objective = record.objective
        personas = record.personas
        turns = record.turns
        scores = record.scores
        recordId = record.id
        createdAt = record.createdAt
        interjections.removeAll()
        log.removeAll()
        currentRound = 0
        activePersona = nil
        directedTargets = []
        phase = .done
        note(.info, "Loaded saved research · \(turns.count) contribution\(turns.count == 1 ? "" : "s")")
    }

    /// The discussion so far, formatted as the engine's transcript lines (errors
    /// excluded). Fed back when continuing so agents build on prior turns.
    private func seedTranscript() -> [String] {
        turns.compactMap { turn in
            guard !turn.text.isEmpty, !turn.text.hasPrefix("⚠︎") else { return nil }
            if turn.personaId == Persona.you.id { return "**You**: \(turn.text)" }
            guard let p = persona(turn.personaId) else { return nil }
            return "**\(p.name) (\(p.role))**: \(turn.text)"
        }
    }

    private func saveRecord() {
        RoundtableStore.shared.save(snapshotRecord())
    }

    /// This session's current state as a `RoundtableRecord` — the same value that
    /// gets persisted, reused by the PDF exporter so live and saved runs share
    /// one report path.
    func snapshotRecord() -> RoundtableRecord {
        RoundtableRecord(id: recordId, objective: objective,
                         createdAt: createdAt, updatedAt: Date(),
                         personas: personas, turns: turns, scores: scores)
    }

    func reset() {
        driver?.cancel()
        driver = nil
        isReplayPlaying = false
        turns.removeAll()
        scores.removeAll()
        interjections.removeAll()
        replayTurns.removeAll()
        replayScores.removeAll()
        playhead = 0
        activePersona = nil
        currentRound = 0
        directedTargets = []
        phase = .idle
    }

    // MARK: Replay

    /// Replay the current finished debate as a timeline: clear the canvas, then
    /// re-animate every turn from the stored record. Works for a just-finished
    /// debate or one reopened from History.
    func startReplay() {
        guard !turns.isEmpty, phase == .done else { return }
        driver?.cancel()
        replayTurns = turns
        replayScores = scores
        turns = []
        scores = []
        log.removeAll()
        playhead = 0
        currentRound = 0
        activePersona = nil
        phase = .replaying
        note(.info, "Replaying \(replayTurns.count) contributions")
        play()
    }

    func toggleReplayPlay() {
        guard phase == .replaying else { return }
        if isReplayPlaying { pauseReplay() } else { play() }
    }

    func pauseReplay() {
        isReplayPlaying = false
        driver?.cancel()
    }

    /// Jump to a point on the timeline (number of turns revealed). Pauses play.
    func seek(to index: Int) {
        guard phase == .replaying else { return }
        pauseReplay()
        let n = max(0, min(index, replayTotal))
        turns = replayTurns.prefix(n).map { var t = $0; t.status = .done; return t }
        scores = n >= replayTotal ? replayScores : []
        activePersona = n > 0 ? replayTurns[n - 1].personaId : nil
        currentRound = n > 0 ? replayTurns[n - 1].round : 0
        playhead = n
    }

    func cycleReplaySpeed() {
        replaySpeed = replaySpeed == 1 ? 2 : (replaySpeed == 2 ? 0.5 : 1)
    }

    /// Leave replay: restore the full debate in its finished state.
    func exitReplay() {
        pauseReplay()
        turns = replayTurns
        scores = replayScores
        activePersona = nil
        currentRound = replayTurns.last?.round ?? 0
        phase = .done
    }

    private func play() {
        guard phase == .replaying, !isReplayPlaying else { return }
        if playhead >= replayTotal { seek(to: 0) }   // restart from the top
        isReplayPlaying = true
        driver = Task {
            while self.isReplayPlaying && self.playhead < self.replayTurns.count {
                await self.revealNext()
                if Task.isCancelled { return }
                try? await Task.sleep(nanoseconds: UInt64(0.35 / max(self.replaySpeed, 0.1) * 1_000_000_000))
            }
            if self.playhead >= self.replayTurns.count {
                self.isReplayPlaying = false
                self.scores = self.replayScores      // reveal the scorecard at the end
                self.activePersona = nil
            }
        }
    }

    /// Reveal the next stored turn with the typewriter effect (the re-animation).
    private func revealNext() async {
        let idx = playhead
        guard idx < replayTurns.count else { return }
        let source = replayTurns[idx]
        currentRound = source.round
        activePersona = source.personaId

        var t = source
        let full = t.text
        t.text = ""
        t.status = full.hasPrefix("⚠︎") ? .done : .streaming
        turns.append(t)               // index == idx (turns was prefix(idx))
        playhead = turns.count

        guard !full.hasPrefix("⚠︎") else { return }
        for word in full.split(separator: " ", omittingEmptySubsequences: false) {
            if Task.isCancelled || !isReplayPlaying { turns[idx].text = full; turns[idx].status = .done; return }
            turns[idx].text += (turns[idx].text.isEmpty ? "" : " ") + word
            try? await Task.sleep(nanoseconds: UInt64(0.02 / max(replaySpeed, 0.1) * 1_000_000_000))
        }
        turns[idx].text = full
        turns[idx].status = .done
    }

    /// Inject a guiding idea into the running debate. It surfaces on the canvas
    /// immediately and is pushed to the engine, which folds it into the prompts
    /// of every agent that speaks from here on.
    func interject(_ text: String) {
        let t = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !t.isEmpty else { return }
        interjections.append(Interjection(text: t, atRound: currentRound))
        if isRunning, let client {
            let sid = sessionId
            Task { await client.interject(sessionId: sid, text: t) }
        }
    }

    // MARK: Stream consumption

    private func consume(client: KBClient, sessionId: String, personas: [[String: Any]],
                         transcript: [String], guidance: [String],
                         targets: [String] = [], baseRound: Int? = nil) async {
        note(.info, "Connecting to engine…")
        var count = 0
        do {
            for try await ev in client.brainstorm(objective: objective, personas: personas,
                                                  rounds: rounds, sessionId: sessionId,
                                                  transcript: transcript, guidance: guidance,
                                                  score: scoreEnabled, converge: convergeEnabled,
                                                  moderated: moderatedEnabled,
                                                  targets: targets, baseRound: baseRound) {
                if Task.isCancelled { return }
                if count == 0 { note(.ok, "Stream opened") }
                count += 1
                await handle(ev)
            }
        } catch {
            if !Task.isCancelled { reportError(error.localizedDescription) }
        }
        note(.info, "Stream closed · \(count) event\(count == 1 ? "" : "s")")
        if count == 0 {
            note(.error, "No events received — is the engine running and reachable?")
        }
        if phase == .running { phase = .done }
        activePersona = nil
        directedTargets = []
        if !turns.isEmpty { saveRecord() }   // persist the research
    }

    private func handle(_ ev: RoundtableEvent) async {
        switch ev.type {
        case "start":
            currentRound = 0
            note(.info, "▸ Objective accepted")
        case "turn_start":
            activePersona = ev.personaId
            if let r = ev.round { currentRound = r }
            turns.append(AgentTurn(personaId: ev.personaId ?? "", round: ev.round ?? 0, status: .thinking))
            note(.active, "\(name(ev.personaId)) · round \(ev.round ?? 0) — thinking")
        case "kb_query":
            updateLast(ev.personaId) { $0.status = .queryingKB }
            note(.active, "\(name(ev.personaId)) — searching the corpus")
        case "citation":
            if let title = ev.title {
                updateLast(ev.personaId) {
                    $0.citations.append(AgentCitation(title: title,
                                                      sectionType: ev.sectionType ?? "",
                                                      page: ev.page))
                }
                note(.info, "   cited “\(title)”")
            }
        case "turn":
            let text = ev.text ?? ""
            note(.ok, "\(name(ev.personaId)) — spoke (\(text.count) chars)")
            await reveal(ev.personaId, text: text)
        case "converged":
            let pct = Int((ev.similarity ?? 0) * 100)
            note(.ok, "Agents converged after round \(ev.round ?? 0) (\(pct)% similar) — jumping to synthesis, saving rounds")
        case "moderator":
            moderatorNote = ev.reason
            if let r = ev.reason, !r.isEmpty { note(.active, "Moderator: \(r)") }
        case "recruited":
            if let id = ev.personaId, let nm = ev.name, let rl = ev.role {
                if !personas.contains(where: { $0.id == id }) {
                    personas.append(recruitedPersona(id: id, name: nm, role: rl, modelId: ev.model ?? LLMModel.gpt4o.id))
                }
                note(.ok, "Moderator recruited \(nm) — \(rl)")
            }
        case "score":
            if let f = ev.feasibility, let m = ev.market, let d = ev.defensibility, let t = ev.timing {
                scores.removeAll { $0.personaId == ev.personaId }
                scores.append(AgentScore(personaId: ev.personaId ?? "", feasibility: f, market: m,
                                         defensibility: d, timing: t, rationale: ev.rationale ?? ""))
                let avg = (f + m + d + t) / 4
                note(.ok, "\(name(ev.personaId)) scored the idea · avg \(String(format: "%.1f", avg))/10")
            }
        case "retry":
            // A transient failure is being retried — keep the node "thinking".
            updateLast(ev.personaId) { $0.status = .thinking }
            note(.warn, "\(name(ev.personaId)) — retry \(ev.attempt ?? 1)/\(2) after error: \(ev.message ?? "")")
        case "interjected":
            note(.warn, "↳ steering added: \(ev.text ?? "")")
        case "done":
            phase = .done
            activePersona = nil
            note(.ok, "✓ Debate complete")
        case "error":
            // Per-agent failure (e.g. a missing provider key). Mark this agent's
            // turn inline and keep going — other agents may still succeed.
            note(.error, "⚠︎ \(ev.message ?? "this agent failed")")
            markActiveTurnFailed(ev.message ?? "this agent failed")
        default:
            break
        }
    }

    /// Attach an error to the agent currently holding the floor, without ending
    /// the session — the stream continues to the next agent.
    private func markActiveTurnFailed(_ message: String) {
        updateLast(activePersona) {
            $0.text = "⚠︎ \(message)"
            $0.status = .done
        }
    }

    /// Mutate the most recent turn for a persona (status/citations updates).
    private func updateLast(_ personaId: String?, _ mutate: (inout AgentTurn) -> Void) {
        guard let pid = personaId, let idx = turns.lastIndex(where: { $0.personaId == pid }) else { return }
        var t = turns[idx]
        mutate(&t)
        turns[idx] = t
    }

    /// Typewriter-reveal a completed turn into its card so it animates on screen.
    private func reveal(_ personaId: String?, text: String) async {
        guard let pid = personaId, let idx = turns.lastIndex(where: { $0.personaId == pid }) else { return }
        turns[idx].status = .streaming
        turns[idx].text = ""
        for word in text.split(separator: " ", omittingEmptySubsequences: false) {
            if Task.isCancelled { turns[idx].text = text; break }
            turns[idx].text += (turns[idx].text.isEmpty ? "" : " ") + word
            try? await Task.sleep(nanoseconds: UInt64(Double.random(in: 0.01...0.035) * 1_000_000_000))
        }
        turns[idx].text = text
        turns[idx].status = .done
    }

    private func reportError(_ message: String) {
        note(.error, "Stream error: \(message)")
        if let pid = activePersona, let idx = turns.lastIndex(where: { $0.personaId == pid }) {
            turns[idx].text = "⚠︎ \(message)"
            turns[idx].status = .done
        }
        phase = .done
        activePersona = nil
    }
}
