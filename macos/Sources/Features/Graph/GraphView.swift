import SwiftUI

// Native, interactive knowledge graph (LOCAL_UI_PRD §4.5 / NEW_SWIFT_FEATURES §2).
// Renders the engine's `/graph` (HippoRAG-style link + similarity edges) with a
// force-directed layout drawn in a Canvas. Pan by dragging the background, zoom
// with pinch or the controls, drag a node to reposition it, click a node to
// inspect and open it.
@MainActor
struct GraphView: View {
    let client: KBClient

    @State private var graph = ForceGraph()
    @State private var neighbors = 3
    @State private var loading = true
    @State private var error: String?

    // Viewport.
    @State private var scale: CGFloat = 1
    @State private var baseScale: CGFloat = 1
    @State private var offset: CGSize = .zero
    @State private var baseOffset: CGSize = .zero
    @State private var viewCenter: CGPoint = .zero

    // Interaction.
    @State private var selected: String?
    @State private var hovered: Int?
    @State private var hoverPoint: CGPoint?
    @State private var drag: DragMode = .none

    // Browser-style tabs: the graph canvas is "home"; clicking a node opens that
    // paper's abstract + PDF as a new tab — the same reading flow as Library.
    @State private var nav = PaperTabs()

    // Connection count per node id — drives sizing, label priority, and the
    // inspector's "N connections" without rescanning edges each frame.
    @State private var degree: [String: Int] = [:]

    private enum DragMode: Equatable { case none, pan, node(Int) }

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

        Button { applyZoom(factor: 1.25, focal: viewCenter, animated: true) } label: { Image(systemName: "plus.magnifyingglass") }
            .help("Zoom in")
        Button { applyZoom(factor: 0.8, focal: viewCenter, animated: true) } label: { Image(systemName: "minus.magnifyingglass") }
            .help("Zoom out")
        Button { withAnimation(.snappy) { fitToView() } } label: { Image(systemName: "scope") }
            .help("Fit graph to view")
    }

    // MARK: Canvas

    private var canvas: some View {
        GeometryReader { geo in
            let center = CGPoint(x: geo.size.width / 2, y: geo.size.height / 2)
            Canvas { ctx, _ in
                draw(ctx, center: center)
            }
            .background(.background)
            .contentShape(Rectangle())
            .gesture(dragGesture(center: center))
            .gesture(MagnifyGesture()
                .onChanged { v in
                    let target = clampScale(baseScale * v.magnification)
                    applyZoom(factor: target / scale, focal: hoverPoint ?? center)
                }
                .onEnded { _ in baseScale = scale })
            .onContinuousHover { phase in
                switch phase {
                case .active(let loc):
                    hoverPoint = loc
                    if drag == .none { hovered = nodeIndex(at: loc, center: center) }
                case .ended:
                    hovered = nil; hoverPoint = nil
                }
            }
            .overlay { ScrollZoom { factor, loc in applyZoom(factor: factor, focal: loc) } }
            .overlay(alignment: .topLeading) { legend.padding(12) }
            .overlay(alignment: .topTrailing) { inspector.padding(12) }
            .overlay(alignment: .topLeading) { hoverTooltip }
            .overlay(alignment: .bottomTrailing) { zoomBadge.padding(12) }
            .onTapGesture { } // absorb taps so background taps don't propagate oddly
            .onAppear { viewCenter = center }
            .onChange(of: geo.size) { viewCenter = center }
        }
    }

    private func draw(_ ctx: GraphicsContext, center: CGPoint) {
        let neighborIds = selectedNeighborIds
        let hoveredId = hovered.flatMap { graph.nodes.indices.contains($0) ? graph.nodes[$0].id : nil }
        // A node is a "hub" if it's in the most-connected fraction of the graph;
        // hubs keep their labels at any zoom so the map always reads as a map.
        let hubFloor = max(2, hubDegreeFloor)
        func screen(_ p: CGPoint) -> CGPoint {
            CGPoint(x: center.x + offset.width + p.x * scale,
                    y: center.y + offset.height + p.y * scale)
        }

        // Edges.
        for e in graph.edges {
            let pa = screen(graph.nodes[e.a].pos), pb = screen(graph.nodes[e.b].pos)
            let endsId = [graph.nodes[e.a].id, graph.nodes[e.b].id]
            let active = selected == nil && hoveredId == nil
            let incident = endsId.contains(selected ?? "\0") || endsId.contains(hoveredId ?? "\0")
            var path = Path(); path.move(to: pa); path.addLine(to: pb)
            let isLink = e.kind == "link"
            let color: Color = isLink ? .accentColor : .gray
            let baseOpacity = isLink ? 0.55 : 0.30
            let opacity = (active || incident) ? baseOpacity : 0.05
            let width = (isLink ? 1.4 : 0.6 + e.weight * 1.2) * (incident ? 1.6 : 1)
            ctx.stroke(path, with: .color(color.opacity(opacity)),
                       style: StrokeStyle(lineWidth: width, dash: isLink ? [] : [4, 3]))
        }

        // Nodes.
        for node in graph.nodes {
            let p = screen(node.pos)
            let r = radius(node) * max(scale, 0.6)
            let isSel = node.id == selected
            let isHover = node.id == hoveredId
            let neutral = selected == nil
            let highlighted = neutral || isSel || neighborIds.contains(node.id)
            let fill = Theme.kindColor(node.kind)
            let rect = CGRect(x: p.x - r, y: p.y - r, width: r * 2, height: r * 2)

            // Soft glow under selected / hovered nodes so they pop.
            if isSel || isHover {
                ctx.fill(Circle().path(in: rect.insetBy(dx: -6, dy: -6)),
                         with: .color(fill.opacity(0.25)))
            }
            ctx.fill(Circle().path(in: rect),
                     with: .color(fill.opacity(highlighted ? 1 : 0.22)))
            // Thin rim for definition against the background.
            ctx.stroke(Circle().path(in: rect),
                       with: .color(.black.opacity(highlighted ? 0.25 : 0.08)), lineWidth: 0.75)
            if isSel || isHover {
                ctx.stroke(Circle().path(in: rect.insetBy(dx: -3, dy: -3)),
                           with: .color(isSel ? .primary : .secondary), lineWidth: 2)
            }

            // Labels: selected node, its neighbors, hubs, the hovered node, or
            // — once zoomed in a little — everything.
            let deg = degree[node.id] ?? 0
            let showLabel = isSel || isHover || neighborIds.contains(node.id)
                || deg >= hubFloor || scale > 1.1
            if showLabel && highlighted {
                let weight: Font.Weight = (isSel || isHover) ? .semibold : .regular
                let label = Text(String(node.title.prefix(40)))
                    .font(.system(size: 10, weight: weight))
                    .foregroundStyle(.primary)
                // Plate behind the text so it stays legible over edges/nodes.
                let resolved = ctx.resolve(label)
                let size = resolved.measure(in: CGSize(width: 240, height: 40))
                let ly = p.y + r + 4
                let plate = CGRect(x: p.x - size.width / 2 - 3, y: ly - 1,
                                   width: size.width + 6, height: size.height + 2)
                ctx.fill(RoundedRectangle(cornerRadius: 4).path(in: plate),
                         with: .color(Color(nsColor: .windowBackgroundColor).opacity(0.7)))
                ctx.draw(resolved, at: CGPoint(x: p.x, y: ly), anchor: .top)
            }
        }
    }

    // MARK: Legend & inspector

    private var legend: some View {
        VStack(alignment: .leading, spacing: 6) {
            // What the ball colors mean — only the kinds actually present.
            ForEach(presentKinds, id: \.self) { kind in
                HStack(spacing: 6) {
                    Circle().fill(Theme.kindColor(kind)).frame(width: 9, height: 9)
                    Text(kind.capitalized).font(.caption2).foregroundStyle(.secondary)
                }
            }
            if !presentKinds.isEmpty { Divider().frame(width: 90) }
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

    private var presentKinds: [String] {
        let order = ["paper", "note", "idea", "reflection"]
        let present = Set(graph.nodes.map { $0.kind.lowercased() })
        return order.filter(present.contains)
    }

    /// A lightweight tooltip that follows the cursor so you can read a node
    /// without clicking — shows title, kind, and connection count.
    @ViewBuilder private var hoverTooltip: some View {
        if let i = hovered, graph.nodes.indices.contains(i), let pt = hoverPoint,
           graph.nodes[i].id != selected {
            let node = graph.nodes[i]
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 5) {
                    Image(systemName: Theme.kindGlyph(node.kind))
                        .font(.caption2).foregroundStyle(Theme.kindColor(node.kind))
                    Text(node.kind.capitalized).font(.caption2.weight(.semibold))
                        .foregroundStyle(.secondary)
                }
                Text(node.title).font(.caption).lineLimit(2)
                Text("\(degree[node.id] ?? 0) connections")
                    .font(.caption2).foregroundStyle(.tertiary)
            }
            .padding(8)
            .frame(maxWidth: 240, alignment: .leading)
            .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 8))
            .overlay(RoundedRectangle(cornerRadius: 8).stroke(.separator, lineWidth: 0.5))
            .offset(x: pt.x + 14, y: pt.y + 14)
            .allowsHitTesting(false)
        }
    }

    private var zoomBadge: some View {
        Text("\(Int((scale * 100).rounded()))%")
            .font(.caption2.monospacedDigit())
            .foregroundStyle(.secondary)
            .padding(.horizontal, 7).padding(.vertical, 3)
            .background(.regularMaterial, in: Capsule())
    }

    @ViewBuilder private var inspector: some View {
        if let id = selected, let node = graph.nodes.first(where: { $0.id == id }) {
            VStack(alignment: .leading, spacing: 10) {
                HStack {
                    Image(systemName: Theme.kindGlyph(node.kind)).foregroundStyle(Theme.kindColor(node.kind))
                    Text(node.kind.capitalized).font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                    Spacer()
                    Button { selected = nil } label: { Image(systemName: "xmark.circle.fill") }
                        .buttonStyle(.borderless).foregroundStyle(.secondary)
                }
                Text(node.title).font(.headline).lineLimit(4)
                HStack(spacing: 6) {
                    Chip(text: "\(node.chunks) chunks")
                    ForEach(node.tags.prefix(3), id: \.self) { Chip(text: $0, color: .accentColor, filled: true) }
                }
                Text("\(connectionCount(of: id)) connections").font(.caption).foregroundStyle(.tertiary)
                Button {
                    withAnimation(.snappy) { nav.open(node.id, title: node.title) }
                } label: { Label("Open Paper", systemImage: "arrow.up.forward.square") }
                    .buttonStyle(.borderedProminent)
            }
            .padding(12)
            .frame(width: 280, alignment: .leading)
            .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 12))
            .overlay(RoundedRectangle(cornerRadius: 12).stroke(.separator, lineWidth: 0.5))
            .transition(.move(edge: .trailing).combined(with: .opacity))
        }
    }

    // MARK: Interaction

    private func dragGesture(center: CGPoint) -> some Gesture {
        DragGesture(minimumDistance: 0)
            .onChanged { value in
                switch drag {
                case .none:
                    if let hit = nodeIndex(at: value.startLocation, center: center) {
                        drag = .node(hit)
                        graph.pin(hit, true)
                    } else {
                        drag = .pan
                        baseOffset = offset
                    }
                case .pan:
                    offset = CGSize(width: baseOffset.width + value.translation.width,
                                    height: baseOffset.height + value.translation.height)
                case .node(let i):
                    let model = CGPoint(x: (value.location.x - center.x - offset.width) / scale,
                                        y: (value.location.y - center.y - offset.height) / scale)
                    graph.setPosition(i, to: model)
                }
            }
            .onEnded { value in
                let moved = abs(value.translation.width) + abs(value.translation.height) > 4
                switch drag {
                case .node(let i):
                    graph.pin(i, false)
                    if moved { graph.reheat() } else {
                        // Tap → keep it selected (so its neighborhood stays lit
                        // on return) and open it in a tab, like a Library click.
                        let node = graph.nodes[i]
                        selected = node.id
                        withAnimation(.snappy) { nav.open(node.id, title: node.title) }
                    }
                case .pan:
                    if !moved { withAnimation(.snappy) { selected = nil } }        // tap on background
                case .none:
                    break
                }
                drag = .none
            }
    }

    private func nodeIndex(at location: CGPoint, center: CGPoint) -> Int? {
        let model = CGPoint(x: (location.x - center.x - offset.width) / scale,
                            y: (location.y - center.y - offset.height) / scale)
        var best: (Int, CGFloat)?
        for (i, node) in graph.nodes.enumerated() {
            let dx = node.pos.x - model.x, dy = node.pos.y - model.y
            let d = sqrt(dx * dx + dy * dy)
            let hitR = radius(node) + 6 / scale
            if d <= hitR, best == nil || d < best!.1 { best = (i, d) }
        }
        return best?.0
    }

    /// Node radius in model space: base size from chunk count, plus a bump for
    /// well-connected nodes so hubs visually stand out.
    private func radius(_ node: ForceGraph.Node) -> CGFloat {
        let base = graph.radius(node)
        let deg = CGFloat(degree[node.id] ?? 0)
        return base + min(8, sqrt(deg) * 1.6)
    }

    /// Degree threshold above which a node is treated as a hub (always labeled).
    /// Roughly the 70th percentile, with a sane floor.
    private var hubDegreeFloor: Int {
        let degs = degree.values.sorted()
        guard !degs.isEmpty else { return .max }
        let idx = Int(Double(degs.count - 1) * 0.7)
        return max(3, degs[idx])
    }

    private var selectedNeighborIds: Set<String> {
        guard let id = selected,
              let idx = graph.nodes.firstIndex(where: { $0.id == id }) else { return [] }
        var ids = Set<String>()
        for e in graph.edges {
            if e.a == idx { ids.insert(graph.nodes[e.b].id) }
            if e.b == idx { ids.insert(graph.nodes[e.a].id) }
        }
        return ids
    }

    private func connectionCount(of id: String) -> Int { degree[id] ?? 0 }

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
        let fit = min((viewCenter.x * 2 - 80) / w, (viewCenter.y * 2 - 80) / h)
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
        loading = true; error = nil; selected = nil; hovered = nil
        do {
            let resp = try await client.graph(neighbors: neighbors)
            graph.load(resp)
            recomputeDegree()
            resetViewport()
            graph.settle()
        } catch {
            self.error = error.localizedDescription
        }
        loading = false
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
