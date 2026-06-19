import SwiftUI

// Research history: saved roundtables, most-recent first. Selecting one reopens
// it (read-only, ready to continue). Swipe or right-click to delete.
struct RoundtableHistoryView: View {
    let currentId: UUID
    let onOpen: (RoundtableRecord) -> Void
    let onClose: () -> Void

    @State private var records: [RoundtableRecord] = []

    private static let dateFormat: RelativeDateTimeFormatter = {
        let f = RelativeDateTimeFormatter()
        f.unitsStyle = .abbreviated
        return f
    }()

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Label("Research History", systemImage: "clock.arrow.circlepath").font(.headline)
                Spacer()
                Button("Done", action: onClose).keyboardShortcut(.defaultAction)
            }
            .padding(.horizontal, 16).padding(.vertical, 12)
            .background(.bar)
            Divider()

            if records.isEmpty {
                EmptyStateView(icon: "sparkles.rectangle.stack",
                               title: "No saved research yet",
                               message: "Run a roundtable and it's saved here automatically — reopen and continue it anytime.")
            } else {
                List {
                    ForEach(records) { record in
                        Button { onOpen(record) } label: { row(record) }
                            .buttonStyle(.plain)
                            .listRowBackground(record.id == currentId ? Color.accentColor.opacity(0.10) : Color.clear)
                            .contextMenu {
                                Button("Delete", systemImage: "trash", role: .destructive) { delete(record) }
                            }
                            .swipeActions {
                                Button("Delete", systemImage: "trash", role: .destructive) { delete(record) }
                            }
                    }
                }
                .listStyle(.inset)
            }
        }
        .frame(width: 520, height: 560)
        .onAppear(perform: reload)
    }

    private func row(_ record: RoundtableRecord) -> some View {
        HStack(spacing: 12) {
            Image(systemName: "person.3.sequence.fill").foregroundStyle(.tint)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text(record.title).font(.headline).lineLimit(1)
                    if record.id == currentId {
                        Text("open").font(.caption2.weight(.semibold)).foregroundStyle(.tint)
                    }
                }
                HStack(spacing: 6) {
                    Text("\(record.turns.count) contributions")
                    Text("·")
                    Text("\(record.personas.count) agents")
                    if record.synthesis != nil {
                        Text("·")
                        Label("synthesis", systemImage: "checkmark.seal.fill").foregroundStyle(.green)
                    }
                    Text("·")
                    Text(Self.dateFormat.localizedString(for: record.updatedAt, relativeTo: Date()))
                }
                .font(.caption).foregroundStyle(.secondary)
            }
            Spacer()
            Image(systemName: "chevron.right").font(.caption).foregroundStyle(.tertiary)
        }
        .padding(.vertical, 4)
        .contentShape(Rectangle())
    }

    private func reload() {
        records = RoundtableStore.shared.all()
    }

    private func delete(_ record: RoundtableRecord) {
        RoundtableStore.shared.delete(record.id)
        reload()
    }
}
