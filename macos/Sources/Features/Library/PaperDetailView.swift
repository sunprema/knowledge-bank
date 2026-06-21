import SwiftUI

@MainActor
struct PaperDetailView: View {
    let client: KBClient
    let paperId: String
    /// Open a related paper (a new tab in single mode; the opposite pane in split).
    var onOpenPaper: (String, String) -> Void = { _, _ in }
    /// When true, render controls in an inline header instead of the window
    /// toolbar — used for split panes so two instances don't fight over the
    /// window toolbar.
    var inlineChrome = false
    /// When set (split panes), show a close-split button in the inline header.
    var onClosePane: (() -> Void)? = nil

    @Environment(ServerController.self) private var server

    @State private var detail: PaperDetail?
    /// The document's body (`sections.md`), read from disk. Shown for non-PDF
    /// docs (web pages, ideas, reflections) whose content has no PDF to render.
    @State private var bodyMarkdown: String?
    @State private var related: [SimilarPaper] = []
    @State private var links: [LinkConn] = []
    @State private var sparksFor: [Spark] = []
    @State private var connTab: ConnTab = .similar
    @State private var loading = true
    @State private var showPDF: Bool
    @State private var readerMode = false
    @State private var toast: String?
    @State private var explain: ExplainRequest?
    @State private var showNotes = false

    enum ConnTab: Hashable { case similar, linked, sparks }
    struct LinkConn: Identifiable { let id: String; let title: String }
    struct ExplainRequest: Identifiable { let id = UUID(); let text: String }

    init(client: KBClient, paperId: String,
         onOpenPaper: @escaping (String, String) -> Void = { _, _ in },
         inlineChrome: Bool = false,
         onClosePane: (() -> Void)? = nil) {
        self.client = client
        self.paperId = paperId
        self.onOpenPaper = onOpenPaper
        self.inlineChrome = inlineChrome
        self.onClosePane = onClosePane
        // Single view shows the PDF beside the paper by default; split panes
        // start as text (toggle per pane) to avoid a cramped 4-column layout.
        _showPDF = State(initialValue: !inlineChrome)
    }

    var body: some View {
        VStack(spacing: 0) {
            if inlineChrome, let detail { inlineHeader(detail) }
            mainContent
        }
        .overlay(alignment: .top) {
            if let toast {
                Text(toast)
                    .font(.callout.weight(.medium))
                    .padding(.horizontal, 14).padding(.vertical, 8)
                    .background(.regularMaterial, in: Capsule())
                    .overlay(Capsule().stroke(.separator, lineWidth: 0.5))
                    .shadow(radius: 8, y: 2)
                    .padding(.top, 14)
                    .transition(.move(edge: .top).combined(with: .opacity))
            }
        }
        .animation(.snappy, value: toast)
        .sheet(item: $explain) { req in
            ExplainView(client: client, paperId: paperId, passage: req.text,
                        onSaved: { Task { if let d = try? await client.paper(paperId) { detail = d } } })
        }
        .sheet(isPresented: $showNotes) {
            if let current = detail {
                NotesSheet(client: client, paperId: paperId, title: current.metadata.title,
                           initialNotes: strippedNotes(current.notes),
                           onSaved: { Task { if let d = try? await client.paper(paperId) { detail = d } } })
            }
        }
        .navigationTitle(inlineChrome ? "" : (detail?.metadata.title ?? "Paper"))
        .toolbar {
            ToolbarItemGroup {
                if !inlineChrome, let detail { actions(detail) }
            }
        }
        .task(id: paperId) { await load() }
    }

    @ViewBuilder private var mainContent: some View {
        if let detail {
            if readerMode {
                ReaderView(client: client, paperId: paperId, title: detail.metadata.title,
                           hasPDF: detail.pdfPath != nil)
            } else if showPDF && detail.pdfPath != nil {
                // Adjustable-width split: paper on the left, PDF on the right.
                HSplitView {
                    detailScroll(detail)
                        .frame(minWidth: 320, idealWidth: 460)
                    PDFPanel(client: client, paperId: paperId,
                             onAddNote: { text, page in addSelectionAsNote(text, page) },
                             onExplain: { text in explain = ExplainRequest(text: text) })
                        .frame(minWidth: 300, idealWidth: 520)
                        .layoutPriority(1)
                }
            } else {
                detailScroll(detail)
            }
        } else if loading {
            ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    private func inlineHeader(_ detail: PaperDetail) -> some View {
        HStack(spacing: 8) {
            Text(detail.metadata.title).font(.headline).lineLimit(1)
            Spacer(minLength: 8)
            actions(detail)
            if let onClosePane {
                Button(action: onClosePane) {
                    Image(systemName: "xmark.circle.fill").font(.title3)
                }
                .buttonStyle(.borderless)
                .foregroundStyle(.secondary)
                .help("Close split view")
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 8)
        .background(.bar)
        .overlay(alignment: .bottom) { Divider() }
    }

    /// The action buttons (read aloud, toggle PDF) shared by the window toolbar
    /// and the inline split-pane header.
    @ViewBuilder private func actions(_ detail: PaperDetail) -> some View {
        ReadAloudButton(text: readableSummary(detail), title: detail.metadata.title)
        Button {
            withAnimation(.snappy) { readerMode.toggle() }
        } label: {
            Label(readerMode ? "Paper" : "Reader",
                  systemImage: readerMode ? "doc.richtext" : "text.alignleft")
        }
        .help(readerMode ? "Back to paper view" : "Reader mode — reflowable text with math")
        if !readerMode && detail.pdfPath != nil {
            Button {
                withAnimation(.snappy) { showPDF.toggle() }
            } label: {
                Label(showPDF ? "Hide PDF" : "Show PDF",
                      systemImage: showPDF ? "rectangle.righthalf.inset.filled"
                                           : "rectangle.righthalf.inset")
            }
            .help(showPDF ? "Hide the PDF panel" : "Show the PDF beside the paper")
        }
    }

    /// The paper's text content (header, abstract, notes, related) as a
    /// scrollable column — used standalone or as the left pane of the split.
    @ViewBuilder
    private func detailScroll(_ detail: PaperDetail) -> some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 22) {
                headerBlock(detail.metadata)
                // PDF docs show their content in the PDF panel; non-PDF docs
                // (web pages, ideas, reflections) render their markdown body here.
                if detail.pdfPath == nil, let body = bodyMarkdown, !body.isEmpty {
                    contentBlock(body)
                } else {
                    abstractBlock(detail.metadata)
                }
                notesCard(detail.notes)
                if !related.isEmpty || !links.isEmpty || !sparksFor.isEmpty { connectionsBlock }
            }
            .padding(24)
            .frame(maxWidth: 820, alignment: .leading)
            .frame(maxWidth: .infinity)
        }
    }

    // MARK: Blocks

    private func headerBlock(_ m: PaperMetadata) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(m.title).font(.system(.largeTitle, design: .serif).weight(.semibold))
            if !m.authors.isEmpty {
                Text(m.authors.joined(separator: ", ")).font(.title3).foregroundStyle(.secondary)
            }
            HStack(spacing: 6) {
                Chip(text: m.kind.capitalized, color: .accentColor, filled: true)
                if !m.publishedAt.isEmpty { Chip(text: Theme.year(m.publishedAt)) }
                ForEach(m.categories.prefix(5), id: \.self) { Chip(text: $0) }
                ForEach(m.tags, id: \.self) { Chip(text: $0, color: .accentColor, filled: true) }
            }
            // Web pages carry the URL they were ingested from — show it as a
            // clickable link back to the original source.
            if let source = m.sourceUrl, let url = URL(string: source) {
                Link(destination: url) {
                    HStack(spacing: 4) {
                        Image(systemName: "globe")
                        Text(source).lineLimit(1).truncationMode(.middle)
                    }
                    .font(.callout)
                }
                .help("Open the original page — \(source)")
            }
        }
    }

    private func abstractBlock(_ m: PaperMetadata) -> some View {
        SectionBox(title: "Abstract", systemImage: "text.alignleft",
                   accessory: AnyView(ReadAloudButton(text: m.abstract, title: m.title + " — abstract", compact: true).buttonStyle(.borderless))) {
            Text(m.abstract.isEmpty ? "No abstract." : m.abstract)
                .font(.system(.body, design: .serif))
                .lineSpacing(4)
                .textSelection(.enabled)
        }
    }

    /// The full markdown body for docs with no PDF (web pages, ideas,
    /// reflections). Rendered with the native markdown renderer.
    private func contentBlock(_ markdown: String) -> some View {
        SectionBox(title: "Content", systemImage: "doc.plaintext") {
            MarkdownText(markdown: markdown)
                .frame(maxWidth: .infinity, alignment: .leading)
                .textSelection(.enabled)
        }
    }

    /// Compact, clickable notes summary under the abstract. The full notes open
    /// as rich markdown in a popup.
    private func notesCard(_ notes: String) -> some View {
        let clean = strippedNotes(notes)
        let empty = clean.isEmpty
        let preview = clean.replacingOccurrences(of: "\n", with: " ")
            .components(separatedBy: .whitespaces).filter { !$0.isEmpty }.joined(separator: " ")
        return Button { showNotes = true } label: {
            Card {
                HStack(spacing: 12) {
                    Image(systemName: empty ? "square.and.pencil" : "note.text")
                        .font(.title3).foregroundStyle(.tint).frame(width: 24)
                    VStack(alignment: .leading, spacing: 3) {
                        HStack(spacing: 6) {
                            Text("My Notes").font(.headline)
                            if !empty {
                                Chip(text: "\(noteCount(clean)) " + (noteCount(clean) == 1 ? "entry" : "entries"))
                            }
                        }
                        Text(empty ? "Add a note — your notes become part of what KB searches." : preview)
                            .font(.callout).foregroundStyle(empty ? .tertiary : .secondary).lineLimit(2)
                            .multilineTextAlignment(.leading)
                    }
                    Spacer(minLength: 0)
                    Image(systemName: empty ? "plus.circle" : "arrow.up.left.and.arrow.down.right")
                        .font(.caption).foregroundStyle(.tertiary)
                }
            }
        }
        .buttonStyle(.plain)
        .help(empty ? "Add notes" : "Edit notes")
    }

    /// Rough entry count: timestamped HTTP-added blocks plus any leading prose.
    private func noteCount(_ clean: String) -> Int {
        let stamped = clean.components(separatedBy: "---").filter {
            !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        }.count
        return max(stamped, 1)
    }

    // Unified Connections: similarity neighbors, explicit graph links, and
    // sparks (surprising cross-document connections) involving this paper.
    private var connectionsBlock: some View {
        SectionBox(title: "Connections", systemImage: "point.3.connected.trianglepath.dotted") {
            VStack(alignment: .leading, spacing: 12) {
                Picker("", selection: $connTab) {
                    Text("Similar (\(related.count))").tag(ConnTab.similar)
                    Text("Linked (\(links.count))").tag(ConnTab.linked)
                    Text("Sparks (\(sparksFor.count))").tag(ConnTab.sparks)
                }
                .pickerStyle(.segmented)
                .labelsHidden()

                switch connTab {
                case .similar: connList(related, empty: "No similar documents.") { similarRow($0) }
                case .linked:  connList(links, empty: "No explicit links to this paper yet.") { linkRow($0) }
                case .sparks:  connList(sparksFor, empty: "No sparks involve this paper yet.") { sparkRow($0) }
                }
            }
        }
    }

    @ViewBuilder
    private func connList<T: Identifiable, Row: View>(_ items: [T], empty: String,
                                                      @ViewBuilder row: @escaping (T) -> Row) -> some View {
        if items.isEmpty {
            Text(empty).font(.caption).foregroundStyle(.tertiary).padding(.vertical, 6)
        } else {
            VStack(spacing: 8) {
                ForEach(items) { item in
                    row(item)
                    if item.id != items.last?.id { Divider() }
                }
            }
        }
    }

    private func similarRow(_ sim: SimilarPaper) -> some View {
        connectionRow(id: sim.paperId, title: sim.title,
                      subtitle: sim.authors.prefix(3).joined(separator: ", "),
                      leading: AnyView(ScoreBadge(score: sim.score)))
    }

    private func linkRow(_ link: LinkConn) -> some View {
        connectionRow(id: link.id, title: link.title, subtitle: "explicit link",
                      leading: AnyView(Image(systemName: "link").foregroundStyle(.tint)))
    }

    private func sparkRow(_ spark: Spark) -> some View {
        let other = spark.src.paperId == paperId ? spark.dst : spark.src
        let mine = spark.src.paperId == paperId ? spark.src : spark.dst
        return connectionRow(
            id: other.paperId, title: other.title,
            subtitle: "\(Theme.sectionLabel(mine.sectionType)) ↔ \(Theme.sectionLabel(other.sectionType))",
            leading: AnyView(ScoreBadge(score: spark.surprise)))
    }

    private func connectionRow(id: String, title: String, subtitle: String, leading: AnyView) -> some View {
        Button { onOpenPaper(id, title) } label: {
            HStack(spacing: 10) {
                leading
                VStack(alignment: .leading, spacing: 2) {
                    Text(title).font(.subheadline.weight(.medium)).lineLimit(1)
                    Text(subtitle).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                }
                Spacer()
                Image(systemName: "arrow.up.forward.square").font(.caption).foregroundStyle(.tertiary)
            }
            .padding(.vertical, 4)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help("Open in a new tab")
    }

    // MARK: Data + helpers

    private func load() async {
        loading = true
        async let d = try? await client.paper(paperId)
        async let r = try? await client.similar(paperId, limit: 8)
        async let g = try? await client.graph(neighbors: 0)   // explicit links only
        async let s = try? await client.sparks(limit: 200)
        detail = await d
        // The body lives in `<kbRoot>/<id>/sections.md` (same disk-read pattern
        // as ReaderView's reader.md). Shown when there's no PDF to render.
        let sections = server.kbRoot.appendingPathComponent(paperId).appendingPathComponent("sections.md")
        bodyMarkdown = try? String(contentsOf: sections, encoding: .utf8)
        related = (await r)?.papers ?? []
        links = linkNeighbors(await g)
        sparksFor = (await s)?.sparks.filter { $0.src.paperId == paperId || $0.dst.paperId == paperId } ?? []
        loading = false
    }

    /// Explicit `link`-kind edges touching this paper, mapped to the other end.
    private func linkNeighbors(_ graph: GraphResponse?) -> [LinkConn] {
        guard let graph else { return [] }
        let titleById = Dictionary(graph.nodes.map { ($0.id, $0.title) }, uniquingKeysWith: { a, _ in a })
        var seen = Set<String>()
        var out: [LinkConn] = []
        for e in graph.edges where e.kind == "link" {
            let other = e.source == paperId ? e.target : (e.target == paperId ? e.source : nil)
            guard let other, !seen.contains(other) else { continue }
            seen.insert(other)
            out.append(LinkConn(id: other, title: titleById[other] ?? other))
        }
        return out
    }

    /// The annotation → notes loop: a highlighted PDF passage becomes a cited
    /// note, which the engine re-embeds so it's searchable across the corpus.
    private func addSelectionAsNote(_ text: String, _ page: Int?) {
        let clean = text
            .replacingOccurrences(of: "\n", with: " ")
            .components(separatedBy: .whitespaces)
            .filter { !$0.isEmpty }
            .joined(separator: " ")
        guard !clean.isEmpty else { return }
        var note = "> \(clean)"
        if let page { note += "\n\n— p. \(page)" }
        Task {
            do {
                _ = try await client.addNote(paperId, note: note)
                if let refreshed = try? await client.paper(paperId) { detail = refreshed }
                await flashToast("Added to notes")
            } catch {
                await flashToast("Couldn't add note")
            }
        }
    }

    private func flashToast(_ message: String) async {
        toast = message
        try? await Task.sleep(for: .seconds(2))
        toast = nil
    }

    private func hasNotes(_ notes: String) -> Bool {
        !strippedNotes(notes).isEmpty
    }

    /// Strip the HTML-comment prompts and the heading the engine seeds notes.md
    /// with, so an untouched template reads as empty.
    private func strippedNotes(_ notes: String) -> String {
        var text = notes
        while let open = text.range(of: "<!--"), let close = text.range(of: "-->", range: open.upperBound..<text.endIndex) {
            text.removeSubrange(open.lowerBound..<close.upperBound)
        }
        return text
            .split(separator: "\n", omittingEmptySubsequences: false)
            .filter { !$0.hasPrefix("# Notes on") }
            .joined(separator: "\n")
            .trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private func readableSummary(_ d: PaperDetail) -> String {
        let m = d.metadata
        var parts = [m.title]
        if !m.authors.isEmpty { parts.append("by " + m.authors.prefix(3).joined(separator: ", ")) }
        if !m.abstract.isEmpty { parts.append(m.abstract) }
        return parts.joined(separator: ". ")
    }
}

/// A titled content box used throughout the detail view.
struct SectionBox<Content: View>: View {
    let title: String
    let systemImage: String
    var accessory: AnyView? = nil
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Label(title, systemImage: systemImage)
                    .font(.headline)
                    .foregroundStyle(.secondary)
                Spacer()
                if let accessory { accessory }
            }
            content
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}
