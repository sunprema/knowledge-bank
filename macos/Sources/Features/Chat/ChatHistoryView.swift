import SwiftUI

// The chat history browser: a list of saved conversations, most-recent first.
// Selecting one resumes it; swipe or right-click to delete.
struct ChatHistoryView: View {
    let currentId: UUID
    let onOpen: (StoredConversation) -> Void
    let onClose: () -> Void

    @State private var convos: [StoredConversation] = []

    private static let dateFormat: RelativeDateTimeFormatter = {
        let f = RelativeDateTimeFormatter()
        f.unitsStyle = .abbreviated
        return f
    }()

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Label("Chat History", systemImage: "clock.arrow.circlepath")
                    .font(.headline)
                Spacer()
                Button("Done", action: onClose).keyboardShortcut(.defaultAction)
            }
            .padding(.horizontal, 16).padding(.vertical, 12)
            .background(.bar)
            Divider()

            if convos.isEmpty {
                EmptyStateView(icon: "clock",
                               title: "No past chats yet",
                               message: "Your conversations are saved here automatically — start chatting and they'll show up.")
            } else {
                List {
                    ForEach(convos) { convo in
                        Button { onOpen(convo) } label: { row(convo) }
                            .buttonStyle(.plain)
                            .listRowBackground(convo.id == currentId ? Color.accentColor.opacity(0.10) : Color.clear)
                            .contextMenu {
                                Button("Delete", systemImage: "trash", role: .destructive) { delete(convo) }
                            }
                            .swipeActions {
                                Button("Delete", systemImage: "trash", role: .destructive) { delete(convo) }
                            }
                    }
                }
                .listStyle(.inset)
            }
        }
        .frame(width: 480, height: 540)
        .onAppear(perform: reload)
    }

    private func row(_ convo: StoredConversation) -> some View {
        HStack(spacing: 12) {
            Image(systemName: "bubble.left.and.bubble.right")
                .foregroundStyle(.tint)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text(convo.title).font(.headline).lineLimit(1)
                    if convo.id == currentId {
                        Text("current").font(.caption2.weight(.semibold))
                            .foregroundStyle(.tint)
                    }
                }
                Text("\(convo.turns.count) messages · \(Self.dateFormat.localizedString(for: convo.updatedAt, relativeTo: Date()))")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Spacer()
            Image(systemName: "chevron.right").font(.caption).foregroundStyle(.tertiary)
        }
        .padding(.vertical, 4)
        .contentShape(Rectangle())
    }

    private func reload() {
        convos = ConversationStore.shared.all()
    }

    private func delete(_ convo: StoredConversation) {
        ConversationStore.shared.delete(convo.id)
        reload()
    }
}
