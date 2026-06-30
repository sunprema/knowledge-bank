import SwiftUI

// The daily brief — the app's landing surface. "The KB comes to you": new
// papers surfaced by your watches (scored by how strongly they connect to what
// you already have), a resurfaced past reflection, and a few fresh sparks.
struct BriefView: View {
    let client: KBClient
    var onOpenPaper: (String, String) -> Void = { _, _ in }
    var onManageWatches: () -> Void = {}

    @State private var brief: Brief?
    @State private var loading = true
    @State private var error: String?
    @State private var refreshing = false
    @State private var refreshNote: String?
    /// arXiv ids currently being ingested (drives per-card spinners).
    @State private var ingesting: Set<String> = []

    var body: some View {
        Group {
            if loading {
                ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if let error {
                EmptyStateView(icon: "exclamationmark.triangle",
                               title: "Couldn't load the brief", message: error)
            } else if let brief {
                content(brief)
            }
        }
        .navigationTitle("Brief")
        .toolbar {
            ToolbarItem {
                Button {
                    Task { await refresh() }
                } label: {
                    if refreshing { ProgressView().controlSize(.small) }
                    else { Label("Refresh", systemImage: "arrow.clockwise") }
                }
                .disabled(refreshing)
                .help("Fetch and score new papers from your watches")
            }
        }
        .task { await load() }
    }

    @ViewBuilder
    private func content(_ brief: Brief) -> some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 20) {
                statsRow(brief.stats)

                if let note = refreshNote {
                    Text(note).font(.caption).foregroundStyle(.secondary)
                }

                // New papers
                sectionHeader("New papers for you", systemImage: "doc.badge.plus")
                if brief.newPapers.isEmpty {
                    emptyPapers(brief.stats)
                } else {
                    ForEach(brief.newPapers) { cand in
                        CandidateCard(
                            candidate: cand,
                            ingesting: ingesting.contains(cand.arxivId),
                            onIngest: { Task { await ingest(cand) } },
                            onDismiss: { Task { await dismiss(cand) } },
                            onOpenArxiv: { openArxiv(cand.arxivId) })
                    }
                }

                // Resurfaced reflection
                if let r = brief.resurfaced {
                    sectionHeader("Resurfaced", systemImage: "arrow.counterclockwise.circle")
                    ResurfacedCard(item: r, onOpen: { onOpenPaper(r.paperId, r.title) })
                }

                // Sparks teaser
                if !brief.sparks.isEmpty {
                    sectionHeader("Fresh sparks", systemImage: "sparkles")
                    ForEach(brief.sparks) { SparkTeaser(spark: $0, onOpenPaper: onOpenPaper) }
                }
            }
            .padding(16)
        }
    }

    private func statsRow(_ s: BriefStats) -> some View {
        HStack(spacing: 10) {
            StatPill(value: s.newCandidates, label: "new", systemImage: "tray.full", tint: .accentColor)
            StatPill(value: s.papers, label: "papers", systemImage: "books.vertical", tint: .secondary)
            Button(action: onManageWatches) {
                StatPill(value: s.watches, label: "watches", systemImage: "binoculars", tint: .purple)
            }
            .buttonStyle(.plain)
            .help("Manage your watches")
            Spacer()
        }
    }

    private func sectionHeader(_ title: String, systemImage: String) -> some View {
        Label(title, systemImage: systemImage)
            .font(.headline)
            .padding(.top, 4)
    }

    private func emptyPapers(_ s: BriefStats) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            if s.watches == 0 {
                Text("No watches yet. Add a standing interest — an arXiv category, an author, or a topic — and KB will surface new papers that connect to your corpus.")
                    .font(.subheadline).foregroundStyle(.secondary)
                Button("Add a watch", systemImage: "binoculars", action: onManageWatches)
                    .buttonStyle(.borderedProminent)
            } else {
                Text("No new papers right now. Hit Refresh to poll your \(s.watches) watch\(s.watches == 1 ? "" : "es") for recent submissions.")
                    .font(.subheadline).foregroundStyle(.secondary)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.vertical, 8)
    }

    // MARK: Actions

    private func load() async {
        loading = brief == nil
        error = nil
        do { brief = try await client.brief() }
        catch { self.error = error.localizedDescription }
        loading = false
    }

    private func refresh() async {
        refreshing = true; refreshNote = nil
        do {
            let summary = try await client.refreshWatches()
            refreshNote = summaryText(summary)
            try? await Task.sleep(for: .milliseconds(150))
            await load()
        } catch {
            refreshNote = "Refresh failed: \(error.localizedDescription)"
        }
        refreshing = false
    }

    private func summaryText(_ s: RefreshSummary) -> String {
        if s.watchesRefreshed == 0 {
            return "No enabled watches — add one in Watches."
        }
        var t = "Polled \(s.watchesRefreshed) watch\(s.watchesRefreshed == 1 ? "" : "es"): \(s.newCandidates) new paper\(s.newCandidates == 1 ? "" : "s")."
        if !s.errors.isEmpty { t += " \(s.errors.count) warning(s)." }
        return t
    }

    private func ingest(_ cand: WatchCandidate) async {
        ingesting.insert(cand.arxivId)
        defer { ingesting.remove(cand.arxivId) }
        do {
            for try await event in client.ingestStream(arxiv: cand.arxivId) {
                if case .error(let msg) = event {
                    refreshNote = "Ingest failed: \(msg)"
                    return
                }
            }
            try? await client.setCandidateStatus(cand.arxivId, status: "ingested")
            removeCandidate(cand.arxivId)
            onOpenPaper(cand.arxivId, cand.title)
        } catch {
            refreshNote = "Ingest failed: \(error.localizedDescription)"
        }
    }

    private func dismiss(_ cand: WatchCandidate) async {
        removeCandidate(cand.arxivId)
        try? await client.setCandidateStatus(cand.arxivId, status: "dismissed")
    }

    private func removeCandidate(_ id: String) {
        brief?.newPapers.removeAll { $0.arxivId == id }
    }

    private func openArxiv(_ id: String) {
        if let url = URL(string: "https://arxiv.org/abs/\(id)") {
            NSWorkspace.shared.open(url)
        }
    }
}

// MARK: - Cards

private struct StatPill: View {
    let value: Int
    let label: String
    let systemImage: String
    var tint: Color = .secondary
    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: systemImage).foregroundStyle(tint)
            Text("\(value)").font(.headline.monospacedDigit())
            Text(label).font(.caption).foregroundStyle(.secondary)
        }
        .padding(.horizontal, 12).padding(.vertical, 8)
        .background(.background.secondary, in: RoundedRectangle(cornerRadius: Theme.corner))
        .overlay {
            RoundedRectangle(cornerRadius: Theme.corner).stroke(.separator.opacity(0.6), lineWidth: 0.5)
        }
    }
}

private struct CandidateCard: View {
    let candidate: WatchCandidate
    var ingesting: Bool = false
    var onIngest: () -> Void = {}
    var onDismiss: () -> Void = {}
    var onOpenArxiv: () -> Void = {}

    var body: some View {
        Card {
            VStack(alignment: .leading, spacing: 10) {
                HStack(alignment: .top, spacing: 8) {
                    ScoreBadge(score: candidate.score)
                    VStack(alignment: .leading, spacing: 3) {
                        // Title is a link to the arXiv abstract page — read it
                        // there before deciding to ingest.
                        Button(action: onOpenArxiv) {
                            HStack(alignment: .firstTextBaseline, spacing: 5) {
                                Text(candidate.title).font(.headline).lineLimit(3)
                                    .multilineTextAlignment(.leading)
                                Image(systemName: "arrow.up.forward.app")
                                    .font(.caption2).foregroundStyle(.tint)
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .help("Open arxiv.org/abs/\(candidate.arxivId)")
                        if !candidate.authors.isEmpty {
                            Text(authorLine).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                        }
                    }
                    Spacer(minLength: 0)
                    Link(candidate.arxivId, destination: arxivURL)
                        .font(.caption2.monospaced())
                        .help("Open on arXiv")
                }

                if !candidate.categories.isEmpty {
                    HStack(spacing: 6) {
                        ForEach(candidate.categories.prefix(4), id: \.self) {
                            Chip(text: $0, color: .blue)
                        }
                    }
                }

                if !candidate.abstract.isEmpty {
                    Text(candidate.abstract).font(.callout).foregroundStyle(.secondary).lineLimit(3)
                }

                if !candidate.why.connections.isEmpty {
                    connectsTo
                }

                HStack(spacing: 8) {
                    Button(action: onIngest) {
                        if ingesting { ProgressView().controlSize(.small) }
                        else { Label("Ingest", systemImage: "tray.and.arrow.down") }
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(ingesting)

                    Button("Dismiss", systemImage: "xmark", action: onDismiss)
                        .buttonStyle(.bordered)
                        .disabled(ingesting)

                    Spacer()
                    Link(destination: arxivURL) {
                        Label("View on arXiv", systemImage: "arrow.up.forward.square")
                    }
                    .buttonStyle(.bordered)
                    .help("Read the abstract on arxiv.org before ingesting")
                    Link(destination: pdfURL) {
                        Label("PDF", systemImage: "doc")
                    }
                    .buttonStyle(.borderless).font(.caption)
                    .help("Open the PDF on arxiv.org")
                }
            }
        }
    }

    private var arxivURL: URL {
        URL(string: "https://arxiv.org/abs/\(candidate.arxivId)")!
    }
    private var pdfURL: URL {
        URL(string: "https://arxiv.org/pdf/\(candidate.arxivId)")!
    }

    private var authorLine: String {
        let names = candidate.authors.prefix(4).joined(separator: ", ")
        return candidate.authors.count > 4 ? names + " et al." : names
    }

    private var connectsTo: some View {
        VStack(alignment: .leading, spacing: 5) {
            Label("connects to", systemImage: "link")
                .font(.caption2.weight(.semibold)).foregroundStyle(.tertiary)
            ForEach(candidate.why.connections.prefix(3)) { c in
                HStack(spacing: 7) {
                    Image(systemName: Theme.kindGlyph(c.kind))
                        .font(.caption2).foregroundStyle(Theme.kindColor(c.kind)).frame(width: 14)
                    Text(c.title).font(.caption).lineLimit(1)
                    if let s = c.sections.first {
                        Chip(text: Theme.sectionLabel(s), color: Theme.sectionColor(s), filled: true)
                    }
                    Spacer(minLength: 0)
                }
            }
            if candidate.why.connectsToSynthesis {
                Label("links to your own synthesis", systemImage: "star.fill")
                    .font(.caption2).foregroundStyle(.purple)
            }
        }
        .padding(.leading, 2)
    }
}

private struct ResurfacedCard: View {
    let item: Resurfaced
    var onOpen: () -> Void = {}
    var body: some View {
        Button(action: onOpen) {
            Card {
                VStack(alignment: .leading, spacing: 8) {
                    HStack(spacing: 8) {
                        Image(systemName: Theme.kindGlyph(item.kind))
                            .foregroundStyle(Theme.kindColor(item.kind))
                        Text(item.title).font(.headline).lineLimit(2)
                        Spacer(minLength: 0)
                        Image(systemName: "arrow.up.forward.square").font(.caption).foregroundStyle(.tertiary)
                    }
                    if !item.snippet.isEmpty {
                        Text(item.snippet).font(.callout).foregroundStyle(.secondary).lineLimit(4)
                    }
                }
            }
        }
        .buttonStyle(.plain)
        .help("Open “\(item.title)”")
    }
}

private struct SparkTeaser: View {
    let spark: BriefSpark
    var onOpenPaper: (String, String) -> Void = { _, _ in }
    var body: some View {
        Card {
            HStack(spacing: 8) {
                Image(systemName: "sparkle").foregroundStyle(.yellow)
                VStack(alignment: .leading, spacing: 2) {
                    Button { onOpenPaper(spark.src.paper, spark.src.paper) } label: {
                        Text(spark.src.paper).font(.caption.weight(.medium))
                    }.buttonStyle(.link)
                    Text(spark.kind.replacingOccurrences(of: "_", with: " "))
                        .font(.caption2).foregroundStyle(.tertiary)
                    Button { onOpenPaper(spark.dst.paper, spark.dst.paper) } label: {
                        Text(spark.dst.paper).font(.caption.weight(.medium))
                    }.buttonStyle(.link)
                }
                Spacer()
                ScoreBadge(score: spark.surprise)
            }
        }
    }
}
