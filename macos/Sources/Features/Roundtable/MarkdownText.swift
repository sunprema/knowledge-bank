import SwiftUI

// A small block-level markdown renderer for agent turns: headings, bullet and
// numbered lists, blockquotes, and fenced code, with inline bold/italic/code
// handled by AttributedString. SwiftUI's `Text(.init(markdown:))` only does
// inline, so this fills the gap without pulling in a dependency.
struct MarkdownText: View {
    let markdown: String

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            ForEach(Array(blocks().enumerated()), id: \.offset) { _, block in
                block.view
            }
        }
        .textSelection(.enabled)
    }

    private func blocks() -> [Block] {
        var out: [Block] = []
        var inCode = false
        var codeBuffer: [String] = []

        for raw in markdown.components(separatedBy: "\n") {
            let line = raw
            if line.trimmingCharacters(in: .whitespaces).hasPrefix("```") {
                if inCode {
                    out.append(.code(codeBuffer.joined(separator: "\n")))
                    codeBuffer.removeAll()
                }
                inCode.toggle()
                continue
            }
            if inCode { codeBuffer.append(line); continue }

            let t = line.trimmingCharacters(in: .whitespaces)
            if t.isEmpty { out.append(.spacer); continue }
            if t.hasPrefix("### ") { out.append(.heading(String(t.dropFirst(4)), 3)) }
            else if t.hasPrefix("## ") { out.append(.heading(String(t.dropFirst(3)), 2)) }
            else if t.hasPrefix("# ") { out.append(.heading(String(t.dropFirst(2)), 1)) }
            else if t.hasPrefix("> ") { out.append(.quote(String(t.dropFirst(2)))) }
            else if t.hasPrefix("- ") || t.hasPrefix("* ") { out.append(.bullet(String(t.dropFirst(2)))) }
            else if let num = leadingNumber(t) { out.append(.numbered(num.0, num.1)) }
            else { out.append(.paragraph(t)) }
        }
        if inCode && !codeBuffer.isEmpty { out.append(.code(codeBuffer.joined(separator: "\n"))) }
        return out
    }

    /// "3. text" → ("3", "text"); nil if the line isn't a numbered item.
    private func leadingNumber(_ s: String) -> (String, String)? {
        let digits = s.prefix { $0.isNumber }
        guard !digits.isEmpty else { return nil }
        let rest = s.dropFirst(digits.count)
        guard rest.hasPrefix(". ") else { return nil }
        return (String(digits), String(rest.dropFirst(2)))
    }
}

/// Inline markdown (`**bold**`, `*italic*`, `` `code` ``) → AttributedString,
/// degrading to plain text if parsing fails (e.g. a half-streamed token).
private func inline(_ s: String) -> AttributedString {
    (try? AttributedString(markdown: s,
        options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)))
        ?? AttributedString(s)
}

private enum Block {
    case heading(String, Int)
    case paragraph(String)
    case bullet(String)
    case numbered(String, String)
    case quote(String)
    case code(String)
    case spacer

    @ViewBuilder var view: some View {
        switch self {
        case let .heading(text, level):
            Text(inline(text))
                .font(level == 1 ? .title3.bold() : level == 2 ? .headline : .subheadline.weight(.semibold))
                .padding(.top, 2)
        case let .paragraph(text):
            Text(inline(text)).font(.callout).frame(maxWidth: .infinity, alignment: .leading)
        case let .bullet(text):
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Circle().fill(.tint).frame(width: 5, height: 5).padding(.top, 5)
                Text(inline(text)).font(.callout).frame(maxWidth: .infinity, alignment: .leading)
            }
        case let .numbered(num, text):
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text("\(num).").font(.callout.monospacedDigit().weight(.semibold)).foregroundStyle(.tint)
                Text(inline(text)).font(.callout).frame(maxWidth: .infinity, alignment: .leading)
            }
        case let .quote(text):
            HStack(spacing: 8) {
                RoundedRectangle(cornerRadius: 2).fill(.tint.opacity(0.5)).frame(width: 3)
                Text(inline(text)).font(.callout.italic()).foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        case let .code(text):
            Text(text).font(.caption.monospaced())
                .padding(10)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(.background.secondary, in: RoundedRectangle(cornerRadius: 8))
        case .spacer:
            Color.clear.frame(height: 2)
        }
    }
}
