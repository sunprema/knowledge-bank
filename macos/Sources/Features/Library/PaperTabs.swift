import SwiftUI
import Observation

// Browser-style tabs for the Library: the document list is "home", and each
// opened paper becomes a tab. Opening a paper that's already open just selects
// it (no duplicates). Related-paper clicks open new tabs through `open`.
@MainActor
@Observable
final class PaperTabs {
    enum Selection: Hashable { case home, paper(String) }

    struct Tab: Identifiable, Hashable {
        let paperId: String
        let title: String
        var id: String { paperId }
    }

    private(set) var tabs: [Tab] = []
    var selection: Selection = .home

    // Split view: `selection` is the left pane; `splitPaperId` is the right
    // pane. `choosingSplit` means the right pane is awaiting a paper choice.
    private(set) var splitPaperId: String? = nil
    private(set) var choosingSplit = false

    var isSplit: Bool { splitPaperId != nil || choosingSplit }

    func title(for paperId: String) -> String {
        tabs.first { $0.paperId == paperId }?.title ?? paperId
    }

    private func ensureTab(_ paperId: String, _ title: String) {
        if !tabs.contains(where: { $0.paperId == paperId }) {
            tabs.append(Tab(paperId: paperId, title: title))
        }
    }

    /// Open/select a paper. While choosing a split, this fills the right pane;
    /// otherwise it becomes the active (left/single) selection.
    func open(_ paperId: String, title: String) {
        ensureTab(paperId, title)
        if choosingSplit {
            splitPaperId = paperId
            choosingSplit = false
        } else {
            selection = .paper(paperId)
        }
    }

    /// Begin a split with `paperId` as the left pane; the right awaits a choice.
    func startSplit(with paperId: String, title: String) {
        ensureTab(paperId, title)
        selection = .paper(paperId)
        splitPaperId = nil
        choosingSplit = true
    }

    /// Load a paper into a specific pane (used by Related clicks for cross-reading).
    func setLeft(_ paperId: String, title: String) {
        ensureTab(paperId, title)
        selection = .paper(paperId)
    }
    func setRight(_ paperId: String, title: String) {
        ensureTab(paperId, title)
        splitPaperId = paperId
        choosingSplit = false
    }

    func closeSplit() {
        splitPaperId = nil
        choosingSplit = false
    }

    func goHome() {
        selection = .home
        closeSplit()
    }

    func close(_ paperId: String) {
        if splitPaperId == paperId { splitPaperId = nil }
        let wasSelected = selection == .paper(paperId)
        let idx = tabs.firstIndex { $0.paperId == paperId }
        tabs.removeAll { $0.paperId == paperId }
        guard wasSelected else { return }
        // Select the neighbor that took its place, else the last tab, else home.
        if let idx, idx < tabs.count {
            selection = .paper(tabs[idx].paperId)
        } else if let last = tabs.last {
            selection = .paper(last.paperId)
        } else {
            selection = .home
            closeSplit()
        }
    }
}

// Horizontal tab bar shown above the content when papers are open. The "home"
// tab leads back to the host screen (the Library list, or the Graph canvas).
struct PaperTabStrip: View {
    let nav: PaperTabs
    var homeTitle = "Library"
    var homeIcon = "books.vertical"

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                TabChip(title: homeTitle, systemImage: homeIcon,
                        selected: nav.selection == .home && !nav.isSplit,
                        onSelect: { withAnimation(.snappy) { nav.goHome() } },
                        onClose: nil)
                ForEach(nav.tabs) { tab in
                    TabChip(title: tab.title, systemImage: "doc.text",
                            selected: nav.selection == .paper(tab.paperId) || nav.splitPaperId == tab.paperId,
                            onSelect: { withAnimation(.snappy) { nav.open(tab.paperId, title: tab.title) } },
                            onClose: { withAnimation(.snappy) { nav.close(tab.paperId) } })
                        .contextMenu {
                            Button("Open in Split View", systemImage: "rectangle.split.2x1") {
                                withAnimation(.snappy) { nav.startSplit(with: tab.paperId, title: tab.title) }
                            }
                            if nav.isSplit {
                                Button("Set as Right Pane", systemImage: "rectangle.righthalf.inset.filled") {
                                    withAnimation(.snappy) { nav.setRight(tab.paperId, title: tab.title) }
                                }
                                Button("Set as Left Pane", systemImage: "rectangle.lefthalf.inset.filled") {
                                    withAnimation(.snappy) { nav.setLeft(tab.paperId, title: tab.title) }
                                }
                            }
                            Divider()
                            Button("Close Tab", systemImage: "xmark") {
                                withAnimation(.snappy) { nav.close(tab.paperId) }
                            }
                        }
                }
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
        }
        .background(.bar)
    }
}

private struct TabChip: View {
    let title: String
    let systemImage: String
    let selected: Bool
    let onSelect: () -> Void
    let onClose: (() -> Void)?

    @State private var hovering = false

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: systemImage)
                .font(.caption2)
                .foregroundStyle(selected ? Color.accentColor : .secondary)
            Text(title)
                .font(.callout)
                .lineLimit(1)
                .frame(maxWidth: 160, alignment: .leading)
            if let onClose {
                Button(action: onClose) {
                    Image(systemName: "xmark")
                        .font(.system(size: 8, weight: .bold))
                        .padding(3)
                }
                .buttonStyle(.borderless)
                .opacity(hovering || selected ? 1 : 0)
                .help("Close tab")
            }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
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
