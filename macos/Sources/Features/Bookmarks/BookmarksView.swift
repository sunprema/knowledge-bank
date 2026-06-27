import SwiftUI

// The reading list: documents the user flagged with the bookmark toggle in the
// paper toolbar. A cover shelf like the Library; tapping a cover opens it in the
// reader, and each cover carries a "Remove" affordance (hover button + context
// menu). Bookmarks are persisted server-side (`/bookmarks`) and survive reindex.
@MainActor
struct BookmarksView: View {
    let client: KBClient
    /// Open a bookmarked document — routed to the Library reader by MainView.
    var onOpenPaper: (String, String) -> Void = { _, _ in }

    @State private var papers: [PaperMetadata] = []
    @State private var loading = true
    @State private var error: String?
    @State private var note: String?
    @State private var hoverID: String?

    private let coverW: CGFloat = 168
    private let coverH: CGFloat = 224

    var body: some View {
        Group {
            if loading {
                ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if let error {
                EmptyStateView(icon: "exclamationmark.triangle",
                               title: "Couldn't load bookmarks", message: error)
            } else if papers.isEmpty {
                EmptyStateView(icon: "bookmark",
                               title: "No bookmarks yet",
                               message: "Open a paper and tap the bookmark button in the toolbar to save it here.")
            } else {
                shelf
            }
        }
        .navigationTitle("Bookmarks")
        .task { await load() }
    }

    private var shelf: some View {
        ScrollView {
            if let note {
                Text(note).font(.caption).foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 28).padding(.top, 16)
            }
            LazyVGrid(columns: [GridItem(.adaptive(minimum: coverW, maximum: coverW), spacing: 30)],
                      alignment: .leading, spacing: 30) {
                ForEach(papers) { paper in coverCard(paper) }
            }
            .padding(28)
        }
    }

    private func coverCard(_ paper: PaperMetadata) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            CoverImage(paper: paper, client: client)
                .frame(width: coverW, height: coverH)
                .clipShape(RoundedRectangle(cornerRadius: 10))
                .overlay(RoundedRectangle(cornerRadius: 10).stroke(.black.opacity(0.12), lineWidth: 0.5))
                .shadow(color: .black.opacity(0.28), radius: hoverID == paper.id ? 14 : 8,
                        x: 0, y: hoverID == paper.id ? 9 : 5)
                .scaleEffect(hoverID == paper.id ? 1.04 : 1, anchor: .bottom)
                .overlay(alignment: .topTrailing) {
                    if hoverID == paper.id { removeButton(paper) }
                }
                .onHover { inside in hoverID = inside ? paper.id : (hoverID == paper.id ? nil : hoverID) }
                .onTapGesture { onOpenPaper(paper.arxivId, paper.title) }
                .animation(.snappy(duration: 0.22), value: hoverID)
                .contextMenu {
                    Button("Open") { onOpenPaper(paper.arxivId, paper.title) }
                    Button("Remove from Bookmarks", role: .destructive) {
                        Task { await remove(paper) }
                    }
                }

            Text(paper.title)
                .font(.subheadline.weight(.medium))
                .lineLimit(2)
                .frame(width: coverW, alignment: .leading)
            if let first = paper.authors.first {
                Text(first + (paper.authors.count > 1 ? " et al." : ""))
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .frame(width: coverW, alignment: .leading)
            }
        }
        .contentShape(Rectangle())
    }

    private func removeButton(_ paper: PaperMetadata) -> some View {
        Button {
            Task { await remove(paper) }
        } label: {
            Image(systemName: "bookmark.slash.fill")
                .font(.callout)
                .padding(6)
                .background(.regularMaterial, in: Circle())
        }
        .buttonStyle(.plain)
        .padding(6)
        .help("Remove from Bookmarks")
    }

    // MARK: Actions

    private func load() async {
        loading = papers.isEmpty
        error = nil
        do { papers = try await client.bookmarks() }
        catch { self.error = error.localizedDescription }
        loading = false
    }

    private func remove(_ paper: PaperMetadata) async {
        papers.removeAll { $0.id == paper.id }
        note = nil
        do { try await client.removeBookmark(paper.arxivId) }
        catch {
            note = "Couldn't remove bookmark: \(error.localizedDescription)"
            await load()
        }
    }
}
