import Foundation

// Durable highlights, stored as a per-paper JSON sidecar in the app's
// Application Support directory. We deliberately do NOT write annotations back
// into `paper.pdf` — that file is canonical and must never be mutated (KB
// invariant: files are forever). A highlight is a set of page-space quads (one
// per selected line) plus the captured text.
struct StoredHighlight: Codable, Identifiable {
    let id: UUID
    var text: String
    var createdAt: Date
    var quads: [Quad]

    struct Quad: Codable {
        let page: Int
        let x, y, w, h: Double
    }
}

final class HighlightStore {
    static let shared = HighlightStore()

    private let dir: URL
    private let queue = DispatchQueue(label: "kb.highlights")

    private init() {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
        dir = base.appendingPathComponent("com.sunprema.kb/highlights", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    }

    func highlights(for paperId: String) -> [StoredHighlight] {
        queue.sync {
            guard let data = try? Data(contentsOf: url(paperId)),
                  let list = try? JSONDecoder().decode([StoredHighlight].self, from: data)
            else { return [] }
            return list
        }
    }

    func add(_ highlight: StoredHighlight, for paperId: String) {
        mutate(paperId) { $0.append(highlight) }
    }

    func remove(_ id: UUID, for paperId: String) {
        mutate(paperId) { $0.removeAll { $0.id == id } }
    }

    private func mutate(_ paperId: String, _ change: (inout [StoredHighlight]) -> Void) {
        queue.sync {
            var list = (try? Data(contentsOf: url(paperId)))
                .flatMap { try? JSONDecoder().decode([StoredHighlight].self, from: $0) } ?? []
            change(&list)
            if let data = try? JSONEncoder().encode(list) {
                try? data.write(to: url(paperId), options: .atomic)
            }
        }
    }

    private func url(_ paperId: String) -> URL {
        let safe = paperId.replacingOccurrences(of: "/", with: "_")
        return dir.appendingPathComponent("\(safe).json")
    }
}
