import SwiftUI
import Observation

// MARK: - Conversation DAG

/// A side of a node card — where a connector lives and which way a branch grows.
enum NodeSide: String, Codable, CaseIterable {
    case top, bottom, leading, trailing

    /// Unit direction pointing away from the card on this side.
    var vector: CGVector {
        switch self {
        case .top:      CGVector(dx: 0, dy: -1)
        case .bottom:   CGVector(dx: 0, dy: 1)
        case .leading:  CGVector(dx: -1, dy: 0)
        case .trailing: CGVector(dx: 1, dy: 0)
        }
    }
    var opposite: NodeSide {
        switch self {
        case .top: .bottom; case .bottom: .top; case .leading: .trailing; case .trailing: .leading
        }
    }
    var isHorizontal: Bool { self == .leading || self == .trailing }
    var label: String {
        switch self { case .top: "Up"; case .bottom: "Down"; case .leading: "Left"; case .trailing: "Right" }
    }
    var glyph: String {
        switch self {
        case .top: "arrow.up"; case .bottom: "arrow.down"
        case .leading: "arrow.left"; case .trailing: "arrow.right"
        }
    }
}

/// An incoming edge: which parent it comes from, the user-given **edge name**
/// (the branch's lens, e.g. "venture capitalist view"), and the side of the
/// parent it departs from (so the edge and the child grow in that direction).
struct ParentLink: Codable, Equatable {
    var id: String
    var label: String = ""
    var side: NodeSide = .bottom
}

/// One node on the Explore canvas. The conversation is a *directed acyclic
/// graph*, not a list: a node's `links` are its incoming edges. A `.user`
/// prompt usually has one parent (the node it branches from), but a **join**
/// gives it several — and its context merges every parent branch's history. An
/// `.assistant` node is one agent's reply to a prompt; a single prompt fanned
/// out to N agents yields N sibling answers. Branches can leave any side of a
/// node, so the graph spreads in all four directions.
struct ConvNode: Identifiable, Codable, Equatable {
    enum Role: String, Codable { case user, assistant }
    enum Status: String, Codable { case streaming, done, error }

    var id: String = UUID().uuidString
    /// Incoming edges (labeled + directional). `parents` is the id-only view.
    var links: [ParentLink] = []
    var role: Role
    var text: String
    var createdAt: Date = Date()

    // Canvas placement (content-space). Stored as scalars so the node is plain
    // Codable; `point` is the SwiftUI view bridge.
    var x: Double
    var y: Double

    /// The side this node grows its own children toward (inherited from the
    /// branch that created it). Keeps a branch flowing in one direction.
    var branchSide: NodeSide = .bottom

    // Assistant attribution (the persona that produced this reply, if any).
    var personaId: String? = nil
    var personaName: String? = nil
    var personaIcon: String? = nil
    var personaColorName: String? = nil

    var sources: [ChatSource] = []
    var status: Status = .done

    var parents: [String] { links.map(\.id) }

    var point: CGPoint {
        get { CGPoint(x: x, y: y) }
        set { x = newValue.x; y = newValue.y }
    }

    var color: Color {
        role == .user ? .accentColor : PersonaPalette.color(personaColorName ?? "accent")
    }

    init(id: String = UUID().uuidString, links: [ParentLink] = [], role: Role, text: String,
         createdAt: Date = Date(), x: Double, y: Double, branchSide: NodeSide = .bottom,
         personaId: String? = nil, personaName: String? = nil, personaIcon: String? = nil,
         personaColorName: String? = nil, sources: [ChatSource] = [], status: Status = .done) {
        self.id = id; self.links = links; self.role = role; self.text = text
        self.createdAt = createdAt; self.x = x; self.y = y; self.branchSide = branchSide
        self.personaId = personaId; self.personaName = personaName
        self.personaIcon = personaIcon; self.personaColorName = personaColorName
        self.sources = sources; self.status = status
    }

    enum CodingKeys: String, CodingKey {
        case id, links, parents, role, text, createdAt, x, y, branchSide,
             personaId, personaName, personaIcon, personaColorName, sources, status
    }

    // Tolerant decode: boards saved before labeled edges stored `parents:
    // [String]` and no `branchSide`, so fall back to those.
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decodeIfPresent(String.self, forKey: .id) ?? UUID().uuidString
        if let l = try c.decodeIfPresent([ParentLink].self, forKey: .links) {
            links = l
        } else if let p = try c.decodeIfPresent([String].self, forKey: .parents) {
            links = p.map { ParentLink(id: $0) }
        }
        role = try c.decode(Role.self, forKey: .role)
        text = try c.decode(String.self, forKey: .text)
        createdAt = try c.decodeIfPresent(Date.self, forKey: .createdAt) ?? Date()
        x = try c.decode(Double.self, forKey: .x)
        y = try c.decode(Double.self, forKey: .y)
        branchSide = try c.decodeIfPresent(NodeSide.self, forKey: .branchSide) ?? .bottom
        personaId = try c.decodeIfPresent(String.self, forKey: .personaId)
        personaName = try c.decodeIfPresent(String.self, forKey: .personaName)
        personaIcon = try c.decodeIfPresent(String.self, forKey: .personaIcon)
        personaColorName = try c.decodeIfPresent(String.self, forKey: .personaColorName)
        sources = try c.decodeIfPresent([ChatSource].self, forKey: .sources) ?? []
        status = try c.decodeIfPresent(Status.self, forKey: .status) ?? .done
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(id, forKey: .id)
        try c.encode(links, forKey: .links)
        try c.encode(role, forKey: .role)
        try c.encode(text, forKey: .text)
        try c.encode(createdAt, forKey: .createdAt)
        try c.encode(x, forKey: .x)
        try c.encode(y, forKey: .y)
        try c.encode(branchSide, forKey: .branchSide)
        try c.encodeIfPresent(personaId, forKey: .personaId)
        try c.encodeIfPresent(personaName, forKey: .personaName)
        try c.encodeIfPresent(personaIcon, forKey: .personaIcon)
        try c.encodeIfPresent(personaColorName, forKey: .personaColorName)
        try c.encode(sources, forKey: .sources)
        try c.encode(status, forKey: .status)
    }
}

/// One saved exploration: the node graph, view transform, and list metadata —
/// the canvas analogue of `StoredConversation`. Persisted one file per board so
/// explorations are storable and reopenable from history.
struct StoredExplore: Codable, Identifiable {
    let id: UUID
    var title: String
    var createdAt: Date
    var updatedAt: Date
    var nodes: [ConvNode]
    var offsetWidth: Double
    var offsetHeight: Double
    var zoom: Double

    /// A human title from the first user prompt (fallback "New exploration").
    static func title(from nodes: [ConvNode]) -> String {
        let first = nodes
            .filter { $0.role == .user }
            .min(by: { $0.createdAt < $1.createdAt })?
            .text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let first, !first.isEmpty else { return "New exploration" }
        return String(first.prefix(60))
    }
}

// MARK: - Archive (durable, multi-board)

/// Durable store of saved explorations — one JSON file per board under the app's
/// Application Support directory (mirrors `ConversationStore`). Empty boards are
/// never written; saving an existing id overwrites in place.
final class ExploreArchive {
    static let shared = ExploreArchive()

    private let dir: URL
    private let queue = DispatchQueue(label: "kb.explores")

    private init() {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
        dir = base.appendingPathComponent("com.sunprema.kb/explores", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        migrateLegacyBoardIfNeeded(base: base.appendingPathComponent("com.sunprema.kb", isDirectory: true))
    }

    /// All explorations, most recently updated first.
    func all() -> [StoredExplore] {
        queue.sync {
            let files = (try? FileManager.default.contentsOfDirectory(at: dir, includingPropertiesForKeys: nil)) ?? []
            return files
                .filter { $0.pathExtension == "json" }
                .compactMap { try? Data(contentsOf: $0) }
                .compactMap { try? JSONDecoder().decode(StoredExplore.self, from: $0) }
                .sorted { $0.updatedAt > $1.updatedAt }
        }
    }

    /// Persist (insert or replace). No-ops on an empty board so blank canvases
    /// don't litter the history.
    func save(_ board: StoredExplore) {
        guard !board.nodes.isEmpty else { return }
        queue.sync {
            if let data = try? JSONEncoder().encode(board) {
                try? data.write(to: url(board.id), options: .atomic)
            }
        }
    }

    func delete(_ id: UUID) {
        queue.sync { try? FileManager.default.removeItem(at: url(id)) }
    }

    private func url(_ id: UUID) -> URL {
        dir.appendingPathComponent("\(id.uuidString).json")
    }

    /// One-time import of the pre-multi-board single file into a real board so
    /// the user's in-progress canvas isn't lost.
    private func migrateLegacyBoardIfNeeded(base: URL) {
        let legacy = base.appendingPathComponent("explore-board.json")
        guard FileManager.default.fileExists(atPath: legacy.path) else { return }
        struct Legacy: Codable { var nodes: [ConvNode] = []; var offsetWidth = 0.0; var offsetHeight = 0.0; var zoom = 1.0 }
        if let data = try? Data(contentsOf: legacy),
           let old = try? JSONDecoder().decode(Legacy.self, from: data),
           !old.nodes.isEmpty {
            let rec = StoredExplore(id: UUID(), title: StoredExplore.title(from: old.nodes),
                                    createdAt: Date(), updatedAt: Date(), nodes: old.nodes,
                                    offsetWidth: old.offsetWidth, offsetHeight: old.offsetHeight, zoom: old.zoom)
            if let d = try? JSONEncoder().encode(rec) { try? d.write(to: url(rec.id), options: .atomic) }
        }
        try? FileManager.default.removeItem(at: legacy)
    }
}

// MARK: - Working model

/// The live Explore canvas: one exploration's conversation graph, selection, and
/// view transform, bound to the views. Structural changes (add / finish /
/// drag-end) snapshot to the `ExploreArchive`; streaming deltas mutate in place
/// without hitting disk.
@MainActor
@Observable
final class ExploreStore {
    private(set) var id = UUID()
    private(set) var createdAt = Date()

    private(set) var nodes: [ConvNode] = []
    /// Selected nodes — the parents a new prompt will continue from (a join when
    /// more than one). Empty ⇒ the next prompt starts a fresh root topic.
    var selection: Set<String> = []

    // View transform (persisted on gesture end, not mid-pan).
    var offset: CGSize = .zero
    var zoom: CGFloat = 1

    private let layerGap: CGFloat = 180
    private let answerSpread: CGFloat = 320

    init() {}

    /// This board's list title, derived from its first prompt.
    var title: String { StoredExplore.title(from: nodes) }

    /// Replace the working state with a saved exploration.
    func load(_ rec: StoredExplore) {
        id = rec.id
        createdAt = rec.createdAt
        nodes = rec.nodes
        selection = []
        offset = CGSize(width: rec.offsetWidth, height: rec.offsetHeight)
        zoom = CGFloat(rec.zoom)
    }

    /// Start a fresh, empty exploration (the current one is already archived).
    func newBoard() {
        id = UUID()
        createdAt = Date()
        nodes = []
        selection = []
        offset = .zero
        zoom = 1
    }

    private func snapshot() -> StoredExplore {
        StoredExplore(id: id, title: title, createdAt: createdAt, updatedAt: Date(),
                      nodes: nodes, offsetWidth: offset.width, offsetHeight: offset.height,
                      zoom: Double(zoom))
    }

    // MARK: Lookups

    func node(_ id: String) -> ConvNode? { nodes.first { $0.id == id } }

    func children(of id: String) -> [ConvNode] {
        nodes.filter { $0.parents.contains(id) }
    }

    /// Every transitive ancestor of `id`, deduped and ordered oldest-first — the
    /// merged conversation history feeding a node (a join unions its branches).
    func ancestors(of id: String) -> [ConvNode] {
        var collected = Set<String>()
        var stack = node(id)?.parents ?? []
        while let pid = stack.popLast() {
            guard collected.insert(pid).inserted else { continue }
            stack.append(contentsOf: node(pid)?.parents ?? [])
        }
        return collected.compactMap { node($0) }.sorted { $0.createdAt < $1.createdAt }
    }

    /// The chat history for a prompt node: its ancestor path mapped to chat
    /// turns (skipping errored / empty replies). The prompt's own text is sent
    /// separately as the live query.
    func history(forPrompt id: String) -> [ChatMessage] {
        ancestors(of: id).compactMap { n in
            guard n.status != .error, !n.text.trimmingCharacters(in: .whitespaces).isEmpty else { return nil }
            return ChatMessage(role: n.role == .user ? "user" : "assistant", content: n.text)
        }
    }

    // MARK: Mutation

    /// Add a user prompt node from the given incoming edges (empty ⇒ a root
    /// topic). A single link branches in that link's direction (and the prompt
    /// inherits it as its `branchSide`); several links are a join, placed below.
    /// Returns its id.
    @discardableResult
    func addPrompt(_ text: String, links: [ParentLink]) -> String {
        let side: NodeSide = links.count == 1 ? links[0].side : .bottom
        let pos = promptPosition(links: links)
        let n = ConvNode(links: links, role: .user, text: text,
                         x: pos.x, y: pos.y, branchSide: side)
        nodes.append(n)
        save()
        return n.id
    }

    /// Add a streaming assistant node replying to `prompt`, fanned across
    /// `count` siblings (one per attached agent) along the prompt's branch
    /// direction. Returns its id.
    @discardableResult
    func addAnswer(prompt: String, persona: Persona?, index: Int, count: Int) -> String {
        let promptNode = node(prompt)
        let side = promptNode?.branchSide ?? .bottom
        let pos = answerPosition(prompt: promptNode, side: side, index: index, count: count)
        var n = ConvNode(links: [ParentLink(id: prompt, side: side)], role: .assistant,
                         text: "", x: pos.x, y: pos.y, branchSide: side, status: .streaming)
        if let p = persona {
            n.personaId = p.id
            n.personaName = p.name
            n.personaIcon = p.icon
            n.personaColorName = p.colorName
        }
        nodes.append(n)
        save()
        return n.id
    }

    /// Rename an incoming edge (the branch lens) of `childId` coming from `parentId`.
    func renameEdge(child childId: String, parent parentId: String, label: String) {
        guard let i = nodes.firstIndex(where: { $0.id == childId }) else { return }
        guard let j = nodes[i].links.firstIndex(where: { $0.id == parentId }) else { return }
        nodes[i].links[j].label = label
        save()
    }

    /// Manually wire `parentId` → `childId` (the child gains the parent's branch
    /// as context). Rejects self-links, duplicates, and anything that would
    /// create a cycle. `side` is the parent side the edge leaves from.
    @discardableResult
    func connect(from parentId: String, to childId: String, side: NodeSide) -> Bool {
        guard parentId != childId else { return false }
        guard let ci = nodes.firstIndex(where: { $0.id == childId }) else { return false }
        guard !nodes[ci].links.contains(where: { $0.id == parentId }) else { return false }
        // Cycle guard: the child must not already be an ancestor of the parent.
        if ancestors(of: parentId).contains(where: { $0.id == childId }) { return false }
        nodes[ci].links.append(ParentLink(id: parentId, side: side))
        save()
        return true
    }

    /// Remove the edge into `childId` from `parentId`.
    func disconnect(child childId: String, parent parentId: String) {
        guard let i = nodes.firstIndex(where: { $0.id == childId }) else { return }
        nodes[i].links.removeAll { $0.id == parentId }
        save()
    }

    func appendDelta(_ text: String, to id: String) {
        guard let i = nodes.firstIndex(where: { $0.id == id }) else { return }
        nodes[i].text += text
    }

    func setSources(_ sources: [ChatSource], on id: String) {
        guard let i = nodes.firstIndex(where: { $0.id == id }) else { return }
        nodes[i].sources = sources
    }

    func finish(_ id: String, status: ConvNode.Status, text: String? = nil) {
        guard let i = nodes.firstIndex(where: { $0.id == id }) else { return }
        nodes[i].status = status
        if let text { nodes[i].text = text }
        save()
    }

    func move(_ id: String, to point: CGPoint) {
        guard let i = nodes.firstIndex(where: { $0.id == id }) else { return }
        nodes[i].point = point
    }

    func delete(_ ids: Set<String>) {
        nodes.removeAll { ids.contains($0.id) }
        // Drop dangling edges so they don't point at gone nodes.
        for i in nodes.indices { nodes[i].links.removeAll { ids.contains($0.id) } }
        selection.subtract(ids)
        save()
    }

    // MARK: Selection

    func select(_ id: String, additive: Bool) {
        if additive {
            if selection.contains(id) { selection.remove(id) } else { selection.insert(id) }
        } else {
            selection = [id]
        }
    }

    // MARK: Positioning

    /// How far a child sits from its parent along a branch direction (horizontal
    /// branches need more room because cards are wider than they are tall).
    private func step(_ side: NodeSide) -> Double { side.isHorizontal ? 420 : 200 }

    private func promptPosition(links: [ParentLink]) -> CGPoint {
        guard !links.isEmpty else {
            // A fresh topic: drop it clear of any existing content, top-ish.
            let x = (nodes.map(\.x).max() ?? 160) + (nodes.isEmpty ? 0 : 380)
            return CGPoint(x: nodes.isEmpty ? 520 : x, y: 120)
        }
        if links.count == 1, let p = node(links[0].id) {
            // Branch in the chosen direction; fan siblings on the same side.
            let side = links[0].side
            let v = side.vector
            let siblings = children(of: p.id).filter { $0.links.first?.side == side }.count
            let perp = CGVector(dx: -v.dy, dy: v.dx)
            let fan = Double(siblings) * 70
            return CGPoint(x: p.x + v.dx * step(side) + perp.dx * fan,
                           y: p.y + v.dy * step(side) + perp.dy * fan)
        }
        // Join: gather below the parents.
        let parents = links.compactMap { node($0.id) }
        let x = parents.map(\.x).reduce(0, +) / Double(max(parents.count, 1))
        let y = (parents.map(\.y).max() ?? 0) + Double(layerGap)
        return CGPoint(x: x, y: y)
    }

    private func answerPosition(prompt: ConvNode?, side: NodeSide, index: Int, count: Int) -> CGPoint {
        let px = prompt?.x ?? 520
        let py = prompt?.y ?? 120
        let v = side.vector
        let base = CGPoint(x: px + v.dx * step(side), y: py + v.dy * step(side))
        guard count > 1 else { return base }
        // Spread siblings perpendicular to the branch direction.
        let perp = CGVector(dx: -v.dy, dy: v.dx)
        let spacing = side.isHorizontal ? 170.0 : Double(answerSpread)
        let t = Double(index) - Double(count - 1) / 2
        return CGPoint(x: base.x + perp.dx * t * spacing, y: base.y + perp.dy * t * spacing)
    }

    // MARK: Persistence

    /// Archive the current board (no-op while empty). Called after structural
    /// changes and gesture ends — never on every streaming delta.
    func save() {
        ExploreArchive.shared.save(snapshot())
    }
}
