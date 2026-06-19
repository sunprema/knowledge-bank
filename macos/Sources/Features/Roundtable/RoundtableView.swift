import SwiftUI

// The Roundtable screen is a workspace of debate tabs (like the Library): each
// tab is an independent debate session, so one objective can stream live while
// you read another — or view two side by side in split view. Editing the panel
// of agents and browsing research history are workspace-level sheets.
@MainActor
struct RoundtableView: View {
    let client: KBClient
    /// Objective seeded from another view (e.g. a Problems "Brainstorm" action).
    @Binding var seed: String?
    /// Switch the app to the Personas section to author the library.
    var onManagePersonas: () -> Void = {}

    @Environment(PersonaStore.self) private var personaStore
    @State private var workspace = RoundtableWorkspace()
    @State private var showHistory = false

    var body: some View {
        VStack(spacing: 0) {
            WorkspaceTabStrip(workspace: workspace)
            Divider()
            content
        }
        .navigationTitle("Roundtable")
        .toolbar {
            ToolbarItemGroup {
                Button { withAnimation(.snappy) { _ = workspace.newTab() } } label: {
                    Image(systemName: "plus.rectangle")
                }
                .help("New debate tab")
                Button { onManagePersonas() } label: { Image(systemName: "person.2.badge.gearshape") }
                    .help("Manage personas")
                Button { showHistory = true } label: { Image(systemName: "clock.arrow.circlepath") }
                    .help("Research history")
            }
        }
        .sheet(isPresented: $showHistory) {
            RoundtableHistoryView(currentId: workspace.selected?.session.recordId ?? UUID(),
                                  onOpen: { record in workspace.openRecord(record); showHistory = false },
                                  onClose: { showHistory = false })
        }
        .onAppear(perform: consumeSeed)
        .onChange(of: seed) { consumeSeed() }
    }

    @ViewBuilder private var content: some View {
        if workspace.isSplit, let left = workspace.selected, let right = workspace.splitTab {
            HSplitView {
                DebateView(tab: left, store: personaStore, client: client,
                           onManagePersonas: onManagePersonas)
                    .id(left.id).frame(minWidth: 380)
                DebateView(tab: right, store: personaStore, client: client,
                           onManagePersonas: onManagePersonas)
                    .id(right.id).frame(minWidth: 380)
            }
        } else if let sel = workspace.selected {
            DebateView(tab: sel, store: personaStore, client: client,
                       onManagePersonas: onManagePersonas)
                .id(sel.id)
        }
    }

    /// Apply an objective seeded from elsewhere: reuse the selected tab if it's
    /// idle, otherwise open a fresh one.
    private func consumeSeed() {
        guard let s = seed else { return }
        if let sel = workspace.selected, sel.session.phase == .idle {
            sel.objective = s
        } else {
            workspace.newTab(objective: s)
        }
        seed = nil
    }
}

// MARK: - One debate

@MainActor
private struct DebateView: View {
    @Bindable var tab: DebateTab
    @Bindable var store: PersonaStore
    let client: KBClient
    let onManagePersonas: () -> Void

    @State private var idea = ""
    @State private var showLog = true

    private var session: RoundtableSession { tab.session }

    /// The personas chosen for this debate (empty selection ⇒ the whole library).
    private var chosenPersonas: [Persona] {
        tab.selectedPersonaIds.isEmpty
            ? store.personas
            : store.personas.filter { tab.selectedPersonaIds.contains($0.id) }
    }

    var body: some View {
        if session.phase == .idle {
            SetupView(tab: tab, store: store, goToPersonas: onManagePersonas, onStart: startDebate)
        } else {
            liveLayout
        }
    }

    private func startDebate() {
        session.objective = tab.objective
        session.personas = chosenPersonas
        session.rounds = tab.rounds
        session.scoreEnabled = tab.scoreEnabled
        session.convergeEnabled = tab.convergeEnabled
        session.moderatedEnabled = tab.moderatedEnabled
        session.start(client: client)
    }

    // MARK: Live layout

    private var liveLayout: some View {
        VStack(spacing: 0) {
            statusBar
            Divider()
            HSplitView {
                RoundtableCanvas(session: session)
                    .frame(minWidth: 380)
                    .padding(10)
                TranscriptPanel(session: session)
                    .frame(minWidth: 300, idealWidth: 380)
            }
            if showLog {
                Divider()
                LogConsole(session: session).frame(height: 150)
            }
            if session.isRunning {
                Divider()
                composer
            } else if session.isReplaying {
                Divider()
                replayBar
            } else if session.phase == .done {
                Divider()
                DirectedComposer(session: session, client: client)
            }
        }
    }

    private var statusBar: some View {
        HStack(spacing: 12) {
            Image(systemName: "person.3.sequence.fill").foregroundStyle(.tint)
            VStack(alignment: .leading, spacing: 1) {
                Text(session.objective).font(.subheadline.weight(.semibold)).lineLimit(1)
                Text(phaseLabel).font(.caption).foregroundStyle(.secondary).lineLimit(1)
            }
            Spacer()
            statusIndicator
            controls
        }
        .padding(.horizontal, 16).padding(.vertical, 10)
    }

    @ViewBuilder private var statusIndicator: some View {
        if session.isRunning {
            HStack(spacing: 6) {
                ProgressView().controlSize(.small)
                if session.directedTargets.isEmpty {
                    Text("Round \(min(session.currentRound, session.rounds)) / \(session.rounds)")
                        .font(.caption.monospacedDigit()).foregroundStyle(.secondary)
                } else {
                    Text("Asking \(session.directedNames)")
                        .font(.caption).foregroundStyle(.secondary).lineLimit(1)
                }
            }
        } else if session.isReplaying {
            Label("Replay \(session.playhead)/\(session.replayTotal)", systemImage: "play.rectangle.fill")
                .font(.caption.weight(.semibold)).foregroundStyle(.tint)
        } else if session.phase == .done {
            Label("Saved · \(session.turns.count)", systemImage: "checkmark.seal.fill")
                .font(.caption.weight(.semibold)).foregroundStyle(.green)
        }
    }

    private var controls: some View {
        HStack(spacing: 8) {
            if session.phase == .done, !session.turns.isEmpty {
                Button { session.startReplay() } label: { Image(systemName: "play.rectangle") }
                    .help("Replay this debate on a timeline")
                if session.synthesisText != nil {
                    Button { session.saveAsIdea(client: client) } label: { Image(systemName: "lightbulb.fill") }
                        .help("Save synthesis to the knowledge bank as an idea")
                }
            }
            Button { showLog.toggle() } label: { Image(systemName: "terminal") }
                .help(showLog ? "Hide activity log" : "Show activity log")
            Button { session.reset(); tab.objective = "" } label: { Image(systemName: "square.and.pencil") }
                .help("New debate in this tab")
        }
        .buttonStyle(.borderless)
    }

    private var phaseLabel: String {
        switch session.phase {
        case .idle: return ""
        case .running:
            if let id = session.activePersona, let p = session.persona(id) {
                return "\(p.name) · \(p.role) is contributing…"
            }
            if let note = session.moderatorNote { return "Moderator: \(note)" }
            return "Convening the table…"
        case .done: return "Debate complete — @mention agents below to ask them directly"
        case .replaying:
            if let id = session.activePersona, let p = session.persona(id) {
                return "Replaying — \(p.name) · \(p.role)"
            }
            return "Replay — scrub the timeline or press play"
        }
    }

    private var replayBar: some View {
        HStack(spacing: 12) {
            Button { session.toggleReplayPlay() } label: {
                Image(systemName: session.isReplayPlaying ? "pause.fill" : "play.fill").font(.title3)
            }
            .buttonStyle(.borderless)
            .help(session.isReplayPlaying ? "Pause" : "Play")

            Slider(value: Binding(get: { Double(session.playhead) },
                                  set: { session.seek(to: Int($0.rounded())) }),
                   in: 0...Double(max(session.replayTotal, 1)), step: 1)

            Text("\(session.playhead)/\(session.replayTotal)")
                .font(.caption.monospacedDigit().weight(.medium))
                .foregroundStyle(.secondary).frame(width: 46)

            Button { session.cycleReplaySpeed() } label: {
                Text(session.replaySpeed == 1 ? "1×" : (session.replaySpeed == 2 ? "2×" : "0.5×"))
                    .font(.caption.weight(.semibold).monospacedDigit()).frame(width: 26)
            }
            .buttonStyle(.bordered).controlSize(.small).help("Playback speed")

            Button { session.exitReplay() } label: {
                Image(systemName: "xmark.circle.fill")
            }
            .buttonStyle(.borderless).foregroundStyle(.secondary).help("Exit replay")
        }
        .padding(.horizontal, 14).padding(.vertical, 10)
        .background(.bar)
    }

    private var composer: some View {
        HStack(spacing: 10) {
            Image(systemName: "lightbulb").foregroundStyle(.yellow)
            TextField("Steer the live debate — add a guiding idea…", text: $idea, axis: .vertical)
                .textFieldStyle(.plain)
                .lineLimit(1...3)
                .onSubmit(submitIdea)
            Button(action: submitIdea) {
                Image(systemName: "arrow.up.circle.fill").font(.title2)
            }
            .buttonStyle(.borderless)
            .disabled(idea.trimmingCharacters(in: .whitespaces).isEmpty)
        }
        .padding(.horizontal, 14).padding(.vertical, 10)
    }

    private func submitIdea() {
        let t = idea.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !t.isEmpty else { return }
        session.interject(t)
        idea = ""
    }
}

// MARK: - Directed composer (@mention chat)

/// The conversational composer shown once the opening debate finishes: the user
/// addresses specific agents with `@mentions` and only those reply (then the
/// synthesizer re-synthesizes and the debaters re-score). Typing `@` opens an
/// autocomplete of the panel; picking an agent inserts `@Name`.
@MainActor
private struct DirectedComposer: View {
    @Bindable var session: RoundtableSession
    let client: KBClient

    @State private var text = ""
    @FocusState private var focused: Bool

    private var personas: [Persona] { session.personas }
    private var targetIds: [String] { RoundtableSession.parseMentions(text, personas: personas) }

    /// The in-progress `@mention` at the end of the text (its range + the typed
    /// fragment), or nil if the user isn't currently typing one.
    private var activeMention: (range: Range<String.Index>, query: String)? {
        guard let at = text.range(of: "@", options: .backwards) else { return nil }
        let after = text[at.upperBound...]
        if after.contains(where: { $0 == " " || $0 == "\n" || $0 == "\t" }) { return nil }
        return (at.lowerBound..<text.endIndex, String(after))
    }

    private func matches(_ query: String) -> [Persona] {
        let q = query.lowercased()
        return personas.filter { q.isEmpty || $0.name.lowercased().hasPrefix(q) }
    }

    var body: some View {
        VStack(spacing: 0) {
            if let mention = activeMention, !matches(mention.query).isEmpty {
                mentionMenu(mention)
                Divider()
            }
            HStack(spacing: 10) {
                Image(systemName: "at").foregroundStyle(.tint)
                TextField("Address the table — type @ to pick an agent…", text: $text, axis: .vertical)
                    .textFieldStyle(.plain)
                    .lineLimit(1...4)
                    .focused($focused)
                    .onSubmit(send)
                Button(action: send) {
                    Image(systemName: "arrow.up.circle.fill").font(.title2)
                }
                .buttonStyle(.borderless)
                .disabled(targetIds.isEmpty)
                .help(targetIds.isEmpty ? "@mention at least one agent to ask them" : "Ask the addressed agents")
            }
            .padding(.horizontal, 14).padding(.vertical, 10)
        }
        .background(.bar)
    }

    private func mentionMenu(_ mention: (range: Range<String.Index>, query: String)) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(matches(mention.query)) { p in
                Button { insert(p, replacing: mention.range) } label: {
                    HStack(spacing: 8) {
                        Image(systemName: p.icon).foregroundStyle(p.color).frame(width: 18)
                        Text(p.name).font(.callout.weight(.medium))
                        Text(p.role).font(.caption).foregroundStyle(.secondary)
                        Spacer()
                        if p.isSynth { Chip(text: "synthesizer", color: .accentColor, filled: true) }
                        else if p.isFactChecker { Chip(text: "fact-checker", color: .teal, filled: true) }
                    }
                    .padding(.horizontal, 12).padding(.vertical, 5)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
        }
        .padding(.vertical, 4)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.background)
    }

    private func insert(_ p: Persona, replacing range: Range<String.Index>) {
        text.replaceSubrange(range, with: "@\(p.name) ")
        focused = true
    }

    private func send() {
        let ids = targetIds
        guard !ids.isEmpty else { return }
        session.directedExchange(client: client, message: text, targetIds: ids)
        text = ""
    }
}

// MARK: - Setup

@MainActor
private struct SetupView: View {
    @Bindable var tab: DebateTab
    @Bindable var store: PersonaStore
    let goToPersonas: () -> Void
    let onStart: () -> Void

    @Environment(ServerController.self) private var server

    /// Personas chosen for this debate (empty selection ⇒ the whole library).
    private var chosen: [Persona] {
        tab.selectedPersonaIds.isEmpty
            ? store.personas
            : store.personas.filter { tab.selectedPersonaIds.contains($0.id) }
    }
    private var needsAnthropicKey: Bool {
        !server.hasAnthropicKey && chosen.contains { $0.model.provider == .anthropic }
    }
    private var needsOpenAIKey: Bool {
        !server.hasOpenAIKey && chosen.contains { $0.model.provider == .openai }
    }
    private var noSynthesizer: Bool { !chosen.contains { $0.isSynth } }

    var body: some View {
        ScrollView {
            VStack(spacing: 24) {
                header
                objectiveField
                roster
                if noSynthesizer && !chosen.isEmpty { synthWarning }
                roundsField
                if needsAnthropicKey || needsOpenAIKey { keyWarning }
                Button(action: onStart) {
                    Label("Start Roundtable", systemImage: "play.fill").frame(maxWidth: 240)
                }
                .buttonStyle(.borderedProminent)
                .controlSize(.large)
                .disabled(tab.objective.trimmingCharacters(in: .whitespaces).isEmpty || chosen.isEmpty)
            }
            .padding(32)
            .frame(maxWidth: 720)
            .frame(maxWidth: .infinity)
        }
        .onAppear {
            // Default the table to the whole library the first time this tab opens.
            if tab.selectedPersonaIds.isEmpty {
                tab.selectedPersonaIds = Set(store.personas.map(\.id))
            }
        }
    }

    private var synthWarning: some View {
        Label("No synthesizer selected — the debate won't get a final synthesis. Include a persona with the Synthesizer role.",
              systemImage: "exclamationmark.triangle.fill")
            .font(.caption).foregroundStyle(.orange)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(10)
            .background(.orange.opacity(0.12), in: RoundedRectangle(cornerRadius: Theme.corner))
    }

    private var header: some View {
        VStack(spacing: 8) {
            Image(systemName: "person.3.sequence.fill")
                .font(.system(size: 40)).foregroundStyle(.tint)
            Text("Brainstorm Roundtable").font(.title2.weight(.bold))
            Text("Specialist agents debate your objective — grounded in your knowledge bank — and converge on a synthesis. Steer them live, then continue as deep as you like.")
                .font(.callout).foregroundStyle(.secondary)
                .multilineTextAlignment(.center).frame(maxWidth: 460)
        }
    }

    private var objectiveField: some View {
        Card {
            VStack(alignment: .leading, spacing: 8) {
                Label("Objective", systemImage: "target").font(.subheadline.weight(.semibold))
                TextField("e.g. An AI study companion for medical residents",
                          text: $tab.objective, axis: .vertical)
                    .textFieldStyle(.plain)
                    .lineLimit(2...4)
                    .font(.body)
                    .padding(10)
                    .background(.background, in: RoundedRectangle(cornerRadius: Theme.corner))
            }
        }
    }

    private var roster: some View {
        Card {
            VStack(alignment: .leading, spacing: 12) {
                HStack {
                    Label("The panel", systemImage: "person.3").font(.subheadline.weight(.semibold))
                    Text("\(chosen.count) of \(store.personas.count)")
                        .font(.caption.monospacedDigit()).foregroundStyle(.tertiary)
                    Spacer()
                    Button { goToPersonas() } label: { Label("Manage personas", systemImage: "slider.horizontal.3") }
                        .font(.caption)
                }
                Text("Pick who sits at the table for this debate.")
                    .font(.caption).foregroundStyle(.secondary)
                if store.personas.isEmpty {
                    Text("No personas yet — create some in the Personas section.")
                        .font(.callout).foregroundStyle(.tertiary).padding(.vertical, 6)
                }
                ForEach(store.personas) { persona in
                    let isOn = tab.selectedPersonaIds.contains(persona.id)
                    HStack(spacing: 12) {
                        Image(systemName: isOn ? "checkmark.circle.fill" : "circle")
                            .foregroundStyle(isOn ? Color.accentColor : .secondary)
                            .font(.title3)
                        ZStack {
                            Circle().fill(persona.color.opacity(0.18)).frame(width: 34, height: 34)
                            Image(systemName: persona.icon).foregroundStyle(persona.color)
                        }
                        VStack(alignment: .leading, spacing: 1) {
                            HStack(spacing: 6) {
                                Text(persona.name).font(.subheadline.weight(.semibold))
                                if persona.isSynth { Chip(text: "synthesizer", color: .accentColor, filled: true) }
                                if persona.isFactChecker { Chip(text: "fact-checker", color: .teal, filled: true) }
                            }
                            Text(persona.role).font(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                        HStack(spacing: 4) {
                            Image(systemName: persona.model.providerGlyph)
                                .font(.system(size: 9)).foregroundStyle(persona.model.providerColor)
                            Text(persona.model.label).font(.caption2).foregroundStyle(.secondary)
                        }
                    }
                    .padding(.vertical, 3)
                    .contentShape(Rectangle())
                    .onTapGesture { toggle(persona.id) }
                    .opacity(isOn ? 1 : 0.55)
                }
            }
        }
    }

    private func toggle(_ id: String) {
        if tab.selectedPersonaIds.contains(id) {
            tab.selectedPersonaIds.remove(id)
        } else {
            tab.selectedPersonaIds.insert(id)
        }
    }

    private var roundsField: some View {
        Card {
            VStack(spacing: 12) {
                Stepper(value: $tab.rounds, in: 1...10) {
                    HStack {
                        Label("Opening rounds", systemImage: "arrow.clockwise")
                            .font(.subheadline.weight(.semibold))
                        Spacer()
                        Text("\(tab.rounds)").font(.body.monospacedDigit()).foregroundStyle(.secondary)
                    }
                }
                Divider()
                Toggle(isOn: $tab.scoreEnabled) {
                    HStack {
                        Label("Score the idea", systemImage: "chart.dots.scatter")
                            .font(.subheadline.weight(.semibold))
                        Spacer()
                        Text("radar on the canvas").font(.caption).foregroundStyle(.tertiary)
                    }
                }
                Divider()
                Toggle(isOn: $tab.convergeEnabled) {
                    HStack {
                        Label("Stop early on convergence", systemImage: "arrow.triangle.merge")
                            .font(.subheadline.weight(.semibold))
                        Spacer()
                        Text("save rounds when agents agree").font(.caption).foregroundStyle(.tertiary)
                    }
                }
                .disabled(tab.moderatedEnabled)
                Divider()
                Toggle(isOn: $tab.moderatedEnabled) {
                    HStack {
                        Label("Moderated debate", systemImage: "person.badge.shield.checkmark")
                            .font(.subheadline.weight(.semibold))
                        Spacer()
                        Text("a facilitator runs it & recruits experts").font(.caption).foregroundStyle(.tertiary)
                    }
                }
            }
        }
    }

    private var keyWarning: some View {
        let providers = [needsOpenAIKey ? "OpenAI" : nil, needsAnthropicKey ? "Anthropic" : nil]
            .compactMap { $0 }.joined(separator: " and ")
        return HStack(alignment: .top, spacing: 8) {
            Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.orange)
            VStack(alignment: .leading, spacing: 3) {
                Text("Add an \(providers) key to run those agents")
                    .font(.callout.weight(.medium))
                Text("Agents on a provider without a key are skipped with an error. Set keys in Settings, or reassign those agents in the Personas panel.")
                    .font(.caption).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                SettingsLink { Text("Open Settings…") }.font(.caption)
            }
            Spacer(minLength: 0)
        }
        .padding(12)
        .background(.orange.opacity(0.12), in: RoundedRectangle(cornerRadius: Theme.corner))
    }
}

// MARK: - Transcript

private struct TranscriptPanel: View {
    @Bindable var session: RoundtableSession

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 14) {
                    ForEach(session.turns) { turn in
                        TranscriptTurn(turn: turn, persona: session.persona(turn.personaId))
                            .id(turn.id)
                    }
                }
                .padding(16)
            }
            .background(.background.secondary.opacity(0.3))
            .onChange(of: session.turns.last?.text) {
                if let last = session.turns.last {
                    withAnimation(.easeOut(duration: 0.15)) { proxy.scrollTo(last.id, anchor: .bottom) }
                }
            }
        }
    }
}

private struct TranscriptTurn: View {
    let turn: AgentTurn
    let persona: Persona?

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Image(systemName: persona?.icon ?? "person")
                    .foregroundStyle(persona?.color ?? .secondary)
                Text(persona?.name ?? "Agent").font(.subheadline.weight(.semibold))
                Text(persona?.role ?? "").font(.caption).foregroundStyle(.secondary)
                Spacer()
                if let m = persona?.model, !m.label.isEmpty {
                    Text(m.label).font(.caption2).foregroundStyle(.tertiary)
                }
            }
            if turn.status == .queryingKB {
                Label("Searching the corpus…", systemImage: "books.vertical")
                    .font(.caption).foregroundStyle(.secondary)
            }
            if !turn.text.isEmpty {
                MarkdownText(markdown: turn.text)
            }
            if !turn.citations.isEmpty { citations }
        }
        .padding(14)
        .background(.background, in: RoundedRectangle(cornerRadius: Theme.cardCorner))
        .overlay(alignment: .leading) {
            RoundedRectangle(cornerRadius: 2)
                .fill(persona?.color ?? .secondary)
                .frame(width: 3)
                .padding(.vertical, 8)
        }
    }

    private var citations: some View {
        VStack(alignment: .leading, spacing: 5) {
            Text("Pulled from your knowledge bank")
                .font(.caption2.weight(.semibold)).foregroundStyle(.secondary)
            ForEach(turn.citations) { c in
                HStack(spacing: 8) {
                    Image(systemName: "doc.richtext").font(.caption2).foregroundStyle(.tint)
                    Text(c.title).font(.caption).lineLimit(1)
                    Chip(text: Theme.sectionLabel(c.sectionType),
                         color: Theme.sectionColor(c.sectionType), filled: true)
                    if let p = c.page { Text("p.\(p)").font(.caption2.monospacedDigit()).foregroundStyle(.tertiary) }
                    Spacer()
                }
            }
        }
        .padding(10)
        .background(.background.secondary, in: RoundedRectangle(cornerRadius: 8))
    }
}

// MARK: - Activity log (newest first)

/// A live console of engine activity — newest at the top — so it's always
/// visible that the app is working, and any failure is plainly readable.
private struct LogConsole: View {
    @Bindable var session: RoundtableSession

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 6) {
                Label("Activity", systemImage: "terminal")
                    .font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                Spacer()
                Text("\(session.log.count) events · newest first")
                    .font(.caption2.monospacedDigit()).foregroundStyle(.tertiary)
            }
            .padding(.horizontal, 12).padding(.vertical, 6)
            Divider()
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 2) {
                        if session.log.isEmpty {
                            Text("Waiting for engine activity…")
                                .font(.caption.monospaced()).foregroundStyle(.tertiary)
                                .padding(.vertical, 4)
                        }
                        ForEach(session.log.reversed()) { entry in
                            HStack(alignment: .firstTextBaseline, spacing: 8) {
                                Text(entry.time, format: .dateTime.hour().minute().second())
                                    .font(.caption2.monospaced()).foregroundStyle(.tertiary)
                                Image(systemName: entry.glyph)
                                    .font(.system(size: 9)).foregroundStyle(entry.color)
                                Text(entry.text)
                                    .font(.caption.monospaced())
                                    .foregroundStyle(entry.level == .info ? .secondary : .primary)
                                    .textSelection(.enabled)
                                Spacer(minLength: 0)
                            }
                            .id(entry.id)
                        }
                    }
                    .padding(.horizontal, 12).padding(.vertical, 6)
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                .onChange(of: session.log.count) {
                    if let newest = session.log.last {
                        withAnimation(.easeOut(duration: 0.12)) { proxy.scrollTo(newest.id, anchor: .top) }
                    }
                }
            }
        }
        .background(.background.secondary.opacity(0.5))
    }
}
