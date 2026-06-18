import SwiftUI

// Browse the corpus. A filterable list of documents; opening one (from the list
// or from a paper's Related panel) adds a browser-style tab.
@MainActor
struct LibraryView: View {
    let client: KBClient

    @State private var papers: [PaperMetadata] = []
    @State private var filter = ""
    @State private var loading = true
    @State private var error: String?
    @State private var nav = PaperTabs()

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
                list
            }
        }
        .navigationTitle("Library")
        .searchable(text: $filter, placement: .toolbar, prompt: "Filter by title, author, tag")
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
                        if !paper.publishedAt.isEmpty { Chip(text: Theme.year(paper.publishedAt)) }
                        ForEach(paper.categories.prefix(3), id: \.self) { Chip(text: $0) }
                        ForEach(paper.tags.prefix(3), id: \.self) { Chip(text: $0, color: .accentColor, filled: true) }
                    }
                }
                Spacer(minLength: 0)
                Image(systemName: "chevron.right").font(.caption).foregroundStyle(.tertiary)
            }
        }
    }
}
