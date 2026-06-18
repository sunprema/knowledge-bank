import SwiftUI
import WebKit

// Reusable rich-markdown renderer in a WKWebView: marked.js for markdown,
// KaTeX for LaTeX math, highlight.js for code syntax highlighting, with clean
// typography that follows the system light/dark appearance. Used by the notes
// popup (and available for any markdown surface).
struct MarkdownView: NSViewRepresentable {
    let markdown: String

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeNSView(context: Context) -> WKWebView {
        let web = WKWebView()
        web.setValue(false, forKey: "drawsBackground")
        return web
    }

    func updateNSView(_ web: WKWebView, context: Context) {
        if context.coordinator.loaded != markdown {
            context.coordinator.loaded = markdown
            web.loadHTMLString(Self.html(markdown: markdown), baseURL: nil)
        }
    }

    final class Coordinator { var loaded: String? }

    static func html(markdown: String) -> String {
        let mdLiteral = (try? String(data: JSONEncoder().encode(markdown), encoding: .utf8)) ?? "\"\""
        return """
        <!DOCTYPE html><html><head><meta charset="utf-8">
        <meta name="viewport" content="width=device-width, initial-scale=1">
        <link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/katex@0.16.9/dist/katex.min.css">
        <link rel="stylesheet" href="https://cdn.jsdelivr.net/gh/highlightjs/cdn-release@11.9.0/build/styles/github.min.css" media="(prefers-color-scheme: light)">
        <link rel="stylesheet" href="https://cdn.jsdelivr.net/gh/highlightjs/cdn-release@11.9.0/build/styles/github-dark.min.css" media="(prefers-color-scheme: dark)">
        <script src="https://cdn.jsdelivr.net/npm/katex@0.16.9/dist/katex.min.js"></script>
        <script src="https://cdn.jsdelivr.net/npm/marked/marked.min.js"></script>
        <script src="https://cdn.jsdelivr.net/gh/highlightjs/cdn-release@11.9.0/build/highlight.min.js"></script>
        <style>
          html { color-scheme: light dark; }
          body {
            font: 16px/1.65 -apple-system, "SF Pro Text", system-ui, sans-serif;
            color: #1d1d1f; background: transparent;
            margin: 0; padding: 22px 26px 60px; -webkit-font-smoothing: antialiased;
          }
          @media (prefers-color-scheme: dark) { body { color: #e8e8ea; } }
          h1 { font-size: 1.5em; margin: 1.2em 0 .4em; }
          h2 { font-size: 1.25em; margin: 1.1em 0 .35em; }
          h3,h4 { font-size: 1.08em; margin: 1em 0 .3em; }
          p { margin: 0 0 .9em; }
          a { color: #0a84ff; text-decoration: none; }
          hr { border: none; border-top: 1px solid color-mix(in srgb, currentColor 15%, transparent); margin: 1.4em 0; }
          em { color: color-mix(in srgb, currentColor 65%, transparent); }
          blockquote { margin: .9em 0; padding: .3em 1em;
            border-left: 3px solid color-mix(in srgb, var(--accent, #0a84ff) 70%, transparent);
            background: color-mix(in srgb, currentColor 5%, transparent);
            border-radius: 0 8px 8px 0; }
          code { font-family: "SF Mono", ui-monospace, monospace; font-size: .88em; }
          :not(pre) > code { background: color-mix(in srgb, currentColor 10%, transparent);
            padding: .12em .35em; border-radius: 5px; }
          pre { background: color-mix(in srgb, currentColor 7%, transparent);
            padding: 12px 14px; border-radius: 10px; overflow-x: auto; }
          pre code { background: none; padding: 0; }
          table { border-collapse: collapse; margin: 1em 0; }
          th, td { border: 1px solid color-mix(in srgb, currentColor 20%, transparent); padding: 6px 10px; }
          .katex-display { overflow-x: auto; }
        </style></head>
        <body><div id="content"></div>
        <script>
          const MD = \(mdLiteral);
          function protectMath(md){
            const store=[];
            const push=(t,d)=>{store.push({t:t,d:d});return "@@KBMATH"+(store.length-1)+"@@";};
            md=md.replace(/```math\\n([\\s\\S]+?)```/g,(m,x)=>push(x.trim(),true));
            md=md.replace(/\\$\\$([\\s\\S]+?)\\$\\$/g,(m,x)=>push(x,true));
            md=md.replace(/\\$`([^`]+?)`\\$/g,(m,x)=>push(x,false));
            md=md.replace(/\\$([^\\$\\n]+?)\\$/g,(m,x)=>push(x,false));
            return {md:md,store:store};
          }
          const p=protectMath(MD);
          let html=marked.parse(p.md);
          html=html.replace(/@@KBMATH(\\d+)@@/g,(m,i)=>{
            const e=p.store[+i];
            try { return katex.renderToString(e.t,{displayMode:e.d,throwOnError:false}); }
            catch(err){ return m; }
          });
          document.getElementById('content').innerHTML=html;
          document.querySelectorAll('#content pre code').forEach(el=>{ try{hljs.highlightElement(el);}catch(e){} });
        </script></body></html>
        """
    }
}

// The notes popup: a live markdown editor with rendered preview. Saving
// overwrites notes.md via the engine (PUT) which re-embeds it.
@MainActor
struct NotesSheet: View {
    let client: KBClient
    let paperId: String
    let title: String
    let initialNotes: String
    var onSaved: () -> Void = {}

    @Environment(\.dismiss) private var dismiss
    @State private var text: String
    @State private var preview: String
    @State private var saving = false
    @State private var status: String?
    @State private var savedText: String?
    @State private var previewTask: Task<Void, Never>?

    init(client: KBClient, paperId: String, title: String, initialNotes: String, onSaved: @escaping () -> Void = {}) {
        self.client = client; self.paperId = paperId; self.title = title
        self.initialNotes = initialNotes; self.onSaved = onSaved
        _text = State(initialValue: initialNotes)
        _preview = State(initialValue: initialNotes)
    }

    private var dirty: Bool { text != (savedText ?? initialNotes) }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 10) {
                Image(systemName: "note.text").foregroundStyle(.tint)
                VStack(alignment: .leading, spacing: 1) {
                    Text("Notes").font(.headline)
                    Text(title).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                }
                Spacer()
                if let status { Text(status).font(.caption).foregroundStyle(.secondary) }
                ReadAloudButton(text: text, title: "\(title) — notes").buttonStyle(.borderless)
                Button {
                    Task { await save() }
                } label: { Label(saving ? "Saving…" : "Save", systemImage: "arrow.down.doc") }
                    .buttonStyle(.borderedProminent)
                    .disabled(!dirty || saving)
                    .keyboardShortcut("s", modifiers: .command)
                Button("Done") { dismiss() }.keyboardShortcut(.cancelAction)
            }
            .padding(12)
            Divider()

            HSplitView {
                VStack(alignment: .leading, spacing: 0) {
                    paneLabel("Markdown")
                    TextEditor(text: $text)
                        .font(.system(.body, design: .monospaced))
                        .padding(8)
                        .scrollContentBackground(.hidden)
                }
                .frame(minWidth: 280)

                VStack(alignment: .leading, spacing: 0) {
                    paneLabel("Preview")
                    if preview.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                        Text("Nothing to preview yet.")
                            .font(.callout).foregroundStyle(.tertiary)
                            .frame(maxWidth: .infinity, maxHeight: .infinity)
                    } else {
                        MarkdownView(markdown: preview)
                    }
                }
                .frame(minWidth: 280)
                .layoutPriority(1)
            }
        }
        .frame(width: 860, height: 660)
        .onChange(of: text) { schedulePreview() }
    }

    private func paneLabel(_ s: String) -> some View {
        Text(s).font(.caption2.weight(.semibold)).foregroundStyle(.secondary)
            .padding(.horizontal, 12).padding(.vertical, 6)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(.bar)
            .overlay(alignment: .bottom) { Divider() }
    }

    // Debounce the (relatively heavy) WebView re-render while typing.
    private func schedulePreview() {
        status = dirty ? "Unsaved changes" : nil
        previewTask?.cancel()
        previewTask = Task {
            try? await Task.sleep(for: .milliseconds(400))
            if !Task.isCancelled { preview = text }
        }
    }

    private func save() async {
        saving = true; status = nil
        do {
            _ = try await client.putNotes(paperId, notes: text)
            savedText = text
            onSaved()
            status = "Saved"
        } catch {
            status = "Save failed"
        }
        saving = false
    }
}
