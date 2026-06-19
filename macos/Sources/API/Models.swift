import Foundation

// Codable mirrors of the engine's `Serialize` structs (src/search/retrieval.rs,
// src/cortex/mod.rs, src/lib.rs). Decoded with `.convertFromSnakeCase`, so Rust
// `snake_case` fields map to Swift `camelCase` here. Keep in lockstep with the
// engine — drift here is the likeliest source of bugs (see LOCAL_UI_PRD §3).

// MARK: - Ingest

/// The finished document from an ingest run (the SSE `done` frame).
struct IngestResult: Decodable {
    let ok: Bool
    let id: String
    let title: String
    let chunks: Int
    let sourceFormat: String
}

/// One Server-Sent frame from `POST /ingest`: live status, a final result, or
/// a failure. Mirrors the engine's tagged `IngestEvent` (src/server/http.rs).
enum IngestEvent: Decodable {
    case progress(String)
    case done(IngestResult)
    case error(String)

    private enum CodingKeys: String, CodingKey {
        case type, message, id, title, chunks, sourceFormat
    }
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        switch try c.decode(String.self, forKey: .type) {
        case "done":
            self = .done(IngestResult(
                ok: true,
                id: try c.decode(String.self, forKey: .id),
                title: try c.decode(String.self, forKey: .title),
                chunks: try c.decode(Int.self, forKey: .chunks),
                sourceFormat: try c.decode(String.self, forKey: .sourceFormat)))
        case "error":
            self = .error(try c.decode(String.self, forKey: .message))
        default:   // "progress"
            self = .progress(try c.decode(String.self, forKey: .message))
        }
    }
}

// MARK: - Papers

struct PaperMetadata: Codable, Identifiable, Hashable {
    let arxivId: String
    var kind: String = "paper"
    var version: String?
    let title: String
    var authors: [String] = []
    var abstract: String = ""
    var categories: [String] = []
    var publishedAt: String = ""
    var updatedAt: String?
    var ingestedAt: String?
    var sourceFormat: String?
    var tags: [String] = []

    var id: String { arxivId }

    enum CodingKeys: String, CodingKey {
        case arxivId, kind, version, title, authors, abstract
        case categories, publishedAt, updatedAt, ingestedAt, sourceFormat, tags
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        arxivId = try c.decode(String.self, forKey: .arxivId)
        title = try c.decode(String.self, forKey: .title)
        kind = (try? c.decode(String.self, forKey: .kind)) ?? "paper"
        version = try? c.decode(String.self, forKey: .version)
        authors = (try? c.decode([String].self, forKey: .authors)) ?? []
        abstract = (try? c.decode(String.self, forKey: .abstract)) ?? ""
        categories = (try? c.decode([String].self, forKey: .categories)) ?? []
        publishedAt = (try? c.decode(String.self, forKey: .publishedAt)) ?? ""
        updatedAt = try? c.decode(String.self, forKey: .updatedAt)
        ingestedAt = try? c.decode(String.self, forKey: .ingestedAt)
        sourceFormat = try? c.decode(String.self, forKey: .sourceFormat)
        tags = (try? c.decode([String].self, forKey: .tags)) ?? []
    }
}

struct PaperDetail: Codable {
    let metadata: PaperMetadata
    var notes: String = ""
    var pdfPath: String?
}

// MARK: - Search

struct SearchResponse: Codable {
    let query: String
    let mode: String
    var papers: [PaperGroup] = []
    var totalChunks: Int = 0
}

struct PaperGroup: Codable, Identifiable {
    let paperId: String
    let bestScore: Float
    var matchedSections: [String] = []
    var chunks: [ChunkHit] = []
    let paper: PaperInfo
    var tags: [String] = []

    var id: String { paperId }
}

struct ChunkHit: Codable, Identifiable {
    let chunkId: String
    let sectionType: String
    let score: Float
    let snippet: String
    var page: Int?
    var deepLink: String = ""

    var id: String { chunkId }
}

struct PaperInfo: Codable {
    var kind: String = "paper"
    var project: String?
    let title: String
    var authors: [String] = []
    var abstract: String = ""
    var categories: [String] = []
    var publishedAt: String = ""
}

// MARK: - Chat

struct ChatMessage: Codable, Hashable {
    let role: String     // "user" | "assistant"
    let content: String
}

struct ChatRequest: Codable {
    let query: String
    let history: [ChatMessage]
}

struct ChatResponse: Codable {
    let answer: String
    var sources: [ChatSource] = []
}

struct ChatSource: Codable, Identifiable {
    let n: Int
    let paperId: String
    let title: String
    let sectionType: String
    var page: Int?
    let chunkId: String
    var snippet: String = ""
    var hasPdf: Bool = false

    var id: Int { n }
}

// MARK: - Problems (ResearchAgent)

struct ProblemsResponse: Codable {
    var domain: String?
    var problems: [ProblemCandidate] = []
}

struct ProblemCandidate: Codable, Identifiable {
    let problemChunkId: String
    let problemPaperId: String
    let problemTitle: String
    let sectionType: String       // "limitations" | "future_work"
    let statement: String
    var page: Int?
    var deepLink: String = ""
    let gapType: String           // "greenfield" | "synthesis_opportunity"
    var solutions: [SolutionHit] = []

    var id: String { problemChunkId }
}

struct SolutionHit: Codable, Identifiable {
    let paperId: String
    let title: String
    let chunkId: String
    let sectionType: String
    let score: Float
    var snippet: String = ""
    var page: Int?
    var deepLink: String = ""

    var id: String { chunkId }
}

// MARK: - Similar

struct SimilarResponse: Codable {
    let paperId: String
    var papers: [SimilarPaper] = []
}

struct SimilarPaper: Codable, Identifiable {
    let paperId: String
    let score: Float
    let title: String
    var authors: [String] = []
    var kind: String = "paper"
    var categories: [String] = []
    var tags: [String] = []
    var publishedAt: String = ""
    var hasPdf: Bool = false

    var id: String { paperId }
}

// MARK: - Sparks (Cortex)

struct SparksResponse: Codable {
    var sparks: [Spark] = []
}

struct Spark: Codable, Identifiable {
    let kind: String
    var directed: Bool = false
    let surprise: Float
    let similarity: Float
    let src: SparkEnd
    let dst: SparkEnd

    var id: String { "\(src.chunkId)→\(dst.chunkId)" }
}

struct SparkEnd: Codable {
    let paperId: String
    let title: String
    let sectionType: String
    let chunkId: String
    var snippet: String = ""
}

// MARK: - Knowledge graph

struct GraphResponse: Codable {
    var nodes: [GraphNode] = []
    var edges: [GraphEdge] = []
}

struct GraphNode: Codable, Identifiable {
    let id: String
    let title: String
    var kind: String = "paper"
    var project: String?
    var tags: [String] = []
    var categories: [String] = []
    var publishedAt: String = ""
    var chunks: Int = 0
}

struct GraphEdge: Codable {
    let source: String
    let target: String
    let kind: String     // "link" | "similar"
    var weight: Float = 1
}

// MARK: - Stats

struct Stats: Codable {
    var papers: Int = 0
    var tags: [String: Int] = [:]
    var db: DB = DB()

    struct DB: Codable {
        var chunks: Int = 0
        var papers: Int = 0
    }
}
