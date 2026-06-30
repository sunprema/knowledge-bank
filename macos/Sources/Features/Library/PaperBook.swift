import SwiftUI
import WebKit
import AppKit

/// Per-paper generated HTML books (the `write-paper-book` skill). A book lives at
/// `$KB_ROOT/<id>/book/`, right next to the paper's `metadata.json` / `sections.md`,
/// so it travels with the paper and the app finds it by convention.
///
/// This type resolves those paths, detects whether a paper has a book yet, and
/// launches the build by handing a tiny `.command` to Terminal — the same trick
/// BookBank uses (no Automation/TCC prompt, no idle timeout, app stays free).
enum PaperBook {
    /// The book folder for a paper, given the corpus root (`ServerController.kbRoot`).
    static func bookDir(root: URL, id: String) -> URL {
        root.appendingPathComponent(id, isDirectory: true)
            .appendingPathComponent("book", isDirectory: true)
    }

    /// The book's landing page; its existence is how the app knows a book is ready.
    static func indexURL(root: URL, id: String) -> URL {
        bookDir(root: root, id: id).appendingPathComponent("index.html")
    }

    static func hasBook(root: URL, id: String) -> Bool {
        FileManager.default.fileExists(atPath: indexURL(root: root, id: id).path)
    }

    /// A generated book discovered on disk (one per paper that has a `book/`).
    struct Entry: Identifiable, Hashable {
        let paperId: String
        let title: String         // the book's title
        let paperTitle: String    // the underlying paper's title
        let summary: String
        let created: String       // ISO yyyy-MM-dd
        let chapters: Int
        let ready: Bool           // book.json status == "ready"
        var id: String { paperId }
    }

    /// What we decode out of a book's `book.json` manifest.
    private struct Manifest: Decodable {
        var paper_id: String?
        var title: String?
        var paper_title: String?
        var summary: String?
        var created: String?
        var status: String?
        var concepts: [Concept]?
        struct Concept: Decodable {}
    }

    /// Scan the corpus for generated books: `<root>/<id>/book/book.json` with an
    /// `index.html` beside it. Newest first. Cheap directory + small-JSON reads, so
    /// it's fine to call on view appear / refresh.
    static func allBooks(root: URL) -> [Entry] {
        let fm = FileManager.default
        guard let dirs = try? fm.contentsOfDirectory(
            at: root, includingPropertiesForKeys: nil,
            options: [.skipsHiddenFiles]) else { return [] }

        var out: [Entry] = []
        for dir in dirs {
            let id = dir.lastPathComponent
            let bookDir = dir.appendingPathComponent("book", isDirectory: true)
            let manifest = bookDir.appendingPathComponent("book.json")
            let index = bookDir.appendingPathComponent("index.html")
            guard fm.fileExists(atPath: index.path),
                  let data = try? Data(contentsOf: manifest),
                  let m = try? JSONDecoder().decode(Manifest.self, from: data) else { continue }
            out.append(Entry(
                paperId: m.paper_id ?? id,
                title: (m.title?.isEmpty == false ? m.title! : (m.paper_title ?? id)),
                paperTitle: m.paper_title ?? "",
                summary: m.summary ?? "",
                created: m.created ?? "",
                chapters: m.concepts?.count ?? 0,
                ready: (m.status ?? "") == "ready"))
        }
        return out.sorted { $0.created > $1.created }
    }

    /// The KB project folder that holds `.claude/skills/write-paper-book`. Override
    /// with `$KB_PROJECT_DIR`; otherwise walk up from the app bundle looking for a
    /// `.claude/skills` directory (dev bundle lives at `<kb>/macos/build/KB.app`),
    /// falling back to three levels up from the bundle.
    static var projectDir: URL {
        if let p = ProcessInfo.processInfo.environment["KB_PROJECT_DIR"], !p.isEmpty {
            return URL(fileURLWithPath: (p as NSString).expandingTildeInPath, isDirectory: true)
        }
        let fm = FileManager.default
        var dir = Bundle.main.bundleURL.deletingLastPathComponent()   // strip KB.app
        for _ in 0..<6 {
            if fm.fileExists(atPath: dir.appendingPathComponent(".claude/skills").path) {
                return dir
            }
            dir = dir.deletingLastPathComponent()
        }
        // Fallback: <kb>/macos/build/KB.app → <kb>
        return Bundle.main.bundleURL
            .deletingLastPathComponent()   // build
            .deletingLastPathComponent()   // macos
            .deletingLastPathComponent()   // kb
    }

    /// Open Terminal in the project folder and start an interactive `claude`, seeded
    /// to run the `write-paper-book` skill on `id`. We write an executable `.command`
    /// and `open` it: macOS runs `.command` files in the user's terminal with no
    /// Automation prompt. Returns false if the script couldn't be written/opened.
    @discardableResult
    static func launchBuild(id: String, title: String) -> Bool {
        let prompt = "/goal Use the write-paper-book skill to build a beautiful HTML book "
            + "for paper \(id) (\(title)). Read the full paper body first, then "
            + "generate it into $KB_ROOT/\(id)/book/. "
            + "Once the goal is completed, stop any loops if they are active."
        var script = "#!/bin/zsh\n"
        script += "cd \(q(projectDir.path)) || exit 1\n"
        // Make $KB_ROOT / API keys available to the skill if the repo exports them.
        script += "[ -f .env.local.sh ] && source .env.local.sh\n"
        script += "exec claude \(q(prompt))\n"

        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("kb-build-book-\(id).command")
        do {
            try script.write(to: url, atomically: true, encoding: .utf8)
            try FileManager.default.setAttributes(
                [.posixPermissions: 0o755], ofItemAtPath: url.path)
        } catch {
            return false
        }
        NSWorkspace.shared.open(url)
        return true
    }

    /// Single-quote a string for safe inclusion in a POSIX shell script.
    private static func q(_ s: String) -> String {
        "'" + s.replacingOccurrences(of: "'", with: "'\\''") + "'"
    }
}

/// Thin WKWebView wrapper for reading a generated paper book inside the app. Loads a
/// local `file://` page with read access scoped to the book folder, so the book's
/// own `concepts/*.html`, `assets/*`, and images all resolve. Bump `reloadToken` to
/// force a reload (e.g. after the skill regenerates the book).
///
/// Injects the page-turn keys (→ next, ← previous, ↑ first page) following the
/// `write-paper-book` nav contract, and — for image-slot placeholders the skill
/// emitted — wires each into a live drop target that writes the dropped image into
/// the book folder and reloads.
struct BookWebView: NSViewRepresentable {
    let url: URL
    var readAccess: URL?
    var reloadToken: Int = 0
    var bookDir: URL? = nil
    var onImageAttached: (() -> Void)? = nil

    func makeNSView(context: Context) -> WKWebView {
        let ucc = WKUserContentController()
        ucc.addUserScript(WKUserScript(source: Self.navScript,
                                       injectionTime: .atDocumentEnd, forMainFrameOnly: true))
        if bookDir != nil {
            ucc.add(context.coordinator, name: "kbbook")
            ucc.addUserScript(WKUserScript(source: Self.dropScript,
                                           injectionTime: .atDocumentEnd, forMainFrameOnly: true))
        }
        let config = WKWebViewConfiguration()
        config.userContentController = ucc

        let v = WKWebView(frame: .zero, configuration: config)
        v.setValue(false, forKey: "drawsBackground")   // let book CSS own the backdrop
        sync(context)
        load(into: v, context: context)
        DispatchQueue.main.async { v.window?.makeFirstResponder(v) }
        return v
    }

    func updateNSView(_ v: WKWebView, context: Context) {
        sync(context)
        if context.coordinator.url != url || context.coordinator.token != reloadToken {
            load(into: v, context: context)
        }
    }

    private func sync(_ context: Context) {
        context.coordinator.bookDir = bookDir
        context.coordinator.onImageAttached = onImageAttached
    }

    private func load(into v: WKWebView, context: Context) {
        context.coordinator.url = url
        context.coordinator.token = reloadToken
        let access = readAccess ?? url.deletingLastPathComponent()
        v.loadFileURL(url, allowingReadAccessTo: access)
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

    final class Coordinator: NSObject, WKScriptMessageHandler {
        var url: URL?
        var token: Int = -1
        var bookDir: URL?
        var onImageAttached: (() -> Void)?

        func userContentController(_ ucc: WKUserContentController,
                                   didReceive message: WKScriptMessage) {
            guard message.name == "kbbook",
                  let body = message.body as? [String: Any],
                  let file = body["file"] as? String, !file.isEmpty,
                  let dataURL = body["dataURL"] as? String,
                  let bookDir,
                  let data = Self.decodeDataURL(dataURL) else { return }

            // Resolve the target inside the book folder and refuse to escape it.
            let target = bookDir.appendingPathComponent(file).standardizedFileURL
            let root = bookDir.standardizedFileURL.path
            guard target.path == root || target.path.hasPrefix(root + "/") else { return }

            do {
                try FileManager.default.createDirectory(
                    at: target.deletingLastPathComponent(), withIntermediateDirectories: true)
                try Self.encoded(data, forExtension: target.pathExtension).write(to: target)
            } catch { return }

            let cb = onImageAttached
            DispatchQueue.main.async {
                message.webView?.reload()   // the <img> now resolves; placeholder hides
                cb?()
            }
        }

        static func decodeDataURL(_ s: String) -> Data? {
            guard let comma = s.firstIndex(of: ",") else { return nil }
            return Data(base64Encoded: String(s[s.index(after: comma)...]),
                        options: .ignoreUnknownCharacters)
        }

        /// Re-encode to the slot's declared extension so on-disk bytes match the
        /// `<img>` filename. Falls back to raw bytes for formats AppKit can't open.
        static func encoded(_ data: Data, forExtension ext: String) -> Data {
            let e = ext.lowercased()
            guard ["png", "jpg", "jpeg"].contains(e),
                  let img = NSImage(data: data),
                  let tiff = img.tiffRepresentation,
                  let rep = NSBitmapImageRep(data: tiff) else { return data }
            let type: NSBitmapImageRep.FileType = (e == "jpg" || e == "jpeg") ? .jpeg : .png
            return rep.representation(using: type, properties: [:]) ?? data
        }
    }

    /// Page-turn shortcuts: **→** next, **←** previous, **↑** first page. Prefers a
    /// book-provided pager (`window.kbPager`) and otherwise follows the page's
    /// `rel="next/prev/home"` links. Sets `window.__kbNav` so the book's own
    /// keydown handler defers to us (no double-turn). Ignores typing + ⌘/⌃/⌥ combos.
    private static let navScript = """
    (function(){
      if (window.__kbNav) return; window.__kbNav = true;
      function matches(text, kind){
        text = (text||'').toLowerCase();
        if(kind==='next') return text.indexOf('next')>=0 || text.indexOf('\\u2192')>=0 || text.indexOf('\\u00bb')>=0;
        if(kind==='prev') return text.indexOf('previous')>=0 || text.indexOf('prev')>=0 || text.indexOf('back')>=0 || text.indexOf('\\u2190')>=0 || text.indexOf('\\u00ab')>=0;
        return text.indexOf('contents')>=0 || text.indexOf('home')>=0 || text.indexOf('cover')>=0;
      }
      function pick(kind){
        var sel = kind==='next' ? 'a[rel~="next"],a[data-nav="next"]'
                : kind==='prev' ? 'a[rel~="prev"],a[data-nav="prev"]'
                : 'a[rel~="home"],a[data-nav="home"]';
        var el = document.querySelector(sel);
        if(!el && kind==='home') el = document.querySelector('a[href$="index.html"]');
        if(!el){
          var as = document.querySelectorAll('a[href]');
          for(var i=0;i<as.length;i++){ if(matches(as[i].textContent, kind)){ el=as[i]; break; } }
        }
        return (el && el.href) ? el.href : null;
      }
      function nav(kind){ var h=pick(kind); if(h){ window.location.href=h; return true; } return false; }
      function turn(kind){
        var p = window.kbPager;
        if(p && typeof p[kind]==='function'){ p[kind](); return true; }
        return nav(kind);
      }
      window.addEventListener('keydown', function(e){
        if(e.metaKey||e.ctrlKey||e.altKey) return;
        var t=e.target;
        if(t && (t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName||''))) return;
        var done=false;
        if(e.key==='ArrowRight') done=turn('next');
        else if(e.key==='ArrowLeft') done=turn('prev');
        else if(e.key==='ArrowUp') done=turn('home');
        if(done) e.preventDefault();
      }, false);
    })();
    """

    /// Guarantees the image-slot show/hide behavior, wires each slot as a drop
    /// target, and makes "Copy prompt" work. Idempotent across navigations.
    private static let dropScript = """
    (function(){
      if (window.__kbImg) return; window.__kbImg = true;
      var st = document.createElement('style');
      st.textContent = '.img-slot .img-drop{display:none}'
        + '.img-slot.img-missing .img-real{display:none}'
        + '.img-slot.img-missing .img-drop{display:block}'
        + '.img-slot.dragover{outline:2px dashed currentColor;outline-offset:6px}';
      (document.head || document.documentElement).appendChild(st);

      function copyText(t){
        function fb(){try{var ta=document.createElement('textarea');ta.value=t;
          document.body.appendChild(ta);ta.select();document.execCommand('copy');
          document.body.removeChild(ta);}catch(e){}}
        if(navigator.clipboard&&navigator.clipboard.writeText){
          navigator.clipboard.writeText(t).catch(fb);} else { fb(); }
      }
      function wire(fig){
        if(fig.__wired) return; fig.__wired = true;
        fig.addEventListener('dragover',function(e){e.preventDefault();fig.classList.add('dragover');});
        fig.addEventListener('dragleave',function(){fig.classList.remove('dragover');});
        fig.addEventListener('drop',function(e){
          e.preventDefault(); fig.classList.remove('dragover');
          var f = e.dataTransfer && e.dataTransfer.files && e.dataTransfer.files[0];
          if(!f) return;
          var r = new FileReader();
          r.onload = function(){
            try{ window.webkit.messageHandlers.kbbook.postMessage({
              slot: fig.getAttribute('data-img-slot'),
              file: fig.getAttribute('data-img-file'),
              name: f.name, dataURL: r.result });
            }catch(err){}
          };
          r.readAsDataURL(f);
        });
        var b = fig.querySelector('.img-copy');
        if(b){ b.addEventListener('click',function(){
          var p = fig.querySelector('.img-prompt');
          if(p){ copyText(p.textContent.trim());
            var o=b.textContent; b.textContent='Copied ✓';
            setTimeout(function(){b.textContent=o;},1200); }
        }); }
      }
      function wireAll(){ document.querySelectorAll('.img-slot').forEach(wire); }
      wireAll();
      document.addEventListener('DOMContentLoaded', wireAll);
    })();
    """
}
