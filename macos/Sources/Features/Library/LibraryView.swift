import SwiftUI
import AppKit

/// A request to open a specific paper in the Library, handed in from another
/// section (the Add view opens a just-ingested document this way).
struct LibraryOpen: Equatable {
    let id: String
    let title: String
}

// Browse the corpus. A filterable list of documents; opening one (from the list
// or from a paper's Related panel) adds a browser-style tab.
@MainActor
struct LibraryView: View {
    let client: KBClient
    /// A paper another section asked us to open (e.g. a just-added document from
    /// the Add view). Consumed on appear/change, then cleared.
    var openRequest: Binding<LibraryOpen?> = .constant(nil)

    @State private var papers: [PaperMetadata] = []
    @State private var filter = ""
    @State private var loading = true
    @State private var error: String?
    @State private var nav = PaperTabs()

    // Shelf state: cover grid vs. compact list, the cover-preview overlay, and
    // the hover lift. `coverNS` drives the cover→preview morph.
    @State private var layout: Layout = .grid
    @State private var preview: PaperMetadata?
    @State private var hoverID: String?
    @State private var copiedID: String?            // shows a brief ✓ on the copied card
    @Namespace private var coverNS

    enum Layout: Hashable { case grid, list }

    private let coverW: CGFloat = 168
    private let coverH: CGFloat = 224

    private var filtered: [PaperMetadata] {
        guard !filter.isEmpty else { return papers }
        let q = filter.lowercased()
        return papers.filter {
            $0.title.lowercased().contains(q)
            || $0.authors.joined(separator: " ").lowercased().contains(q)
            || $0.tags.contains { $0.lowercased().contains(q) }
        }
    }

    var body: some View {
        VStack(spacing: 0) {
            if !nav.tabs.isEmpty {
                PaperTabStrip(nav: nav)
                Divider()
            }
            content
        }
        .task { await load() }
        .onAppear { consumeOpenRequest() }
        .onChange(of: openRequest.wrappedValue) { _, _ in consumeOpenRequest() }
    }

    /// Open the requested paper as a tab, then clear the request so it doesn't
    /// re-fire. Works without the home list being loaded — the detail view
    /// fetches the paper by id.
    private func consumeOpenRequest() {
        guard let req = openRequest.wrappedValue else { return }
        nav.open(req.id, title: req.title)
        openRequest.wrappedValue = nil
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
                home
            case .paper(let id):
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

    @ViewBuilder private var home: some View {
        Group {
            if loading {
                ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if let error {
                EmptyStateView(icon: "exclamationmark.triangle", title: "Couldn't load papers", message: error)
            } else if papers.isEmpty {
                EmptyStateView(icon: "books.vertical",
                               title: "Your library is empty",
                               message: "Add papers from the terminal with `kb add <arxiv-id>` — they'll show up here.")
            } else {
                switch layout {
                case .grid: grid
                case .list: list
                }
            }
        }
        .navigationTitle("Library")
        .searchable(text: $filter, placement: .toolbar, prompt: "Filter by title, author, tag")
        .toolbar {
            if !papers.isEmpty {
                ToolbarItem(placement: .primaryAction) {
                    Picker("Layout", selection: $layout.animation(.snappy)) {
                        Image(systemName: "square.grid.2x2").tag(Layout.grid)
                        Image(systemName: "list.bullet").tag(Layout.list)
                    }
                    .pickerStyle(.segmented)
                    .help("Switch between shelf and list")
                }
            }
        }
        .overlay { if let p = preview { previewOverlay(p) } }
    }

    // The shelf: a grid of book-style covers. Tapping a cover morphs it into a
    // preview card (abstract + Read) rather than jumping straight into the tab.
    private var grid: some View {
        ScrollView {
            LazyVGrid(columns: [GridItem(.adaptive(minimum: coverW, maximum: coverW), spacing: 30)],
                      alignment: .leading, spacing: 30) {
                ForEach(filtered) { paper in coverCard(paper) }
            }
            .padding(28)
        }
    }

    private func coverCard(_ paper: PaperMetadata) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            ZStack {
                // While this card is expanded in the overlay, hold its slot with
                // a placeholder so the grid doesn't reflow under the preview.
                if preview?.id == paper.id {
                    Color.clear
                } else {
                    CoverImage(paper: paper, client: client)
                        .frame(width: coverW, height: coverH)
                        .clipShape(RoundedRectangle(cornerRadius: 10))
                        .overlay(RoundedRectangle(cornerRadius: 10).stroke(.black.opacity(0.12), lineWidth: 0.5))
                        .shadow(color: .black.opacity(0.28), radius: hoverID == paper.id ? 14 : 8,
                                x: 0, y: hoverID == paper.id ? 9 : 5)
                        .matchedGeometryEffect(id: paper.id, in: coverNS)
                        .scaleEffect(hoverID == paper.id ? 1.04 : 1, anchor: .bottom)
                        .onHover { inside in hoverID = inside ? paper.id : (hoverID == paper.id ? nil : hoverID) }
                        .onTapGesture {
                            withAnimation(.spring(response: 0.42, dampingFraction: 0.82)) { preview = paper }
                        }
                }
            }
            .frame(width: coverW, height: coverH)
            .animation(.snappy(duration: 0.22), value: hoverID)

            Text(paper.title)
                .font(.subheadline.weight(.medium))
                .lineLimit(2)
                .frame(width: coverW, alignment: .leading)
            if let first = paper.authors.first {
                Text(first + (paper.authors.count > 1 ? " et al." : ""))
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .frame(width: coverW, alignment: .leading)
            }
            idRow(paper)
        }
        .contentShape(Rectangle())
        .contextMenu {
            Button { copyID(paper.id) } label: { Label("Copy paper ID", systemImage: "doc.on.doc") }
        }
    }

    /// The paper's canonical id (the value every skill and `kb` command uses),
    /// shown small under each cover with a one-click copy button.
    private func idRow(_ paper: PaperMetadata) -> some View {
        HStack(spacing: 4) {
            Text(paper.id)
                .font(.system(.caption2, design: .monospaced))
                .foregroundStyle(.secondary)
                .lineLimit(1).truncationMode(.middle)
                .textSelection(.enabled)
            Button { copyID(paper.id) } label: {
                Image(systemName: copiedID == paper.id ? "checkmark" : "doc.on.doc")
                    .font(.caption2)
            }
            .buttonStyle(.plain)
            .foregroundStyle(copiedID == paper.id ? Color.green : Color.secondary)
            .help("Copy paper ID")
        }
        .frame(width: coverW, alignment: .leading)
    }

    /// Copy a paper id to the pasteboard and flash a ✓ briefly.
    private func copyID(_ id: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(id, forType: .string)
        copiedID = id
        Task {
            try? await Task.sleep(for: .seconds(1.2))
            if copiedID == id { copiedID = nil }
        }
    }

    // The expanded "book detail": the cover morphs out of the grid (matched
    // geometry) while the summary and Read action fade in beside it.
    private func previewOverlay(_ paper: PaperMetadata) -> some View {
        let dismiss = { withAnimation(.spring(response: 0.42, dampingFraction: 0.85)) { preview = nil } }
        return ZStack {
            Rectangle().fill(.black.opacity(0.4)).ignoresSafeArea()
                .transition(.opacity)
                .onTapGesture(perform: dismiss)

            HStack(alignment: .top, spacing: 28) {
                CoverImage(paper: paper, client: client)
                    .frame(width: 260, height: 347)
                    .clipShape(RoundedRectangle(cornerRadius: 14))
                    .overlay(RoundedRectangle(cornerRadius: 14).stroke(.black.opacity(0.15), lineWidth: 0.5))
                    .shadow(color: .black.opacity(0.4), radius: 24, y: 12)
                    .matchedGeometryEffect(id: paper.id, in: coverNS)

                VStack(alignment: .leading, spacing: 14) {
                    Text(paper.title)
                        .font(.system(.title, design: .serif).weight(.bold))
                        .lineLimit(4)
                    if !paper.authors.isEmpty {
                        Text(paper.authors.prefix(6).joined(separator: ", ")
                             + (paper.authors.count > 6 ? " et al." : ""))
                            .font(.title3).foregroundStyle(.secondary).lineLimit(2)
                    }
                    HStack(spacing: 6) {
                        Chip(text: paper.kind.capitalized, color: .accentColor, filled: true)
                        if !paper.publishedAt.isEmpty { Chip(text: Theme.year(paper.publishedAt)) }
                        ForEach(paper.categories.prefix(3), id: \.self) { Chip(text: $0) }
                        ForEach(paper.tags.prefix(3), id: \.self) { Chip(text: $0, color: .accentColor, filled: true) }
                    }
                    HStack(spacing: 8) {
                        Image(systemName: "number").font(.caption).foregroundStyle(.tertiary)
                        Text(paper.id)
                            .font(.system(.callout, design: .monospaced))
                            .foregroundStyle(.secondary)
                            .textSelection(.enabled)
                        Button { copyID(paper.id) } label: {
                            Label(copiedID == paper.id ? "Copied" : "Copy ID",
                                  systemImage: copiedID == paper.id ? "checkmark" : "doc.on.doc")
                        }
                        .buttonStyle(.bordered).controlSize(.small)
                        .tint(copiedID == paper.id ? .green : .accentColor)
                        .help("Copy the paper ID used by every skill and kb command")
                    }
                    if !paper.abstract.isEmpty {
                        ScrollView {
                            Text(paper.abstract)
                                .font(.system(.body, design: .serif))
                                .lineSpacing(4)
                                .textSelection(.enabled)
                                .frame(maxWidth: .infinity, alignment: .leading)
                        }
                        .frame(maxHeight: 200)
                    }
                    Spacer(minLength: 0)
                    HStack(spacing: 12) {
                        Button {
                            let id = paper.arxivId, title = paper.title
                            withAnimation(.spring(response: 0.4, dampingFraction: 0.85)) { preview = nil }
                            nav.open(id, title: title)
                        } label: {
                            Label("Read", systemImage: "book").frame(maxWidth: 150)
                        }
                        .buttonStyle(.borderedProminent).controlSize(.large)
                        .keyboardShortcut(.defaultAction)

                        Button("Close", action: dismiss)
                            .controlSize(.large).keyboardShortcut(.cancelAction)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .transition(.opacity.combined(with: .move(edge: .trailing)))
            }
            .padding(32)
            .frame(maxWidth: 820, maxHeight: 540)
            .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 24))
            .overlay(RoundedRectangle(cornerRadius: 24).stroke(.separator, lineWidth: 0.5))
            .shadow(color: .black.opacity(0.3), radius: 40, y: 20)
            .padding(40)
        }
    }

    private var list: some View {
        ScrollView {
            LazyVStack(spacing: 10) {
                ForEach(filtered) { paper in
                    Button { nav.open(paper.arxivId, title: paper.title) } label: {
                        PaperRow(paper: paper)
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(16)
        }
    }

    private func load() async {
        loading = true; error = nil
        do { papers = try await client.papers().sorted { $0.publishedAt > $1.publishedAt } }
        catch { self.error = error.localizedDescription }
        loading = false
    }
}

// The right pane while a split is being set up: pick the second paper to read
// alongside the first (or click another tab in the strip). Shared with Chat.
struct SplitChooser: View {
    let papers: [PaperMetadata]
    let onPick: (String, String) -> Void
    let onCancel: () -> Void
    @State private var filter = ""

    private var filtered: [PaperMetadata] {
        guard !filter.isEmpty else { return papers }
        let q = filter.lowercased()
        return papers.filter {
            $0.title.lowercased().contains(q)
            || $0.authors.joined(separator: " ").lowercased().contains(q)
        }
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: "rectangle.righthalf.inset.filled").foregroundStyle(.tint)
                Text("Choose a paper for this pane").font(.headline)
                Spacer()
                Button("Cancel", action: onCancel).keyboardShortcut(.cancelAction)
            }
            .padding(.horizontal, 14).padding(.vertical, 8)
            .background(.bar)
            .overlay(alignment: .bottom) { Divider() }

            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                TextField("Filter papers…", text: $filter).textFieldStyle(.plain)
            }
            .padding(8).padding(.horizontal, 6)

            ScrollView {
                LazyVStack(spacing: 8) {
                    ForEach(filtered) { paper in
                        Button { onPick(paper.arxivId, paper.title) } label: {
                            PaperRow(paper: paper)
                        }
                        .buttonStyle(.plain)
                    }
                }
                .padding(12)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(.background)
    }
}

private struct PaperRow: View {
    let paper: PaperMetadata
    var body: some View {
        Card {
            HStack(alignment: .top, spacing: 12) {
                Image(systemName: Theme.kindGlyph(paper.kind))
                    .font(.title2)
                    .foregroundStyle(.tint)
                    .frame(width: 28)
                VStack(alignment: .leading, spacing: 5) {
                    Text(paper.title).font(.headline).lineLimit(2)
                    if !paper.authors.isEmpty {
                        Text(paper.authors.prefix(4).joined(separator: ", ") + (paper.authors.count > 4 ? " et al." : ""))
                            .font(.subheadline).foregroundStyle(.secondary).lineLimit(1)
                    }
                    HStack(spacing: 6) {
                        Text(paper.arxivId)
                            .font(.system(.caption2, design: .monospaced))
                            .foregroundStyle(.tertiary)
                        if !paper.publishedAt.isEmpty { Chip(text: Theme.year(paper.publishedAt)) }
                        ForEach(paper.categories.prefix(3), id: \.self) { Chip(text: $0) }
                        ForEach(paper.tags.prefix(3), id: \.self) { Chip(text: $0, color: .accentColor, filled: true) }
                    }
                    // Web pages: show the source host so the origin is visible at
                    // a glance (the full clickable link lives in the detail view).
                    if let source = paper.sourceUrl, let host = URL(string: source)?.host {
                        Label(host, systemImage: "globe")
                            .font(.caption).foregroundStyle(.secondary).lineLimit(1)
                    }
                }
                Spacer(minLength: 0)
                Image(systemName: "chevron.right").font(.caption).foregroundStyle(.tertiary)
            }
        }
    }
}
