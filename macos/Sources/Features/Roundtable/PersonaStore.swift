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

    func add() {
        personas.append(Persona(
            name: "New Specialist", role: "Lens / Role",
            icon: "person.fill.questionmark", colorName: "blue",
            modelId: LLMModel.gpt4o.id, isSynth: false, queriesKB: true))
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
