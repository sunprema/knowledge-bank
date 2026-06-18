import SwiftUI

// "Explain this": a selected passage is sent to the chat model for a plain-
// language explanation (defining notation/jargon), shown in a sheet with the
// option to read it aloud or save it back to the paper's notes (re-embedded,
// so the explanation becomes searchable too). Uses /chat → needs an OpenAI key.
struct ExplainView: View {
    let client: KBClient
    let paperId: String
    let passage: String
    /// Called after the explanation is saved, so the caller can refresh notes.
    var onSaved: () -> Void = {}

    @Environment(\.dismiss) private var dismiss
    @State private var answer: String?
    @State private var error: String?
    @State private var loading = true
    @State private var saving = false
    @State private var saved = false

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 10) {
                Image(systemName: "sparkles").foregroundStyle(.tint)
                Text("Explain This").font(.headline)
                Spacer()
                if let answer {
                    Button { Task { await save() } } label: {
                        Label(saved ? "Saved" : "Save to Notes",
                              systemImage: saved ? "checkmark.circle.fill" : "note.text.badge.plus")
                    }
                    .disabled(saving || saved)
                    .help("Append the passage and explanation to this paper's notes")

                    ReadAloudButton(text: answer, title: "Explanation").buttonStyle(.borderless)
                }
                Button("Done") { dismiss() }.keyboardShortcut(.cancelAction)
            }
            .padding(12)
            Divider()

            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    Text(passage)
                        .font(.callout.italic())
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                        .padding(12)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .background(.background.secondary, in: RoundedRectangle(cornerRadius: 10))
                        .overlay(alignment: .leading) {
                            Rectangle().fill(.tint).frame(width: 3)
                                .clipShape(RoundedRectangle(cornerRadius: 2))
                        }

                    if loading {
                        HStack(spacing: 8) {
                            ProgressView().controlSize(.small)
                            Text("Thinking…").font(.callout).foregroundStyle(.secondary)
                        }
                    } else if let answer {
                        Text(answer)
                            .font(.body).lineSpacing(3)
                            .textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    } else if let error {
                        EmptyStateView(icon: "exclamationmark.triangle",
                                       title: "Couldn't explain this",
                                       message: error)
                    }
                }
                .padding(16)
            }
        }
        .frame(width: 560, height: 480)
        .task { await explain() }
    }

    private func explain() async {
        loading = true; error = nil
        let prompt = """
        Explain the following passage from a research paper in clear, plain language. \
        Define any notation, symbols, or jargon, and say why it matters. Be concise.

        Passage:
        "\(passage)"
        """
        do {
            let resp = try await client.chat(prompt, history: [])
            answer = resp.answer
        } catch {
            self.error = error.localizedDescription
        }
        loading = false
    }

    private func save() async {
        guard let answer, !saving else { return }
        saving = true
        let quoted = passage
            .replacingOccurrences(of: "\n", with: " ")
            .components(separatedBy: .whitespaces).filter { !$0.isEmpty }
            .joined(separator: " ")
        let note = "> \(quoted)\n\n**Explanation:** \(answer)"
        do {
            _ = try await client.addNote(paperId, note: note)
            saved = true
            onSaved()
        } catch {
            self.error = error.localizedDescription
        }
        saving = false
    }
}
