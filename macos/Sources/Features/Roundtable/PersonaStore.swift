import Foundation
import Observation

// The editable panel of personas, persisted to disk so customizations survive
// launches. Edited in the Roundtable screen's Personas tab; snapshotted into a
// session when a debate starts.
@MainActor
@Observable
final class PersonaStore {
    var personas: [Persona] {
        didSet { persist() }
    }

    private let url: URL

    init() {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
        let dir = base.appendingPathComponent("com.sunprema.kb", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        url = dir.appendingPathComponent("personas.json")

        if let data = try? Data(contentsOf: url),
           let saved = try? JSONDecoder().decode([Persona].self, from: data),
           !saved.isEmpty {
            personas = saved          // didSet does not fire on init assignment
        } else {
            personas = Persona.defaultPanel
        }
    }

    /// At least one synthesizer should exist; the engine runs it last.
    var hasSynthesizer: Bool { personas.contains { $0.isSynth } }

    /// Create a fresh blank persona and return its id (so a UI can select it).
    @discardableResult
    func add() -> String {
        let p = Persona(
            name: "New Persona", role: "Lens / Role",
            prompt: "", icon: "person.fill.questionmark", colorName: "blue",
            modelId: LLMModel.opus.id, isSynth: false, queriesKB: true)
        personas.append(p)
        return p.id
    }

    /// Duplicate an existing persona (fresh id, "(copy)" name) and return its id.
    @discardableResult
    func duplicate(_ id: String) -> String? {
        guard let src = personas.first(where: { $0.id == id }) else { return nil }
        var copy = src
        copy.id = UUID().uuidString
        copy.name = "\(src.name) (copy)"
        if let idx = personas.firstIndex(where: { $0.id == id }) {
            personas.insert(copy, at: personas.index(after: idx))
        } else {
            personas.append(copy)
        }
        return copy.id
    }

    func delete(_ id: String) {
        personas.removeAll { $0.id == id }
    }

    func resetToDefaults() {
        personas = Persona.defaultPanel
    }

    private func persist() {
        if let data = try? JSONEncoder().encode(personas) {
            try? data.write(to: url, options: .atomic)
        }
    }
}
