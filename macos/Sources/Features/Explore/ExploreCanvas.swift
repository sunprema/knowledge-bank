import SwiftUI
import AppKit

/// Named coordinate space for the canvas, so a connector's drag reports cursor
/// positions in the canvas's screen space (matching the edge math).
private let kExploreSpace = "exploreCanvas"

/// A connection being dragged from a node's connector toward a drop target.
private struct LinkDrag {
    let sourceId: String
    let side: NodeSide
    var location: CGPoint   // canvas screen space
}

// The Explore canvas: an XYFlow-style, free-form board where the conversation
// grows as a tree/DAG of message nodes. Pan (drag the background), zoom (pinch),
// drag nodes to rearrange, click to select, drag a connector to wire two nodes.
// Selection drives what a new prompt continues from — pick several (⌘-click) to
// join branches. Pure presentation + gestures; ExploreView owns the streaming.
@MainActor
struct ExploreCanvas: View {
    @Bindable var store: ExploreStore
    /// Opens a clicked citation (e.g. into a reader tab) — wired by ExploreView.
    var onOpenSource: (ChatSource) -> Void = { _ in }
    /// Branch from a node out of a side, with a named edge — wired by ExploreView.
    var onBranch: (String, NodeSide, String) -> Void = { _, _, _ in }

    @State private var panStart: CGSize?
    @State private var zoomStart: CGFloat?
    @State private var didCenter = false

    // Manual connecting: a drag in progress from a node's connector, plus the
    // measured size of each card (content space) for drop hit-testing.
    @State private var linking: LinkDrag?
    @State private var nodeSizes: [String: CGSize] = [:]

    var body: some View {
        GeometryReader { geo in
            ZStack(alignment: .topLeading) {
                GridBackground(offset: store.offset, zoom: store.zoom)

                // Edges + their names in screen space (computed from the
                // transform), so we never rasterize a giant canvas.
                EdgesOverlay(store: store, offset: store.offset, zoom: store.zoom)
                EdgeLabelsOverlay(store: store, offset: store.offset, zoom: store.zoom)
                if let linking { pendingLink(linking) }

                // Node layer: positioned in content space, then transformed.
                ZStack(alignment: .topLeading) {
                    ForEach(store.nodes) { node in
                        NodeCardView(store: store, node: node,
                                     isSelected: store.selection.contains(node.id),
                                     zoom: store.zoom, onOpenSource: onOpenSource, onBranch: onBranch,
                                     onMeasure: { nodeSizes[$0] = $1 },
                                     onLinkChanged: { id, side, loc in
                                         linking = LinkDrag(sourceId: id, side: side, location: loc)
                                     },
                                     onLinkEnded: { id, side, loc in endLink(from: id, side: side, at: loc) })
                            .position(node.point)
                    }
                }
                .frame(width: geo.size.width, height: geo.size.height, alignment: .topLeading)
                .scaleEffect(store.zoom, anchor: .topLeading)
                .offset(store.offset)
            }
            .frame(width: geo.size.width, height: geo.size.height)
            .background(Color(nsColor: .underPageBackgroundColor))
            .contentShape(Rectangle())
            .coordinateSpace(name: kExploreSpace)
            .gesture(panGesture)
            .simultaneousGesture(zoomGesture)
            .onTapGesture { store.selection = [] }
            .focusable()
            .onDeleteCommand { if !store.selection.isEmpty { store.delete(store.selection) } }
            .clipped()
            .onAppear {
                if !didCenter {
                    didCenter = true
                    if store.offset == .zero, let first = store.nodes.min(by: { $0.createdAt < $1.createdAt }) {
                        store.offset = CGSize(width: geo.size.width / 2 - first.x * store.zoom,
                                              height: 60)
                    }
                }
            }
            .overlay(alignment: .bottomTrailing) { zoomControls }
        }
    }

    /// The rubber-band line drawn from the source connector to the cursor while
    /// dragging a new connection.
    private func pendingLink(_ drag: LinkDrag) -> some View {
        Canvas { ctx, _ in
            guard let src = store.node(drag.sourceId) else { return }
            let from = CGPoint(x: src.x * store.zoom + store.offset.width,
                               y: src.y * store.zoom + store.offset.height)
            let (path, _) = edgeBezier(from: from, to: drag.location, exitSide: drag.side)
            ctx.stroke(path, with: .color(src.color.opacity(0.8)),
                       style: StrokeStyle(lineWidth: 2, lineCap: .round, dash: [5, 5]))
            ctx.fill(Path(ellipseIn: CGRect(x: drag.location.x - 4, y: drag.location.y - 4,
                                            width: 8, height: 8)),
                     with: .color(src.color))
        }
        .allowsHitTesting(false)
    }

    /// Finish a connection drag: find the node under the drop point and wire it.
    private func endLink(from sourceId: String, side: NodeSide, at location: CGPoint) {
        defer { linking = nil }
        // Screen → content space.
        let p = CGPoint(x: (location.x - store.offset.width) / store.zoom,
                        y: (location.y - store.offset.height) / store.zoom)
        let target = store.nodes.last { n in
            guard n.id != sourceId else { return false }
            let size = nodeSizes[n.id] ?? CGSize(width: 300, height: 160)
            let rect = CGRect(x: n.x - size.width / 2, y: n.y - size.height / 2,
                              width: size.width, height: size.height)
            return rect.contains(p)
        }
        if let target { store.connect(from: sourceId, to: target.id, side: side) }
    }

    private var panGesture: some Gesture {
        DragGesture(minimumDistance: 2)
            .onChanged { v in
                if panStart == nil { panStart = store.offset }
                store.offset = CGSize(width: (panStart?.width ?? 0) + v.translation.width,
                                      height: (panStart?.height ?? 0) + v.translation.height)
            }
            .onEnded { _ in panStart = nil; store.save() }
    }

    private var zoomGesture: some Gesture {
        MagnifyGesture()
            .onChanged { v in
                if zoomStart == nil { zoomStart = store.zoom }
                store.zoom = min(2.2, max(0.35, (zoomStart ?? 1) * v.magnification))
            }
            .onEnded { _ in zoomStart = nil; store.save() }
    }

    private var zoomControls: some View {
        HStack(spacing: 2) {
            Button { setZoom(store.zoom - 0.15) } label: { Image(systemName: "minus") }
            Divider().frame(height: 14)
            Button { store.zoom = 1; store.save() } label: { Text("\(Int(store.zoom * 100))%").monospacedDigit() }
                .frame(width: 46)
            Divider().frame(height: 14)
            Button { setZoom(store.zoom + 0.15) } label: { Image(systemName: "plus") }
        }
        .font(.caption)
        .buttonStyle(.borderless)
        .padding(.horizontal, 8).padding(.vertical, 5)
        .background(.regularMaterial, in: Capsule())
        .overlay(Capsule().stroke(.separator, lineWidth: 0.5))
        .padding(12)
    }

    private func setZoom(_ z: CGFloat) { store.zoom = min(2.2, max(0.35, z)); store.save() }
}

// MARK: - Grid

/// Faint dotted grid that pans/zooms with the board (the signature XYFlow look).
private struct GridBackground: View {
    let offset: CGSize
    let zoom: CGFloat

    var body: some View {
        Canvas { ctx, size in
            let step = max(8, 26 * zoom)
            let dot = CGSize(width: 1.6, height: 1.6)
            let ox = offset.width.truncatingRemainder(dividingBy: step)
            let oy = offset.height.truncatingRemainder(dividingBy: step)
            var y = oy - step
            while y < size.height {
                var x = ox - step
                while x < size.width {
                    ctx.fill(Path(ellipseIn: CGRect(origin: CGPoint(x: x, y: y), size: dot)),
                             with: .color(.secondary.opacity(0.16)))
                    x += step
                }
                y += step
            }
        }
        .background(.background.secondary.opacity(0.35))
        .allowsHitTesting(false)
    }
}

// MARK: - Edges

/// A cubic bezier between two card centers that leaves the parent on `exitSide`
/// and enters the child on the opposite side, plus the curve's midpoint (for the
/// edge label). Works in whatever space the points are given (here, screen).
private func edgeBezier(from: CGPoint, to: CGPoint, exitSide: NodeSide)
    -> (path: Path, mid: CGPoint)
{
    let dist = max(40, hypot(to.x - from.x, to.y - from.y))
    let k = min(dist * 0.5, 130)
    let exit = exitSide.vector
    let enter = exitSide.opposite.vector
    let cp1 = CGPoint(x: from.x + exit.dx * k, y: from.y + exit.dy * k)
    let cp2 = CGPoint(x: to.x + enter.dx * k, y: to.y + enter.dy * k)
    var path = Path()
    path.move(to: from)
    path.addCurve(to: to, control1: cp1, control2: cp2)
    let mid = CGPoint(x: 0.125 * from.x + 0.375 * cp1.x + 0.375 * cp2.x + 0.125 * to.x,
                      y: 0.125 * from.y + 0.375 * cp1.y + 0.375 * cp2.y + 0.125 * to.y)
    return (path, mid)
}

/// Draws a directional curved edge for every incoming link, leaving the parent
/// on the link's side. A node with several links (a join) shows one per branch.
@MainActor
private struct EdgesOverlay: View {
    @Bindable var store: ExploreStore
    let offset: CGSize
    let zoom: CGFloat

    var body: some View {
        Canvas { ctx, _ in
            for child in store.nodes {
                let to = screen(child.point)
                for link in child.links {
                    guard let parent = store.node(link.id) else { continue }
                    let from = screen(parent.point)
                    let (path, _) = edgeBezier(from: from, to: to, exitSide: link.side)
                    let active = child.status == .streaming
                    ctx.stroke(path, with: .color(child.color.opacity(active ? 0.7 : 0.32)),
                               style: StrokeStyle(lineWidth: active ? 2 : 1.5, lineCap: .round))
                }
            }
        }
        .allowsHitTesting(false)
    }

    private func screen(_ p: CGPoint) -> CGPoint {
        CGPoint(x: p.x * zoom + offset.width, y: p.y * zoom + offset.height)
    }
}

// MARK: - Edge labels

/// The named-edge pills (e.g. "venture capitalist view"), floated at each
/// labeled edge's midpoint. Tapping one renames the branch.
@MainActor
private struct EdgeLabelsOverlay: View {
    @Bindable var store: ExploreStore
    let offset: CGSize
    let zoom: CGFloat

    private struct Edge: Identifiable {
        let childId: String, parentId: String, label: String
        let mid: CGPoint
        var id: String { childId + "←" + parentId }
    }

    @State private var editing: String?
    @State private var draft = ""

    private var edges: [Edge] {
        var out: [Edge] = []
        for child in store.nodes {
            let to = screen(child.point)
            for link in child.links {
                guard let parent = store.node(link.id) else { continue }
                let mid = edgeBezier(from: screen(parent.point), to: to, exitSide: link.side).mid
                out.append(Edge(childId: child.id, parentId: link.id, label: link.label, mid: mid))
            }
        }
        return out
    }

    var body: some View {
        ZStack(alignment: .topLeading) {
            ForEach(edges) { edge in
                marker(edge)
                    .position(edge.mid)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }

    /// A named edge shows its label as a pill; an unnamed one shows a small dot.
    /// Either opens a popover to rename or delete the edge.
    @ViewBuilder private func marker(_ edge: Edge) -> some View {
        Group {
            if edge.label.isEmpty {
                Image(systemName: "slider.horizontal.3")
                    .font(.system(size: 8, weight: .bold))
                    .foregroundStyle(.secondary)
                    .frame(width: 18, height: 18)
                    .background(Circle().fill(.regularMaterial))
                    .overlay(Circle().stroke(.separator, lineWidth: 0.5))
            } else {
                Text(edge.label)
                    .font(.system(size: 10, weight: .semibold))
                    .lineLimit(1)
                    .padding(.horizontal, 8).padding(.vertical, 3)
                    .background(Capsule().fill(.regularMaterial))
                    .overlay(Capsule().stroke(.separator, lineWidth: 0.5))
            }
        }
        .shadow(color: .black.opacity(0.12), radius: 3, y: 1)
        .fixedSize()
        .onTapGesture { draft = edge.label; editing = edge.id }
        .popover(isPresented: Binding(get: { editing == edge.id },
                                      set: { if !$0 { editing = nil } })) {
            VStack(alignment: .leading, spacing: 8) {
                Text("Edge").font(.subheadline.weight(.semibold))
                TextField("Name this branch (optional)", text: $draft)
                    .textFieldStyle(.roundedBorder).frame(width: 240)
                    .onSubmit { commit(edge) }
                HStack {
                    Button("Delete edge", role: .destructive) {
                        store.disconnect(child: edge.childId, parent: edge.parentId)
                        editing = nil
                    }
                    Spacer()
                    Button("Save") { commit(edge) }.keyboardShortcut(.defaultAction)
                }
            }
            .padding(12)
        }
    }

    private func commit(_ edge: Edge) {
        store.renameEdge(child: edge.childId, parent: edge.parentId,
                         label: draft.trimmingCharacters(in: .whitespacesAndNewlines))
        editing = nil
    }

    private func screen(_ p: CGPoint) -> CGPoint {
        CGPoint(x: p.x * zoom + offset.width, y: p.y * zoom + offset.height)
    }
}

// MARK: - Node card

@MainActor
private struct NodeCardView: View {
    @Bindable var store: ExploreStore
    let node: ConvNode
    let isSelected: Bool
    let zoom: CGFloat
    var onOpenSource: (ChatSource) -> Void = { _ in }
    var onBranch: (String, NodeSide, String) -> Void = { _, _, _ in }
    var onMeasure: (String, CGSize) -> Void = { _, _ in }
    var onLinkChanged: (String, NodeSide, CGPoint) -> Void = { _, _, _ in }
    var onLinkEnded: (String, NodeSide, CGPoint) -> Void = { _, _, _ in }

    @State private var drag: CGSize = .zero
    @State private var showSources = false
    @State private var showZen = false
    @State private var branchSide: NodeSide?
    @State private var branchName = ""

    @Environment(PersonaStore.self) private var personaStore: PersonaStore?

    private var width: CGFloat { node.role == .user ? 250 : 300 }

    var body: some View {
        VStack(spacing: 0) {
            accentStrip
            header
            Divider().overlay(node.color.opacity(0.18))
            VStack(alignment: .leading, spacing: 8) {
                if node.role == .user { promptBody } else { assistantBody }
            }
            .padding(10)
        }
        .frame(width: width)
        .background(Color(nsColor: .controlBackgroundColor))
        .clipShape(RoundedRectangle(cornerRadius: 11, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 11, style: .continuous)
                .stroke(isSelected ? node.color : node.color.opacity(0.35),
                        lineWidth: isSelected ? 2 : 1)
        }
        .background(GeometryReader { g in
            Color.clear
                .onAppear { onMeasure(node.id, g.size) }
                .onChange(of: g.size) { _, new in onMeasure(node.id, new) }
        })
        .shadow(color: .black.opacity(isSelected ? 0.18 : 0.08),
                radius: isSelected ? 12 : 6, y: 3)
        .overlay(alignment: .top) { connector(.top) }
        .overlay(alignment: .bottom) { connector(.bottom) }
        .overlay(alignment: .leading) { connector(.leading) }
        .overlay(alignment: .trailing) { connector(.trailing) }
        .overlay(alignment: .topTrailing) { if isSelected { deleteButton } }
        .offset(drag)
        .gesture(dragGesture)
        .onTapGesture { store.select(node.id, additive: NSEvent.modifierFlags.contains(.command)) }
        .contextMenu {
            Button("Select") { store.select(node.id, additive: false) }
            Button(isSelected ? "Remove from selection" : "Add to selection") {
                store.select(node.id, additive: true)
            }
            Divider()
            Menu("Branch…") {
                ForEach(NodeSide.allCases, id: \.self) { side in
                    Button { startBranch(side) } label: { Label(side.label, systemImage: side.glyph) }
                }
            }
            Button("Delete node", role: .destructive) { store.delete([node.id]) }
        }
        .sheet(isPresented: $showZen) {
            NodeZenView(node: node,
                        modelLabel: node.personaId.flatMap(personaModelLabel),
                        onOpenSource: { s in showZen = false; onOpenSource(s) })
        }
    }

    /// The colored top edge — the signature node-editor accent.
    private var accentStrip: some View {
        Rectangle().fill(node.color).frame(height: 3)
    }

    /// A small red ✕ shown on a selected node for quick deletion.
    private var deleteButton: some View {
        Button { store.delete([node.id]) } label: {
            Image(systemName: "xmark")
                .font(.system(size: 7, weight: .black))
                .foregroundStyle(.white)
                .frame(width: 15, height: 15)
                .background(Circle().fill(.red))
        }
        .buttonStyle(.plain)
        .offset(x: 6, y: -6)
        .help("Delete node")
    }

    // MARK: Connectors (one per side)

    /// A connection dot on a side. **Click** to start a named branch (new node)
    /// out of that side; **drag** to another node to connect them.
    private func connector(_ side: NodeSide) -> some View {
        Circle()
            .fill(Color(nsColor: .windowBackgroundColor))
            .frame(width: 12, height: 12)
            .overlay(Circle().stroke(node.color.opacity(0.9), lineWidth: 1.6))
            .overlay(Image(systemName: "plus").font(.system(size: 6, weight: .black))
                .foregroundStyle(node.color.opacity(0.9)))
            .contentShape(Circle())
            .offset(x: side == .leading ? -5 : (side == .trailing ? 5 : 0),
                    y: side == .top ? -5 : (side == .bottom ? 5 : 0))
            .help("Drag to connect · click to branch \(side.label.lowercased())")
            .onTapGesture { startBranch(side) }
            .gesture(
                DragGesture(minimumDistance: 4, coordinateSpace: .named(kExploreSpace))
                    .onChanged { v in onLinkChanged(node.id, side, v.location) }
                    .onEnded { v in onLinkEnded(node.id, side, v.location) }
            )
            .popover(isPresented: Binding(get: { branchSide == side },
                                          set: { if !$0 { branchSide = nil } }),
                     arrowEdge: arrowEdge(side)) {
                branchForm(side)
            }
    }

    private func startBranch(_ side: NodeSide) {
        branchName = ""
        branchSide = side
    }

    private func arrowEdge(_ side: NodeSide) -> Edge {
        switch side { case .top: .top; case .bottom: .bottom; case .leading: .leading; case .trailing: .trailing }
    }

    @ViewBuilder private func branchForm(_ side: NodeSide) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Label("Branch \(side.label.lowercased())", systemImage: side.glyph)
                .font(.subheadline.weight(.semibold))
            Text("Name the branch — it becomes the lens the model answers through.")
                .font(.caption).foregroundStyle(.secondary).frame(maxWidth: 240, alignment: .leading)
            TextField("e.g. Venture capitalist view", text: $branchName)
                .textFieldStyle(.roundedBorder).frame(width: 250)
                .onSubmit { commitBranch(side) }
            FlowChips(options: ["Venture capitalist view", "Tech founder view",
                                "Skeptic's view", "First principles", "Risks & downsides"]) {
                branchName = $0
            }
            HStack {
                Spacer()
                Button("Cancel") { branchSide = nil }
                Button("Branch") { commitBranch(side) }
                    .buttonStyle(.borderedProminent)
                    .disabled(branchName.trimmingCharacters(in: .whitespaces).isEmpty)
            }
        }
        .padding(14)
        .frame(width: 290)
    }

    private func commitBranch(_ side: NodeSide) {
        let name = branchName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else { return }
        branchSide = nil
        onBranch(node.id, side, name)
    }

    private var dragGesture: some Gesture {
        DragGesture()
            .onChanged { v in drag = CGSize(width: v.translation.width / zoom,
                                            height: v.translation.height / zoom) }
            .onEnded { _ in
                store.move(node.id, to: CGPoint(x: node.x + drag.width, y: node.y + drag.height))
                drag = .zero
                store.save()
            }
    }

    // MARK: Header

    private var header: some View {
        HStack(spacing: 8) {
            Image(systemName: node.role == .user
                  ? (node.parents.count > 1 ? "arrow.triangle.merge" : "person.crop.circle.fill")
                  : (node.personaIcon ?? "sparkles"))
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(node.color)
                .symbolEffect(.pulse, options: .repeating,
                              isActive: node.status == .streaming && node.text.isEmpty)
                .frame(width: 16)
            VStack(alignment: .leading, spacing: 0) {
                Text(title).font(.system(size: 12, weight: .semibold)).lineLimit(1)
                if let sub = subtitle {
                    Text(sub).font(.system(size: 9)).foregroundStyle(.secondary).lineLimit(1)
                }
            }
            Spacer(minLength: 0)
            statusDot
        }
        .padding(.horizontal, 10).padding(.vertical, 7)
        .background(node.color.opacity(0.07))
    }

    private var title: String {
        if node.role == .user { return node.parents.count > 1 ? "Join · \(node.parents.count) branches" : "You" }
        return node.personaName ?? "Assistant"
    }

    private var subtitle: String? {
        guard node.role == .assistant else { return nil }
        if let pid = node.personaId, let model = personaModelLabel(pid) { return model }
        return node.personaId == nil ? "Research assistant" : nil
    }

    @ViewBuilder private var statusDot: some View {
        switch node.status {
        case .streaming:
            if node.text.isEmpty { ProgressView().controlSize(.mini) } else { StreamingDot() }
        case .done:
            Image(systemName: "checkmark.circle.fill").font(.system(size: 11)).foregroundStyle(.green)
        case .error:
            Image(systemName: "exclamationmark.triangle.fill").font(.system(size: 11)).foregroundStyle(.orange)
        }
    }

    // MARK: User prompt body

    private var promptBody: some View {
        VStack(alignment: .leading, spacing: 6) {
            ioLabel("prompt", "text", onExpand: node.text.isEmpty ? nil : { showZen = true })
            contentBox { cappedMarkdown(cap: 130, alignment: .top) }
        }
    }

    // MARK: Assistant body

    @ViewBuilder private var assistantBody: some View {
        ioLabel("response", "text", onExpand: node.text.isEmpty ? nil : { showZen = true })
        if node.status == .streaming && node.text.isEmpty {
            contentBox {
                HStack(spacing: 6) {
                    Image(systemName: "magnifyingglass").font(.system(size: 10)).foregroundStyle(.secondary)
                    Text("Searching the corpus…").font(.caption).foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        } else if node.status == .error {
            contentBox {
                Text(node.text).font(.callout).foregroundStyle(.orange)
                    .frame(maxWidth: .infinity, alignment: .leading).textSelection(.enabled)
            }
        } else {
            contentBox {
                cappedMarkdown(cap: 230, alignment: node.status == .streaming ? .bottom : .top)
            }
        }
        if !node.sources.isEmpty { sourcesSection }
    }

    /// Markdown clamped to `cap` points — short text stays snug, long text clips
    /// with a bottom fade hinting "there's more" (open zen mode to read it all).
    private func cappedMarkdown(cap: CGFloat, alignment: Alignment) -> some View {
        let overflowing = node.text.count > Int(cap * 1.7)
        return MarkdownText(markdown: node.text)
            .frame(maxWidth: .infinity, alignment: .leading)
            .frame(maxHeight: cap, alignment: alignment)
            .clipped()
            .overlay(alignment: .bottom) {
                if overflowing {
                    LinearGradient(colors: [.clear, Color(nsColor: .controlBackgroundColor)],
                                   startPoint: .top, endPoint: .bottom)
                        .frame(height: 22)
                        .allowsHitTesting(false)
                }
            }
    }

    // MARK: Collapsible sources ("Outputs"-style section)

    private var sourcesSection: some View {
        VStack(alignment: .leading, spacing: 6) {
            Button { withAnimation(.snappy(duration: 0.2)) { showSources.toggle() } } label: {
                HStack(spacing: 6) {
                    Image(systemName: "quote.opening").font(.system(size: 9)).foregroundStyle(node.color)
                    Text("Sources").font(.system(size: 11, weight: .semibold))
                    Text("\(node.sources.count)").font(.system(size: 10, design: .monospaced))
                        .foregroundStyle(.secondary)
                    Spacer(minLength: 0)
                    Image(systemName: "chevron.down").font(.system(size: 9, weight: .semibold))
                        .foregroundStyle(.secondary)
                        .rotationEffect(.degrees(showSources ? 0 : -90))
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            if showSources {
                VStack(alignment: .leading, spacing: 7) {
                    ForEach(node.sources) { s in
                        Button { onOpenSource(s) } label: { sourceRow(s) }
                            .buttonStyle(.plain)
                            .disabled(!s.hasPdf)
                    }
                }
            }
        }
        .padding(.top, 2)
        .overlay(alignment: .top) { Divider().overlay(node.color.opacity(0.12)) }
    }

    private func sourceRow(_ s: ChatSource) -> some View {
        HStack(alignment: .top, spacing: 8) {
            Text("\(s.n)")
                .font(.system(size: 9, weight: .bold, design: .monospaced))
                .frame(width: 16, height: 16)
                .background(Circle().fill(Theme.sectionColor(s.sectionType).opacity(0.2)))
                .foregroundStyle(Theme.sectionColor(s.sectionType))
            VStack(alignment: .leading, spacing: 2) {
                Text(s.title).font(.caption.weight(.medium)).lineLimit(2)
                HStack(spacing: 5) {
                    Chip(text: Theme.sectionLabel(s.sectionType),
                         color: Theme.sectionColor(s.sectionType), filled: true)
                    if let p = s.page {
                        Text("p.\(p)").font(.caption2.monospacedDigit()).foregroundStyle(.tertiary)
                    }
                    if s.hasPdf {
                        Image(systemName: "arrow.up.right.square").font(.system(size: 9)).foregroundStyle(.tertiary)
                    }
                }
            }
            Spacer(minLength: 0)
        }
        .contentShape(Rectangle())
    }

    // MARK: Building blocks

    /// A small typed-port label like the node editor's `name  type` rows, with
    /// an optional "zen mode" expand button on the trailing edge.
    private func ioLabel(_ name: String, _ type: String, onExpand: (() -> Void)? = nil) -> some View {
        HStack(spacing: 6) {
            ioLabelBadge(name, type)
            Spacer(minLength: 0)
            if let onExpand {
                Button(action: onExpand) {
                    Image(systemName: "arrow.up.left.and.arrow.down.right")
                        .font(.system(size: 9, weight: .semibold))
                }
                .buttonStyle(.plain)
                .foregroundStyle(.secondary)
                .help("Zen mode — read the full text")
            }
        }
    }

    private func ioLabelBadge(_ name: String, _ type: String) -> some View {
        HStack(spacing: 6) {
            Text("T")
                .font(.system(size: 8, weight: .heavy, design: .rounded))
                .frame(width: 14, height: 14)
                .background(RoundedRectangle(cornerRadius: 3).fill(node.color.opacity(0.15)))
                .foregroundStyle(node.color)
            Text(name).font(.system(size: 11, weight: .semibold))
            Text(type).font(.system(size: 10)).foregroundStyle(.secondary)
            Spacer(minLength: 0)
        }
    }

    /// The muted preview box that wraps a node's content.
    private func contentBox<C: View>(@ViewBuilder _ content: () -> C) -> some View {
        content()
            .padding(8)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: 6).fill(Color.secondary.opacity(0.06)))
            .overlay(RoundedRectangle(cornerRadius: 6).stroke(.separator.opacity(0.5), lineWidth: 0.5))
    }

    /// Resolve a persona's model label for the card subtitle (best-effort).
    private func personaModelLabel(_ pid: String) -> String? {
        guard let p = personaStore?.personas.first(where: { $0.id == pid }) else { return nil }
        return p.model.label
    }
}

// MARK: - Zen mode

/// "Zen mode": one node's full content in a roomy, scrollable sheet — formatted
/// markdown plus its sources — for comfortable reading away from the canvas.
private struct NodeZenView: View {
    let node: ConvNode
    var modelLabel: String?
    var onOpenSource: (ChatSource) -> Void
    @Environment(\.dismiss) private var dismiss

    private var title: String {
        if node.role == .user { return node.parents.count > 1 ? "Join" : "You" }
        return node.personaName ?? "Assistant"
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 10) {
                ZStack {
                    Circle().fill(node.color.opacity(0.18)).frame(width: 30, height: 30)
                    Image(systemName: node.role == .user
                          ? "person.crop.circle.fill" : (node.personaIcon ?? "sparkles"))
                        .font(.system(size: 14, weight: .semibold)).foregroundStyle(node.color)
                }
                VStack(alignment: .leading, spacing: 1) {
                    Text(title).font(.headline)
                    if let modelLabel { Text(modelLabel).font(.caption).foregroundStyle(.secondary) }
                }
                Spacer()
                Button("Done") { dismiss() }.keyboardShortcut(.defaultAction)
            }
            .padding(.horizontal, 18).padding(.vertical, 12)
            .background(.bar)
            Divider()
            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    MarkdownText(markdown: node.text)
                    if !node.sources.isEmpty {
                        Divider()
                        VStack(alignment: .leading, spacing: 10) {
                            Label("Sources", systemImage: "quote.opening")
                                .font(.subheadline.weight(.semibold)).foregroundStyle(.secondary)
                            ForEach(node.sources) { s in
                                Button { onOpenSource(s) } label: { zenSourceRow(s) }
                                    .buttonStyle(.plain).disabled(!s.hasPdf)
                            }
                        }
                    }
                }
                .padding(22)
                .frame(maxWidth: 720, alignment: .leading)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .frame(minWidth: 580, idealWidth: 720, minHeight: 480, idealHeight: 640)
    }

    private func zenSourceRow(_ s: ChatSource) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Text("\(s.n)")
                .font(.caption.bold().monospacedDigit())
                .frame(width: 20, height: 20)
                .background(Circle().fill(Theme.sectionColor(s.sectionType).opacity(0.2)))
                .foregroundStyle(Theme.sectionColor(s.sectionType))
            VStack(alignment: .leading, spacing: 3) {
                Text(s.title).font(.callout.weight(.medium))
                HStack(spacing: 6) {
                    Chip(text: Theme.sectionLabel(s.sectionType),
                         color: Theme.sectionColor(s.sectionType), filled: true)
                    if let p = s.page { Text("p.\(p)").font(.caption.monospacedDigit()).foregroundStyle(.tertiary) }
                    if s.hasPdf { Image(systemName: "arrow.up.right.square").font(.caption2).foregroundStyle(.tertiary) }
                }
            }
            Spacer(minLength: 0)
        }
        .contentShape(Rectangle())
    }
}

// MARK: - Card building blocks

/// A small blinking dot used as the "live" status while a node streams.
private struct StreamingDot: View {
    @State private var on = false
    var body: some View {
        Circle().fill(.green).frame(width: 7, height: 7)
            .opacity(on ? 1 : 0.3)
            .animation(.easeInOut(duration: 0.7).repeatForever(autoreverses: true), value: on)
            .onAppear { on = true }
    }
}

/// Tappable suggestion chips for naming a branch, wrapping to multiple rows.
private struct FlowChips: View {
    let options: [String]
    let onPick: (String) -> Void

    var body: some View {
        FlowLayout(spacing: 5) {
            ForEach(options, id: \.self) { opt in
                Button { onPick(opt) } label: {
                    Text(opt)
                        .font(.caption2)
                        .padding(.horizontal, 8).padding(.vertical, 3)
                        .background(Capsule().fill(Color.secondary.opacity(0.15)))
                        .foregroundStyle(.primary)
                }
                .buttonStyle(.plain)
            }
        }
        .frame(width: 250, alignment: .leading)
    }
}

/// A minimal wrapping (flow) layout — rows fill left-to-right, wrapping when the
/// proposed width runs out.
private struct FlowLayout: Layout {
    var spacing: CGFloat = 6

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let maxW = proposal.width ?? .infinity
        var x: CGFloat = 0, y: CGFloat = 0, rowH: CGFloat = 0
        for s in subviews {
            let sz = s.sizeThatFits(.unspecified)
            if x > 0, x + sz.width > maxW { x = 0; y += rowH + spacing; rowH = 0 }
            x += sz.width + spacing
            rowH = max(rowH, sz.height)
        }
        return CGSize(width: maxW.isFinite ? maxW : x, height: y + rowH)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        let maxW = bounds.width
        var x = bounds.minX, y = bounds.minY, rowH: CGFloat = 0
        for s in subviews {
            let sz = s.sizeThatFits(.unspecified)
            if x > bounds.minX, x - bounds.minX + sz.width > maxW { x = bounds.minX; y += rowH + spacing; rowH = 0 }
            s.place(at: CGPoint(x: x, y: y), anchor: .topLeading, proposal: ProposedViewSize(sz))
            x += sz.width + spacing
            rowH = max(rowH, sz.height)
        }
    }
}
