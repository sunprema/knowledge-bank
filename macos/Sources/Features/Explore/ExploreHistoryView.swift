import SwiftUI

// The exploration browser: saved canvases, most-recent first. Selecting one
// reopens it; swipe or right-click to delete. Mirrors ChatHistoryView.
struct ExploreHistoryView: View {
    let currentId: UUID
    let onOpen: (StoredExplore) -> Void
    let onClose: () -> Void

    @State private var boards: [StoredExplore] = []

    private static let dateFormat: RelativeDateTimeFormatter = {
        let f = RelativeDateTimeFormatter()
        f.unitsStyle = .abbreviated
        return f
    }()

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Label("Explorations", systemImage: "point.3.filled.connected.trianglepath.dotted")
                    .font(.headline)
                Spacer()
                Button("Done", action: onClose).keyboardShortcut(.defaultAction)
            }
            .padding(.horizontal, 16).padding(.vertical, 12)
            .background(.bar)
            Divider()

            if boards.isEmpty {
                EmptyStateView(icon: "point.3.connected.trianglepath.dotted",
                               title: "No saved explorations yet",
                               message: "Canvases are saved here automatically — start one and it'll show up.")
            } else {
                List {
                    ForEach(boards) { board in
                        Button { onOpen(board) } label: { row(board) }
                            .buttonStyle(.plain)
                            .listRowBackground(board.id == currentId ? Color.accentColor.opacity(0.10) : Color.clear)
                            .contextMenu {
                                Button("Delete", systemImage: "trash", role: .destructive) { delete(board) }
                            }
                            .swipeActions {
                                Button("Delete", systemImage: "trash", role: .destructive) { delete(board) }
                            }
                    }
                }
                .listStyle(.inset)
            }
        }
        .frame(width: 480, height: 540)
        .onAppear(perform: reload)
    }

    private func row(_ board: StoredExplore) -> some View {
        HStack(spacing: 12) {
            Image(systemName: "point.3.filled.connected.trianglepath.dotted")
                .foregroundStyle(.tint)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text(board.title).font(.headline).lineLimit(1)
                    if board.id == currentId {
                        Text("current").font(.caption2.weight(.semibold)).foregroundStyle(.tint)
                    }
                }
                Text("\(nodeCount(board)) messages · \(Self.dateFormat.localizedString(for: board.updatedAt, relativeTo: Date()))")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Spacer()
            Image(systemName: "chevron.right").font(.caption).foregroundStyle(.tertiary)
        }
        .padding(.vertical, 4)
        .contentShape(Rectangle())
    }

    private func nodeCount(_ board: StoredExplore) -> Int {
        board.nodes.filter { $0.role == .user || !$0.text.isEmpty }.count
    }

    private func reload() {
        boards = ExploreArchive.shared.all()
    }

    private func delete(_ board: StoredExplore) {
        ExploreArchive.shared.delete(board.id)
        reload()
    }
}
