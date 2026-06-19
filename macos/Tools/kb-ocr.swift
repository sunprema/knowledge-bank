// kb-ocr — a tiny PDF OCR sidecar for the KB engine.
//
// The Rust ingest pipeline calls this when a PDF page has no extractable text
// layer (scanned / image-only papers — PRD §4 step 4's weak path). PDFKit
// renders the requested pages to bitmaps and Vision's text recognizer reads
// them back, so the engine still gets searchable text. It's a standalone
// CLI (its own `main`), compiled by macos/build.sh alongside the app and
// placed next to the `kb` engine so the engine can find it as a sibling.
//
// Usage:
//   kb-ocr <pdf-path> [--pages 1,3,5] [--dpi 300]
//     --pages  1-indexed page numbers to OCR (default: all pages)
//     --dpi    render resolution (default: 300; higher = slower, sharper)
//
// Output (stdout): JSON  [{"page":1,"text":"…"}, …]  — page is 1-indexed.
// Pages that yield no text are still emitted (with "text":"") so the caller
// can tell "OCR ran, found nothing" from "OCR didn't run". Errors go to
// stderr with a non-zero exit.
//
// Notes for the 8 GB M1 target: pages are rendered one at a time into a
// grayscale buffer (a third the memory of RGBA) and released before the next,
// so peak memory stays ~one page regardless of document length.

import Foundation
import PDFKit
import Vision
import CoreGraphics

struct PageText: Codable {
    let page: Int
    let text: String
}

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data(("kb-ocr: " + message + "\n").utf8))
    exit(1)
}

// ---- argument parsing -----------------------------------------------------

var args = Array(CommandLine.arguments.dropFirst())
guard let pdfPath = args.first(where: { !$0.hasPrefix("--") }) else {
    fail("usage: kb-ocr <pdf-path> [--pages 1,3,5] [--dpi 300]")
}

func flagValue(_ name: String) -> String? {
    guard let i = args.firstIndex(of: name), i + 1 < args.count else { return nil }
    return args[i + 1]
}

let dpi: CGFloat = {
    if let v = flagValue("--dpi"), let d = Double(v), d > 0 { return CGFloat(d) }
    return 300
}()

// Requested 1-indexed pages (nil ⇒ all).
let requestedPages: [Int]? = flagValue("--pages").map { csv in
    csv.split(separator: ",").compactMap { Int($0.trimmingCharacters(in: .whitespaces)) }
}

// ---- load the document ----------------------------------------------------

let url = URL(fileURLWithPath: pdfPath)
guard let doc = PDFDocument(url: url) else {
    fail("cannot open PDF at \(pdfPath)")
}
let pageCount = doc.pageCount
guard pageCount > 0 else { fail("PDF has no pages") }

// Resolve the page set, clamped to the document and de-duplicated in order.
let targets: [Int] = {
    let all = Array(1...pageCount)
    guard let req = requestedPages else { return all }
    var seen = Set<Int>()
    return req.filter { $0 >= 1 && $0 <= pageCount && seen.insert($0).inserted }
}()

// ---- render + recognize ---------------------------------------------------

/// Render one PDF page (1-indexed) to a grayscale CGImage at `dpi`.
func render(page index: Int) -> CGImage? {
    guard let page = doc.page(at: index - 1) else { return nil }
    let bounds = page.bounds(for: .mediaBox)
    let scale = dpi / 72.0
    let width = Int((bounds.width * scale).rounded())
    let height = Int((bounds.height * scale).rounded())
    guard width > 0, height > 0 else { return nil }

    let gray = CGColorSpaceCreateDeviceGray()
    guard let ctx = CGContext(
        data: nil,
        width: width,
        height: height,
        bitsPerComponent: 8,
        bytesPerRow: 0,
        space: gray,
        bitmapInfo: CGImageAlphaInfo.none.rawValue
    ) else { return nil }

    // White background, then draw the page in its mediaBox coordinate space.
    ctx.setFillColor(gray: 1, alpha: 1)
    ctx.fill(CGRect(x: 0, y: 0, width: width, height: height))
    ctx.scaleBy(x: scale, y: scale)
    ctx.translateBy(x: -bounds.origin.x, y: -bounds.origin.y)
    page.draw(with: .mediaBox, to: ctx)
    return ctx.makeImage()
}

/// OCR a rendered page image into newline-joined recognized lines.
func recognize(_ image: CGImage) -> String {
    let request = VNRecognizeTextRequest()
    request.recognitionLevel = .accurate
    request.usesLanguageCorrection = true
    let handler = VNImageRequestHandler(cgImage: image, options: [:])
    do {
        try handler.perform([request])
    } catch {
        return ""
    }
    let observations = request.results ?? []
    return observations
        .compactMap { $0.topCandidates(1).first?.string }
        .joined(separator: "\n")
}

var results: [PageText] = []
results.reserveCapacity(targets.count)
for p in targets {
    autoreleasepool {
        let text = render(page: p).map(recognize) ?? ""
        results.append(PageText(page: p, text: text))
    }
}

// ---- emit JSON ------------------------------------------------------------

let encoder = JSONEncoder()
encoder.outputFormatting = []
do {
    let data = try encoder.encode(results)
    FileHandle.standardOutput.write(data)
} catch {
    fail("failed to encode results: \(error)")
}
