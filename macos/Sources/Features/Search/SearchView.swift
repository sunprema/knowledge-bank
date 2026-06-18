import SwiftUI

// Semantic search across the corpus. Paper-grouped results (the engine
// deduplicates at the paper level); each matched chunk opens the reader at its
// page.
@MainActor
struct SearchView: View {
    let client: KBClient

    @State private var query = ""
    @State private var mode: SearchMode = .narrow
    @State private var response: SearchResponse?
    @State private var searching = false
    @State private var error: String?
    @State private var reader: ReaderTarget?
    @Environment(ServerController.self) private var server

    var body: some View {
        VStack(spacing: 0) {
            if server.hasOpenAIKey {
                queryBar
                Divider()
            }
            results
        }
        .navigationTitle("Search")
        .sheet(item: $reader) { t in
            PDFReaderView(client: client, paperId: t.paperId, title: t.title, targetPage: t.page)
        }
    }

    private var queryBar: some View {
        VStack(spacing: 10) {
            HStack(spacing: 10) {
                Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                TextField("Search your knowledge bank…", text: $query)
                    .textFieldStyle(.plain)
                    .font(.title3)
                    .onSubmit { Task { await run() } }
                if searching { ProgressView().controlSize(.small) }
            }
            .padding(12)
            .background(.background.secondary, in: RoundedRectangle(cornerRadius: Theme.corner))

            HStack {
                Picker("Mode", selection: $mode) {
                    ForEach(SearchMode.allCases) { Text($0.label).tag($0) }
                }
                .pickerStyle(.segmented)
                .fixedSize()
                .help(mode.help)
                Text(mode.help).font(.caption).foregroundStyle(.tertiary)
                Spacer()
                if let r = response {
                    Text("\(r.papers.count) papers · \(r.totalChunks) sections")
                        .font(.caption).foregroundStyle(.secondary)
                }
            }
        }
        .padding(16)
    }

    @ViewBuilder private var results: some View {
        if !server.hasOpenAIKey {
            ConnectOpenAIState(action: "search your knowledge bank")
        } else if let error {
            EmptyStateView(icon: "exclamationmark.triangle", title: "Search failed", message: error)
        } else if let r = response {
            if r.papers.isEmpty {
                EmptyStateView(icon: "magnifyingglass",
                               title: "No matches",
                               message: "Nothing crossed the relevance bar. Try Wide mode, or different words — search is by meaning, not keywords.")
            } else {
                ScrollView {
                    LazyVStack(spacing: 12) {
                        ForEach(r.papers) { group in
                            ResultCard(group: group) { chunk in
                                reader = ReaderTarget(paperId: group.paperId, title: group.paper.title, page: chunk.page)
                            }
                        }
                    }
                    .padding(16)
                }
            }
        } else {
            EmptyStateView(icon: "sparkle.magnifyingglass",
                           title: "Search by meaning",
                           message: "Ask for a concept, an idea, or a question. Results are sections of your papers ranked by semantic similarity.")
        }
    }

    private func run() async {
        let q = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !q.isEmpty else { return }
        searching = true; error = nil
        do { response = try await client.search(q, mode: mode) }
        catch { self.error = error.localizedDescription }
        searching = false
    }
}

private struct ReaderTarget: Identifiable {
    let paperId: String, title: String
    let page: Int?
    var id: String { "\(paperId)#\(page ?? 0)" }
}

private struct ResultCard: View {
    let group: PaperGroup
    let openChunk: (ChunkHit) -> Void

    var body: some View {
        Card {
            VStack(alignment: .leading, spacing: 10) {
                HStack(alignment: .top) {
                    VStack(alignment: .leading, spacing: 3) {
                        Text(group.paper.title).font(.headline).lineLimit(2)
                        Text(group.paper.authors.prefix(3).joined(separator: ", "))
                            .font(.caption).foregroundStyle(.secondary).lineLimit(1)
                    }
                    Spacer()
                    ScoreBadge(score: group.bestScore)
                }
                Divider()
                ForEach(group.chunks) { chunk in
                    Button { openChunk(chunk) } label: {
                        HStack(alignment: .top, spacing: 10) {
                            Chip(text: Theme.sectionLabel(chunk.sectionType),
                                 color: Theme.sectionColor(chunk.sectionType), filled: true)
                                .frame(width: 92, alignment: .leading)
                            Text(chunk.snippet).font(.callout).foregroundStyle(.primary)
                                .lineLimit(2).multilineTextAlignment(.leading)
                            Spacer(minLength: 6)
                            if let p = chunk.page {
                                Text("p.\(p)").font(.caption2.monospacedDigit()).foregroundStyle(.tertiary)
                            }
                        }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .help("Open in reader")
                }
            }
        }
    }
}
