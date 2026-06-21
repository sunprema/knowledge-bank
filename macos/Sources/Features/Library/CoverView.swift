import SwiftUI
import PDFKit

// Apple Books–style covers for the Library shelf. Two tiers:
//   • Papers with a PDF → the real first page, rendered once and cached to
//     `<kbRoot>/<id>/cover.png` so it's instant on every later launch.
//   • Everything else (notes, ideas, reflections, web pages) → a designed
//     gradient cover keyed off the document's identity, so the shelf has
//     variety without depending on the network or any image service.

/// A document's cover: the cached/rendered PDF page when one exists, otherwise
/// the procedural gradient. Always shows the gradient first so the grid never
/// flashes empty, then fades the real page in on top.
@MainActor
struct CoverImage: View {
    let paper: PaperMetadata
    let client: KBClient
    @Environment(ServerController.self) private var server
    @State private var image: NSImage?

    var body: some View {
        ZStack {
            GradientCover(paper: paper)
            if let image {
                Image(nsImage: image)
                    .resizable()
                    .scaledToFill()
                    .transition(.opacity)
            }
        }
        .animation(.easeOut(duration: 0.25), value: image != nil)
        .task(id: paper.id) {
            guard image == nil else { return }
            if let data = await CoverCache.shared.cover(for: paper, kbRoot: server.kbRoot, client: client) {
                image = NSImage(data: data)
            }
        }
    }
}

/// A generated cover for documents with no PDF page to show — a gradient picked
/// deterministically from the id, the kind glyph watermarked behind, and the
/// title typeset large in serif. Stable across launches (no random hashing).
struct GradientCover: View {
    let paper: PaperMetadata

    var body: some View {
        let colors = Self.palette(for: paper.id)
        ZStack {
            LinearGradient(colors: colors, startPoint: .topLeading, endPoint: .bottomTrailing)

            Image(systemName: Theme.kindGlyph(paper.kind))
                .font(.system(size: 96, weight: .light))
                .foregroundStyle(.white.opacity(0.13))
                .rotationEffect(.degrees(-12))
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topTrailing)
                .offset(x: 26, y: -18)
                .clipped()

            VStack(alignment: .leading, spacing: 6) {
                Spacer(minLength: 0)
                Text(paper.kind.uppercased())
                    .font(.system(size: 9, weight: .bold))
                    .tracking(1.5)
                    .foregroundStyle(.white.opacity(0.7))
                Text(paper.title)
                    .font(.system(.subheadline, design: .serif).weight(.semibold))
                    .foregroundStyle(.white)
                    .lineLimit(5)
                    .minimumScaleFactor(0.75)
                    .multilineTextAlignment(.leading)
                Rectangle().fill(.white.opacity(0.5)).frame(width: 24, height: 2).padding(.top, 2)
            }
            .padding(14)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottomLeading)
        }
    }

    // Booklike gradient pairs. Picked by a stable sum-of-bytes hash so a given
    // document keeps the same cover across launches (String.hashValue is salted
    // per process and would reshuffle every run).
    private static let palettes: [[Color]] = [
        [.indigo, .purple],
        [.blue, .teal],
        [.orange, .pink],
        [.green, .teal],
        [.pink, .purple],
        [.brown, .orange],
        [.red, .orange],
        [.cyan, .blue],
        [.mint, .green],
        [.purple, .blue],
    ]

    static func palette(for id: String) -> [Color] {
        let h = id.utf8.reduce(0) { $0 &+ Int($1) }
        return palettes[h % palettes.count]
    }
}

/// Renders + caches PDF first-page covers. An actor so concurrent grid cells
/// share one render per id; results are memoized in-process and on disk. Deals
/// in PNG `Data` (Sendable) — the view turns it into an `NSImage` on the main
/// actor — so nothing non-Sendable crosses the actor boundary.
actor CoverCache {
    static let shared = CoverCache()

    private var memory: [String: Data] = [:]
    private var noCover: Set<String> = []          // ids known to have no PDF cover (this session)
    private var inflight: [String: Task<Data?, Never>] = [:]

    func cover(for paper: PaperMetadata, kbRoot: URL, client: KBClient) async -> Data? {
        let id = paper.id
        if let cached = memory[id] { return cached }
        if noCover.contains(id) { return nil }
        if let task = inflight[id] { return await task.value }

        let task = Task<Data?, Never> {
            await Self.render(paper: paper, kbRoot: kbRoot, client: client)
        }
        inflight[id] = task
        let result = await task.value
        inflight[id] = nil
        if let result { memory[id] = result } else { noCover.insert(id) }
        return result
    }

    private static func render(paper: PaperMetadata, kbRoot: URL, client: KBClient) async -> Data? {
        let dir = kbRoot.appendingPathComponent(paper.id)
        let cacheURL = dir.appendingPathComponent("cover.png")
        if let disk = try? Data(contentsOf: cacheURL) { return disk }

        // Only papers carry a PDF to render a cover from; other kinds fall back
        // to the gradient. (Fetching a missing PDF would just 404.)
        guard paper.kind.lowercased() == "paper" else { return nil }
        guard let data = try? await client.pdfData(paper.id),
              let doc = PDFDocument(data: data),
              let page = doc.page(at: 0) else { return nil }

        let bounds = page.bounds(for: .cropBox)
        guard bounds.width > 0 else { return nil }
        let targetW: CGFloat = 600
        let scale = targetW / bounds.width
        let size = NSSize(width: targetW, height: bounds.height * scale)
        let thumb = page.thumbnail(of: size, for: .cropBox)

        guard let tiff = thumb.tiffRepresentation,
              let rep = NSBitmapImageRep(data: tiff),
              let png = rep.representation(using: .png, properties: [:]) else { return nil }
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        try? png.write(to: cacheURL)
        return png
    }
}
