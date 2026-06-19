import SwiftUI
import UniformTypeIdentifiers

// Add documents to the corpus — the `kb add` flow brought into the app. Pick a
// source (arXiv, a web page, or a local PDF), and the engine downloads, chunks,
// and embeds it. Adds land in a session log so you can see what just went in.
@MainActor
struct AddView: View {
    let client: KBClient
    @Environment(ServerController.self) private var server

    @State private var source: AddSource = .arxiv
    @State private var text = ""
    @State private var pdfURL: URL?
    @State private var picking = false
    @State private var busy = false
    /// Latest live status line from the engine while a run is in flight.
    @State private var status: String?
    @State private var error: String?
    /// Documents added this session, newest first.
    @State private var added: [IngestResult] = []

    var body: some View {
        VStack(spacing: 0) {
            if server.hasOpenAIKey {
                form
                Divider()
                log
            } else {
                ConnectOpenAIState(action: "add documents to your knowledge bank")
            }
        }
        .navigationTitle("Add")
        .fileImporter(isPresented: $picking, allowedContentTypes: [.pdf]) { result in
            if case .success(let url) = result { pdfURL = url }
        }
    }

    // MARK: Input

    private var form: some View {
        VStack(alignment: .leading, spacing: 14) {
            Picker("Source", selection: $source) {
                ForEach(AddSource.allCases) { Label($0.label, systemImage: $0.icon).tag($0) }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .onChange(of: source) { _, _ in error = nil }

            switch source {
            case .arxiv, .url:
                HStack(spacing: 10) {
                    Image(systemName: source.icon).foregroundStyle(.secondary)
                    TextField(source.placeholder, text: $text)
                        .textFieldStyle(.plain)
                        .font(.title3)
                        .onSubmit { Task { await add() } }
                        .disabled(busy)
                }
                .padding(12)
                .background(.background.secondary, in: RoundedRectangle(cornerRadius: Theme.corner))
            case .pdf:
                Button { picking = true } label: {
                    HStack(spacing: 10) {
                        Image(systemName: "doc.fill").foregroundStyle(.secondary)
                        Text(pdfURL?.lastPathComponent ?? "Choose a PDF…")
                            .font(.title3)
                            .foregroundStyle(pdfURL == nil ? .secondary : .primary)
                            .lineLimit(1).truncationMode(.middle)
                        Spacer()
                        Text("Browse").font(.callout).foregroundStyle(.tint)
                    }
                    .padding(12)
                    .background(.background.secondary, in: RoundedRectangle(cornerRadius: Theme.corner))
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .disabled(busy)
            }

            HStack(spacing: 8) {
                if busy {
                    ProgressView().controlSize(.small)
                    Text(status ?? "Starting…")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .id(status)
                        .transition(.opacity)
                } else {
                    Text(source.hint).font(.caption).foregroundStyle(.tertiary)
                }
                Spacer()
                Button(busy ? "Adding…" : "Add to KB") { Task { await add() } }
                    .buttonStyle(.borderedProminent)
                    .disabled(busy || !canSubmit)
                    .keyboardShortcut(.return, modifiers: .command)
            }
            .animation(.easeInOut(duration: 0.2), value: status)
            .animation(.easeInOut(duration: 0.2), value: busy)

            if let error {
                Label(error, systemImage: "exclamationmark.triangle.fill")
                    .font(.callout)
                    .foregroundStyle(.red)
                    .padding(10)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Color.red.opacity(0.10), in: RoundedRectangle(cornerRadius: Theme.corner))
            }
        }
        .padding(16)
    }

    private var canSubmit: Bool {
        switch source {
        case .arxiv, .url: return !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        case .pdf: return pdfURL != nil
        }
    }

    // MARK: Session log

    @ViewBuilder private var log: some View {
        if added.isEmpty {
            EmptyStateView(icon: "tray.and.arrow.down",
                           title: "Grow your knowledge bank",
                           message: "Add an arXiv paper, a web page, or a local PDF. It's downloaded, chunked, and embedded — searchable the moment it lands.")
        } else {
            ScrollView {
                LazyVStack(spacing: 10) {
                    ForEach(Array(added.enumerated()), id: \.offset) { _, doc in
                        AddedCard(doc: doc)
                    }
                }
                .padding(16)
            }
        }
    }

    // MARK: Action

    private func add() async {
        guard canSubmit, !busy else { return }
        busy = true; error = nil; status = nil
        defer { busy = false; status = nil }

        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        let stream: AsyncThrowingStream<IngestEvent, Error>
        switch source {
        case .arxiv: stream = client.ingestStream(arxiv: trimmed)
        case .url:   stream = client.ingestStream(url: trimmed)
        case .pdf:
            guard let url = pdfURL else { return }
            stream = client.ingestStream(pdfPath: url.path)
        }

        do {
            for try await event in stream {
                switch event {
                case .progress(let message):
                    status = message
                case .done(let result):
                    added.insert(result, at: 0)
                    text = ""
                    pdfURL = nil
                case .error(let message):
                    error = message
                }
            }
        } catch {
            self.error = error.localizedDescription
        }
    }
}

private enum AddSource: String, CaseIterable, Identifiable {
    case arxiv, url, pdf
    var id: String { rawValue }
    var label: String {
        switch self {
        case .arxiv: "arXiv"
        case .url: "Web Page"
        case .pdf: "PDF"
        }
    }
    var icon: String {
        switch self {
        case .arxiv: "doc.text"
        case .url: "globe"
        case .pdf: "doc.richtext"
        }
    }
    var placeholder: String {
        switch self {
        case .arxiv: "arXiv id or URL — e.g. 2504.19874"
        case .url: "https://example.com/post"
        case .pdf: ""
        }
    }
    var hint: String {
        switch self {
        case .arxiv: "Pulls the LaTeX source when available, else the PDF."
        case .url: "Readability-extracted; the main article text is kept."
        case .pdf: "The filename becomes the document id."
        }
    }
}

private struct AddedCard: View {
    let doc: IngestResult
    var body: some View {
        Card {
            HStack(alignment: .top, spacing: 12) {
                Image(systemName: "checkmark.circle.fill")
                    .font(.title3)
                    .foregroundStyle(.green)
                VStack(alignment: .leading, spacing: 4) {
                    Text(doc.title).font(.headline).lineLimit(2)
                    HStack(spacing: 8) {
                        Text(doc.id).font(.caption.monospaced()).foregroundStyle(.secondary)
                        Chip(text: doc.sourceFormat.uppercased(), color: .accentColor, filled: true)
                        Text("\(doc.chunks) sections").font(.caption).foregroundStyle(.tertiary)
                    }
                }
                Spacer(minLength: 0)
            }
        }
    }
}
