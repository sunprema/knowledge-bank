import SwiftUI

// The Personas studio: a library of reusable AI agents. Master list on the left
// (search, add, duplicate, delete), the rich editor on the right. Personas live
// in one shared, persisted store (PersonaStore) and are used across the
// Roundtable and @persona chat — this screen is where they're authored.
@MainActor
struct PersonasView: View {
    @Environment(PersonaStore.self) private var store

    @State private var selection: String?
    @State private var query = ""

    private var filtered: [Persona] {
        let q = query.trimmingCharacters(in: .whitespaces).lowercased()
        guard !q.isEmpty else { return store.personas }
        return store.personas.filter {
            $0.name.lowercased().contains(q) || $0.role.lowercased().contains(q)
            || $0.prompt.lowercased().contains(q)
        }
    }

    var body: some View {
        HSplitView {
            sidebar
                .frame(minWidth: 240, idealWidth: 280, maxWidth: 360)
            detail
                .frame(minWidth: 460)
                .layoutPriority(1)
        }
        .navigationTitle("Personas")
        .onAppear { if selection == nil { selection = store.personas.first?.id } }
    }

    // MARK: Master list

    private var sidebar: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: "magnifyingglass").foregroundStyle(.secondary).font(.caption)
                TextField("Search personas", text: $query).textFieldStyle(.plain)
            }
            .padding(8)
            .background(.background.secondary, in: RoundedRectangle(cornerRadius: 8))
            .padding(10)

            Divider()

            ScrollView {
                LazyVStack(spacing: 6) {
                    ForEach(filtered) { p in
                        PersonaRow(persona: p, selected: selection == p.id)
                            .contentShape(Rectangle())
                            .onTapGesture { selection = p.id }
                            .contextMenu {
                                Button { duplicate(p.id) } label: { Label("Duplicate", systemImage: "plus.square.on.square") }
                                Button(role: .destructive) { delete(p.id) } label: { Label("Delete", systemImage: "trash") }
                            }
                    }
                    if filtered.isEmpty {
                        Text(query.isEmpty ? "No personas yet" : "No matches")
                            .font(.callout).foregroundStyle(.tertiary).padding(.top, 30)
                    }
                }
                .padding(10)
            }

            Divider()
            HStack(spacing: 8) {
                Button { addPersona() } label: { Label("New", systemImage: "plus") }
                Spacer()
                Button(role: .destructive) {
                    store.resetToDefaults()
                    selection = store.personas.first?.id
                } label: { Label("Reset", systemImage: "arrow.counterclockwise") }
                    .help("Restore the default panel")
            }
            .buttonStyle(.borderless)
            .font(.callout)
            .padding(.horizontal, 12).padding(.vertical, 8)
        }
        .background(.background)
    }

    // MARK: Detail

    @ViewBuilder
    private var detail: some View {
        if let id = selection, store.personas.contains(where: { $0.id == id }) {
            PersonaEditor(
                persona: binding(for: id),
                onDuplicate: { duplicate(id) },
                onDelete: { delete(id) })
            .id(id)
        } else {
            EmptyStateView(
                icon: "person.crop.rectangle.stack",
                title: "Personas",
                message: "Reusable AI agents you can drop into a Roundtable or address in chat with @name. Create one to get started.")
            .overlay(alignment: .bottom) {
                Button { addPersona() } label: { Label("New persona", systemImage: "plus.circle.fill") }
                    .buttonStyle(.borderedProminent).controlSize(.large).padding(.bottom, 60)
            }
        }
    }

    /// A read/write binding into the store's persona with this id. Mutations go
    /// through `store.personas`, whose `didSet` persists to disk.
    private func binding(for id: String) -> Binding<Persona> {
        Binding(
            get: { store.personas.first(where: { $0.id == id }) ?? Persona.you },
            set: { newValue in
                if let idx = store.personas.firstIndex(where: { $0.id == id }) {
                    store.personas[idx] = newValue
                }
            })
    }

    // MARK: Actions

    private func addPersona() {
        selection = store.add()
        query = ""
    }

    private func duplicate(_ id: String) {
        if let newId = store.duplicate(id) { selection = newId }
    }

    private func delete(_ id: String) {
        let remaining = store.personas.filter { $0.id != id }
        store.delete(id)
        if selection == id { selection = remaining.first?.id }
    }
}

// MARK: - Row

private struct PersonaRow: View {
    let persona: Persona
    let selected: Bool

    var body: some View {
        HStack(spacing: 10) {
            ZStack {
                Circle().fill(persona.color.opacity(0.18)).frame(width: 34, height: 34)
                Image(systemName: persona.icon).foregroundStyle(persona.color)
            }
            VStack(alignment: .leading, spacing: 2) {
                Text(persona.name).font(.subheadline.weight(.semibold)).lineLimit(1)
                Text(persona.role).font(.caption).foregroundStyle(.secondary).lineLimit(1)
            }
            Spacer()
            VStack(alignment: .trailing, spacing: 3) {
                Image(systemName: persona.model.providerGlyph)
                    .font(.system(size: 9)).foregroundStyle(persona.model.providerColor)
                if persona.tools {
                    Image(systemName: "wrench.and.screwdriver.fill")
                        .font(.system(size: 8)).foregroundStyle(.purple)
                }
            }
        }
        .padding(.horizontal, 10).padding(.vertical, 8)
        .background {
            RoundedRectangle(cornerRadius: 8)
                .fill(selected ? Color.accentColor.opacity(0.15) : .clear)
        }
        .overlay {
            RoundedRectangle(cornerRadius: 8)
                .stroke(selected ? Color.accentColor.opacity(0.4) : .clear, lineWidth: 0.5)
        }
    }
}
