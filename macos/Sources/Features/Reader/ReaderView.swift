import SwiftUI
import WebKit

// Reader mode (NEW_SWIFT_FEATURES §1): a reflowable, typographic view of the
// paper's extracted `sections.md` with LaTeX math rendered (KaTeX) and a native
// outline rail for jump-to-section. The markdown is read directly from the KB
// root (the engine's derived, canonical-adjacent file); rendering uses a
// WKWebView with marked.js + KaTeX because there's no native LaTeX layout.
@MainActor
struct ReaderView: View {
    let paperId: String
    let title: String

    @Environment(ServerController.self) private var server

    @State private var markdown: String?
    @State private var outline: [Heading] = []
    @State private var error: String?
    @State private var fontSize: Double = 17
    @State private var scrollIndex: Int?
    @State private var scrollNonce = 0

    struct Heading: Identifiable {
        let id = UUID()
        let index: Int      // position among all headings (matches DOM order)
        let level: Int
        let title: String
    }

    var body: some View {
        Group {
            if let markdown {
                HSplitView {
                    outlineRail
                        .frame(minWidth: 180, idealWidth: 220, maxWidth: 320)
                    ReaderWebView(markdown: markdown, fontSize: fontSize,
                                  scrollIndex: scrollIndex, scrollNonce: scrollNonce)
                        .layoutPriority(1)
                }
            } else if let error {
                EmptyStateView(icon: "doc.plaintext",
                               title: "No reader text",
                               message: error)
            } else {
                ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .safeAreaInset(edge: .top) { fontBar }
        .task(id: paperId) { await load() }
    }

    private var fontBar: some View {
        HStack(spacing: 12) {
            Image(systemName: "text.alignleft").foregroundStyle(.secondary)
            Text("Reader").font(.caption.weight(.medium)).foregroundStyle(.secondary)
            Spacer()
            Button { fontSize = max(12, fontSize - 1) } label: { Image(systemName: "textformat.size.smaller") }
                .help("Smaller text")
            Button { fontSize = min(28, fontSize + 1) } label: { Image(systemName: "textformat.size.larger") }
                .help("Larger text")
        }
        .buttonStyle(.borderless)
        .padding(.horizontal, 12).padding(.vertical, 6)
        .background(.bar)
        .overlay(alignment: .bottom) { Divider() }
    }

    private var outlineRail: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 1) {
                if outline.isEmpty {
                    Text("No sections").font(.caption).foregroundStyle(.tertiary).padding()
                }
                ForEach(outline) { h in
                    Button {
                        scrollIndex = h.index
                        scrollNonce += 1
                    } label: {
                        Text(h.title)
                            .font(.system(size: h.level <= 1 ? 13 : 12,
                                          weight: h.level <= 1 ? .semibold : .regular))
                            .foregroundStyle(h.level <= 1 ? .primary : .secondary)
                            .lineLimit(2)
                            .multilineTextAlignment(.leading)
                            .padding(.leading, CGFloat(max(0, h.level - 1)) * 12)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .padding(.vertical, 3).padding(.horizontal, 8)
                }
            }
            .padding(8)
        }
        .background(.background.secondary)
    }

    private func load() async {
        markdown = nil; error = nil; outline = []
        let url = server.kbRoot.appendingPathComponent(paperId).appendingPathComponent("sections.md")
        guard let text = try? String(contentsOf: url, encoding: .utf8) else {
            error = "Couldn't read the extracted text for this paper. View the PDF instead."
            return
        }
        outline = Self.parseOutline(text)
        markdown = text
    }

    /// Headings in document order — index aligns with the DOM heading order the
    /// web view assigns, so tapping a row scrolls to the right place.
    static func parseOutline(_ md: String) -> [Heading] {
        var result: [Heading] = []
        var inFence = false
        var i = 0
        for raw in md.split(separator: "\n", omittingEmptySubsequences: false) {
            let line = String(raw)
            if line.hasPrefix("```") { inFence.toggle(); continue }
            guard !inFence else { continue }
            guard let hashes = line.range(of: #"^#{1,6}\s+"#, options: .regularExpression) else { continue }
            let level = line.distance(from: line.startIndex, to: line[hashes].firstIndex(where: { $0 == " " }) ?? hashes.upperBound)
            var titleText = String(line[hashes.upperBound...])
            // Strip simple inline markdown for a clean rail label.
            titleText = titleText.replacingOccurrences(of: "`", with: "")
                .replacingOccurrences(of: "*", with: "")
                .replacingOccurrences(of: "_", with: "")
                .trimmingCharacters(in: .whitespaces)
            result.append(Heading(index: i, level: level, title: titleText))
            i += 1
        }
        return result
    }
}

// WKWebView that renders the markdown (with math) into a clean reading page.
private struct ReaderWebView: NSViewRepresentable {
    let markdown: String
    let fontSize: Double
    var scrollIndex: Int?
    var scrollNonce: Int

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeNSView(context: Context) -> WKWebView {
        let web = WKWebView()
        web.setValue(false, forKey: "drawsBackground")
        context.coordinator.web = web
        return web
    }

    func updateNSView(_ web: WKWebView, context: Context) {
        if context.coordinator.loadedMarkdown != markdown {
            context.coordinator.loadedMarkdown = markdown
            web.loadHTMLString(Self.html(markdown: markdown, fontSize: fontSize), baseURL: nil)
        } else {
            web.evaluateJavaScript("window.kbSetFont && kbSetFont(\(Int(fontSize)));")
        }
        if context.coordinator.lastScrollNonce != scrollNonce, let i = scrollIndex {
            context.coordinator.lastScrollNonce = scrollNonce
            web.evaluateJavaScript("window.kbScrollTo && kbScrollTo(\(i));")
        }
    }

    final class Coordinator {
        var web: WKWebView?
        var loadedMarkdown: String?
        var lastScrollNonce = 0
    }

    static func html(markdown: String, fontSize: Double) -> String {
        // Encode the markdown as a safe JS string literal.
        let mdLiteral = (try? String(data: JSONEncoder().encode(markdown), encoding: .utf8))
            ?? "\"\""
        return """
        <!DOCTYPE html><html><head><meta charset="utf-8">
        <meta name="viewport" content="width=device-width, initial-scale=1">
        <link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/katex@0.16.9/dist/katex.min.css">
        <script src="https://cdn.jsdelivr.net/npm/katex@0.16.9/dist/katex.min.js"></script>
        <script src="https://cdn.jsdelivr.net/npm/marked/marked.min.js"></script>
        <style>
          :root { --fs: \(Int(fontSize))px; }
          html { color-scheme: light dark; }
          body {
            font: var(--fs)/1.7 -apple-system, "SF Pro Text", system-ui, sans-serif;
            color: #1d1d1f; background: transparent;
            margin: 0 auto; padding: 28px 32px 120px; max-width: 720px;
            -webkit-font-smoothing: antialiased;
          }
          @media (prefers-color-scheme: dark) { body { color: #e8e8ea; } }
          h1 { font-size: 1.7em; margin: 1.6em 0 .4em; line-height: 1.25; }
          h2 { font-size: 1.35em; margin: 1.5em 0 .4em; }
          h3, h4, h5 { font-size: 1.12em; margin: 1.3em 0 .3em; }
          p { margin: 0 0 1em; }
          a { color: #0a84ff; text-decoration: none; }
          code { font-family: "SF Mono", ui-monospace, monospace; font-size: .9em;
                 background: color-mix(in srgb, currentColor 10%, transparent);
                 padding: .1em .35em; border-radius: 4px; }
          pre { background: color-mix(in srgb, currentColor 8%, transparent);
                padding: 12px 14px; border-radius: 10px; overflow-x: auto; }
          pre code { background: none; padding: 0; }
          blockquote { margin: 1em 0; padding: .2em 1em; border-left: 3px solid color-mix(in srgb, currentColor 25%, transparent);
                       color: color-mix(in srgb, currentColor 70%, transparent); }
          table { border-collapse: collapse; margin: 1em 0; display: block; overflow-x: auto; }
          th, td { border: 1px solid color-mix(in srgb, currentColor 20%, transparent); padding: 6px 10px; }
          img { max-width: 100%; }
          .katex-display { overflow-x: auto; overflow-y: hidden; padding: 4px 0; }
          h1,h2,h3,h4,h5,h6 { scroll-margin-top: 16px; }
        </style></head>
        <body><div id="content"></div>
        <script>
          const MD = \(mdLiteral);
          function protectMath(md){
            const store=[];
            const push=(t,d)=>{store.push({t:t,d:d});return "@@KBMATH"+(store.length-1)+"@@";};
            md=md.replace(/```math\\n([\\s\\S]+?)```/g,(m,x)=>push(x.trim(),true));
            md=md.replace(/\\$\\$([\\s\\S]+?)\\$\\$/g,(m,x)=>push(x,true));
            md=md.replace(/\\\\\\[([\\s\\S]+?)\\\\\\]/g,(m,x)=>push(x,true));
            md=md.replace(/\\$`([^`]+?)`\\$/g,(m,x)=>push(x,false));
            md=md.replace(/\\\\\\(([\\s\\S]+?)\\\\\\)/g,(m,x)=>push(x,false));
            md=md.replace(/\\$([^\\$\\n]+?)\\$/g,(m,x)=>push(x,false));
            return {md:md,store:store};
          }
          function render(){
            const p=protectMath(MD);
            let html=marked.parse(p.md);
            html=html.replace(/@@KBMATH(\\d+)@@/g,(m,i)=>{
              const e=p.store[+i];
              try { return katex.renderToString(e.t,{displayMode:e.d,throwOnError:false}); }
              catch(err){ return m; }
            });
            const c=document.getElementById('content');
            c.innerHTML=html;
            c.querySelectorAll('h1,h2,h3,h4,h5,h6').forEach((h,i)=>h.id='kbh'+i);
          }
          window.kbScrollTo=function(i){const el=document.getElementById('kbh'+i); if(el) el.scrollIntoView({behavior:'smooth',block:'start'});};
          window.kbSetFont=function(px){document.documentElement.style.setProperty('--fs',px+'px');};
          render();
        </script></body></html>
        """
    }
}
