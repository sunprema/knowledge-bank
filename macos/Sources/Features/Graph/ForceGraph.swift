import SwiftUI
import Observation

// A small spring-electrical (force-directed) layout engine for the knowledge
// graph. Nodes repel each other (Coulomb), edges pull connected nodes together
// (Hooke), and a weak gravity keeps the whole thing centered. Positions live in
// an arbitrary model space centered on the origin; the view maps them to screen
// with its own pan/zoom. Corpus sizes are small (tens–low hundreds of nodes),
// so the O(n²) repulsion pass is cheap.
@MainActor
@Observable
final class ForceGraph {
    struct Node: Identifiable {
        let id: String
        let title: String
        let kind: String
        let chunks: Int
        let tags: [String]
        var pos: CGPoint
        var vel: CGVector = .zero
        var pinned = false
    }
    struct Edge {
        let a: Int          // node index
        let b: Int
        let kind: String    // "link" | "similar"
        let weight: Double
    }

    private(set) var nodes: [Node] = []
    private(set) var edges: [Edge] = []
    private var settleTask: Task<Void, Never>?

    // Tuning. Spaced for cover-sized nodes (the Graph renders each node as a
    // book cover, not a small dot), so nodes rest well clear of one another.
    private let repulsion = 32_000.0
    private let springLength = 230.0
    private let springK = 0.07
    private let gravity = 0.012
    private let damping = 0.85

    var radius: (Node) -> CGFloat = { n in 8 + min(18, CGFloat(Double(n.chunks).squareRoot() * 2.2)) }

    func load(_ resp: GraphResponse) {
        settleTask?.cancel()
        var indexOf: [String: Int] = [:]
        // Seed on a circle so the layout opens out deterministically.
        let n = max(resp.nodes.count, 1)
        nodes = resp.nodes.enumerated().map { i, gn in
            let angle = Double(i) / Double(n) * 2 * .pi
            // Seed on a circle sized to the node count so the layout opens out
            // from a roomy start and settles spread apart rather than clumped.
            let r = max(320.0, Double(n) * 26.0)
            indexOf[gn.id] = i
            return Node(id: gn.id, title: gn.title, kind: gn.kind, chunks: gn.chunks,
                        tags: gn.tags,
                        pos: CGPoint(x: cos(angle) * r, y: sin(angle) * r))
        }
        edges = resp.edges.compactMap { e in
            guard let a = indexOf[e.source], let b = indexOf[e.target], a != b else { return nil }
            return Edge(a: a, b: b, kind: e.kind, weight: Double(max(e.weight, 0.1)))
        }
    }

    /// Solve the layout synchronously to a settled state — no per-frame
    /// animation. Used by the cover graph, which wants the map to land instantly
    /// rather than visibly jiggle into place.
    func settleNow(iterations: Int = 420) {
        settleTask?.cancel()
        guard nodes.count > 1 else { return }
        var temperature = 1.0
        for _ in 0..<iterations {
            step(temperature: temperature)
            temperature *= 0.99
        }
    }

    /// Animate the layout settling: step repeatedly with cooling, yielding to
    /// the main run loop so SwiftUI redraws each frame.
    func settle() {
        settleTask?.cancel()
        settleTask = Task { @MainActor in
            var temperature = 1.0
            for _ in 0..<240 {
                if Task.isCancelled { return }
                step(temperature: temperature)
                temperature *= 0.985
                try? await Task.sleep(for: .milliseconds(12))
            }
        }
    }

    /// Re-energize the simulation (e.g. after dragging a node).
    func reheat() {
        settleTask?.cancel()
        settleTask = Task { @MainActor in
            for _ in 0..<90 {
                if Task.isCancelled { return }
                step(temperature: 0.6)
                try? await Task.sleep(for: .milliseconds(12))
            }
        }
    }

    func setPosition(_ index: Int, to p: CGPoint) {
        guard nodes.indices.contains(index) else { return }
        nodes[index].pos = p
        nodes[index].vel = .zero
    }

    func pin(_ index: Int, _ pinned: Bool) {
        guard nodes.indices.contains(index) else { return }
        nodes[index].pinned = pinned
    }

    private func step(temperature: Double) {
        let count = nodes.count
        guard count > 1 else { return }
        var force = [CGVector](repeating: .zero, count: count)

        // Repulsion between every pair.
        for i in 0..<count {
            for j in (i + 1)..<count {
                var dx = nodes[i].pos.x - nodes[j].pos.x
                var dy = nodes[i].pos.y - nodes[j].pos.y
                var distSq = dx * dx + dy * dy
                if distSq < 0.01 { dx = .random(in: -1...1); dy = .random(in: -1...1); distSq = 1 }
                let dist = sqrt(distSq)
                let f = repulsion / distSq
                let ux = dx / dist, uy = dy / dist
                force[i].dx += ux * f; force[i].dy += uy * f
                force[j].dx -= ux * f; force[j].dy -= uy * f
            }
        }

        // Spring attraction along edges.
        for e in edges {
            let dx = nodes[e.b].pos.x - nodes[e.a].pos.x
            let dy = nodes[e.b].pos.y - nodes[e.a].pos.y
            let dist = max(sqrt(dx * dx + dy * dy), 0.01)
            let f = springK * e.weight * (dist - springLength)
            let ux = dx / dist, uy = dy / dist
            force[e.a].dx += ux * f; force[e.a].dy += uy * f
            force[e.b].dx -= ux * f; force[e.b].dy -= uy * f
        }

        // Integrate.
        for i in 0..<count {
            if nodes[i].pinned { nodes[i].vel = .zero; continue }
            force[i].dx -= nodes[i].pos.x * gravity
            force[i].dy -= nodes[i].pos.y * gravity
            nodes[i].vel.dx = (nodes[i].vel.dx + force[i].dx) * damping
            nodes[i].vel.dy = (nodes[i].vel.dy + force[i].dy) * damping
            let maxStep = 30.0 * temperature
            let vx = min(max(nodes[i].vel.dx, -maxStep), maxStep)
            let vy = min(max(nodes[i].vel.dy, -maxStep), maxStep)
            nodes[i].pos.x += vx
            nodes[i].pos.y += vy
        }
    }
}
