import SwiftUI

// A small, cohesive design vocabulary so every screen feels like one app:
// consistent section-type color coding, score formatting, and reusable chips,
// badges, and card chrome. Tuned for both light and dark via semantic colors.

enum Theme {
    static let corner: CGFloat = 12
    static let cardCorner: CGFloat = 14
    static let accent = Color.accentColor

    /// Stable color per section type — the same "method" hue everywhere.
    static func sectionColor(_ type: String) -> Color {
        switch type.lowercased() {
        case "abstract":      return .teal
        case "introduction":  return .blue
        case "background":    return .indigo
        case "method":        return .purple
        case "experiments":   return .orange
        case "applications":  return .green
        case "limitations":   return .red
        case "future_work":   return .pink
        case "conclusion":    return .brown
        case "user_notes":    return .yellow
        default:              return .gray
        }
    }

    static func sectionLabel(_ type: String) -> String {
        type.replacingOccurrences(of: "_", with: " ").capitalized
    }

    static func kindColor(_ kind: String) -> Color {
        switch kind.lowercased() {
        case "note":        return .yellow
        case "idea":        return .green
        case "reflection":  return .purple
        default:            return .accentColor   // paper
        }
    }

    static func kindGlyph(_ kind: String) -> String {
        switch kind.lowercased() {
        case "note":        return "note.text"
        case "idea":        return "lightbulb"
        case "reflection":  return "quote.bubble"
        default:            return "doc.richtext"
        }
    }

    /// Score in 0…1 → a compact percent-ish badge string.
    static func scoreText(_ score: Float) -> String {
        String(format: "%.2f", score)
    }

    static func year(_ iso: String) -> String {
        String(iso.prefix(4))
    }
}

// MARK: - Reusable components

/// A small rounded label — section types, tags, categories.
struct Chip: View {
    let text: String
    var color: Color = .secondary
    var filled: Bool = false

    var body: some View {
        Text(text)
            .font(.caption2.weight(.medium))
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
            .background {
                Capsule().fill(filled ? color.opacity(0.18) : Color.secondary.opacity(0.10))
            }
            .overlay {
                if filled { Capsule().stroke(color.opacity(0.35), lineWidth: 0.5) }
            }
            .foregroundStyle(filled ? color : .secondary)
    }
}

/// Relevance score pill, color-graded by strength.
struct ScoreBadge: View {
    let score: Float
    private var color: Color {
        switch score {
        case 0.45...: return .green
        case 0.30..<0.45: return .yellow
        default: return .secondary
        }
    }
    var body: some View {
        Text(Theme.scoreText(score))
            .font(.caption.monospacedDigit().weight(.semibold))
            .foregroundStyle(color)
            .padding(.horizontal, 7).padding(.vertical, 2)
            .background(Capsule().fill(color.opacity(0.15)))
    }
}

/// Standard card background used across result/paper/spark lists.
struct Card<Content: View>: View {
    @ViewBuilder var content: Content
    var body: some View {
        content
            .padding(14)
            .background(.background.secondary, in: RoundedRectangle(cornerRadius: Theme.cardCorner))
            .overlay {
                RoundedRectangle(cornerRadius: Theme.cardCorner)
                    .stroke(.separator.opacity(0.6), lineWidth: 0.5)
            }
    }
}

/// Consistent empty-state across screens.
struct EmptyStateView: View {
    let icon: String
    let title: String
    var message: String = ""
    var body: some View {
        VStack(spacing: 10) {
            Image(systemName: icon)
                .font(.system(size: 38, weight: .light))
                .foregroundStyle(.tertiary)
            Text(title).font(.headline).foregroundStyle(.secondary)
            if !message.isEmpty {
                Text(message)
                    .font(.subheadline)
                    .foregroundStyle(.tertiary)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 360)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(40)
    }
}
