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

    /// Hunt the corpus for unsolved problems (limitations/future_work) paired
    /// with the nearest method/applications work elsewhere. `domain` focuses the
    /// hunt; omit it to scan broadly.
    func problems(domain: String? = nil, k: Int = 8) async throws -> ProblemsResponse {
        var body: [String: Any] = ["k": k]
        if let domain, !domain.isEmpty { body["domain"] = domain }
        return try await post("/problems", json: body)
    }

    /// Chat over the corpus. When `persona` is supplied, the engine answers in
    /// that persona's voice on its model (with its tool/KB settings) — the
    /// `@persona` chat mode — otherwise the default research-assistant chat.
    func chat(_ query: String, history: [ChatMessage], persona: Persona? = nil) async throws -> ChatResponse {
        var body: [String: Any] = [
            "query": query,
            "history": history.map { ["role": $0.role, "content": $0.content] },
        ]
        if let p = persona {
            body["persona"] = [
                "prompt": p.prompt, "model": p.modelId,
                "tools": p.tools, "queries_kb": p.queriesKB,
            ]
        }
        return try await post("/chat", json: body)
    }

    /// Streaming chat-over-corpus (`POST /chat/stream`, SSE). Same inputs as
    /// `chat`, but the answer arrives live: the engine emits `.searching`, then
    /// `.sources` once retrieval lands, then `.delta` token fragments, ending in
    /// `.done`. A server-side failure finishes the stream by throwing. Cancelling
    /// the consuming task tears down the connection. Used by the Explore canvas,
    /// which runs one stream per attached agent.
    func chatStream(_ query: String, history: [ChatMessage], persona: Persona? = nil)
        -> AsyncThrowingStream<ChatStreamEvent, Error>
    {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    var req = request(path: "/chat/stream", method: "POST")
                    req.setValue("application/json", forHTTPHeaderField: "Content-Type")
                    req.setValue("text/event-stream", forHTTPHeaderField: "Accept")
                    req.timeoutInterval = 300
                    var body: [String: Any] = [
                        "query": query,
                        "history": history.map { ["role": $0.role, "content": $0.content] },
                    ]
                    if let p = persona {
                        body["persona"] = [
                            "prompt": p.prompt, "model": p.modelId,
                            "tools": p.tools, "queries_kb": p.queriesKB,
                        ]
                    }
                    req.httpBody = try JSONSerialization.data(withJSONObject: body)
                    let (bytes, resp) = try await URLSession.shared.bytes(for: req)
                    guard let http = resp as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
                        throw KBError.server(status: (resp as? HTTPURLResponse)?.statusCode ?? -1,
                                             message: "chat stream request failed")
                    }
                    // One JSON object per `data:` line (same framing as brainstorm).
                    for try await line in bytes.lines {
                        guard line.hasPrefix("data:") else { continue }
                        let json = line.dropFirst(5).drop(while: { $0 == " " })
                        guard let d = String(json).data(using: .utf8),
                              let wire = try? Self.decoder.decode(ChatStreamWire.self, from: d)
                        else { continue }
                        switch wire.type {
                        case "searching": continuation.yield(.searching)
                        case "sources":   continuation.yield(.sources(wire.sources ?? []))
                        case "delta":     continuation.yield(.delta(wire.text ?? ""))
                        case "done":
                            continuation.yield(.done(answer: wire.answer ?? ""))
                            continuation.finish()
                            return
                        case "error":
                            continuation.finish(throwing: KBError.server(
                                status: 500, message: wire.message ?? "chat failed"))
                            return
                        default: break
                        }
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
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

    /// Save an idea note into the corpus (the engine re-embeds it, making it
    /// searchable). Used to capture a roundtable's synthesis back into the KB.
    @discardableResult
    func createIdea(title: String, body: String, tags: [String] = [], links: [String] = [], project: String? = nil) async throws -> String {
        struct Result: Decodable { var ok = false; var message = ""; var slug = "" }
        var json: [String: Any] = ["title": title, "body": body, "tags": tags, "links": links]
        if let project { json["project"] = project }
        let r: Result = try await post("/ideas", json: json)
        return r.message.isEmpty ? "Saved idea" : r.message
    }

    /// Ingest a new document into the corpus — the `kb add` flow, streamed.
    /// Pass exactly one of: an arXiv id/URL, a web page `url`, or a local
    /// `pdfPath`. The engine pushes `IngestEvent`s as Server-Sent Events while
    /// it downloads, chunks, and embeds; the run ends with one `.done` (or
    /// `.error`). Cancelling the consuming task tears down the connection.
    func ingestStream(arxiv: String? = nil, url: String? = nil, pdfPath: String? = nil) -> AsyncThrowingStream<IngestEvent, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    var req = request(path: "/ingest", method: "POST")
                    req.setValue("application/json", forHTTPHeaderField: "Content-Type")
                    req.setValue("text/event-stream", forHTTPHeaderField: "Accept")
                    req.timeoutInterval = 600   // download + chunk + embed can be slow
                    var body: [String: Any] = [:]
                    if let arxiv { body["arxiv"] = arxiv }
                    if let url { body["url"] = url }
                    if let pdfPath { body["pdf_path"] = pdfPath }
                    req.httpBody = try JSONSerialization.data(withJSONObject: body)
                    let (bytes, resp) = try await URLSession.shared.bytes(for: req)
                    guard let http = resp as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
                        throw KBError.server(status: (resp as? HTTPURLResponse)?.statusCode ?? -1,
                                             message: "ingest request failed")
                    }
                    // One JSON object per `data:` line (same framing as brainstorm).
                    for try await line in bytes.lines {
                        guard line.hasPrefix("data:") else { continue }
                        let json = line.dropFirst(5).drop(while: { $0 == " " })
                        if let d = String(json).data(using: .utf8),
                           let ev = try? Self.decoder.decode(IngestEvent.self, from: d) {
                            continuation.yield(ev)
                        }
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
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

    /// Stream a live brainstorming roundtable. The engine runs the panel of
    /// agents and pushes `RoundtableEvent`s as Server-Sent Events; each yielded
    /// value is one decoded event. `personas` is the wire payload (id/name/role/
    /// model/is_synth/queries_kb) the macOS panel built. Cancelling the consuming
    /// task tears down the HTTP connection.
    func brainstorm(objective: String, personas: [[String: Any]], rounds: Int, sessionId: String,
                    transcript: [String] = [], guidance: [String] = [], score: Bool = true,
                    converge: Bool = true, moderated: Bool = false,
                    targets: [String] = [], baseRound: Int? = nil) -> AsyncThrowingStream<RoundtableEvent, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    var req = request(path: "/brainstorm", method: "POST")
                    req.setValue("application/json", forHTTPHeaderField: "Content-Type")
                    req.setValue("text/event-stream", forHTTPHeaderField: "Accept")
                    req.timeoutInterval = 600
                    var body: [String: Any] = [
                        "objective": objective, "personas": personas, "rounds": rounds,
                        "session_id": sessionId, "score": score, "converge": converge,
                        "moderated": moderated,
                    ]
                    if !transcript.isEmpty { body["transcript"] = transcript }
                    if !guidance.isEmpty { body["guidance"] = guidance }
                    if !targets.isEmpty { body["targets"] = targets }
                    if let baseRound { body["base_round"] = baseRound }
                    req.httpBody = try JSONSerialization.data(withJSONObject: body)
                    let (bytes, resp) = try await URLSession.shared.bytes(for: req)
                    guard let http = resp as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
                        throw KBError.server(status: (resp as? HTTPURLResponse)?.statusCode ?? -1,
                                             message: "brainstorm request failed")
                    }
                    // Parse SSE. Each engine event is exactly one `data:` line of
                    // JSON, so decode and yield per line — no dependence on the
                    // blank separator line (which `.lines` may not surface).
                    // Lines that aren't `data:` (`:` keep-alives, blanks) are skipped.
                    for try await line in bytes.lines {
                        guard line.hasPrefix("data:") else { continue }
                        let json = line.dropFirst(5).drop(while: { $0 == " " })
                        if let d = String(json).data(using: .utf8),
                           let ev = try? Self.decoder.decode(RoundtableEvent.self, from: d) {
                            continuation.yield(ev)
                        }
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    /// Push a guiding idea into a running roundtable (best-effort — a finished
    /// or unknown session just 404s, which we ignore).
    func interject(sessionId: String, text: String) async {
        var req = request(path: "/brainstorm/\(encode(sessionId))/interject", method: "POST")
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.httpBody = try? JSONSerialization.data(withJSONObject: ["text": text])
        _ = try? await send(req)
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
