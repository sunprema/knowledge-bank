import SwiftUI

// Chat over the corpus. The engine wide-retrieves context and answers with the
// chat model, returning the answer plus numbered citations the user can open.
@MainActor
struct ChatView: View {
    let client: KBClient

    @State private var turns: [ChatTurn] = []
    @State private var draft = ""
    @State private var sending = false
    @FocusState private var composerFocused: Bool
    @Environment(SpeechController.self) private var speech
    @Environment(ServerController.self) private var server
    @Environment(PersonaStore.self) private var personaStore

    // The conversation currently in the transcript. Persisted (under this id)
    // after every exchange so it can be resumed from History later.
    @State private var conversationId = UUID()
    @State private var conversationStart = Date()
    @State private var showHistory = false

    // Browser-style tabs: the conversation is "home"; clicking a citation opens
    // that paper's abstract + PDF as a new tab — same reading flow as Library,
    // including split view.
    @State private var nav = PaperTabs()
    @State private var papers: [PaperMetadata] = []   // for the split-view chooser

    var body: some View {
        VStack(spacing: 0) {
            if !nav.tabs.isEmpty {
                PaperTabStrip(nav: nav, homeTitle: "Chat",
                              homeIcon: "bubble.left.and.bubble.right")
                Divider()
            }
            content
        }
        .task { await loadPapers() }
        .sheet(isPresented: $showHistory) {
            ChatHistoryView(currentId: conversationId,
                            onOpen: { resume($0) },
                            onClose: { showHistory = false })
        }
    }

    @ViewBuilder private var content: some View {
        if nav.isSplit, case .paper(let leftId) = nav.selection {
            HSplitView {
                // Left pane. Related clicks load into the right pane (cross-reading).
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
        } else {
            switch nav.selection {
            case .home:
                chatHome
                    .navigationTitle("Chat")
                    .toolbar { ToolbarItemGroup { chatControls } }
            case .paper(let id):
                // Same reader as Library; related-paper clicks open further tabs.
                PaperDetailView(client: client, paperId: id,
                                onOpenPaper: { pid, title in nav.open(pid, title: title) })
                    .id(id)   // fresh detail state per paper
            }
        }
    }

    @ViewBuilder private var rightPane: some View {
        if let rightId = nav.splitPaperId {
            // Related clicks here load into the left pane (cross-reading).
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

    @ViewBuilder private var chatHome: some View {
        if server.hasOpenAIKey {
            VStack(spacing: 0) {
                transcript
                Divider()
                composer
            }
        } else {
            ConnectOpenAIState(action: "chat over your corpus")
        }
    }

    private func loadPapers() async {
        guard papers.isEmpty else { return }
        papers = (try? await client.papers().sorted { $0.publishedAt > $1.publishedAt }) ?? []
    }

    // MARK: History

    @ViewBuilder private var chatControls: some View {
        Button { newChat() } label: { Image(systemName: "square.and.pencil") }
            .help("New chat")
            .disabled(turns.isEmpty && !sending)
        Button { showHistory = true } label: { Image(systemName: "clock.arrow.circlepath") }
            .help("Chat history")
    }

    /// Start a fresh conversation. The current one is already persisted.
    private func newChat() {
        guard !turns.isEmpty else { return }
        turns = []
        conversationId = UUID()
        conversationStart = Date()
    }

    /// Load a saved conversation into the transcript to continue it.
    private func resume(_ convo: StoredConversation) {
        turns = convo.turns
        conversationId = convo.id
        conversationStart = convo.createdAt
        nav.goHome()
        showHistory = false
    }

    private func persist() {
        ConversationStore.shared.save(id: conversationId, turns: turns, createdAt: conversationStart)
    }

    private var transcript: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 16) {
                    if turns.isEmpty {
                        EmptyStateView(icon: "bubble.left.and.bubble.right",
                                       title: "Ask your knowledge bank",
                                       message: "Questions are answered from your papers, with citations you can open to verify.")
                            .padding(.top, 60)
                    }
                    ForEach(turns) { turn in
                        TurnView(turn: turn,
                                 openSource: { s in withAnimation(.snappy) { nav.open(s.paperId, title: s.title) } })
                            .id(turn.id)
                    }
                    if sending { thinking }
                }
                .padding(16)
            }
            .onChange(of: turns.count) {
                if let last = turns.last { withAnimation { proxy.scrollTo(last.id, anchor: .bottom) } }
            }
        }
    }

    private var thinking: some View {
        HStack(spacing: 8) {
            ProgressView().controlSize(.small)
            Text("Searching the corpus…").font(.callout).foregroundStyle(.secondary)
        }
        .padding(.vertical, 4)
    }

    private var composer: some View {
        VStack(spacing: 0) {
            if let mention = activeMention, !mentionMatches(mention.query).isEmpty {
                mentionMenu(mention)
                Divider()
            }
            HStack(spacing: 10) {
                TextField(addressedPersona == nil
                          ? "Ask anything — type @ to address a persona…"
                          : "Asking \(addressedPersona!.name)…", text: $draft, axis: .vertical)
                    .textFieldStyle(.plain)
                    .lineLimit(1...5)
                    .focused($composerFocused)
                    .padding(10)
                    .background(.background.secondary, in: RoundedRectangle(cornerRadius: Theme.corner))
                    .onSubmit { send() }
                Button { send() } label: {
                    Image(systemName: "arrow.up.circle.fill").font(.title)
                }
                .buttonStyle(.borderless)
                .disabled(sending || draft.trimmingCharacters(in: .whitespaces).isEmpty)
            }
            .padding(12)
        }
    }

    // MARK: @persona mention autocomplete (mirrors the Roundtable composer)

    /// The persona currently addressed by an `@mention` in the draft, if any.
    private var addressedPersona: Persona? {
        let ids = RoundtableSession.parseMentions(draft, personas: personaStore.personas)
        return ids.first.flatMap { id in personaStore.personas.first { $0.id == id } }
    }

    /// The in-progress `@mention` at the end of the draft (range + typed fragment).
    private var activeMention: (range: Range<String.Index>, query: String)? {
        guard let at = draft.range(of: "@", options: .backwards) else { return nil }
        let after = draft[at.upperBound...]
        if after.contains(where: { $0 == " " || $0 == "\n" || $0 == "\t" }) { return nil }
        return (at.lowerBound..<draft.endIndex, String(after))
    }

    private func mentionMatches(_ query: String) -> [Persona] {
        let q = query.lowercased()
        return personaStore.personas.filter { q.isEmpty || $0.name.lowercased().hasPrefix(q) }
    }

    private func mentionMenu(_ mention: (range: Range<String.Index>, query: String)) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(mentionMatches(mention.query)) { p in
                Button { insertMention(p, replacing: mention.range) } label: {
                    HStack(spacing: 8) {
                        Image(systemName: p.icon).foregroundStyle(p.color).frame(width: 18)
                        Text(p.name).font(.callout.weight(.medium))
                        Text(p.role).font(.caption).foregroundStyle(.secondary)
                        Spacer()
                        Image(systemName: p.model.providerGlyph)
                            .font(.system(size: 9)).foregroundStyle(p.model.providerColor)
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

    private func insertMention(_ p: Persona, replacing range: Range<String.Index>) {
        draft.replaceSubrange(range, with: "@\(p.name) ")
        composerFocused = true
    }

    private func send() {
        let q = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !q.isEmpty, !sending else { return }
        let persona = addressedPersona
        draft = ""
        turns.append(ChatTurn(role: .user, text: q))
        persist()
        sending = true
        Task {
            let history = turns.dropLast().flatMap { t -> [ChatMessage] in
                [ChatMessage(role: t.role == .user ? "user" : "assistant", content: t.text)]
            }
            do {
                let resp = try await client.chat(q, history: Array(history), persona: persona)
                var turn = ChatTurn(role: .assistant, text: resp.answer, sources: resp.sources)
                if let p = persona {
                    turn.personaName = p.name
                    turn.personaIcon = p.icon
                    turn.personaColorName = p.colorName
                }
                turns.append(turn)
            } catch {
                turns.append(ChatTurn(role: .assistant, text: "⚠︎ \(error.localizedDescription)"))
            }
            persist()
            sending = false
        }
    }
}

private struct TurnView: View {
    let turn: ChatTurn
    let openSource: (ChatSource) -> Void
    @Environment(SpeechController.self) private var speech

    private var personaColor: Color { PersonaPalette.color(turn.personaColorName ?? "accent") }

    var body: some View {
        if turn.role == .user {
            HStack {
                Spacer(minLength: 60)
                Text(turn.text)
                    .padding(.horizontal, 14).padding(.vertical, 10)
                    .background(Color.accentColor, in: RoundedRectangle(cornerRadius: 16))
                    .foregroundStyle(.white)
                    .textSelection(.enabled)
            }
        } else {
            VStack(alignment: .leading, spacing: 10) {
                if let name = turn.personaName {
                    HStack(spacing: 6) {
                        Image(systemName: turn.personaIcon ?? "person.fill").foregroundStyle(personaColor)
                        Text(name).font(.caption.weight(.semibold)).foregroundStyle(personaColor)
                    }
                }
                HStack(alignment: .top, spacing: 10) {
                    Image(systemName: turn.personaName == nil ? "sparkles" : (turn.personaIcon ?? "person.fill"))
                        .foregroundStyle(turn.personaName == nil ? AnyShapeStyle(.tint) : AnyShapeStyle(personaColor))
                        .font(.title3)
                    Text(turn.text)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                if !turn.sources.isEmpty { sources }
                HStack {
                    ReadAloudButton(text: turn.text, title: "Answer", compact: false)
                        .buttonStyle(.borderless).controlSize(.small).font(.caption)
                }
            }
            .padding(14)
            .background(.background.secondary, in: RoundedRectangle(cornerRadius: 16))
        }
    }

    private var sources: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Sources").font(.caption.weight(.semibold)).foregroundStyle(.secondary)
            ForEach(turn.sources) { s in
                Button { openSource(s) } label: {
                    HStack(spacing: 8) {
                        Text("\(s.n)")
                            .font(.caption2.bold().monospacedDigit())
                            .frame(width: 18, height: 18)
                            .background(Circle().fill(Theme.sectionColor(s.sectionType).opacity(0.2)))
                            .foregroundStyle(Theme.sectionColor(s.sectionType))
                        Text(s.title).font(.caption).lineLimit(1)
                        Chip(text: Theme.sectionLabel(s.sectionType), color: Theme.sectionColor(s.sectionType), filled: true)
                        if let p = s.page { Text("p.\(p)").font(.caption2.monospacedDigit()).foregroundStyle(.tertiary) }
                        Spacer()
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .disabled(!s.hasPdf)
            }
        }
        .padding(.leading, 30)
    }
}
