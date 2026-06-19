import SwiftUI

// ResearchAgent feed: unsolved problems the corpus surfaces (papers'
// limitations / future_work), each paired with the nearest method/applications
// work elsewhere. A "synthesis_opportunity" has solution pieces to assemble; a
// "greenfield" gap has none. Optionally focused by a domain query.
struct ProblemsView: View {
    let client: KBClient
    /// Send a problem to the Roundtable as a brainstorming objective.
    var onBrainstorm: (String) -> Void = { _ in }

    @State private var problems: [ProblemCandidate] = []
    @State private var domain = ""
    @State private var loading = true
    @State private var error: String?

    var body: some View {
        VStack(spacing: 0) {
            searchBar
            Divider()
            content
        }
        .navigationTitle("Problems")
        .task { await load() }
    }

    private var searchBar: some View {
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
            TextField("Focus a domain (optional) — e.g. vector quantization", text: $domain)
                .textFieldStyle(.plain)
                .onSubmit { Task { await load() } }
            if !domain.isEmpty {
                Button { domain = ""; Task { await load() } } label: {
                    Image(systemName: "xmark.circle.fill").foregroundStyle(.tertiary)
                }
                .buttonStyle(.plain)
            }
            Button("Hunt") { Task { await load() } }
                .buttonStyle(.borderedProminent)
        }
        .padding(12)
    }

    @ViewBuilder
    private var content: some View {
        if loading {
            ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if let error {
            EmptyStateView(icon: "exclamationmark.triangle", title: "Couldn't hunt for problems", message: error)
        } else if problems.isEmpty {
            EmptyStateView(icon: "lightbulb",
                           title: "No problems surfaced",
                           message: "Problems are mined from papers' limitations and future-work sections. Ingest a few papers (and let them embed), then hunt again.")
        } else {
            ScrollView {
                LazyVStack(spacing: 12) {
                    ForEach(problems) { problem in
                        ProblemCard(problem: problem, onBrainstorm: onBrainstorm)
                    }
                }
                .padding(16)
            }
        }
    }

    private func load() async {
        loading = true; error = nil
        do { problems = try await client.problems(domain: domain, k: 12).problems }
        catch { self.error = error.localizedDescription }
        loading = false
    }
}

private struct ProblemCard: View {
    let problem: ProblemCandidate
    let onBrainstorm: (String) -> Void

    private var isSynthesis: Bool { problem.gapType == "synthesis_opportunity" }

    var body: some View {
        Card {
            VStack(alignment: .leading, spacing: 12) {
                HStack(spacing: 8) {
                    Chip(text: Theme.sectionLabel(problem.sectionType),
                         color: Theme.sectionColor(problem.sectionType), filled: true)
                    Spacer()
                    Chip(text: isSynthesis ? "Synthesis opportunity" : "Greenfield",
                         color: isSynthesis ? .green : .orange, filled: true)
                }

                Text(problem.problemTitle)
                    .font(.subheadline.weight(.medium)).lineLimit(1)
                Text(problem.statement)
                    .font(.callout).foregroundStyle(.secondary).lineLimit(4)

                if problem.solutions.isEmpty {
                    Text("No existing method in the corpus addresses this yet.")
                        .font(.caption).foregroundStyle(.tertiary)
                } else {
                    Divider()
                    Text("Solution pieces elsewhere in your corpus")
                        .font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                    ForEach(problem.solutions) { SolutionRow(hit: $0) }
                }

                Divider()
                HStack {
                    Spacer()
                    Button {
                        onBrainstorm("Brainstorm a solution to this unsolved problem: \(problem.statement)")
                    } label: {
                        Label("Brainstorm a solution", systemImage: "person.3.sequence")
                    }
                    .buttonStyle(.borderless)
                    .font(.callout.weight(.medium))
                }
            }
        }
    }
}

private struct SolutionRow: View {
    let hit: SolutionHit
    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Chip(text: Theme.sectionLabel(hit.sectionType),
                 color: Theme.sectionColor(hit.sectionType), filled: true)
                .frame(width: 92, alignment: .leading)
            VStack(alignment: .leading, spacing: 2) {
                Text(hit.title).font(.subheadline.weight(.medium)).lineLimit(1)
                if !hit.snippet.isEmpty {
                    Text(hit.snippet).font(.caption).foregroundStyle(.secondary).lineLimit(2)
                }
            }
            Spacer()
            ScoreBadge(score: hit.score)
        }
    }
}
