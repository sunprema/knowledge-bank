import SwiftUI
import UniformTypeIdentifiers

// The ⌘, Settings window. v0.1 manages the OpenAI key and the corpus location.
struct SettingsView: View {
    @Environment(ServerController.self) private var server
    @State private var picking = false

    var body: some View {
        Form {
            Section("OpenAI") {
                APIKeyForm()
                    .padding(.vertical, 4)
            }
            Section("Anthropic (Roundtable)") {
                AnthropicKeyForm()
                    .padding(.vertical, 4)
            }
            Section("Knowledge Bank") {
                LabeledContent("Folder") {
                    HStack(spacing: 8) {
                        Text(server.kbRoot.path)
                            .font(.callout.monospaced())
                            .foregroundStyle(.secondary)
                            .lineLimit(1).truncationMode(.middle)
                            .textSelection(.enabled)
                        Spacer(minLength: 8)
                        Button("Choose…") { picking = true }
                            .disabled(server.kbRootIsFromEnv)
                    }
                }
                if server.kbRootIsFromEnv {
                    Label("Set by the KB_ROOT environment variable, which overrides this setting.",
                          systemImage: "terminal")
                        .font(.caption).foregroundStyle(.tertiary)
                } else {
                    Text("Changing the folder restarts the engine.")
                        .font(.caption).foregroundStyle(.tertiary)
                }
            }
        }
        .formStyle(.grouped)
        .frame(width: 540, height: 480)
        .fileImporter(isPresented: $picking, allowedContentTypes: [.folder]) { result in
            if case .success(let url) = result { server.setKBRoot(url) }
        }
    }
}
