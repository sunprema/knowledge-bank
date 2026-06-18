import Foundation

// A single chat turn, persistable so conversations survive across launches.
// Top-level (not nested in ChatView) so the store can serialize it directly.
struct ChatTurn: Identifiable, Codable {
    enum Role: String, Codable { case user, assistant }
    var id = UUID()
    let role: Role
    let text: String
    var sources: [ChatSource] = []
}

// One saved conversation: its turns plus metadata for the history list.
struct StoredConversation: Codable, Identifiable {
    let id: UUID
    var title: String
    var createdAt: Date
    var updatedAt: Date
    var turns: [ChatTurn]

    /// A human title derived from the first user message (fallback "New chat").
    static func title(from turns: [ChatTurn]) -> String {
        let first = turns.first { $0.role == .user }?.text
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard let first, !first.isEmpty else { return "New chat" }
        return String(first.prefix(60))
    }
}

// Durable chat history, one JSON file per conversation under the app's
// Application Support directory (mirrors HighlightStore). Empty conversations
// are never written, and saving an existing id overwrites in place.
final class ConversationStore {
    static let shared = ConversationStore()

    private let dir: URL
    private let queue = DispatchQueue(label: "kb.conversations")

    private init() {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
        dir = base.appendingPathComponent("com.sunprema.kb/conversations", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    }

    /// All conversations, most recently updated first.
    func all() -> [StoredConversation] {
        queue.sync {
            let files = (try? FileManager.default.contentsOfDirectory(at: dir,
                includingPropertiesForKeys: nil)) ?? []
            let convos = files
                .filter { $0.pathExtension == "json" }
                .compactMap { try? Data(contentsOf: $0) }
                .compactMap { try? JSONDecoder().decode(StoredConversation.self, from: $0) }
            return convos.sorted { $0.updatedAt > $1.updatedAt }
        }
    }

    /// Persist (insert or replace) a conversation. No-ops on empty turns.
    func save(id: UUID, turns: [ChatTurn], createdAt: Date) {
        guard !turns.isEmpty else { return }
        let convo = StoredConversation(id: id,
                                       title: StoredConversation.title(from: turns),
                                       createdAt: createdAt,
                                       updatedAt: Date(),
                                       turns: turns)
        queue.sync {
            if let data = try? JSONEncoder().encode(convo) {
                try? data.write(to: url(id), options: .atomic)
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
