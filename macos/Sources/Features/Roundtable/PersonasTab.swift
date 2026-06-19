import SwiftUI

// Edit the panel of agents: name, role, avatar, color, model, and flags. Add new
// specialists or remove them. Changes persist immediately (PersonaStore) and are
// snapshotted into a debate when it starts.
@MainActor
struct PersonasTab: View {
    @Bindable var store: PersonaStore

    var body: some View {
        ScrollView {
            VStack(spacing: 16) {
                header
                if !store.hasSynthesizer { synthWarning }
                ForEach($store.personas) { $persona in
                    PersonaEditorCard(persona: $persona) { store.delete(persona.id) }
                }
                Button { store.add() } label: {
                    Label("Add specialist", systemImage: "plus.circle.fill").frame(maxWidth: .infinity)
                }
                .buttonStyle(.bordered)
                .controlSize(.large)
            }
            .padding(24)
            .frame(maxWidth: 760)
            .frame(maxWidth: .infinity)
        }
    }

    private var header: some View {
        HStack(alignment: .top) {
            VStack(alignment: .leading, spacing: 4) {
                Text("Panel of agents").font(.title3.weight(.bold))
                Text("Each specialist debates with its own lens and model. The synthesizer runs last and writes the final summary.")
                    .font(.callout).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer()
            Button(role: .destructive) { store.resetToDefaults() } label: {
                Label("Reset", systemImage: "arrow.counterclockwise")
            }
            .help("Restore the default panel")
        }
    }

    private var synthWarning: some View {
        Label("No synthesizer in the panel — add one (toggle “Synthesizer” on an agent) so the debate gets a final summary.",
              systemImage: "exclamationmark.triangle.fill")
            .font(.caption).foregroundStyle(.orange)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(10)
            .background(.orange.opacity(0.12), in: RoundedRectangle(cornerRadius: Theme.corner))
    }
}

private struct PersonaEditorCard: View {
    @Binding var persona: Persona
    let onDelete: () -> Void

    var body: some View {
        Card {
            VStack(alignment: .leading, spacing: 14) {
                HStack(spacing: 12) {
                    iconMenu
                    VStack(alignment: .leading, spacing: 6) {
                        TextField("Name", text: $persona.name)
                            .textFieldStyle(.plain).font(.headline)
                        TextField("Role / lens (e.g. Business & GTM)", text: $persona.role)
                            .textFieldStyle(.plain).font(.subheadline).foregroundStyle(.secondary)
                    }
                    Spacer()
                    Button(role: .destructive, action: onDelete) {
                        Image(systemName: "trash")
                    }
                    .buttonStyle(.borderless)
                    .help("Remove this agent")
                }

                colorRow

                HStack(spacing: 16) {
                    modelMenu
                    Spacer()
                    Toggle(isOn: $persona.queriesKB) { Text("Uses KB").font(.caption) }
                        .toggleStyle(.checkbox)
                    Toggle(isOn: $persona.isFactChecker) { Text("Fact-checker").font(.caption) }
                        .toggleStyle(.checkbox)
                    Toggle(isOn: $persona.isSynth) { Text("Synthesizer").font(.caption) }
                        .toggleStyle(.checkbox)
                    Toggle(isOn: $persona.tools) { Text("Tools").font(.caption) }
                        .toggleStyle(.checkbox)
                        .disabled(!persona.modelId.hasPrefix("claude"))
                        .help(persona.modelId.hasPrefix("claude")
                              ? "Let this agent search the corpus live (kb_search / kb_get_paper) during its turn"
                              : "Tools require a Claude model (Anthropic tool-use)")
                }
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
                Circle().fill(persona.color.opacity(0.18)).frame(width: 44, height: 44)
                Image(systemName: persona.icon).font(.title3).foregroundStyle(persona.color)
            }
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .help("Choose an icon")
    }

    private var colorRow: some View {
        HStack(spacing: 8) {
            ForEach(PersonaPalette.options, id: \.name) { opt in
                Circle()
                    .fill(opt.color)
                    .frame(width: 20, height: 20)
                    .overlay {
                        Circle().stroke(.primary, lineWidth: persona.colorName == opt.name ? 2 : 0)
                    }
                    .overlay {
                        if persona.colorName == opt.name {
                            Image(systemName: "checkmark").font(.system(size: 9, weight: .bold))
                                .foregroundStyle(.white)
                        }
                    }
                    .onTapGesture { persona.colorName = opt.name }
            }
            Spacer()
        }
    }

    private var modelMenu: some View {
        Menu {
            ForEach(LLMModel.all) { m in
                Button { persona.modelId = m.id } label: {
                    Label("\(m.label) · \(m.provider.rawValue)", systemImage: m.providerGlyph)
                }
            }
        } label: {
            HStack(spacing: 5) {
                Image(systemName: persona.model.providerGlyph)
                    .foregroundStyle(persona.model.providerColor)
                Text(persona.model.label).font(.caption.weight(.medium))
                Image(systemName: "chevron.up.chevron.down").font(.caption2)
            }
        }
        .menuStyle(.borderlessButton)
        .fixedSize()
    }
}
