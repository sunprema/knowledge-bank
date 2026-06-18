import SwiftUI

// Reusable OpenAI-key editor (the control cluster only — callers supply the
// surrounding chrome). Reads/writes through ServerController, which persists to
// the Keychain and restarts the engine to apply the change.
struct APIKeyForm: View {
    @Environment(ServerController.self) private var server
    /// Called after a save/remove/dismiss so a sheet can close itself.
    var onDone: (() -> Void)? = nil

    @State private var text = ""
    @State private var reveal = false

    private var trimmed: String { text.trimmingCharacters(in: .whitespacesAndNewlines) }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 6) {
                Group {
                    if reveal { TextField("sk-…", text: $text) }
                    else { SecureField("sk-…", text: $text) }
                }
                .textFieldStyle(.roundedBorder)
                .font(.body.monospaced())
                Button { reveal.toggle() } label: {
                    Image(systemName: reveal ? "eye.slash" : "eye")
                }
                .buttonStyle(.borderless)
                .help(reveal ? "Hide" : "Show")
            }

            if server.hasOpenAIKey {
                Label("A key is set — search, chat, and sparks are enabled.",
                      systemImage: "checkmark.seal.fill")
                    .font(.caption).foregroundStyle(.green)
            } else {
                Text("Stored in the macOS Keychain. Only ever sent to the local engine on your machine.")
                    .font(.caption).foregroundStyle(.secondary)
            }

            HStack {
                if server.hasOpenAIKey {
                    Button("Remove", role: .destructive) {
                        server.clearOpenAIKey(); onDone?()
                    }
                }
                Spacer()
                if onDone != nil {
                    Button("Later") { onDone?() }
                        .keyboardShortcut(.cancelAction)
                }
                Button(server.hasOpenAIKey ? "Replace & Restart" : "Save & Restart") {
                    server.setOpenAIKey(trimmed); onDone?()
                }
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(.defaultAction)
                .disabled(trimmed.isEmpty)
            }
        }
    }
}

// First-run / on-demand onboarding presented as a sheet.
struct KeyOnboardingSheet: View {
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 10) {
                Image(systemName: "key.fill").font(.title).foregroundStyle(.tint)
                Text("Connect OpenAI").font(.title2.weight(.bold))
            }
            Text("Search, Chat, and Sparks use OpenAI embeddings to understand your corpus. Browsing your Library and reading PDFs works without a key.")
                .font(.callout).foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            APIKeyForm(onDone: { dismiss() })
        }
        .padding(24)
        .frame(width: 480)
    }
}

// Shown in Search/Chat when no key is configured — a friendly path to add one.
struct ConnectOpenAIState: View {
    let action: String   // e.g. "search your knowledge bank"

    var body: some View {
        VStack(spacing: 12) {
            Image(systemName: "key.horizontal")
                .font(.system(size: 38, weight: .light))
                .foregroundStyle(.tertiary)
            Text("Connect OpenAI to \(action)")
                .font(.headline).foregroundStyle(.secondary)
            Text("This feature uses embeddings to understand your papers. Add your API key to enable it.")
                .font(.subheadline).foregroundStyle(.tertiary)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 360)
            SettingsLink {
                Label("Add OpenAI API Key", systemImage: "key")
            }
            .buttonStyle(.borderedProminent)
            .padding(.top, 4)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(40)
    }
}
