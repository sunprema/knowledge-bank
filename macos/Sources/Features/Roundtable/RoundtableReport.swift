import SwiftUI
import AppKit
import UniformTypeIdentifiers

// Export a roundtable as a styled, shareable PDF report — the synthesis up top,
// the scorecard, then the full panel discussion. Rendered from SwiftUI via
// `ImageRenderer` so it carries the app's typography and persona colors, then
// paginated into US-Letter pages. Drives off `RoundtableRecord`, so live and
// saved sessions share one path (RoundtableSession.snapshotRecord()).

// MARK: - The printable report

private let reportDateFormat: DateFormatter = {
    let f = DateFormatter()
    f.dateStyle = .long
    f.timeStyle = .short
    return f
}()

struct RoundtableReportView: View {
    let record: RoundtableRecord

    private var debaters: [Persona] { record.personas.filter { !$0.isSynth } }

    /// Real contributions only — empty placeholders and the engine's warning
    /// turns (prefixed ⚠) are dropped, matching the transcript/idea builders.
    private var visibleTurns: [AgentTurn] {
        record.turns.filter { !$0.text.isEmpty && !$0.text.hasPrefix("⚠︎") && !$0.text.hasPrefix("⚠") }
    }

    private var synthesis: String? {
        guard let synthId = record.personas.first(where: { $0.isSynth })?.id else { return nil }
        return record.turns.last {
            $0.personaId == synthId && !$0.text.isEmpty && !$0.text.hasPrefix("⚠︎") && !$0.text.hasPrefix("⚠")
        }?.text
    }

    private var reportTitle: String {
        let t = record.objective.trimmingCharacters(in: .whitespacesAndNewlines)
        return t.isEmpty ? "Untitled research" : t
    }

    private func persona(_ id: String) -> Persona? {
        if id == Persona.you.id { return .you }
        return record.personas.first { $0.id == id }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            header
            if let avg = ScoreDimensions.averages(record.scores) { scorecard(avg) }
            if let synth = synthesis { synthesisSection(synth) }
            discussion
            footer
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("ROUNDTABLE REPORT")
                .font(.caption2.weight(.bold)).tracking(1.6)
                .foregroundStyle(.secondary)
            Text(reportTitle)
                .font(.system(size: 24, weight: .bold))
                .fixedSize(horizontal: false, vertical: true)
            Text("\(reportDateFormat.string(from: record.createdAt))  ·  \(record.personas.count) agents  ·  \(visibleTurns.count) contributions")
                .font(.footnote).foregroundStyle(.secondary)
            if !debaters.isEmpty {
                Text(debaters.map { "\($0.name) — \($0.role)" }.joined(separator: "   ·   "))
                    .font(.caption).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Rectangle().fill(.secondary.opacity(0.3)).frame(height: 1).padding(.top, 4)
        }
    }

    private func scorecard(_ avg: [Double]) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Scorecard").font(.headline)
            HStack(spacing: 10) {
                ForEach(Array(zip(ScoreDimensions.labels, avg)), id: \.0) { label, value in
                    VStack(spacing: 3) {
                        Text(String(format: "%.1f", value))
                            .font(.title3.weight(.bold).monospacedDigit())
                        Text(label).font(.caption2).foregroundStyle(.secondary)
                    }
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 10)
                    .background(Color.gray.opacity(0.08), in: RoundedRectangle(cornerRadius: 8))
                }
            }
        }
    }

    private func synthesisSection(_ synth: String) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Label("Synthesis", systemImage: "wand.and.stars").font(.headline)
            MarkdownText(markdown: synth)
                .padding(14)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Color.accentColor.opacity(0.06), in: RoundedRectangle(cornerRadius: 10))
                .overlay {
                    RoundedRectangle(cornerRadius: 10).stroke(Color.accentColor.opacity(0.25), lineWidth: 1)
                }
        }
    }

    private var discussion: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Discussion").font(.headline)
            ForEach(visibleTurns) { turn in
                let p = persona(turn.personaId)
                VStack(alignment: .leading, spacing: 6) {
                    HStack(spacing: 7) {
                        Circle().fill(p?.color ?? .gray).frame(width: 9, height: 9)
                        Text(p?.name ?? "Agent").font(.subheadline.weight(.semibold))
                        if let role = p?.role, !role.isEmpty, turn.personaId != Persona.you.id {
                            Text("· \(role)").font(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                        Text("R\(turn.round)").font(.caption2.monospacedDigit()).foregroundStyle(.tertiary)
                    }
                    MarkdownText(markdown: turn.text)
                }
            }
        }
    }

    private var footer: some View {
        VStack(alignment: .leading, spacing: 6) {
            Rectangle().fill(.secondary.opacity(0.3)).frame(height: 1)
            Text("Generated by KB · \(reportDateFormat.string(from: Date()))")
                .font(.caption2).foregroundStyle(.tertiary)
        }
        .padding(.top, 4)
    }
}

// MARK: - PDF rendering & save

enum RoundtablePDF {
    /// US Letter, 72 dpi points.
    static let pageSize = CGSize(width: 612, height: 792)
    static let margin: CGFloat = 48

    /// Render the report into a temporary multi-page PDF. Returns the file URL,
    /// or nil if the PDF context couldn't be created.
    @MainActor
    static func render(_ record: RoundtableRecord) -> URL? {
        let contentWidth = pageSize.width - margin * 2
        let content = RoundtableReportView(record: record)
            .frame(width: contentWidth, alignment: .leading)
            .padding(.vertical, margin)        // top/bottom breathing room baked in
            .background(Color.white)
            .environment(\.colorScheme, .light) // a report is always light-on-white
            .tint(.accentColor)

        let renderer = ImageRenderer(content: content)
        renderer.proposedSize = ProposedViewSize(width: contentWidth, height: nil)

        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("Roundtable-\(UUID().uuidString.prefix(8)).pdf")

        var ok = false
        renderer.render { size, drawInContext in
            var box = CGRect(origin: .zero, size: pageSize)
            guard let pdf = CGContext(url as CFURL, mediaBox: &box, nil) else { return }
            let pageCount = max(1, Int(ceil(size.height / pageSize.height)))
            for page in 0..<pageCount {
                pdf.beginPDFPage(nil)
                pdf.saveGState()
                // Place the content so page `page`'s horizontal slice lands in
                // the media box; the left margin comes from the x shift. CG's
                // origin is bottom-left, hence the (pageHeight - contentHeight)
                // term to pin the top of the content to the top of page 0.
                let dy = pageSize.height - size.height + CGFloat(page) * pageSize.height
                pdf.translateBy(x: margin, y: dy)
                drawInContext(pdf)
                pdf.restoreGState()
                pdf.endPDFPage()
            }
            pdf.closePDF()
            ok = true
        }
        return ok ? url : nil
    }

    /// Render, then prompt for a save location and write the PDF there.
    @MainActor
    static func exportWithPanel(_ record: RoundtableRecord) {
        guard let tmp = render(record) else { NSSound.beep(); return }
        let panel = NSSavePanel()
        panel.allowedContentTypes = [.pdf]
        panel.nameFieldStringValue = suggestedName(record)
        panel.canCreateDirectories = true
        panel.title = "Export Roundtable Report"
        if panel.runModal() == .OK, let dest = panel.url {
            try? FileManager.default.removeItem(at: dest)
            try? FileManager.default.copyItem(at: tmp, to: dest)
        }
        try? FileManager.default.removeItem(at: tmp)
    }

    private static func suggestedName(_ record: RoundtableRecord) -> String {
        let base = record.title
            .components(separatedBy: CharacterSet(charactersIn: "/\\:?%*|\"<>"))
            .joined(separator: "-")
            .trimmingCharacters(in: .whitespaces)
        return "\(base.isEmpty ? "Roundtable" : base).pdf"
    }
}
