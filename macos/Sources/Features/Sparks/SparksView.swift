import SwiftUI

// The Cortex associative layer: surprising cross-document connections, most
// surprising first. Each spark links two chunks from (usually) different papers.
struct SparksView: View {
    let client: KBClient

    @State private var sparks: [Spark] = []
    @State private var loading = true
    @State private var error: String?
    @Environment(SpeechController.self) private var speech

    var body: some View {
        Group {
            if loading {
                ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if let error {
                EmptyStateView(icon: "exclamationmark.triangle", title: "Couldn't load sparks", message: error)
            } else if sparks.isEmpty {
                EmptyStateView(icon: "sparkles",
                               title: "No sparks yet",
                               message: "Sparks surface once the corpus is embedded and the associative layer is built (`kb cortex build`). They reveal non-obvious links across your papers.")
            } else {
                ScrollView {
                    LazyVStack(spacing: 12) {
                        ForEach(sparks) { SparkCard(spark: $0) }
                    }
                    .padding(16)
                }
            }
        }
        .navigationTitle("Sparks")
        .toolbar {
            if !sparks.isEmpty {
                ToolbarItem {
                    ReadAloudButton(text: spokenDigest, title: "Today's sparks")
                }
            }
        }
        .task { await load() }
    }

    private var spokenDigest: String {
        sparks.prefix(5).enumerated().map { i, s in
            "Connection \(i + 1). \(s.src.title), \(Theme.sectionLabel(s.src.sectionType)), connects to \(s.dst.title), \(Theme.sectionLabel(s.dst.sectionType))."
        }.joined(separator: " ")
    }

    private func load() async {
        loading = true; error = nil
        do { sparks = try await client.sparks(limit: 60).sparks }
        catch { self.error = error.localizedDescription }
        loading = false
    }
}

private struct SparkCard: View {
    let spark: Spark
    var body: some View {
        Card {
            VStack(alignment: .leading, spacing: 12) {
                HStack(spacing: 8) {
                    Image(systemName: "sparkle").foregroundStyle(.yellow)
                    Text("Surprise").font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                    ScoreBadge(score: spark.surprise)
                    Spacer()
                    Chip(text: spark.kind, color: .purple, filled: true)
                }
                end(spark.src)
                HStack(spacing: 6) {
                    Image(systemName: spark.directed ? "arrow.down" : "arrow.up.arrow.down")
                        .font(.caption).foregroundStyle(.tertiary)
                        .frame(width: 92)
                    Rectangle().fill(.separator).frame(height: 0.5)
                }
                end(spark.dst)
            }
        }
    }

    private func end(_ e: SparkEnd) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Chip(text: Theme.sectionLabel(e.sectionType),
                 color: Theme.sectionColor(e.sectionType), filled: true)
                .frame(width: 92, alignment: .leading)
            VStack(alignment: .leading, spacing: 2) {
                Text(e.title).font(.subheadline.weight(.medium)).lineLimit(1)
                if !e.snippet.isEmpty {
                    Text(e.snippet).font(.caption).foregroundStyle(.secondary).lineLimit(2)
                }
            }
        }
    }
}
