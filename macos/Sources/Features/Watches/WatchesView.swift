import SwiftUI

// Manage standing interests (ArXiv Watch). Each watch is an arXiv category,
// author, or free-text query; `kb feed refresh` (here: Refresh) polls them for
// recent submissions and scores each against the corpus. Output lands in Brief.
struct WatchesView: View {
    let client: KBClient

    @State private var watches: [Watch] = []
    @State private var loading = true
    @State private var error: String?
    @State private var refreshing = false
    @State private var note: String?

    // Add form
    @State private var newKind = "category"
    @State private var newValue = ""
    @State private var adding = false

    private let kinds = ["category", "author", "query"]

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                addForm

                if let note {
                    Text(note).font(.caption).foregroundStyle(.secondary)
                }

                if loading {
                    ProgressView().frame(maxWidth: .infinity).padding(.top, 24)
                } else if let error {
                    EmptyStateView(icon: "exclamationmark.triangle",
                                   title: "Couldn't load watches", message: error)
                } else if watches.isEmpty {
                    EmptyStateView(icon: "binoculars",
                                   title: "No watches yet",
                                   message: "Add a category like cs.LG, an author, or a topic. KB will surface new papers that connect to your corpus.")
                } else {
                    ForEach(watches) { w in
                        WatchRow(watch: w, onRemove: { Task { await remove(w) } })
                    }
                }
            }
            .padding(16)
        }
        .navigationTitle("Watches")
        .toolbar {
            ToolbarItem {
                Button {
                    Task { await refresh() }
                } label: {
                    if refreshing { ProgressView().controlSize(.small) }
                    else { Label("Refresh", systemImage: "arrow.clockwise") }
                }
                .disabled(refreshing || watches.isEmpty)
                .help("Poll every watch and score new papers")
            }
        }
        .task { await load() }
    }

    private var addForm: some View {
        Card {
            VStack(alignment: .leading, spacing: 10) {
                Text("Add a watch").font(.headline)
                HStack(spacing: 8) {
                    Picker("", selection: $newKind) {
                        ForEach(kinds, id: \.self) { Text($0.capitalized).tag($0) }
                    }
                    .labelsHidden()
                    .frame(width: 120)

                    TextField(placeholder, text: $newValue)
                        .textFieldStyle(.roundedBorder)
                        .onSubmit { Task { await add() } }

                    Button {
                        Task { await add() }
                    } label: {
                        if adding { ProgressView().controlSize(.small) }
                        else { Text("Add") }
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(adding || newValue.trimmingCharacters(in: .whitespaces).isEmpty)
                }
                Text(hint).font(.caption2).foregroundStyle(.tertiary)
            }
        }
    }

    private var placeholder: String {
        switch newKind {
        case "category": return "e.g. cs.LG"
        case "author":   return "e.g. Vaswani"
        default:         return "e.g. retrieval augmented generation"
        }
    }

    private var hint: String {
        switch newKind {
        case "category": return "An arXiv category code (cs.LG, cs.IR, stat.ML, …)."
        case "author":   return "An author surname or full name."
        default:         return "Any topic; matched across title, abstract, and authors."
        }
    }

    // MARK: Actions

    private func load() async {
        loading = watches.isEmpty
        error = nil
        do { watches = try await client.watches() }
        catch { self.error = error.localizedDescription }
        loading = false
    }

    private func add() async {
        let value = newValue.trimmingCharacters(in: .whitespaces)
        guard !value.isEmpty else { return }
        adding = true; note = nil
        do {
            _ = try await client.addWatch(kind: newKind, value: value)
            newValue = ""
            await load()
        } catch {
            note = "Couldn't add watch: \(error.localizedDescription)"
        }
        adding = false
    }

    private func remove(_ w: Watch) async {
        watches.removeAll { $0.id == w.id }
        do { try await client.removeWatch(w.id) }
        catch { note = "Couldn't remove watch: \(error.localizedDescription)"; await load() }
    }

    private func refresh() async {
        refreshing = true; note = nil
        do {
            let s = try await client.refreshWatches()
            note = "Polled \(s.watchesRefreshed) watch\(s.watchesRefreshed == 1 ? "" : "es"): \(s.newCandidates) new paper\(s.newCandidates == 1 ? "" : "s"). See them in Brief."
            await load()
        } catch {
            note = "Refresh failed: \(error.localizedDescription)"
        }
        refreshing = false
    }
}

private struct WatchRow: View {
    let watch: Watch
    var onRemove: () -> Void = {}
    var body: some View {
        Card {
            HStack(spacing: 10) {
                Image(systemName: glyph).foregroundStyle(color).frame(width: 18)
                VStack(alignment: .leading, spacing: 2) {
                    Text(watch.value).font(.body.weight(.medium))
                    Text(subtitle).font(.caption2).foregroundStyle(.tertiary)
                }
                Spacer()
                Chip(text: watch.kind, color: color, filled: true)
                Button(role: .destructive, action: onRemove) {
                    Image(systemName: "trash")
                }
                .buttonStyle(.borderless)
                .help("Remove this watch")
            }
        }
    }

    private var glyph: String {
        switch watch.kind {
        case "category": return "tag"
        case "author":   return "person"
        default:         return "text.magnifyingglass"
        }
    }
    private var color: Color {
        switch watch.kind {
        case "category": return .blue
        case "author":   return .green
        default:         return .purple
        }
    }
    private var subtitle: String {
        let last = watch.lastRefreshedAt.map { "last refreshed " + String($0.prefix(10)) } ?? "never refreshed"
        return (watch.enabled ? "" : "disabled · ") + last
    }
}
