import SwiftUI

// The scorecard: a radar over the four dimensions (Feasibility / Market /
// Defensibility / Timing). The panel average is the filled polygon; each agent's
// own rating is a thin colored outline, so disagreement is visible at a glance.
struct RadarChart: View {
    let scores: [AgentScore]
    /// persona id → its color (for the per-agent outlines).
    let color: (String) -> Color

    var body: some View {
        Canvas { ctx, size in
            let n = ScoreDimensions.count
            let radius = min(size.width, size.height) / 2 - 20
            let center = CGPoint(x: size.width / 2, y: size.height / 2)

            func point(_ value: Double, _ i: Int, scale: CGFloat = 1) -> CGPoint {
                let angle = (-90.0 + Double(i) * (360.0 / Double(n))) * .pi / 180.0
                let r = CGFloat(value / ScoreDimensions.maxValue) * radius * scale
                return CGPoint(x: center.x + r * CGFloat(cos(angle)),
                               y: center.y + r * CGFloat(sin(angle)))
            }
            func polygon(_ values: [Double]) -> Path {
                var p = Path()
                for i in 0..<n {
                    let q = point(values[i], i)
                    if i == 0 { p.move(to: q) } else { p.addLine(to: q) }
                }
                p.closeSubpath()
                return p
            }

            // Concentric grid rings (2,4,6,8,10) + spokes.
            for ring in stride(from: 2.0, through: ScoreDimensions.maxValue, by: 2.0) {
                ctx.stroke(polygon(Array(repeating: ring, count: n)),
                           with: .color(.secondary.opacity(0.16)), lineWidth: 0.5)
            }
            for i in 0..<n {
                var spoke = Path()
                spoke.move(to: center)
                spoke.addLine(to: point(ScoreDimensions.maxValue, i))
                ctx.stroke(spoke, with: .color(.secondary.opacity(0.22)), lineWidth: 0.5)
            }

            // Each agent's polygon.
            for s in scores {
                ctx.stroke(polygon(s.values), with: .color(color(s.personaId).opacity(0.55)), lineWidth: 1)
            }

            // Panel average, filled.
            if let avg = ScoreDimensions.averages(scores) {
                let path = polygon(avg)
                ctx.fill(path, with: .color(.accentColor.opacity(0.20)))
                ctx.stroke(path, with: .color(.accentColor), lineWidth: 2)
            }

            // Axis labels just outside the rings.
            for i in 0..<n {
                let lp = point(ScoreDimensions.maxValue, i, scale: 1.18)
                ctx.draw(Text(ScoreDimensions.labels[i])
                            .font(.system(size: 8, weight: .semibold))
                            .foregroundStyle(.secondary),
                         at: lp, anchor: .center)
            }
        }
    }
}

/// Compact scorecard pinned on the canvas once ratings arrive: the radar, the
/// overall score, per-dimension averages, and an agent legend.
@MainActor
struct ScorecardCard: View {
    @Bindable var session: RoundtableSession

    private var averages: [Double]? { ScoreDimensions.averages(session.scores) }
    private var overall: Double? { averages.map { $0.reduce(0, +) / Double($0.count) } }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Label("Scorecard", systemImage: "chart.dots.scatter")
                    .font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                Spacer()
                if let overall {
                    Text(String(format: "%.1f/10", overall))
                        .font(.caption.bold().monospacedDigit()).foregroundStyle(.tint)
                }
            }

            RadarChart(scores: session.scores) { session.persona($0)?.color ?? .secondary }
                .frame(width: 178, height: 178)

            if let averages {
                VStack(spacing: 2) {
                    ForEach(Array(ScoreDimensions.labels.enumerated()), id: \.offset) { i, label in
                        HStack {
                            Text(label).font(.caption2).foregroundStyle(.secondary)
                            Spacer()
                            Text(String(format: "%.1f", averages[i]))
                                .font(.caption2.monospacedDigit().weight(.medium))
                        }
                    }
                }
            }

            // Agent legend.
            FlowLegend(scores: session.scores, name: { session.persona($0)?.name ?? "" },
                       color: { session.persona($0)?.color ?? .secondary })
        }
        .padding(12)
        .frame(width: 220)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: Theme.cardCorner))
        .overlay {
            RoundedRectangle(cornerRadius: Theme.cardCorner).stroke(.separator.opacity(0.5), lineWidth: 0.5)
        }
        .shadow(color: .black.opacity(0.18), radius: 12, y: 4)
    }
}

/// A simple wrapping row of agent legend chips.
private struct FlowLegend: View {
    let scores: [AgentScore]
    let name: (String) -> String
    let color: (String) -> Color

    var body: some View {
        let columns = [GridItem(.adaptive(minimum: 66), spacing: 6)]
        LazyVGrid(columns: columns, alignment: .leading, spacing: 4) {
            ForEach(scores) { s in
                HStack(spacing: 4) {
                    Circle().fill(color(s.personaId)).frame(width: 6, height: 6)
                    Text(name(s.personaId)).font(.caption2).foregroundStyle(.secondary).lineLimit(1)
                }
            }
        }
    }
}
