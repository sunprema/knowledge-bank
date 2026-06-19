import Foundation

// Durable research history, one JSON file per roundtable under Application
// Support (mirrors ConversationStore). Lets completed and in-progress debates
// be reopened and continued anytime.
final class RoundtableStore {
    static let shared = RoundtableStore()

    private let dir: URL
    private let queue = DispatchQueue(label: "kb.roundtables")

    private init() {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
        dir = base.appendingPathComponent("com.sunprema.kb/roundtables", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    }

    /// All saved roundtables, most recently updated first.
    func all() -> [RoundtableRecord] {
        queue.sync {
            let files = (try? FileManager.default.contentsOfDirectory(at: dir,
                includingPropertiesForKeys: nil)) ?? []
            return files
                .filter { $0.pathExtension == "json" }
                .compactMap { try? Data(contentsOf: $0) }
                .compactMap { try? JSONDecoder().decode(RoundtableRecord.self, from: $0) }
                .sorted { $0.updatedAt > $1.updatedAt }
        }
    }

    /// Insert or replace. No-ops on an empty debate.
    func save(_ record: RoundtableRecord) {
        guard !record.turns.isEmpty else { return }
        queue.sync {
            if let data = try? JSONEncoder().encode(record) {
                try? data.write(to: url(record.id), options: .atomic)
            }
        }
    }

    func delete(_ id: UUID) {
        queue.sync { try? FileManager.default.removeItem(at: url(id)) }
    }

    private func url(_ id: UUID) -> URL {
        dir.appendingPathComponent("\(id.uuidString).json")
    }
}
