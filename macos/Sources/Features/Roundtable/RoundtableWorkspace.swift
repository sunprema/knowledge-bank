import SwiftUI
import Observation

// Browser-style tabs for the Roundtable: each tab is an independent debate with
// its own session, so one objective can run (streaming live) while you read the
// analysis in another tab — or view two side by side in split view.

/// One debate tab: a session plus the setup state that produced it.
@MainActor
@Observable
final class DebateTab: Identifiable {
    let id = UUID()
    let session: RoundtableSession
    var objective: String
    /// Which personas from the shared library sit at this debate's table. Empty
    /// ⇒ "not chosen yet" — the setup screen treats that as all personas and
    /// initializes the set on first appearance.
    var selectedPersonaIds: Set<String> = []
    // Setup options, kept per-tab so each debate remembers its configuration.
    var rounds = 2
    var scoreEnabled = true
    var convergeEnabled = true
    var moderatedEnabled = false

    init(objective: String = "") {
        self.session = RoundtableSession()
        self.objective = objective
    }

    /// Tab label: the objective (typed or running), else a placeholder.
    var title: String {
        let typed = objective.trimmingCharacters(in: .whitespacesAndNewlines)
        if !typed.isEmpty { return typed }
        let running = session.objective.trimmingCharacters(in: .whitespacesAndNewlines)
        return running.isEmpty ? "New debate" : running
    }
}

/// The set of open debate tabs, with selection and split-view state.
@MainActor
@Observable
final class RoundtableWorkspace {
    private(set) var tabs: [DebateTab]
    var selection: UUID
    /// Right pane in split view (left pane is `selection`); nil ⇒ single pane.
    private(set) var splitSelection: UUID?

    init() {
        let first = DebateTab()
        tabs = [first]
        selection = first.id
        splitSelection = nil
    }

    var isSplit: Bool { splitSelection != nil }
    func tab(_ id: UUID) -> DebateTab? { tabs.first { $0.id == id } }
    var selected: DebateTab? { tab(selection) }
    var splitTab: DebateTab? { splitSelection.flatMap { tab($0) } }

    @discardableResult
    func newTab(objective: String = "") -> DebateTab {
        let t = DebateTab(objective: objective)
        tabs.append(t)
        selection = t.id
        splitSelection = nil
        return t
    }

    func select(_ id: UUID) {
        // Selecting the split pane's tab collapses the split onto it.
        if splitSelection == id { splitSelection = nil }
        selection = id
    }

    func openInSplit(_ id: UUID) {
        guard id != selection else { return }   // can't split a tab with itself
        splitSelection = id
    }
    func setLeft(_ id: UUID) { selection = id }
    func setRight(_ id: UUID) { if id != selection { splitSelection = id } }
    func closeSplit() { splitSelection = nil }

    /// Open a saved record in a fresh tab (so it never clobbers a running debate).
    func openRecord(_ record: RoundtableRecord) {
        let t = newTab(objective: record.objective)
        t.session.loadRecord(record)
    }

    func close(_ id: UUID) {
        guard let idx = tabs.firstIndex(where: { $0.id == id }) else { return }
        tabs[idx].session.reset()        // stop any running stream
        if splitSelection == id { splitSelection = nil }
        let wasSelected = selection == id
        tabs.remove(at: idx)
        if tabs.isEmpty {                // always keep at least one tab
            let t = DebateTab()
            tabs = [t]
            selection = t.id
            return
        }
        if wasSelected {
            selection = tabs[min(idx, tabs.count - 1)].id
        }
    }
}

// MARK: - Tab strip

struct WorkspaceTabStrip: View {
    @Bindable var workspace: RoundtableWorkspace

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                ForEach(workspace.tabs) { tab in
                    DebateTabChip(
                        title: tab.title,
                        running: tab.session.isRunning,
                        selected: workspace.selection == tab.id || workspace.splitSelection == tab.id,
                        onSelect: { withAnimation(.snappy) { workspace.select(tab.id) } },
                        onClose: { withAnimation(.snappy) { workspace.close(tab.id) } })
                        .contextMenu {
                            Button("Open in Split View", systemImage: "rectangle.split.2x1") {
                                withAnimation(.snappy) { workspace.openInSplit(tab.id) }
                            }
                            if workspace.isSplit {
                                Button("Set as Right Pane", systemImage: "rectangle.righthalf.inset.filled") {
                                    withAnimation(.snappy) { workspace.setRight(tab.id) }
                                }
                                Button("Set as Left Pane", systemImage: "rectangle.lefthalf.inset.filled") {
                                    withAnimation(.snappy) { workspace.setLeft(tab.id) }
                                }
                                Button("Close Split", systemImage: "rectangle") {
                                    withAnimation(.snappy) { workspace.closeSplit() }
                                }
                            }
                            Divider()
                            Button("Close Tab", systemImage: "xmark") {
                                withAnimation(.snappy) { workspace.close(tab.id) }
                            }
                        }
                }
                Button { withAnimation(.snappy) { _ = workspace.newTab() } } label: {
                    Image(systemName: "plus").font(.callout)
                }
                .buttonStyle(.borderless)
                .padding(.horizontal, 6)
                .help("New debate")
            }
            .padding(.horizontal, 10).padding(.vertical, 6)
        }
        .background(.bar)
    }
}

private struct DebateTabChip: View {
    let title: String
    let running: Bool
    let selected: Bool
    let onSelect: () -> Void
    let onClose: () -> Void

    @State private var hovering = false

    var body: some View {
        HStack(spacing: 6) {
            if running {
                ProgressView().controlSize(.mini).scaleEffect(0.7).frame(width: 12)
            } else {
                Image(systemName: "person.3.sequence")
                    .font(.caption2)
                    .foregroundStyle(selected ? Color.accentColor : .secondary)
            }
            Text(title)
                .font(.callout).lineLimit(1)
                .frame(maxWidth: 150, alignment: .leading)
            Button(action: onClose) {
                Image(systemName: "xmark").font(.system(size: 8, weight: .bold)).padding(3)
            }
            .buttonStyle(.borderless)
            .opacity(hovering || selected ? 1 : 0)
            .help("Close tab")
        }
        .padding(.horizontal, 10).padding(.vertical, 6)
        .background {
            RoundedRectangle(cornerRadius: 8)
                .fill(selected ? Color.accentColor.opacity(0.15) : (hovering ? Color.secondary.opacity(0.1) : .clear))
        }
        .overlay {
            RoundedRectangle(cornerRadius: 8)
                .stroke(selected ? Color.accentColor.opacity(0.4) : .clear, lineWidth: 0.5)
        }
        .contentShape(Rectangle())
        .onTapGesture(perform: onSelect)
        .onHover { hovering = $0 }
    }
}
