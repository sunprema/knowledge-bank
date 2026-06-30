import SwiftUI

/// The Books shelf: every paper that has a generated HTML book (`write-paper-book`),
/// so they're findable in one place instead of hunting through the Library for the
/// ones with a Book button. Tapping a book opens its paper in the Library straight
/// in Book mode.
@MainActor
struct BooksView: View {
    let client: KBClient
    /// Open a paper's book (routes to the Library, which switches into Book mode).
    var onOpenBook: (String, String) -> Void = { _, _ in }

    @Environment(ServerController.self) private var server

    @State private var books: [PaperBook.Entry] = []
    @State private var meta: [String: PaperMetadata] = [:]
    @State private var filter = ""
    @State private var loading = true

    private let coverW: CGFloat = 168
    private let coverH: CGFloat = 224

    private var filtered: [PaperBook.Entry] {
        guard !filter.isEmpty else { return books }
        let q = filter.lowercased()
        return books.filter {
            $0.title.lowercased().contains(q)
            || $0.paperTitle.lowercased().contains(q)
            || $0.summary.lowercased().contains(q)
        }
    }

    var body: some View {
        Group {
            if loading {
                ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if books.isEmpty {
                EmptyStateView(
                    icon: "books.vertical",
                    title: "No books yet",
                    message: "Right-click a paper in the Library and choose “Build paper book with Claude” to generate one. Built books show up here.")
            } else {
                grid
            }
        }
        .navigationTitle("Books")
        .searchable(text: $filter, placement: .toolbar, prompt: "Filter books")
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Button { Task { await load() } } label: { Image(systemName: "arrow.clockwise") }
                    .help("Rescan for newly built books")
            }
        }
        .task { await load() }
    }

    private var grid: some View {
        ScrollView {
            LazyVGrid(columns: [GridItem(.adaptive(minimum: coverW, maximum: coverW), spacing: 30)],
                      alignment: .leading, spacing: 30) {
                ForEach(filtered) { book in card(book) }
            }
            .padding(28)
        }
    }

    @ViewBuilder private func card(_ book: PaperBook.Entry) -> some View {
        Button {
            onOpenBook(book.paperId, meta[book.paperId]?.title ?? book.title)
        } label: {
            VStack(alignment: .leading, spacing: 8) {
                cover(book)
                    .frame(width: coverW, height: coverH)
                    .clipShape(RoundedRectangle(cornerRadius: 10))
                    .overlay(RoundedRectangle(cornerRadius: 10).stroke(.black.opacity(0.12), lineWidth: 0.5))
                    .overlay(alignment: .topTrailing) { bookBadge }
                    .shadow(color: .black.opacity(0.28), radius: 8, x: 0, y: 5)

                Text(book.title)
                    .font(.subheadline.weight(.medium))
                    .lineLimit(2)
                    .frame(width: coverW, alignment: .leading)
                    .multilineTextAlignment(.leading)

                HStack(spacing: 6) {
                    if book.chapters > 0 {
                        Label("\(book.chapters)", systemImage: "doc.on.doc")
                            .labelStyle(.titleAndIcon)
                    }
                    if !book.created.isEmpty {
                        Text(Theme.year(book.created))
                    }
                    if !book.ready {
                        Text("building…").foregroundStyle(.orange)
                    }
                }
                .font(.caption2)
                .foregroundStyle(.secondary)
                .frame(width: coverW, alignment: .leading)
            }
        }
        .buttonStyle(.plain)
    }

    /// The paper's real cover when we have its metadata; otherwise a typographic
    /// fallback so books for not-yet-loaded papers still show.
    @ViewBuilder private func cover(_ book: PaperBook.Entry) -> some View {
        if let m = meta[book.paperId] {
            CoverImage(paper: m, client: client)
        } else {
            ZStack {
                LinearGradient(colors: [.indigo.opacity(0.85), .purple.opacity(0.7)],
                               startPoint: .topLeading, endPoint: .bottomTrailing)
                Text(book.title)
                    .font(.system(.headline, design: .serif).weight(.bold))
                    .foregroundStyle(.white)
                    .multilineTextAlignment(.center)
                    .lineLimit(5)
                    .padding(12)
            }
        }
    }

    private var bookBadge: some View {
        Image(systemName: "book.closed.fill")
            .font(.caption2)
            .foregroundStyle(.white)
            .padding(6)
            .background(.black.opacity(0.55), in: Circle())
            .padding(6)
    }

    private func load() async {
        loading = true
        books = PaperBook.allBooks(root: server.kbRoot)
        if let list = try? await client.papers() {
            meta = Dictionary(uniqueKeysWithValues: list.map { ($0.arxivId, $0) })
        }
        loading = false
    }
}
