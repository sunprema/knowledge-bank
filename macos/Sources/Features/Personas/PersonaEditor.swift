import SwiftUI

// The detail editor for one persona: a hero header (avatar + identity), a
// markdown prompt editor with live preview (the heart of a persona — what the
// LLM is told to be), and the model + capability controls. Edits write straight
// through the binding into the shared PersonaStore, which persists immediately.
@MainActor
struct PersonaEditor: View {
    @Binding var persona: Persona
    var onDuplicate: () -> Void
    var onDelete: () -> Void

    private enum PromptMode: String, CaseIterable, Identifiable {
        case write = "Write", preview = "Preview"
        var id: String { rawValue }
    }
    @State private var promptMode: PromptMode = .write

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                hero
                promptCard
                HStack(alignment: .top, spacing: 16) {
                    modelCard
                    capabilitiesCard
                }
            }
            .padding(24)
            .frame(maxWidth: 820)
            .frame(maxWidth: .infinity)
        }
    }

    // MARK: Hero — avatar + identity

    private var hero: some View {
        Card {
            HStack(alignment: .top, spacing: 16) {
                VStack(spacing: 10) {
                    iconMenu
                    colorRow
                }
                VStack(alignment: .leading, spacing: 8) {
                    TextField("Name", text: $persona.name)
                        .textFieldStyle(.plain)
                        .font(.title2.weight(.bold))
                    TextField("Title / Role — e.g. Business & GTM", text: $persona.role)
                        .textFieldStyle(.plain)
                        .font(.title3)
                        .foregroundStyle(.secondary)
                    HStack(spacing: 6) {
                        if persona.isSynth { Chip(text: "synthesizer", color: .accentColor, filled: true) }
                        if persona.isFactChecker { Chip(text: "fact-checker", color: .teal, filled: true) }
                        if persona.tools { Chip(text: "tools", color: .purple, filled: true) }
                        if persona.queriesKB { Chip(text: "uses KB", color: .blue, filled: true) }
                    }
                }
                Spacer()
                Menu {
                    Button { onDuplicate() } label: { Label("Duplicate", systemImage: "plus.square.on.square") }
                    Divider()
                    Button(role: .destructive) { onDelete() } label: { Label("Delete", systemImage: "trash") }
                } label: {
                    Image(systemName: "ellipsis.circle").font(.title3)
                }
                .menuStyle(.borderlessButton)
                .menuIndicator(.hidden)
                .fixedSize()
            }
        }
    }

    private var iconMenu: some View {
        Menu {
            ForEach(PersonaIcons.options, id: \.self) { ic in
                Button { persona.icon = ic } label: { Label(ic, systemImage: ic) }
            }
        } label: {
            ZStack {
                Circle().fill(persona.color.opacity(0.18)).frame(width: 76, height: 76)
                Image(systemName: persona.icon).font(.system(size: 32)).foregroundStyle(persona.color)
            }
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .help("Choose an icon")
    }

    private var colorRow: some View {
        // Two compact rows of swatches under the avatar.
        let cols = Array(repeating: GridItem(.fixed(18), spacing: 6), count: 5)
        return LazyVGrid(columns: cols, spacing: 6) {
            ForEach(PersonaPalette.options, id: \.name) { opt in
                Circle()
                    .fill(opt.color)
                    .frame(width: 16, height: 16)
                    .overlay { Circle().stroke(.primary, lineWidth: persona.colorName == opt.name ? 2 : 0) }
                    .contentShape(Circle())
                    .onTapGesture { persona.colorName = opt.name }
            }
        }
        .frame(width: 110)
    }

    // MARK: Prompt — the LLM instructions

    private var promptCard: some View {
        Card {
            VStack(alignment: .leading, spacing: 10) {
                HStack {
                    Label("Persona prompt", systemImage: "text.alignleft")
                        .font(.subheadline.weight(.semibold))
                    Spacer()
                    Picker("", selection: $promptMode) {
                        ForEach(PromptMode.allCases) { Text($0.rawValue).tag($0) }
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                    .fixedSize()
                }
                Text("Markdown instructions sent to the model as this persona's system prompt — who they are, how they think, what to focus on.")
                    .font(.caption).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)

                if promptMode == .write {
                    TextEditor(text: $persona.prompt)
                        .font(.callout.monospaced())
                        .scrollContentBackground(.hidden)
                        .padding(8)
                        .frame(minHeight: 220)
                        .background(.background, in: RoundedRectangle(cornerRadius: Theme.corner))
                        .overlay {
                            RoundedRectangle(cornerRadius: Theme.corner).stroke(.separator.opacity(0.6), lineWidth: 0.5)
                        }
                        .overlay(alignment: .topLeading) {
                            if persona.prompt.isEmpty {
                                Text("e.g. You are a rigorous systems researcher. Reason from first principles, cite trade-offs, and call out the riskiest assumption.")
                                    .font(.callout)
                                    .foregroundStyle(.tertiary)
                                    .padding(.horizontal, 13).padding(.vertical, 16)
                                    .allowsHitTesting(false)
                            }
                        }
                } else {
                    Group {
                        if persona.prompt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                            Text("Nothing to preview yet — switch to Write and describe the persona.")
                                .font(.callout).foregroundStyle(.tertiary)
                                .frame(maxWidth: .infinity, alignment: .leading)
                        } else {
                            MarkdownText(markdown: persona.prompt)
                        }
                    }
                    .padding(12)
                    .frame(minHeight: 220, alignment: .topLeading)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(.background, in: RoundedRectangle(cornerRadius: Theme.corner))
                    .overlay {
                        RoundedRectangle(cornerRadius: Theme.corner).stroke(.separator.opacity(0.6), lineWidth: 0.5)
                    }
                }
            }
        }
    }

    // MARK: Model

    private var modelCard: some View {
        Card {
            VStack(alignment: .leading, spacing: 10) {
                Label("Model", systemImage: "cpu").font(.subheadline.weight(.semibold))
                Menu {
                    ForEach(LLMModel.all) { m in
                        Button { persona.modelId = m.id } label: {
                            Label("\(m.label) · \(m.provider.rawValue)", systemImage: m.providerGlyph)
                        }
                    }
                } label: {
                    HStack(spacing: 8) {
                        Image(systemName: persona.model.providerGlyph)
                            .foregroundStyle(persona.model.providerColor)
                        Text(persona.model.label).font(.callout.weight(.medium))
                        Spacer()
                        Image(systemName: "chevron.up.chevron.down").font(.caption2).foregroundStyle(.secondary)
                    }
                    .padding(10)
                    .background(.background, in: RoundedRectangle(cornerRadius: Theme.corner))
                }
                .menuStyle(.borderlessButton)
                Text(persona.model.provider == .anthropic
                     ? "Anthropic — supports live tool use."
                     : "OpenAI — tool use unavailable.")
                    .font(.caption2).foregroundStyle(.tertiary)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    // MARK: Capabilities

    private var capabilitiesCard: some View {
        Card {
            VStack(alignment: .leading, spacing: 10) {
                Label("Capabilities", systemImage: "switch.2").font(.subheadline.weight(.semibold))
                Toggle(isOn: $persona.tools) { Text("Tool calls").font(.callout) }
                    .toggleStyle(.checkbox)
                    .disabled(!persona.modelId.hasPrefix("claude"))
                    .help(persona.modelId.hasPrefix("claude")
                          ? "Let this persona search the corpus live (kb_search / kb_get_paper) mid-answer"
                          : "Tool calls require a Claude model (Anthropic tool-use)")
                Toggle(isOn: $persona.queriesKB) { Text("Uses knowledge bank").font(.callout) }
                    .toggleStyle(.checkbox)
                Divider()
                Toggle(isOn: $persona.isFactChecker) { Text("Fact-checker role").font(.callout) }
                    .toggleStyle(.checkbox)
                    .help("In a roundtable, verifies the panel's claims against the corpus")
                Toggle(isOn: $persona.isSynth) { Text("Synthesizer role").font(.callout) }
                    .toggleStyle(.checkbox)
                    .help("In a roundtable, runs last and writes the final synthesis")
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}
