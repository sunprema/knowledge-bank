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
    /// Web-page adds only: the page the document was ingested from.
    var sourceUrl: String? = nil
}

/// One Server-Sent frame from `POST /ingest`: live status, a final result, or
/// a failure. Mirrors the engine's tagged `IngestEvent` (src/server/http.rs).
enum IngestEvent: Decodable {
    case progress(String)
    case done(IngestResult)
    case error(String)

    private enum CodingKeys: String, CodingKey {
        case type, message, id, title, chunks, sourceFormat, sourceUrl
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
                sourceFormat: try c.decode(String.self, forKey: .sourceFormat),
                sourceUrl: try? c.decode(String.self, forKey: .sourceUrl)))
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
    /// Web pages only (`kb add --url`): the page this was ingested from. `nil`
    /// for arXiv/PDF. Maps the engine's `source_url` (src/lib.rs).
    var sourceUrl: String?
    var tags: [String] = []

    var id: String { arxivId }

    enum CodingKeys: String, CodingKey {
        case arxivId, kind, version, title, authors, abstract
        case categories, publishedAt, updatedAt, ingestedAt, sourceFormat, sourceUrl, tags
    }

    /// Build a lightweight metadata value from data we already have (e.g. a
    /// graph node), enough to drive a cover + preview header before the full
    /// `PaperDetail` (authors, abstract) is fetched.
    init(id: String, kind: String = "paper", title: String,
         authors: [String] = [], abstract: String = "",
         categories: [String] = [], publishedAt: String = "", tags: [String] = []) {
        self.arxivId = id
        self.kind = kind
        self.title = title
        self.authors = authors
        self.abstract = abstract
        self.categories = categories
        self.publishedAt = publishedAt
        self.tags = tags
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
        sourceUrl = try? c.decode(String.self, forKey: .sourceUrl)
        tags = (try? c.decode([String].self, forKey: .tags)) ?? []
    }
}

struct PaperDetail: Codable {
    let metadata: PaperMetadata
    var notes: String = ""
    var pdfPath: String?
    /// Whether a cached Clean Read (`reader.md`) exists for this paper.
    var hasReader: Bool = false
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

struct ChatSource: Codable, Identifiable, Hashable {
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

// MARK: - Streaming chat (`POST /chat/stream`)

/// A decoded event from the streaming chat endpoint (see `KBClient.chatStream`).
enum ChatStreamEvent {
    case searching                 // retrieval started
    case sources([ChatSource])     // citations, emitted once
    case delta(String)             // a fragment of the answer, in order
    case done(answer: String)      // final full answer
}

/// Wire shape of one `/chat/stream` SSE event — `type`-tagged, with the field
/// for that variant present. Decoded then mapped to `ChatStreamEvent`.
struct ChatStreamWire: Codable {
    let type: String
    var sources: [ChatSource]? = nil
    var text: String? = nil
    var answer: String? = nil
    var message: String? = nil
}

// MARK: - Streaming Clean Read (`POST /papers/{id}/reader`)

/// A decoded event from the streaming Clean Read generator (see
/// `KBClient.readerStream`).
enum ReaderStreamEvent {
    case generating              // generation started
    case delta(String)           // a fragment of the rewrite, in order
    case done(reader: String)    // final full markdown (also now cached on disk)
}

/// Wire shape of one `/papers/{id}/reader` SSE event — `type`-tagged, mirroring
/// `ChatStreamWire`. Decoded then mapped to `ReaderStreamEvent`.
struct ReaderStreamWire: Codable {
    let type: String
    var text: String? = nil
    var reader: String? = nil
    var message: String? = nil
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

// MARK: - ArXiv Watch + Daily Brief

/// A standing interest that grows the corpus (an arXiv category, author, or
/// free-text query).
struct Watch: Codable, Identifiable {
    let id: Int
    let kind: String
    let value: String
    var enabled: Bool = true
    var createdAt: String = ""
    var lastRefreshedAt: String?
}

/// What a `feed refresh` did, returned by `POST /watch/refresh`.
struct RefreshSummary: Codable {
    var watchesRefreshed: Int = 0
    var fetched: Int = 0
    var newCandidates: Int = 0
    var errors: [String] = []
}

/// A candidate paper surfaced by a watch, scored by its connection to the
/// corpus. `why` carries the connecting papers/reflections behind the score.
struct WatchCandidate: Codable, Identifiable {
    let arxivId: String
    let title: String
    var abstract: String = ""
    var authors: [String] = []
    var categories: [String] = []
    var publishedAt: String = ""
    let score: Float
    var why: CandidateWhy = .init()
    var status: String = "new"

    var id: String { arxivId }
}

struct CandidateWhy: Codable {
    var connections: [Connection] = []
    var connectsToSynthesis: Bool = false

    struct Connection: Codable, Identifiable {
        let paperId: String
        var title: String = ""
        var kind: String = "paper"
        var score: Float = 0
        var sections: [String] = []

        var id: String { paperId }
    }
}

/// A past reflection/idea resurfaced in the brief (rotated over time).
struct Resurfaced: Codable {
    let paperId: String
    var kind: String = "reflection"
    var title: String = ""
    var snippet: String = ""
}

struct BriefStats: Codable {
    var papers: Int = 0
    var newCandidates: Int = 0
    var watches: Int = 0
}

/// One surprising connection teaser in the brief (a lighter shape than the
/// full Sparks view's `Spark`).
struct BriefSpark: Codable, Identifiable {
    let kind: String
    var surprise: Float = 0
    let src: End
    let dst: End

    var id: String { "\(src.paper)→\(dst.paper):\(kind)" }

    struct End: Codable {
        let paper: String
        var section: String = ""
        var snippet: String = ""
    }
}

/// The assembled daily brief — the app's landing surface.
struct Brief: Codable {
    var generatedAt: String = ""
    var newPapers: [WatchCandidate] = []
    var sparks: [BriefSpark] = []
    var resurfaced: Resurfaced?
    var stats: BriefStats = .init()
}
