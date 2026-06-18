import SwiftUI
import PDFKit
import AppKit

// Core PDF renderer (LOCAL_UI_PRD §4.2). Fetches the PDF bytes from the engine
// and renders them in a PDFView, jumping to a target page when one is given.
//
// Annotation features (right-click a selection):
//   • Highlight        — persisted via HighlightStore (restored on reopen)
//   • Add to KB Notes  — quoted + cited, re-embedded by the engine (onAddNote)
//   • Explain this     — passage → chat model → plain-language sheet (onExplain)
// Right-click an existing highlight to remove it.
struct PDFPanel: View {
    let client: KBClient
    let paperId: String
    var targetPage: Int? = nil
    var onAddNote: ((String, Int?) -> Void)? = nil
    var onExplain: ((String) -> Void)? = nil

    @State private var document: PDFDocument?
    @State private var error: String?
    @State private var loading = true

    var body: some View {
        Group {
            if let document {
                PDFKitView(document: document, paperId: paperId, targetPage: targetPage,
                           onAddNote: onAddNote, onExplain: onExplain)
            } else if loading {
                ProgressView("Loading PDF…")
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                EmptyStateView(icon: "doc.questionmark",
                               title: "Couldn't open the PDF",
                               message: error ?? "")
            }
        }
        .task(id: paperId) { await load() }
    }

    private func load() async {
        loading = true; error = nil
        do {
            let data = try await client.pdfData(paperId)
            if let doc = PDFDocument(data: data) {
                document = doc
            } else {
                error = "The file didn't parse as a PDF."
            }
        } catch {
            self.error = error.localizedDescription
        }
        loading = false
    }
}

// Sheet presentation used by Search/Chat citations: a title bar + Done around
// the panel, opened at the cited page.
struct PDFReaderView: View {
    let client: KBClient
    let paperId: String
    let title: String
    var targetPage: Int?

    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 12) {
                Image(systemName: "doc.richtext").foregroundStyle(.tint)
                Text(title).font(.headline).lineLimit(1)
                Spacer()
                if let page = targetPage {
                    Chip(text: "page \(page)", color: .blue, filled: true)
                }
                Button("Done") { dismiss() }.keyboardShortcut(.cancelAction)
            }
            .padding(12)
            Divider()
            PDFPanel(client: client, paperId: paperId, targetPage: targetPage)
        }
        .frame(minWidth: 640, minHeight: 560)
    }
}

private struct PDFKitView: NSViewRepresentable {
    let document: PDFDocument
    let paperId: String
    var targetPage: Int?
    var onAddNote: ((String, Int?) -> Void)?
    var onExplain: ((String) -> Void)?

    func makeNSView(context: Context) -> AnnotatingPDFView {
        let view = AnnotatingPDFView()
        view.autoScales = true
        view.displayMode = .singlePageContinuous
        view.displayDirection = .vertical
        view.backgroundColor = .windowBackgroundColor
        view.paperId = paperId
        view.onAddNote = onAddNote
        view.onExplain = onExplain
        view.document = document
        view.restoreHighlights()
        jump(view)
        return view
    }

    func updateNSView(_ view: AnnotatingPDFView, context: Context) {
        view.onAddNote = onAddNote
        view.onExplain = onExplain
        if view.document !== document {
            view.paperId = paperId
            view.document = document
            view.restoreHighlights()
            jump(view)
        }
    }

    private func jump(_ view: PDFView) {
        guard let target = targetPage, target > 0,
              let page = document.page(at: target - 1) else { return }
        DispatchQueue.main.async { view.go(to: page) }
    }
}

// PDFView subclass providing the annotation menu and persistent highlights.
final class AnnotatingPDFView: PDFView {
    var paperId: String?
    var onAddNote: ((String, Int?) -> Void)?
    var onExplain: ((String) -> Void)?

    private var annotationsForId: [UUID: [PDFAnnotation]] = [:]
    private var idForAnnotation: [ObjectIdentifier: UUID] = [:]

    override func menu(for event: NSEvent) -> NSMenu? {
        let menu = super.menu(for: event) ?? NSMenu()

        if let selection = currentSelection,
           let text = selection.string,
           !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            var items: [NSMenuItem] = []
            items.append(item("Highlight", "highlighter", #selector(highlightSelection)))
            if onAddNote != nil {
                items.append(item("Add to KB Notes", "note.text.badge.plus", #selector(addSelectionToNotes)))
            }
            if onExplain != nil {
                items.append(item("Explain This", "sparkles", #selector(explainSelection)))
            }
            items.append(.separator())
            for (i, it) in items.enumerated() { menu.insertItem(it, at: i) }
            return menu
        }

        // No selection: offer to remove a highlight under the cursor.
        if let id = highlightId(at: event) {
            let remove = item("Remove Highlight", "highlighter", #selector(removeHighlightUnderCursor))
            objc_setAssociatedObject(self, &Self.pendingRemovalKey, id, .OBJC_ASSOCIATION_RETAIN)
            menu.insertItem(remove, at: 0)
            menu.insertItem(.separator(), at: 1)
        }
        return menu
    }

    private func item(_ title: String, _ symbol: String, _ action: Selector) -> NSMenuItem {
        let it = NSMenuItem(title: title, action: action, keyEquivalent: "")
        it.target = self
        it.image = NSImage(systemSymbolName: symbol, accessibilityDescription: nil)
        return it
    }

    @objc private func highlightSelection() { _ = persistHighlight(currentSelection) }

    @objc private func addSelectionToNotes() {
        guard let selection = currentSelection, let text = selection.string else { return }
        let page = selection.pages.first.flatMap { document?.index(for: $0) }.map { $0 + 1 }
        _ = persistHighlight(selection)
        onAddNote?(text, page)
    }

    @objc private func explainSelection() {
        guard let text = currentSelection?.string, !text.isEmpty else { return }
        onExplain?(text)
    }

    private static var pendingRemovalKey = 0
    @objc private func removeHighlightUnderCursor() {
        if let id = objc_getAssociatedObject(self, &Self.pendingRemovalKey) as? UUID {
            removeHighlight(id)
        }
    }

    // MARK: Highlight persistence

    func restoreHighlights() {
        guard let paperId, let doc = document else { return }
        annotationsForId.removeAll(); idForAnnotation.removeAll()
        for h in HighlightStore.shared.highlights(for: paperId) {
            addAnnotations(for: h, in: doc)
        }
    }

    /// Create yellow highlight annotations over the selection (per line), store
    /// them, and persist. In-memory document only — never written to disk.
    @discardableResult
    private func persistHighlight(_ selection: PDFSelection?) -> StoredHighlight? {
        guard let selection, let doc = document, let paperId else { return nil }
        var quads: [StoredHighlight.Quad] = []
        for line in selection.selectionsByLine() {
            guard let page = line.pages.first else { continue }
            let b = line.bounds(for: page)
            quads.append(.init(page: doc.index(for: page), x: b.minX, y: b.minY, w: b.width, h: b.height))
        }
        guard !quads.isEmpty else { return nil }
        let h = StoredHighlight(id: UUID(), text: selection.string ?? "", createdAt: Date(), quads: quads)
        HighlightStore.shared.add(h, for: paperId)
        addAnnotations(for: h, in: doc)
        clearSelection()
        return h
    }

    private func addAnnotations(for h: StoredHighlight, in doc: PDFDocument) {
        var created: [PDFAnnotation] = []
        for q in h.quads {
            guard let page = doc.page(at: q.page) else { continue }
            let annotation = PDFAnnotation(bounds: CGRect(x: q.x, y: q.y, width: q.w, height: q.h),
                                           forType: .highlight, withProperties: nil)
            annotation.color = NSColor.systemYellow.withAlphaComponent(0.45)
            page.addAnnotation(annotation)
            created.append(annotation)
            idForAnnotation[ObjectIdentifier(annotation)] = h.id
        }
        annotationsForId[h.id] = created
    }

    private func removeHighlight(_ id: UUID) {
        guard let paperId else { return }
        for annotation in annotationsForId[id] ?? [] {
            annotation.page?.removeAnnotation(annotation)
            idForAnnotation.removeValue(forKey: ObjectIdentifier(annotation))
        }
        annotationsForId.removeValue(forKey: id)
        HighlightStore.shared.remove(id, for: paperId)
    }

    private func highlightId(at event: NSEvent) -> UUID? {
        let viewPoint = convert(event.locationInWindow, from: nil)
        guard let page = page(for: viewPoint, nearest: true) else { return nil }
        let pagePoint = convert(viewPoint, to: page)
        guard let annotation = page.annotation(at: pagePoint) else { return nil }
        return idForAnnotation[ObjectIdentifier(annotation)]
    }
}
