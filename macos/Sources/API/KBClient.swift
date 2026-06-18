import Foundation

// Typed async client over the loopback `kb serve` HTTP API. Immutable and
// Sendable: holds the base URL + API key and stamps `X-KB-Key` on every
// request. One instance is created once the engine reports healthy.
struct KBClient: Sendable {
    let baseURL: URL
    let apiKey: String

    private static let decoder: JSONDecoder = {
        let d = JSONDecoder()
        d.keyDecodingStrategy = .convertFromSnakeCase
        return d
    }()

    // MARK: Endpoints

    func health() async throws -> Bool {
        let (_, resp) = try await send(request(path: "/health", auth: false))
        return (resp as? HTTPURLResponse)?.statusCode == 200
    }

    func stats() async throws -> Stats {
        try await get("/stats")
    }

    func papers(tag: String? = nil, category: String? = nil) async throws -> [PaperMetadata] {
        var items: [URLQueryItem] = []
        if let tag { items.append(.init(name: "tag", value: tag)) }
        if let category { items.append(.init(name: "category", value: category)) }
        return try await get("/papers", query: items)
    }

    func paper(_ id: String) async throws -> PaperDetail {
        try await get("/papers/\(encode(id))")
    }

    func similar(_ id: String, limit: Int = 8) async throws -> SimilarResponse {
        try await get("/papers/\(encode(id))/similar", query: [.init(name: "limit", value: String(limit))])
    }

    func search(_ query: String, mode: SearchMode, k: Int? = nil, filters: SearchFilters = .init()) async throws -> SearchResponse {
        var body: [String: Any] = ["query": query, "mode": mode.rawValue]
        if let k { body["k"] = k }
        if let s = filters.sectionTypes, !s.isEmpty { body["section_types"] = s }
        if let t = filters.tags, !t.isEmpty { body["tags"] = t }
        if let kind = filters.kind { body["kind"] = kind }
        return try await post("/search", json: body)
    }

    func chat(_ query: String, history: [ChatMessage]) async throws -> ChatResponse {
        try await post("/chat", json: ["query": query, "history": history.map { ["role": $0.role, "content": $0.content] }])
    }

    /// Append a note to a paper (the engine re-embeds it, making it searchable).
    /// Returns the engine's status message.
    @discardableResult
    func addNote(_ paperId: String, note: String) async throws -> String {
        struct Result: Decodable { var ok = false; var message = "" }
        let r: Result = try await post("/papers/\(encode(paperId))/notes", json: ["note": note])
        return r.message
    }

    /// Overwrite a paper's notes (editable-notes editor); the engine re-embeds.
    @discardableResult
    func putNotes(_ paperId: String, notes: String) async throws -> String {
        struct Result: Decodable { var ok = false; var message = "" }
        let r: Result = try await put("/papers/\(encode(paperId))/notes", json: ["note": notes])
        return r.message
    }

    func graph(neighbors: Int = 3) async throws -> GraphResponse {
        try await get("/graph", query: [.init(name: "neighbors", value: String(neighbors))])
    }

    func sparks(limit: Int = 0, kind: String? = nil) async throws -> SparksResponse {
        var items: [URLQueryItem] = []
        if limit > 0 { items.append(.init(name: "limit", value: String(limit))) }
        if let kind { items.append(.init(name: "kind", value: kind)) }
        return try await get("/sparks", query: items)
    }

    func pdfData(_ paperId: String) async throws -> Data {
        let (data, resp) = try await send(request(path: "/pdf/\(encode(paperId))"))
        try ensureOK(resp, data)
        return data
    }

    /// `GET /open/{chunk_id}` resolves to a 302 the CLI follows; the app keeps
    /// the redirect target instead so it can route to the in-app reader.
    func chunkText(_ chunkId: String) async throws -> String {
        struct Chunk: Decodable { let text: String }
        let c: Chunk = try await get("/chunks/\(encode(chunkId))")
        return c.text
    }

    // MARK: Plumbing

    private func get<T: Decodable>(_ path: String, query: [URLQueryItem] = []) async throws -> T {
        let (data, resp) = try await send(request(path: path, query: query))
        try ensureOK(resp, data)
        return try Self.decoder.decode(T.self, from: data)
    }

    private func post<T: Decodable>(_ path: String, json: [String: Any]) async throws -> T {
        try await body(path, method: "POST", json: json)
    }

    private func put<T: Decodable>(_ path: String, json: [String: Any]) async throws -> T {
        try await body(path, method: "PUT", json: json)
    }

    private func body<T: Decodable>(_ path: String, method: String, json: [String: Any]) async throws -> T {
        var req = request(path: path, method: method)
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.httpBody = try JSONSerialization.data(withJSONObject: json)
        let (data, resp) = try await send(req)
        try ensureOK(resp, data)
        return try Self.decoder.decode(T.self, from: data)
    }

    private func request(path: String, method: String = "GET", query: [URLQueryItem] = [], auth: Bool = true) -> URLRequest {
        var comps = URLComponents(url: baseURL.appendingPathComponent(path), resolvingAgainstBaseURL: false)!
        if !query.isEmpty { comps.queryItems = query }
        var req = URLRequest(url: comps.url!)
        req.httpMethod = method
        req.timeoutInterval = 120   // chat/search hit the embedding API
        if auth { req.setValue(apiKey, forHTTPHeaderField: "X-KB-Key") }
        return req
    }

    private func send(_ req: URLRequest) async throws -> (Data, URLResponse) {
        do {
            return try await URLSession.shared.data(for: req)
        } catch {
            throw KBError.transport(error.localizedDescription)
        }
    }

    private func ensureOK(_ resp: URLResponse, _ data: Data) throws {
        guard let http = resp as? HTTPURLResponse else { throw KBError.transport("no response") }
        guard (200..<300).contains(http.statusCode) else {
            let msg = (try? JSONDecoder().decode([String: String].self, from: data))?["error"]
            throw KBError.server(status: http.statusCode, message: msg ?? "request failed")
        }
    }

    private func encode(_ s: String) -> String {
        s.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? s
    }
}

enum SearchMode: String, CaseIterable, Identifiable {
    case narrow, wide
    var id: String { rawValue }
    var label: String { self == .narrow ? "Narrow" : "Wide" }
    var help: String {
        self == .narrow ? "Precise lookups — high-confidence matches only"
                        : "Synthesis — casts a wider net across the corpus"
    }
}

struct SearchFilters {
    var sectionTypes: [String]? = nil
    var tags: [String]? = nil
    var kind: String? = nil
}

enum KBError: LocalizedError {
    case transport(String)
    case server(status: Int, message: String)

    var errorDescription: String? {
        switch self {
        case .transport(let m): return m
        case .server(let s, let m): return "\(m) (HTTP \(s))"
        }
    }
}
