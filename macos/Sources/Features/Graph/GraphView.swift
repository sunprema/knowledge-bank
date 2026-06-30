import SwiftUI

// Native, interactive knowledge graph (LOCAL_UI_PRD §4.5 / NEW_SWIFT_FEATURES §2).
// Renders the engine's `/graph` (HippoRAG-style link + similarity edges) as a
// constellation of book covers: each document is its Library cover (real PDF
// page or designed gradient), wired to relevant documents by link/similarity
// edges. The layout is solved instantly (no visible jiggle).
//
// Two modes:
//   • Browse — pan/zoom the whole constellation; hover a cover to light its
//     neighborhood, double-click to open.
//   • Focus — single-click a cover: the rest fade away and its connected
//     documents reflow into a ring around it, bigger and closer, so you can
//     study one document's neighborhood. Single-click a neighbor to re-focus on
//     it; double-click any cover to open it; Esc returns to the full graph.
@MainActor
struct GraphView: View {
    let client: KBClient

    @State private var graph = ForceGraph()
    @State private var neighbors = 3
    @State private var loading = true
    @State private var error: String?

    // Viewport (browse mode).
    @State private var scale: CGFloat = 1
    @State private var baseScale: CGFloat = 1
    @State private var offset: CGSize = .zero
    @State private var baseOffset: CGSize = .zero
    @State private var viewCenter: CGPoint = .zero

    // Interaction.
    @State private var hovered: String?
    @State private var focused: String?            // focus mode: the centered node
    @State private var focusOrder: [String] = []   // its neighbors, in ring order
    @State private var preview: PaperMetadata?
    @Namespace private var coverNS

    // Browser-style tabs: the graph canvas is "home"; opening a node opens that
    // paper's abstract + PDF as a new tab — the same reading flow as Library.
    @State private var nav = PaperTabs()

    // Connection count per node id — drives cover sizing, label priority, and
    // the preview's "N connections" without rescanning edges each frame.
    @State private var degree: [String: Int] = [:]

    // Focus-mode cover sizes (fixed; independent of zoom).
    private let focusedW: CGFloat = 132
    private let neighborW: CGFloat = 96

    var body: some View {
        VStack(spacing: 0) {
            if !nav.tabs.isEmpty {
                PaperTabStrip(nav: nav, homeTitle: "Graph",
                              homeIcon: "point.3.connected.trianglepath.dotted")
                Divider()
            }
            content
        }
        .task(id: neighbors) { await load() }
    }

    @ViewBuilder private var content: some View {
        if nav.isSplit, case .paper(let leftId) = nav.selection {
            HSplitView {
                // Left pane. Related clicks load into the right pane (cross-reading).
                PaperDetailView(client: client, paperId: leftId,
                                onOpenPaper: { pid, title in nav.setRight(pid, title: title) },
                                inlineChrome: true,
                                onClosePane: { withAnimation(.snappy) { nav.closeSplit() } })
                    .id("L-\(leftId)")
                    .frame(minWidth: 380)

                rightPane
                    .frame(minWidth: 380)
                    .layoutPriority(1)
            }
        } else {
            switch nav.selection {
            case .home:
                graphHome
                    .navigationTitle("Graph")
                    .toolbar { ToolbarItemGroup { controls } }
            case .paper(let id):
                // Same reader as Library; related-paper clicks open further tabs.
                PaperDetailView(client: client, paperId: id,
                                onOpenPaper: { pid, title in nav.open(pid, title: title) })
                    .id(id)   // fresh detail state per paper
            }
        }
    }

    @ViewBuilder private var rightPane: some View {
        if let rightId = nav.splitPaperId {
            // Related clicks here load into the left pane (cross-reading).
            PaperDetailView(client: client, paperId: rightId,
                            onOpenPaper: { pid, title in nav.setLeft(pid, title: title) },
                            inlineChrome: true,
                            onClosePane: { withAnimation(.snappy) { nav.closeSplit() } })
                .id("R-\(rightId)")
        } else {
            // Pick the second document for this pane from the graph's own nodes.
            GraphSplitChooser(nodes: graph.nodes,
                              onPick: { pid, title in withAnimation(.snappy) { nav.setRight(pid, title: title) } },
                              onCancel: { withAnimation(.snappy) { nav.closeSplit() } })
        }
    }

    @ViewBuilder private var graphHome: some View {
        if loading {
            ProgressView("Building graph…").frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if let error {
            EmptyStateView(icon: "exclamationmark.triangle", title: "Couldn't load the graph", message: error)
        } else if graph.nodes.isEmpty {
            EmptyStateView(icon: "point.3.connected.trianglepath.dotted",
                           title: "No graph yet",
                           message: "Add papers and notes — links and similarity edges will appear here.")
        } else {
            canvas
        }
    }

    // MARK: Controls

    @ViewBuilder private var controls: some View {
        Picker("Links", selection: $neighbors) {
            Text("Links only").tag(0)
            Text("± few").tag(3)
            Text("± more").tag(6)
        }
        .pickerStyle(.menu)
        .help("How many nearest-neighbor similarity edges to draw per node")
        .disabled(focused != nil)

        Button { applyZoom(factor: 1.25, focal: viewCenter, animated: true) } label: { Image(systemName: "plus.magnifyingglass") }
            .help("Zoom in").disabled(focused != nil)
        Button { applyZoom(factor: 0.8, focal: viewCenter, animated: true) } label: { Image(systemName: "minus.magnifyingglass") }
            .help("Zoom out").disabled(focused != nil)
        Button { focused != nil ? reset() : withAnimation(.snappy) { fitToView() } } label: {
            Image(systemName: focused != nil ? "arrow.up.left.and.arrow.down.right.circle" : "scope")
        }
        .help(focused != nil ? "Back to the full graph (Esc)" : "Fit graph to view")
    }

    // MARK: Canvas

    private var canvas: some View {
        GeometryReader { geo in
            let center = CGPoint(x: geo.size.width / 2, y: geo.size.height / 2)
            ZStack {
                // Background carries pan/zoom (covers handle their own hover/tap).
                Rectangle().fill(Color(nsColor: .windowBackgroundColor))
                    .contentShape(Rectangle())
                    .gesture(panGesture)
                    .gesture(MagnifyGesture()
                        .onChanged { v in
                            guard focused == nil else { return }
                            let target = clampScale(baseScale * v.magnification)
                            applyZoom(factor: target / scale, focal: viewCenter)
                        }
                        .onEnded { _ in baseScale = scale })
                    .overlay { ScrollZoom { factor, loc in if focused == nil { applyZoom(factor: factor, focal: loc) } } }
                    .onTapGesture { if focused != nil { reset() } else { withAnimation(.snappy) { hovered = nil } } }

                // Edges (animatable line shapes so they reflow with the covers).
                ForEach(graph.edges.indices, id: \.self) { i in
                    let e = graph.edges[i]
                    EdgeShape(a: nodePoint(graph.nodes[e.a], center: center),
                              b: nodePoint(graph.nodes[e.b], center: center))
                        .stroke(edgeColor(e).opacity(edgeOpacity(e)),
                                style: StrokeStyle(lineWidth: edgeWidth(e), dash: e.kind == "link" ? [] : [4, 3]))
                        .allowsHitTesting(false)
                }

                // Cover nodes.
                ForEach(graph.nodes) { node in
                    nodeCover(node, center: center)
                }
            }
            .clipped()
            .overlay(alignment: .topLeading) { legend.padding(12) }
            .overlay(alignment: .bottomTrailing) { zoomBadge.padding(12) }
            .overlay(alignment: .top) { if focused != nil { focusHint } }
            .overlay { previewOverlay }
            .onExitCommand { if preview == nil, focused != nil { reset() } }   // Esc → full graph
            .onAppear { viewCenter = center }
            .onChange(of: geo.size) { viewCenter = center }
        }
    }

    private var focusHint: some View {
        Text("Focusing connections · click a cover to refocus · double-click to open · Esc to exit")
            .font(.caption2).foregroundStyle(.secondary)
            .padding(.horizontal, 10).padding(.vertical, 5)
            .background(.regularMaterial, in: Capsule())
            .padding(.top, 10)
            .transition(.move(edge: .top).combined(with: .opacity))
    }

    private func screen(_ p: CGPoint, center: CGPoint) -> CGPoint {
        CGPoint(x: center.x + offset.width + p.x * scale,
                y: center.y + offset.height + p.y * scale)
    }

    // MARK: Cover nodes

    private func nodeCover(_ node: ForceGraph.Node, center: CGPoint) -> some View {
        let w = nodeWidth(node)
        let h = w * 4 / 3
        let isHover = hovered == node.id
        let isFocused = focused == node.id

        return VStack(spacing: 4) {
            ZStack {
                if preview?.id == node.id {
                    Color.clear.frame(width: w, height: h)   // held by the expanded preview
                } else {
                    CoverImage(paper: meta(node), client: client)
                        .frame(width: w, height: h)
                        .clipShape(RoundedRectangle(cornerRadius: 6))
                        .overlay(RoundedRectangle(cornerRadius: 6).stroke(.black.opacity(0.18), lineWidth: 0.5))
                        .shadow(color: .black.opacity(isHover || isFocused ? 0.4 : 0.22),
                                radius: isHover || isFocused ? 14 : 5, x: 0, y: isHover || isFocused ? 9 : 3)
                        .matchedGeometryEffect(id: node.id, in: coverNS)
                        .scaleEffect(isHover ? 1.1 : 1, anchor: .center)
                        .onHover { inside in
                            withAnimation(.snappy(duration: 0.18)) {
                                hovered = inside ? node.id : (hovered == node.id ? nil : hovered)
                            }
                        }
                        .onTapGesture(count: 2) {
                            withAnimation(.spring(response: 0.42, dampingFraction: 0.82)) { preview = meta(node) }
                        }
                        .onTapGesture(count: 1) { singleTap(node) }
                }
            }
            .frame(width: w, height: h)

            if labelShown(node, isHover: isHover) {
                Text(node.title)
                    .font(.system(size: isFocused ? 11 : 10, weight: isHover || isFocused ? .semibold : .regular))
                    .lineLimit(2)
                    .multilineTextAlignment(.center)
                    .frame(width: max(w + 24, 96))
                    .padding(.horizontal, 4).padding(.vertical, 1)
                    .background(Color(nsColor: .windowBackgroundColor).opacity(0.72),
                                in: RoundedRectangle(cornerRadius: 4))
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .opacity(nodeOpacity(node, isHover: isHover))
        .zIndex(isHover ? 3 : (isFocused ? 2 : 0))
        .allowsHitTesting(isVisible(node))
        .position(nodePoint(node, center: center))
    }

    private func meta(_ node: ForceGraph.Node) -> PaperMetadata {
        PaperMetadata(id: node.id, kind: node.kind, title: node.title, tags: node.tags)
    }

    /// Click → focus this node's neighborhood (or, on the already-focused node,
    /// return to the full graph). Double-click opens the paper (handled above).
    private func singleTap(_ node: ForceGraph.Node) {
        if focused == node.id { reset(); return }
        focusOrder = orderedNeighbors(of: node.id)
        withAnimation(.spring(response: 0.5, dampingFraction: 0.82)) {
            focused = node.id
            hovered = nil
        }
    }

    private func reset() {
        withAnimation(.spring(response: 0.5, dampingFraction: 0.85)) {
            focused = nil
            hovered = nil
        }
    }

    // MARK: Node geometry (shared by covers + edges so they move together)

    private func nodePoint(_ node: ForceGraph.Node, center: CGPoint) -> CGPoint {
        guard let f = focused else { return screen(node.pos, center: center) }
        if node.id == f { return center }
        if let i = focusOrder.firstIndex(of: node.id) {
            return ringPoint(index: i, count: focusOrder.count, center: center)
        }
        return screen(node.pos, center: center)   // hidden; position doesn't matter
    }

    private func ringPoint(index: Int, count: Int, center: CGPoint) -> CGPoint {
        let r = focusRingRadius(count)
        let angle = -CGFloat.pi / 2 + 2 * .pi * CGFloat(index) / CGFloat(max(count, 1))
        return CGPoint(x: center.x + cos(angle) * r, y: center.y + sin(angle) * r)
    }

    /// Ring radius that clears the centered cover and spaces neighbors apart.
    private func focusRingRadius(_ count: Int) -> CGFloat {
        let clearance = focusedW * 4 / 3 / 2 + neighborW * 4 / 3 / 2 + 40
        let spread = count > 0 ? CGFloat(count) * (neighborW + 30) / (2 * .pi) : 0
        return max(clearance, spread)
    }

    private func nodeWidth(_ node: ForceGraph.Node) -> CGFloat {
        guard let f = focused else { return coverWidth(node) }
        if node.id == f { return focusedW }
        if focusOrder.contains(node.id) { return neighborW }
        return coverWidth(node)
    }

    private func isVisible(_ node: ForceGraph.Node) -> Bool {
        guard let f = focused else { return true }
        return node.id == f || focusOrder.contains(node.id)
    }

    private func nodeOpacity(_ node: ForceGraph.Node, isHover: Bool) -> Double {
        if focused != nil { return isVisible(node) ? 1 : 0 }
        return (hovered == nil || isHover || hoverNeighbors.contains(node.id)) ? 1 : 0.22
    }

    private func labelShown(_ node: ForceGraph.Node, isHover: Bool) -> Bool {
        if focused != nil { return isVisible(node) }
        return isHover || (degree[node.id] ?? 0) >= hubFloor || scale > 1.25
    }

    // MARK: Edges

    private func edgeColor(_ e: ForceGraph.Edge) -> Color { e.kind == "link" ? .accentColor : .gray }

    private func edgeOpacity(_ e: ForceGraph.Edge) -> Double {
        let isLink = e.kind == "link"
        if focused != nil {
            let visible = isVisible(graph.nodes[e.a]) && isVisible(graph.nodes[e.b])
            return visible ? (isLink ? 0.6 : 0.4) : 0
        }
        let aId = graph.nodes[e.a].id, bId = graph.nodes[e.b].id
        let incident = hovered != nil && (aId == hovered || bId == hovered)
        let active = hovered == nil
        let base = isLink ? 0.55 : 0.28
        return (active || incident) ? base : 0.05
    }

    private func edgeWidth(_ e: ForceGraph.Edge) -> CGFloat {
        let base = e.kind == "link" ? 1.4 : 0.6 + e.weight * 1.2
        let incident = focused == nil && hovered != nil
            && (graph.nodes[e.a].id == hovered || graph.nodes[e.b].id == hovered)
        return CGFloat(base) * (incident ? 1.8 : 1)
    }

    // MARK: Preview overlay (cover → book detail morph)

    @ViewBuilder private var previewOverlay: some View {
        if let p = preview {
            ZStack {
                Rectangle().fill(.black.opacity(0.4)).ignoresSafeArea()
                    .transition(.opacity)
                    .onTapGesture(perform: closePreview)
                GraphPreviewCard(paper: p, client: client, ns: coverNS,
                                 connections: degree[p.id] ?? 0,
                                 onRead: { openFromPreview(p) },
                                 onClose: closePreview)
            }
        }
    }

    private func closePreview() {
        withAnimation(.spring(response: 0.42, dampingFraction: 0.85)) { preview = nil }
    }

    private func openFromPreview(_ paper: PaperMetadata) {
        let id = paper.arxivId, title = paper.title
        withAnimation(.spring(response: 0.4, dampingFraction: 0.85)) { preview = nil }
        nav.open(id, title: title)
    }

    // MARK: Legend

    private var legend: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Rectangle().fill(Color.accentColor).frame(width: 16, height: 2)
                Text("explicit link").font(.caption2).foregroundStyle(.secondary)
            }
            HStack(spacing: 6) {
                Rectangle().fill(.gray).frame(width: 16, height: 2)
                    .overlay(Rectangle().fill(.background).frame(width: 4, height: 2).offset(x: 1))
                Text("similarity").font(.caption2).foregroundStyle(.secondary)
            }
            Text("\(graph.nodes.count) docs · \(graph.edges.count) edges")
                .font(.caption2).foregroundStyle(.tertiary)
        }
        .padding(8)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 8))
    }

    private var zoomBadge: some View {
        Text(focused == nil ? "\(Int((scale * 100).rounded()))%" : "Focus")
            .font(.caption2.monospacedDigit())
            .foregroundStyle(.secondary)
            .padding(.horizontal, 7).padding(.vertical, 3)
            .background(.regularMaterial, in: Capsule())
    }

    // MARK: Interaction

    /// Pan the viewport by dragging the background (browse mode only).
    private var panGesture: some Gesture {
        DragGesture(minimumDistance: 3)
            .onChanged { value in
                guard focused == nil else { return }
                offset = CGSize(width: baseOffset.width + value.translation.width,
                                height: baseOffset.height + value.translation.height)
            }
            .onEnded { _ in baseOffset = offset }
    }

    // MARK: Sizing helpers

    /// Cover width in browse mode: a base size bumped for well-connected hubs,
    /// scaled by zoom and clamped so covers stay recognizable at any zoom.
    private func coverWidth(_ node: ForceGraph.Node) -> CGFloat {
        let deg = CGFloat(degree[node.id] ?? 0)
        let base = 64 + min(24, sqrt(deg) * 4.4)
        return min(max(base * scale, 30), 150)
    }

    /// Degree threshold above which a node is a hub (label always shown).
    private var hubFloor: Int {
        let degs = degree.values.sorted()
        guard !degs.isEmpty else { return .max }
        let idx = Int(Double(degs.count - 1) * 0.7)
        return max(3, degs[idx])
    }

    /// Ids adjacent to the hovered node (lit in browse mode; others dim).
    private var hoverNeighbors: Set<String> {
        guard let id = hovered,
              let idx = graph.nodes.firstIndex(where: { $0.id == id }) else { return [] }
        var ids = Set<String>()
        for e in graph.edges {
            if e.a == idx { ids.insert(graph.nodes[e.b].id) }
            if e.b == idx { ids.insert(graph.nodes[e.a].id) }
        }
        return ids
    }

    /// A focused node's neighbors, strongest connection first (explicit links
    /// before similarity), for a stable, meaningful ring order.
    private func orderedNeighbors(of id: String) -> [String] {
        guard let idx = graph.nodes.firstIndex(where: { $0.id == id }) else { return [] }
        var best: [String: Double] = [:]
        for e in graph.edges {
            let other: Int?
            if e.a == idx { other = e.b } else if e.b == idx { other = e.a } else { other = nil }
            guard let o = other else { continue }
            let rank = e.weight + (e.kind == "link" ? 100 : 0)   // links sort first
            best[graph.nodes[o].id] = max(best[graph.nodes[o].id] ?? -1, rank)
        }
        return best.sorted { $0.value > $1.value }.map { $0.key }
    }

    // MARK: Viewport math

    /// Scale by `factor`, keeping the model point under `focal` (a screen point)
    /// fixed — the natural "zoom toward the cursor" behavior.
    private func applyZoom(factor: CGFloat, focal: CGPoint, animated: Bool = false) {
        let target = clampScale(scale * factor)
        let k = target / scale
        guard k != 1 else { return }
        let newOffset = CGSize(
            width: (focal.x - viewCenter.x) * (1 - k) + offset.width * k,
            height: (focal.y - viewCenter.y) * (1 - k) + offset.height * k)
        if animated {
            withAnimation(.snappy) { scale = target; baseScale = target; offset = newOffset; baseOffset = newOffset }
        } else {
            scale = target; baseScale = target; offset = newOffset; baseOffset = newOffset
        }
    }

    private func clampScale(_ s: CGFloat) -> CGFloat { min(max(s, 0.2), 5) }
    private func resetViewport() { scale = 1; baseScale = 1; offset = .zero; baseOffset = .zero }

    /// Frame the whole graph: center its bounding box and pick a scale that
    /// leaves a comfortable margin.
    private func fitToView() {
        guard !graph.nodes.isEmpty, viewCenter != .zero else { resetViewport(); return }
        var minX = CGFloat.greatestFiniteMagnitude, minY = minX
        var maxX = -CGFloat.greatestFiniteMagnitude, maxY = maxX
        for n in graph.nodes {
            minX = min(minX, n.pos.x); maxX = max(maxX, n.pos.x)
            minY = min(minY, n.pos.y); maxY = max(maxY, n.pos.y)
        }
        let w = max(maxX - minX, 1), h = max(maxY - minY, 1)
        let fit = min((viewCenter.x * 2 - 140) / w, (viewCenter.y * 2 - 140) / h)
        scale = clampScale(fit); baseScale = scale
        let cx = (minX + maxX) / 2, cy = (minY + maxY) / 2
        offset = CGSize(width: -cx * scale, height: -cy * scale)
        baseOffset = offset
    }

    private func recomputeDegree() {
        var d: [String: Int] = [:]
        for e in graph.edges {
            d[graph.nodes[e.a].id, default: 0] += 1
            d[graph.nodes[e.b].id, default: 0] += 1
        }
        degree = d
    }

    private func load() async {
        loading = true; error = nil; hovered = nil; preview = nil; focused = nil
        do {
            let resp = try await client.graph(neighbors: neighbors)
            graph.load(resp)
            recomputeDegree()
            graph.settleNow()      // solve instantly — no visible settling
            fitToView()
        } catch {
            self.error = error.localizedDescription
        }
        loading = false
    }
}

// An animatable straight edge between two points, so edges interpolate smoothly
// as covers reflow between browse and focus layouts.
private struct EdgeShape: Shape {
    var a: CGPoint
    var b: CGPoint

    var animatableData: AnimatablePair<AnimatablePair<CGFloat, CGFloat>, AnimatablePair<CGFloat, CGFloat>> {
        get { .init(.init(a.x, a.y), .init(b.x, b.y)) }
        set {
            a = CGPoint(x: newValue.first.first, y: newValue.first.second)
            b = CGPoint(x: newValue.second.first, y: newValue.second.second)
        }
    }

    func path(in rect: CGRect) -> Path {
        Path { p in p.move(to: a); p.addLine(to: b) }
    }
}

// The expanded "book detail" for a graph node: the cover morphs out of the
// constellation (matched geometry) while authors + abstract are fetched and
// faded in beside it. Mirrors the Library shelf's preview card.
private struct GraphPreviewCard: View {
    let paper: PaperMetadata
    let client: KBClient
    let ns: Namespace.ID
    let connections: Int
    let onRead: () -> Void
    let onClose: () -> Void

    @State private var detail: PaperDetail?

    var body: some View {
        HStack(alignment: .top, spacing: 28) {
            CoverImage(paper: paper, client: client)
                .frame(width: 240, height: 320)
                .clipShape(RoundedRectangle(cornerRadius: 14))
                .overlay(RoundedRectangle(cornerRadius: 14).stroke(.black.opacity(0.15), lineWidth: 0.5))
                .shadow(color: .black.opacity(0.4), radius: 24, y: 12)
                .matchedGeometryEffect(id: paper.id, in: ns)

            VStack(alignment: .leading, spacing: 14) {
                Text(paper.title)
                    .font(.system(.title, design: .serif).weight(.bold))
                    .lineLimit(4)

                let authors = detail?.metadata.authors ?? []
                if !authors.isEmpty {
                    Text(authors.prefix(6).joined(separator: ", ")
                         + (authors.count > 6 ? " et al." : ""))
                        .font(.title3).foregroundStyle(.secondary).lineLimit(2)
                }

                HStack(spacing: 6) {
                    Chip(text: paper.kind.capitalized, color: .accentColor, filled: true)
                    let pub = detail?.metadata.publishedAt ?? ""
                    if !pub.isEmpty { Chip(text: Theme.year(pub)) }
                    ForEach((detail?.metadata.categories ?? []).prefix(3), id: \.self) { Chip(text: $0) }
                    ForEach(paper.tags.prefix(3), id: \.self) { Chip(text: $0, color: .accentColor, filled: true) }
                }

                Label("\(connections) connection\(connections == 1 ? "" : "s")",
                      systemImage: "point.3.connected.trianglepath.dotted")
                    .font(.caption).foregroundStyle(.secondary)

                let abstract = detail?.metadata.abstract ?? ""
                if !abstract.isEmpty {
                    ScrollView {
                        Text(abstract)
                            .font(.system(.body, design: .serif))
                            .lineSpacing(4)
                            .textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .frame(maxHeight: 180)
                } else if detail == nil {
                    HStack(spacing: 6) {
                        ProgressView().controlSize(.small)
                        Text("Loading…").font(.caption).foregroundStyle(.secondary)
                    }
                }

                Spacer(minLength: 0)
                HStack(spacing: 12) {
                    Button(action: onRead) { Label("Read", systemImage: "book").frame(maxWidth: 150) }
                        .buttonStyle(.borderedProminent).controlSize(.large)
                        .keyboardShortcut(.defaultAction)
                    Button("Close", action: onClose)
                        .controlSize(.large).keyboardShortcut(.cancelAction)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .transition(.opacity.combined(with: .move(edge: .trailing)))
        }
        .padding(32)
        .frame(maxWidth: 820, maxHeight: 520)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 24))
        .overlay(RoundedRectangle(cornerRadius: 24).stroke(.separator, lineWidth: 0.5))
        .shadow(color: .black.opacity(0.3), radius: 40, y: 20)
        .padding(40)
        .task(id: paper.id) { detail = try? await client.paper(paper.id) }
    }
}

// The right pane while a split is being set up in the Graph: pick the second
// document to read alongside the first, chosen from the graph's own nodes.
private struct GraphSplitChooser: View {
    let nodes: [ForceGraph.Node]
    let onPick: (String, String) -> Void
    let onCancel: () -> Void
    @State private var filter = ""

    private var filtered: [ForceGraph.Node] {
        guard !filter.isEmpty else { return nodes }
        let q = filter.lowercased()
        return nodes.filter { $0.title.lowercased().contains(q) }
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: "rectangle.righthalf.inset.filled").foregroundStyle(.tint)
                Text("Choose a document for this pane").font(.headline)
                Spacer()
                Button("Cancel", action: onCancel).keyboardShortcut(.cancelAction)
            }
            .padding(.horizontal, 14).padding(.vertical, 8)
            .background(.bar)
            .overlay(alignment: .bottom) { Divider() }

            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                TextField("Filter documents…", text: $filter).textFieldStyle(.plain)
            }
            .padding(8).padding(.horizontal, 6)

            ScrollView {
                LazyVStack(spacing: 8) {
                    ForEach(filtered) { node in
                        Button { onPick(node.id, node.title) } label: {
                            Card {
                                HStack(alignment: .top, spacing: 12) {
                                    Image(systemName: Theme.kindGlyph(node.kind))
                                        .font(.title3)
                                        .foregroundStyle(Theme.kindColor(node.kind))
                                        .frame(width: 24)
                                    Text(node.title).font(.headline).lineLimit(2)
                                    Spacer(minLength: 0)
                                    Image(systemName: "chevron.right").font(.caption).foregroundStyle(.tertiary)
                                }
                            }
                        }
                        .buttonStyle(.plain)
                    }
                }
                .padding(12)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(.background)
    }
}
