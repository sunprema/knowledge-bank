import SwiftUI

// Explore — a node/canvas chat. Start a topic, attach one or more agents to
// answer it (each spawns its own branch), then continue from any node or join
// several branches and keep going. Git-esque: the conversation is a tree/DAG,
// and a node's context is its ancestor path (a join merges branches).
//
// Like Chat/Library, the canvas is "home" in a browser-style tab strip: clicking
// a citation opens that paper as a reader tab (split-view capable), and every
// exploration is storable and reopenable from history.
@MainActor
struct ExploreView: View {
    let client: KBClient

    @Environment(ServerController.self) private var server
    @Environment(PersonaStore.self) private var personaStore
    @State private var store = ExploreStore()

    @State private var draft = ""
    @State private var attached: [Persona] = []
    @State private var streams: [String: Task<Void, Never>] = [:]
    @FocusState private var composerFocused: Bool

    // Browser-style tabs: the canvas is "home"; a clicked citation opens that
    // paper's reader as a tab, same flow as Chat/Library (including split view).
    @State private var nav = PaperTabs()
    @State private var papers: [PaperMetadata] = []
    @State private var showHistory = false
    @State private var loadedInitial = false

    var body: some View {
        VStack(spacing: 0) {
            if !nav.tabs.isEmpty {
                PaperTabStrip(nav: nav, homeTitle: "Explore",
                              homeIcon: "point.3.filled.connected.trianglepath.dotted")
                Divider()
            }
            content
        }
        .task {
            await loadPapers()
            resumeLatestIfFresh()
        }
        .sheet(isPresented: $showHistory) {
            ExploreHistoryView(currentId: store.id,
                               onOpen: { resume($0) },
                               onClose: { showHistory = false })
        }
        .onDisappear { for t in streams.values { t.cancel() } }
    }

    // MARK: Tabs / split routing (mirrors ChatView)

    @ViewBuilder private var content: some View {
        if nav.isSplit {
            switch nav.selection {
            case .home:
                // Citation opened alongside the canvas: canvas left, reader right.
                HSplitView {
                    exploreHome
                        .frame(minWidth: 440)
                        .layoutPriority(1)
                    rightPane
                        .frame(minWidth: 360)
                }
                .navigationTitle(store.nodes.isEmpty ? "Explore" : store.title)
                .toolbar { ToolbarItemGroup { controls } }
            case .paper(let leftId):
                HSplitView {
                    PaperDetailView(client: client, paperId: leftId,
                                    onOpenPaper: { pid, title in nav.setRight(pid, title: title) },
                                    inlineChrome: true,
                                    onClosePane: { withAnimation(.snappy) { nav.closeSplit() } })
                        .id("L-\(leftId)")
                        .frame(minWidth: 380)
                    rightPane
                        .frame(minWidth: 380)
                        .layoutPriority(1)
                }
            }
        } else {
            switch nav.selection {
            case .home:
                exploreHome
                    .navigationTitle(store.nodes.isEmpty ? "Explore" : store.title)
                    .toolbar { ToolbarItemGroup { controls } }
            case .paper(let id):
                PaperDetailView(client: client, paperId: id,
                                onOpenPaper: { pid, title in nav.open(pid, title: title) })
                    .id(id)
            }
        }
    }

    @ViewBuilder private var rightPane: some View {
        if let rightId = nav.splitPaperId {
            PaperDetailView(client: client, paperId: rightId,
                            onOpenPaper: { pid, title in nav.setLeft(pid, title: title) },
                            inlineChrome: true,
                            onClosePane: { withAnimation(.snappy) { nav.closeSplit() } })
                .id("R-\(rightId)")
        } else {
            SplitChooser(papers: papers,
                         onPick: { pid, title in withAnimation(.snappy) { nav.setRight(pid, title: title) } },
                         onCancel: { withAnimation(.snappy) { nav.closeSplit() } })
        }
    }

    @ViewBuilder private var exploreHome: some View {
        if !server.hasOpenAIKey {
            ConnectOpenAIState(action: "explore your corpus on a canvas")
        } else if store.nodes.isEmpty {
            startState
        } else {
            board
        }
    }

    @ViewBuilder private var controls: some View {
        Button { centerOnFirst() } label: { Image(systemName: "scope") }
            .help("Recenter")
            .disabled(store.nodes.isEmpty)
        Button { newCanvas() } label: { Image(systemName: "square.and.pencil") }
            .help("New exploration")
            .disabled(store.nodes.isEmpty)
        Button { showHistory = true } label: { Image(systemName: "clock.arrow.circlepath") }
            .help("Exploration history")
    }

    // MARK: Empty state

    private var startState: some View {
        VStack(spacing: 22) {
            VStack(spacing: 8) {
                Image(systemName: "point.3.filled.connected.trianglepath.dotted")
                    .font(.system(size: 44, weight: .light)).foregroundStyle(.tint)
                Text("Start an exploration").font(.title2.weight(.semibold))
                Text("Pose a topic. Attach agents to answer it — each opens its own branch you can carry forward or merge.")
                    .font(.callout).foregroundStyle(.secondary)
                    .multilineTextAlignment(.center).frame(maxWidth: 460)
            }
            composer
                .frame(maxWidth: 620)
                .background(.background.secondary, in: RoundedRectangle(cornerRadius: Theme.cardCorner))
                .overlay(RoundedRectangle(cornerRadius: Theme.cardCorner).stroke(.separator, lineWidth: 0.5))
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(40)
    }

    // MARK: Canvas + docked composer

    private var board: some View {
        ExploreCanvas(store: store,
                      onOpenSource: { s in withAnimation(.snappy) { nav.setRight(s.paperId, title: s.title) } },
                      onBranch: { parentId, side, label in branch(from: parentId, side: side, label: label) })
            .overlay(alignment: .bottom) {
                composer
                    .frame(maxWidth: 720)
                    .background(.regularMaterial, in: RoundedRectangle(cornerRadius: Theme.cardCorner))
                    .overlay(RoundedRectangle(cornerRadius: Theme.cardCorner).stroke(.separator, lineWidth: 0.5))
                    .shadow(color: .black.opacity(0.18), radius: 16, y: 6)
                    .padding(.bottom, 16)
                    .padding(.horizontal, 16)
            }
    }

    // MARK: Composer

    private var composer: some View {
        VStack(alignment: .leading, spacing: 8) {
            contextRow
            agentRow
            HStack(spacing: 10) {
                TextField(promptPlaceholder, text: $draft, axis: .vertical)
                    .textFieldStyle(.plain)
                    .lineLimit(1...4)
                    .focused($composerFocused)
                    .padding(10)
                    .background(.background, in: RoundedRectangle(cornerRadius: Theme.corner))
                    .onSubmit { send() }
                Button { send() } label: { Image(systemName: "arrow.up.circle.fill").font(.title) }
                    .buttonStyle(.borderless)
                    .disabled(draft.trimmingCharacters(in: .whitespaces).isEmpty)
            }
        }
        .padding(12)
    }

    /// What the next prompt will continue from, given the current selection.
    private var contextRow: some View {
        let sel = store.selection
        return HStack(spacing: 6) {
            Image(systemName: sel.count > 1 ? "arrow.triangle.merge"
                  : (sel.isEmpty ? "plus.circle" : "arrow.turn.down.right"))
                .font(.caption2).foregroundStyle(.secondary)
            Text(contextLabel(sel))
                .font(.caption).foregroundStyle(.secondary)
            Spacer(minLength: 0)
            if !sel.isEmpty {
                Button("Clear") { store.selection = [] }
                    .buttonStyle(.borderless).font(.caption2)
            }
        }
    }

    private func contextLabel(_ sel: Set<String>) -> String {
        switch sel.count {
        case 0: return "New topic — starts a fresh root"
        case 1:
            let t = store.node(sel.first!)?.text ?? ""
            return "Continuing from “\(t.prefix(42))\(t.count > 42 ? "…" : "")”"
        default: return "Joining \(sel.count) branches — their contexts merge"
        }
    }

    /// Attached agents (multi). Empty ⇒ the default research assistant answers.
    private var agentRow: some View {
        HStack(spacing: 6) {
            ForEach(attached) { p in
                Button { attached.removeAll { $0.id == p.id } } label: {
                    HStack(spacing: 4) {
                        Image(systemName: p.icon).font(.caption2)
                        Text(p.name).font(.caption2.weight(.medium))
                        Image(systemName: "xmark").font(.system(size: 8))
                    }
                    .padding(.horizontal, 8).padding(.vertical, 4)
                    .background(Capsule().fill(PersonaPalette.color(p.colorName).opacity(0.16)))
                    .foregroundStyle(PersonaPalette.color(p.colorName))
                }
                .buttonStyle(.plain)
                .help("Remove \(p.name)")
            }
            Menu {
                ForEach(availableAgents) { p in
                    Button { attached.append(p) } label: { Label(p.name, systemImage: p.icon) }
                }
                if availableAgents.isEmpty {
                    Text("All personas attached").foregroundStyle(.secondary)
                }
            } label: {
                Label(attached.isEmpty ? "Add agent" : "Add", systemImage: "plus")
                    .font(.caption2.weight(.medium))
            }
            .menuStyle(.borderlessButton)
            .fixedSize()
            if attached.isEmpty {
                Text("· default assistant").font(.caption2).foregroundStyle(.tertiary)
            }
            Spacer(minLength: 0)
        }
    }

    private var availableAgents: [Persona] {
        personaStore.personas.filter { p in !attached.contains { $0.id == p.id } }
    }

    private var promptPlaceholder: String {
        store.selection.isEmpty ? "Pose a topic to explore…" : "Continue the conversation…"
    }

    // MARK: Actions

    private func loadPapers() async {
        guard papers.isEmpty else { return }
        papers = (try? await client.papers().sorted { $0.publishedAt > $1.publishedAt }) ?? []
    }

    /// On first appear, reopen the most recent exploration if the canvas is fresh.
    private func resumeLatestIfFresh() {
        guard !loadedInitial else { return }
        loadedInitial = true
        if store.nodes.isEmpty, let latest = ExploreArchive.shared.all().first {
            store.load(latest)
        }
    }

    private func send() {
        let prompt = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !prompt.isEmpty else { return }
        let links = store.selection.map { ParentLink(id: $0, side: .bottom) }
        let promptId = store.addPrompt(prompt, links: links)
        let history = store.history(forPrompt: promptId)

        // One branch per attached agent; none ⇒ a single default-assistant branch.
        let agents: [Persona?] = attached.isEmpty ? [nil] : attached.map { $0 }
        var newAnswers: [String] = []
        for (i, persona) in agents.enumerated() {
            let answerId = store.addAnswer(prompt: promptId, persona: persona,
                                           index: i, count: agents.count)
            newAnswers.append(answerId)
            streams[answerId] = Task { await run(answerId, query: prompt, history: history, persona: persona) }
        }
        // Carry the new replies forward as the next continuation point.
        store.selection = Set(newAnswers)
        draft = ""
        composerFocused = true
    }

    /// Branch from a node out of a given side, naming the edge (the lens). The
    /// edge name becomes the prompt the model continues through; any agents
    /// attached in the composer answer it (else the default assistant).
    private func branch(from parentId: String, side: NodeSide, label: String) {
        let name = label.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else { return }
        let promptId = store.addPrompt(name, links: [ParentLink(id: parentId, label: name, side: side)])
        let history = store.history(forPrompt: promptId)
        let agents: [Persona?] = attached.isEmpty ? [nil] : attached.map { $0 }
        var newAnswers: [String] = []
        for (i, persona) in agents.enumerated() {
            let answerId = store.addAnswer(prompt: promptId, persona: persona, index: i, count: agents.count)
            newAnswers.append(answerId)
            streams[answerId] = Task { await run(answerId, query: name, history: history, persona: persona) }
        }
        store.selection = Set(newAnswers)
    }

    private func run(_ answerId: String, query: String, history: [ChatMessage], persona: Persona?) async {
        do {
            for try await ev in client.chatStream(query, history: history, persona: persona) {
                if Task.isCancelled { return }
                switch ev {
                case .searching:
                    break
                case .sources(let s):
                    store.setSources(s, on: answerId)
                case .delta(let t):
                    store.appendDelta(t, to: answerId)
                case .done(let answer):
                    store.finish(answerId, status: .done, text: answer.isEmpty ? nil : answer)
                }
            }
            if store.node(answerId)?.status == .streaming {
                store.finish(answerId, status: .done)
            }
        } catch {
            store.finish(answerId, status: .error, text: "⚠︎ \(error.localizedDescription)")
        }
        streams[answerId] = nil
    }

    /// Save the current board (auto-archived) and open a blank one.
    private func newCanvas() {
        for t in streams.values { t.cancel() }
        streams = [:]
        store.save()
        store.newBoard()
        nav.goHome()
    }

    /// Load a saved exploration into the canvas to continue it.
    private func resume(_ rec: StoredExplore) {
        for t in streams.values { t.cancel() }
        streams = [:]
        store.load(rec)
        nav.goHome()
        showHistory = false
    }

    private func centerOnFirst() {
        guard let first = store.nodes.min(by: { $0.createdAt < $1.createdAt }) else { return }
        store.offset = CGSize(width: 480 - first.x * store.zoom, height: 80)
        store.save()
    }
}
