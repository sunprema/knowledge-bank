import SwiftUI

// Clean Read: a faithful, citation-free rewrite of the paper, rendered with the
// NATIVE `MarkdownText` renderer — no WKWebView. (WebKit's helper processes
// can't launch reliably in this app's launch context, and we don't need them:
// the PDF is the original, and the clean read is reflowable prose.) Two layouts:
// the clean read on its own, or side by side with the PDF.
@MainActor
struct ReaderView: View {
    let client: KBClient
    let paperId: String
    let title: String
    let hasPDF: Bool

    @Environment(ServerController.self) private var server

    @State private var readerMarkdown: String?       // Clean Read (cached or streaming)
    @State private var layout: Layout
    @State private var generating = false
    @State private var genError: String?
    @State private var genTask: Task<Void, Never>?
    @State private var selectedModelId = LLMModel.opus.id
    @State private var scrollTarget: Int?

    enum Layout: Hashable { case clean, split }

    init(client: KBClient, paperId: String, title: String, hasPDF: Bool) {
        self.client = client
        self.paperId = paperId
        self.title = title
        self.hasPDF = hasPDF
        // Default to side-by-side when there's a PDF to compare against.
        _layout = State(initialValue: hasPDF ? .split : .clean)
    }

    /// A heading-delimited slice of the clean read, used for the outline rail and
    /// for jump-to-section scrolling.
    struct Section: Identifiable {
        let id: Int
        let level: Int
        let title: String
        let text: String
    }

    var body: some View {
        Group {
            switch layout {
            case .clean:
                cleanContent(outline: true)
            case .split:
                HSplitView {
                    PDFPanel(client: client, paperId: paperId)
                        .frame(minWidth: 300, idealWidth: 460)
                    cleanContent(outline: false)
                        .frame(minWidth: 320)
                        .layoutPriority(1)
                }
            }
        }
        .safeAreaInset(edge: .top) { topBar }
        .task(id: paperId) { await load() }
        .onDisappear { genTask?.cancel() }
    }

    private var topBar: some View {
        HStack(spacing: 12) {
            Image(systemName: "sparkles").foregroundStyle(.secondary)
            if hasPDF {
                Picker("", selection: $layout) {
                    Text("Clean read").tag(Layout.clean)
                    Text("PDF + Clean read").tag(Layout.split)
                }
                .pickerStyle(.segmented).labelsHidden().fixedSize()
                .help("The clean read on its own, or side by side with the PDF")
            } else {
                Text("Clean read").font(.caption.weight(.medium)).foregroundStyle(.secondary)
            }
            if generating { ProgressView().controlSize(.small) }
            Spacer()
            if readerMarkdown != nil && !generating {
                Button { generate() } label: { Image(systemName: "arrow.clockwise") }
                    .help("Regenerate clean read")
            }
        }
        .buttonStyle(.borderless)
        .padding(.horizontal, 12).padding(.vertical, 6)
        .background(.bar)
        .overlay(alignment: .bottom) { Divider() }
    }

    // MARK: Clean-read content

    @ViewBuilder private func cleanContent(outline: Bool) -> some View {
        if let md = readerMarkdown, !md.isEmpty {
            if outline && !generating {
                sectionedReader(md)
            } else {
                // Streaming, or the narrow split column: a plain scroll (no rail).
                markdownScroll(md)
            }
        } else if generating {
            generatingPlaceholder
        } else {
            cleanReadStart
        }
    }

    private func markdownScroll(_ md: String) -> some View {
        ScrollView {
            MarkdownText(markdown: md)
                .frame(maxWidth: 760, alignment: .leading)
                .frame(maxWidth: .infinity, alignment: .center)
                .padding(.horizontal, 28).padding(.vertical, 24)
                .textSelection(.enabled)
        }
    }

    private func sectionedReader(_ md: String) -> some View {
        let sections = Self.splitSections(md)
        return HSplitView {
            outlineRail(sections)
                .frame(minWidth: 180, idealWidth: 220, maxWidth: 300)
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 6) {
                        ForEach(sections) { s in
                            MarkdownText(markdown: s.text)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .id(s.id)
                        }
                    }
                    .frame(maxWidth: 760, alignment: .leading)
                    .frame(maxWidth: .infinity, alignment: .center)
                    .padding(.horizontal, 28).padding(.vertical, 24)
                    .textSelection(.enabled)
                }
                .onChange(of: scrollTarget) { _, t in
                    if let t { withAnimation(.snappy) { proxy.scrollTo(t, anchor: .top) } }
                }
            }
            .layoutPriority(1)
        }
    }

    private func outlineRail(_ sections: [Section]) -> some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 1) {
                let headed = sections.filter { !$0.title.isEmpty }
                if headed.isEmpty {
                    Text("No sections").font(.caption).foregroundStyle(.tertiary).padding()
                }
                ForEach(headed) { s in
                    Button { scrollTarget = s.id } label: {
                        Text(s.title)
                            .font(.system(size: s.level <= 1 ? 13 : 12,
                                          weight: s.level <= 1 ? .semibold : .regular))
                            .foregroundStyle(s.level <= 1 ? .primary : .secondary)
                            .lineLimit(2)
                            .multilineTextAlignment(.leading)
                            .padding(.leading, CGFloat(max(0, s.level - 1)) * 12)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .padding(.vertical, 3).padding(.horizontal, 8)
                }
            }
            .padding(8)
        }
        .background(.background.secondary)
    }

    /// Affordance shown when no clean read exists yet.
    private var cleanReadStart: some View {
        VStack(spacing: 16) {
            Image(systemName: "sparkles")
                .font(.system(size: 40)).foregroundStyle(.tint)
            Text("Clean read")
                .font(.title2.weight(.semibold))
            Text("A faithful rewrite of this paper with inline citations and cross-reference clutter removed — the argument, kept readable.")
                .font(.callout).foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 420)
            HStack(spacing: 10) {
                Picker("Model", selection: $selectedModelId) {
                    ForEach(LLMModel.all) { m in Text(m.label).tag(m.id) }
                }
                .labelsHidden().fixedSize()
                Button { generate() } label: {
                    Label("Generate", systemImage: "wand.and.stars")
                }
                .buttonStyle(.borderedProminent)
            }
            if let genError {
                Text(genError)
                    .font(.caption).foregroundStyle(.red)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 420)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding()
    }

    private var generatingPlaceholder: some View {
        VStack(spacing: 12) {
            ProgressView()
            Text("Generating clean read…")
                .font(.callout).foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    // MARK: Data

    private func load() async {
        genTask?.cancel()
        readerMarkdown = nil; generating = false; genError = nil
        // Load a cached clean read straight from disk (engine writes reader.md).
        let url = server.kbRoot.appendingPathComponent(paperId).appendingPathComponent("reader.md")
        readerMarkdown = try? String(contentsOf: url, encoding: .utf8)
    }

    /// Stream a fresh clean read from the engine, rendering progressively and
    /// caching the result (the engine writes reader.md on success). Deltas are
    /// throttled into the view so the native renderer isn't re-parsing the whole
    /// document on every token.
    private func generate() {
        genTask?.cancel()
        genError = nil
        generating = true
        readerMarkdown = ""   // empty ⇒ shows the placeholder until tokens flow
        let model = selectedModelId
        genTask = Task {
            var acc = ""
            var lastFlush = Date.distantPast
            do {
                for try await ev in client.readerStream(paperId, model: model) {
                    switch ev {
                    case .generating: break
                    case .delta(let t):
                        acc += t
                        if Date().timeIntervalSince(lastFlush) > 0.2 {
                            readerMarkdown = acc
                            lastFlush = Date()
                        }
                    case .done(let full):
                        acc = full
                    }
                }
                readerMarkdown = acc
            } catch is CancellationError {
                // paper switched or view gone — drop silently
            } catch {
                genError = error.localizedDescription
                if acc.isEmpty { readerMarkdown = nil } else { readerMarkdown = acc }
            }
            generating = false
        }
    }

    /// Split markdown into heading-delimited sections. Content before the first
    /// heading becomes section 0 with an empty title (no outline entry). Section
    /// ids are contiguous so the outline rail can scroll to them.
    static func splitSections(_ md: String) -> [Section] {
        var sections: [Section] = []
        var buf = ""
        var level = 0
        var title = ""
        var nextId = 0
        var inFence = false

        func flush() {
            if !buf.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                sections.append(Section(id: nextId, level: level, title: title, text: buf))
                nextId += 1
            }
            buf = ""
        }

        for raw in md.split(separator: "\n", omittingEmptySubsequences: false) {
            let line = String(raw)
            if line.hasPrefix("```") { inFence.toggle() }
            if !inFence, let r = line.range(of: #"^#{1,6}\s+"#, options: .regularExpression) {
                flush()
                level = line.prefix { $0 == "#" }.count
                title = String(line[r.upperBound...])
                    .replacingOccurrences(of: "`", with: "")
                    .replacingOccurrences(of: "*", with: "")
                    .replacingOccurrences(of: "_", with: "")
                    .trimmingCharacters(in: .whitespaces)
            }
            buf += line + "\n"
        }
        flush()
        return sections
    }
}
