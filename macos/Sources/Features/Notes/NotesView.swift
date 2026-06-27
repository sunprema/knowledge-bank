import SwiftUI

// A dedicated home for hand-written markdown notes. Each note is a standalone
// `DocKind::Note` document on the engine (the same store as captured ideas), so
// everything written here is embedded and turns up in Search and the Graph.
//
// Layout is master/detail: a list of notes on the left, a live markdown editor
// (source + rendered preview) on the right — the editor pane mirrors the
// per-paper `NotesEditor` so the two feel the same.
struct NotesView: View {
    let client: KBClient

    @State private var notes: [NoteSummary] = []
    @State private var selection: EditorTarget?
    @State private var loading = true
    @State private var loadError: String?
    @State private var query = ""

    private enum EditorTarget: Hashable {
        case new
        case existing(String)
    }

    private var filtered: [NoteSummary] {
        let q = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard !q.isEmpty else { return notes }
        return notes.filter {
            $0.title.lowercased().contains(q) || $0.preview.lowercased().contains(q)
        }
    }

    var body: some View {
        HSplitView {
            sidebar
                .frame(minWidth: 240, idealWidth: 290, maxWidth: 380, maxHeight: .infinity)
            detail
                .frame(minWidth: 460, maxWidth: .infinity, maxHeight: .infinity)
                .layoutPriority(1)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .task { await reload() }
    }

    // MARK: List

    private var sidebar: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Text("Notes").font(.headline)
                Spacer()
                Button {
                    selection = .new
                } label: {
                    Label("New", systemImage: "square.and.pencil")
                }
                .keyboardShortcut("n", modifiers: .command)
            }
            .padding(.horizontal, 12).padding(.vertical, 10)
            Divider()

            if loading {
                Spacer(); ProgressView(); Spacer()
            } else if let loadError {
                ContentUnavailableView("Couldn't load notes", systemImage: "exclamationmark.triangle",
                                       description: Text(loadError))
            } else if notes.isEmpty {
                ContentUnavailableView {
                    Label("No notes yet", systemImage: "note.text")
                } description: {
                    Text("Create your first markdown note.")
                } actions: {
                    Button("New Note") { selection = .new }.buttonStyle(.borderedProminent)
                }
            } else {
                List(selection: $selection) {
                    ForEach(filtered) { note in
                        NoteRow(note: note)
                            .tag(EditorTarget.existing(note.id))
                    }
                }
                .listStyle(.sidebar)
                .searchable(text: $query, placement: .sidebar, prompt: "Filter notes")
            }
        }
        .frame(maxHeight: .infinity, alignment: .top)
    }

    // MARK: Editor

    @ViewBuilder
    private var detail: some View {
        switch selection {
        case .new:
            NoteEditorPane(client: client, noteID: nil,
                           onSaved: { id in await reload(); selection = .existing(id) },
                           onDeleted: { selection = nil })
                .id("new")
        case .existing(let id):
            NoteEditorPane(client: client, noteID: id,
                           onSaved: { _ in await reload() },
                           onDeleted: { selection = nil; await reload() })
                .id(id)
        case nil:
            ContentUnavailableView {
                Label("Select a note", systemImage: "note.text")
            } description: {
                Text("Pick a note on the left, or create a new one.")
            }
        }
    }

    @discardableResult
    private func reload() async -> Bool {
        do {
            notes = try await client.notes()
            loadError = nil
        } catch {
            loadError = error.localizedDescription
        }
        loading = false
        return true
    }
}

// MARK: - Row

private struct NoteRow: View {
    let note: NoteSummary

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(note.title.isEmpty ? "Untitled" : note.title)
                .font(.body.weight(.medium))
                .lineLimit(1)
            if !note.preview.isEmpty {
                Text(note.preview)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
            }
            if !note.updatedAt.isEmpty {
                Text(note.updatedAt.prefix(10))
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
            }
        }
        .padding(.vertical, 3)
    }
}

// MARK: - Editor pane

// Source + live preview editor for a single note. `noteID == nil` is a fresh,
// unsaved note. Resets on `.id(...)` change when the selection moves.
private struct NoteEditorPane: View {
    let client: KBClient
    let noteID: String?
    var onSaved: (String) async -> Void
    var onDeleted: () async -> Void

    @State private var title = ""
    @State private var text = ""
    @State private var preview = ""
    @State private var savedTitle = ""
    @State private var savedText = ""
    @State private var project = "global"
    @State private var loading: Bool
    @State private var saving = false
    @State private var status: String?
    @State private var previewTask: Task<Void, Never>?
    @State private var confirmDelete = false

    init(client: KBClient, noteID: String?,
         onSaved: @escaping (String) async -> Void,
         onDeleted: @escaping () async -> Void) {
        self.client = client
        self.noteID = noteID
        self.onSaved = onSaved
        self.onDeleted = onDeleted
        _loading = State(initialValue: noteID != nil)
    }

    private var dirty: Bool { title != savedTitle || text != savedText }
    private var canSave: Bool {
        !title.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && dirty && !saving
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            if loading {
                Spacer(); ProgressView(); Spacer()
            } else {
                editor
            }
        }
        .task(id: noteID) { await load() }
        .onChange(of: text) { schedulePreview() }
        .confirmationDialog("Delete this note?", isPresented: $confirmDelete, titleVisibility: .visible) {
            Button("Delete", role: .destructive) { Task { await delete() } }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("This permanently removes the note from your Knowledge Bank.")
        }
    }

    private var header: some View {
        HStack(spacing: 10) {
            Image(systemName: "note.text").foregroundStyle(.tint)
            TextField("Title", text: $title)
                .textFieldStyle(.plain)
                .font(.headline)
            Spacer()
            if let status { Text(status).font(.caption).foregroundStyle(.secondary) }
            if noteID != nil {
                Button(role: .destructive) { confirmDelete = true } label: {
                    Image(systemName: "trash")
                }
                .buttonStyle(.borderless)
            }
            Button {
                Task { await save() }
            } label: { Label(saving ? "Saving…" : "Save", systemImage: "arrow.down.doc") }
                .buttonStyle(.borderedProminent)
                .disabled(!canSave)
                .keyboardShortcut("s", modifiers: .command)
        }
        .padding(12)
    }

    private var editor: some View {
        HSplitView {
            VStack(alignment: .leading, spacing: 0) {
                paneLabel("Markdown")
                TextEditor(text: $text)
                    .font(.system(.body, design: .monospaced))
                    .padding(8)
                    .scrollContentBackground(.hidden)
            }
            .frame(minWidth: 260)

            VStack(alignment: .leading, spacing: 0) {
                paneLabel("Preview")
                if preview.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                    Text("Nothing to preview yet.")
                        .font(.callout).foregroundStyle(.tertiary)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                } else {
                    MarkdownView(markdown: preview)
                }
            }
            .frame(minWidth: 260)
            .layoutPriority(1)
        }
    }

    private func paneLabel(_ s: String) -> some View {
        Text(s).font(.caption2.weight(.semibold)).foregroundStyle(.secondary)
            .padding(.horizontal, 12).padding(.vertical, 6)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(.bar)
            .overlay(alignment: .bottom) { Divider() }
    }

    // Debounce the (relatively heavy) WebView re-render while typing.
    private func schedulePreview() {
        status = dirty ? "Unsaved changes" : nil
        previewTask?.cancel()
        previewTask = Task {
            try? await Task.sleep(for: .milliseconds(400))
            if !Task.isCancelled { preview = text }
        }
    }

    private func load() async {
        guard let noteID else {
            // Fresh note: start blank.
            title = ""; text = ""; preview = ""; savedTitle = ""; savedText = ""
            loading = false
            return
        }
        loading = true
        do {
            let detail = try await client.note(noteID)
            title = detail.title
            text = detail.body
            preview = detail.body
            project = detail.project
            savedTitle = detail.title
            savedText = detail.body
        } catch {
            status = "Couldn't load note"
        }
        loading = false
    }

    private func save() async {
        saving = true; status = nil
        do {
            let id: String
            if let noteID {
                id = try await client.updateNote(noteID, title: title, body: text, project: project)
            } else {
                id = try await client.createNote(title: title, body: text, project: project)
            }
            savedTitle = title; savedText = text
            status = "Saved"
            await onSaved(id)
        } catch {
            status = "Save failed"
        }
        saving = false
    }

    private func delete() async {
        guard let noteID else { return }
        do {
            try await client.deleteNote(noteID)
            await onDeleted()
        } catch {
            status = "Delete failed"
        }
    }
}
