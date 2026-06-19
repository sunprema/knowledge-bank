import SwiftUI

// The "fascinating part": an XYFlow-style canvas where the objective sits at the
// center and the agents orbit it as nodes, wired by edges that *flow* while an
// agent holds the floor. The active speaker's node enlarges to show its live
// stream; KB lookups and user interjections appear as their own nodes. Pure
// presentation — it reads RoundtableSession and renders.
struct RoundtableCanvas: View {
    @Bindable var session: RoundtableSession

    var body: some View {
        GeometryReader { geo in
            let layout = NodeLayout(size: geo.size, session: session)
            ZStack {
                FlowEdges(layout: layout, session: session)
                ForEach(session.personas) { persona in
                    NodeCard(persona: persona, session: session)
                        .position(layout.point(for: persona.id))
                }
                ObjectiveNode(session: session)
                    .position(layout.center)
                InterjectionNodes(layout: layout, session: session)
            }
            .frame(width: geo.size.width, height: geo.size.height)
            .overlay(alignment: .topTrailing) {
                if !session.scores.isEmpty {
                    ScorecardCard(session: session)
                        .padding(12)
                        .transition(.scale.combined(with: .opacity))
                }
            }
            .animation(.snappy, value: session.scores.count)
        }
        .background(GridBackground())
        .clipShape(RoundedRectangle(cornerRadius: Theme.cardCorner))
    }
}

// MARK: - Geometry

/// Computes where every node sits for a given canvas size: objective at center,
/// personas on a ring (synthesizer pinned to the bottom of the ring).
@MainActor
private struct NodeLayout {
    let center: CGPoint
    let radius: CGFloat
    private let points: [String: CGPoint]

    init(size: CGSize, session: RoundtableSession) {
        let c = CGPoint(x: size.width / 2, y: size.height / 2)
        let r = max(120, min(size.width, size.height) * 0.34)
        self.center = c
        self.radius = r

        // Debaters spread across the top arc; synthesizer anchored at the
        // bottom so the eye reads "inputs converge downward into a synthesis."
        var pts: [String: CGPoint] = [:]
        let debaters = session.debaters
        let n = max(debaters.count, 1)
        for (i, persona) in debaters.enumerated() {
            // Spread over the top 230° arc, leaving the bottom for the synth.
            let t = n == 1 ? 0.5 : Double(i) / Double(n - 1)
            let angle = (-205.0 + t * 230.0) * .pi / 180.0   // degrees → radians
            pts[persona.id] = CGPoint(x: c.x + r * CGFloat(cos(angle)),
                                      y: c.y + r * CGFloat(sin(angle)))
        }
        if let synth = session.synthesizer {
            pts[synth.id] = CGPoint(x: c.x, y: c.y + r)
        }
        self.points = pts
    }

    func point(for id: String) -> CGPoint { points[id] ?? center }
}

// MARK: - Background

/// Faint dotted grid, the signature XYFlow look.
private struct GridBackground: View {
    var body: some View {
        Canvas { ctx, size in
            let step: CGFloat = 26
            let dot = CGSize(width: 1.6, height: 1.6)
            var y: CGFloat = 0
            while y < size.height {
                var x: CGFloat = 0
                while x < size.width {
                    let rect = CGRect(origin: CGPoint(x: x, y: y), size: dot)
                    ctx.fill(Path(ellipseIn: rect), with: .color(.secondary.opacity(0.18)))
                    x += step
                }
                y += step
            }
        }
        .background(.background.secondary.opacity(0.4))
    }
}

// MARK: - Edges (animated flow)

/// Spokes from the objective to each persona. The active speaker's spoke flows
/// with a marching-dash animation in that persona's color; idle spokes are
/// faint. When the synthesizer speaks, every debater's edge into it lights up.
private struct FlowEdges: View {
    let layout: NodeLayout
    @Bindable var session: RoundtableSession

    var body: some View {
        TimelineView(.animation) { tl in
            let phase = -tl.date.timeIntervalSinceReferenceDate * 34
            Canvas { ctx, _ in
                for persona in session.personas {
                    let active = session.activePersona == persona.id
                    drawSpoke(&ctx, from: layout.center, to: layout.point(for: persona.id),
                              color: persona.color, active: active, phase: phase)
                }
                // Convergence edges into the synthesizer while it speaks.
                if let synth = session.synthesizer, session.activePersona == synth.id {
                    let to = layout.point(for: synth.id)
                    for d in session.debaters {
                        drawSpoke(&ctx, from: layout.point(for: d.id), to: to,
                                  color: synth.color, active: true, phase: phase)
                    }
                }
            }
        }
    }

    private func drawSpoke(_ ctx: inout GraphicsContext, from: CGPoint, to: CGPoint,
                           color: Color, active: Bool, phase: Double) {
        var path = Path()
        path.move(to: from)
        // Gentle curve via a control point nudged perpendicular to the line.
        let mid = CGPoint(x: (from.x + to.x) / 2, y: (from.y + to.y) / 2)
        let dx = to.x - from.x, dy = to.y - from.y
        let len = max(1, sqrt(dx * dx + dy * dy))
        let ctrl = CGPoint(x: mid.x - dy / len * 18, y: mid.y + dx / len * 18)
        path.addQuadCurve(to: to, control: ctrl)

        if active {
            ctx.stroke(path, with: .color(color.opacity(0.9)),
                       style: StrokeStyle(lineWidth: 2.4, lineCap: .round,
                                          dash: [7, 9], dashPhase: phase))
            // Soft glow underlay.
            ctx.stroke(path, with: .color(color.opacity(0.18)),
                       style: StrokeStyle(lineWidth: 7, lineCap: .round))
        } else {
            ctx.stroke(path, with: .color(.secondary.opacity(0.22)),
                       style: StrokeStyle(lineWidth: 1, lineCap: .round))
        }
    }
}

// MARK: - Objective node

private struct ObjectiveNode: View {
    @Bindable var session: RoundtableSession

    var body: some View {
        VStack(spacing: 5) {
            Label("Objective", systemImage: "target")
                .font(.caption2.weight(.bold))
                .foregroundStyle(.tint)
            Text(session.objective.isEmpty ? "Set an objective to begin" : session.objective)
                .font(.callout.weight(.semibold))
                .multilineTextAlignment(.center)
                .lineLimit(3)
                .frame(maxWidth: 220)
        }
        .padding(.horizontal, 16).padding(.vertical, 12)
        .background(.background, in: RoundedRectangle(cornerRadius: Theme.cardCorner))
        .overlay {
            RoundedRectangle(cornerRadius: Theme.cardCorner)
                .stroke(Color.accentColor.opacity(0.55), lineWidth: 1.5)
        }
        .shadow(color: .accentColor.opacity(0.18), radius: 14, y: 4)
        .frame(maxWidth: 250)
    }
}

// MARK: - Persona node

@MainActor
private struct NodeCard: View {
    let persona: Persona
    @Bindable var session: RoundtableSession

    private var turn: AgentTurn? {
        session.turns.last { $0.personaId == persona.id }
    }
    private var isActive: Bool { session.activePersona == persona.id }

    var body: some View {
        VStack(spacing: 8) {
            header
            if isActive, let turn { activeBody(turn) }
            modelChip
        }
        .padding(12)
        .frame(width: isActive ? 260 : 184)
        .background(.background, in: RoundedRectangle(cornerRadius: Theme.cardCorner))
        .overlay {
            RoundedRectangle(cornerRadius: Theme.cardCorner)
                .stroke(persona.color.opacity(isActive ? 0.8 : 0.3),
                        lineWidth: isActive ? 1.8 : 0.8)
        }
        .shadow(color: persona.color.opacity(isActive ? 0.35 : 0), radius: 18, y: 4)
        .scaleEffect(isActive ? 1.0 : 0.96)
        .animation(.snappy(duration: 0.3), value: isActive)
    }

    private var header: some View {
        HStack(spacing: 10) {
            ZStack {
                Circle().fill(persona.color.opacity(0.18)).frame(width: 38, height: 38)
                Image(systemName: persona.icon)
                    .font(.system(size: 17, weight: .semibold))
                    .foregroundStyle(persona.color)
                    .symbolEffect(.pulse, options: .repeating, isActive: turn?.status == .thinking)
            }
            VStack(alignment: .leading, spacing: 1) {
                Text(persona.name).font(.subheadline.weight(.semibold))
                Text(persona.role).font(.caption2).foregroundStyle(.secondary)
            }
            Spacer(minLength: 0)
            statusDot
        }
    }

    @ViewBuilder private var statusDot: some View {
        switch turn?.status {
        case .thinking:
            ProgressView().controlSize(.mini)
        case .queryingKB:
            Image(systemName: "magnifyingglass").font(.caption).foregroundStyle(persona.color)
                .symbolEffect(.pulse, options: .repeating)
        case .streaming:
            Circle().fill(.green).frame(width: 7, height: 7)
        case .done:
            if turn?.text.hasPrefix("⚠︎") == true {
                Image(systemName: "exclamationmark.triangle.fill").font(.caption2).foregroundStyle(.orange)
            } else {
                Image(systemName: "checkmark").font(.caption2.bold()).foregroundStyle(.green)
            }
        case nil:
            Circle().fill(.secondary.opacity(0.25)).frame(width: 7, height: 7)
        }
    }

    @ViewBuilder private func activeBody(_ turn: AgentTurn) -> some View {
        if turn.status == .queryingKB {
            Label("Searching the corpus…", systemImage: "books.vertical")
                .font(.caption).foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, alignment: .leading)
        } else if !turn.text.isEmpty {
            Text(turn.text.suffix(220).description)
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(4)
                .frame(maxWidth: .infinity, alignment: .leading)
                .transition(.opacity)
        }
        if !turn.citations.isEmpty {
            citationStrip(turn.citations)
        }
    }

    private func citationStrip(_ cites: [AgentCitation]) -> some View {
        HStack(spacing: 5) {
            Image(systemName: "quote.opening").font(.caption2).foregroundStyle(.tertiary)
            ForEach(cites.prefix(2)) { c in
                Chip(text: Theme.sectionLabel(c.sectionType),
                     color: Theme.sectionColor(c.sectionType), filled: true)
            }
            if cites.count > 2 { Text("+\(cites.count - 2)").font(.caption2).foregroundStyle(.tertiary) }
            Spacer(minLength: 0)
        }
    }

    private var modelChip: some View {
        HStack(spacing: 5) {
            Image(systemName: persona.model.providerGlyph)
                .font(.system(size: 9))
                .foregroundStyle(persona.model.providerColor)
            Text(persona.model.label).font(.caption2.weight(.medium)).foregroundStyle(.secondary)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 8).padding(.vertical, 3)
        .background(Capsule().fill(persona.model.providerColor.opacity(0.12)))
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

// MARK: - Interjections

/// User-injected ideas, floated near the objective with a dashed tie-line so
/// it's clear they're steering the table.
private struct InterjectionNodes: View {
    let layout: NodeLayout
    @Bindable var session: RoundtableSession

    var body: some View {
        ForEach(Array(session.interjections.enumerated()), id: \.element.id) { i, idea in
            let pos = CGPoint(x: layout.center.x + 150 + CGFloat(i % 2) * 16,
                              y: layout.center.y - 90 - CGFloat(i) * 54)
            HStack(spacing: 6) {
                Image(systemName: "lightbulb.fill").font(.caption).foregroundStyle(.yellow)
                Text(idea.text).font(.caption).lineLimit(2).frame(maxWidth: 150, alignment: .leading)
            }
            .padding(.horizontal, 10).padding(.vertical, 7)
            .background(.yellow.opacity(0.12), in: RoundedRectangle(cornerRadius: 10))
            .overlay { RoundedRectangle(cornerRadius: 10).stroke(.yellow.opacity(0.4), lineWidth: 0.8) }
            .position(pos)
            .transition(.scale.combined(with: .opacity))
        }
        .animation(.snappy, value: session.interjections.count)
    }
}
