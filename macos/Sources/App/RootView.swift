import SwiftUI

// Launch gate: reflect the engine's lifecycle (starting / ready / failed) and,
// once ready, hand the connected client to the main UI.
struct RootView: View {
    @Environment(ServerController.self) private var server

    var body: some View {
        switch server.phase {
        case .starting(let status):
            LaunchView(status: status)
        case .failed(let message):
            FailureView(message: message)
        case .ready(let client):
            MainView(client: client)
                .transition(.opacity)
        case .stopped:
            StoppedView()
        }
    }
}

private struct StoppedView: View {
    @Environment(ServerController.self) private var server
    var body: some View {
        VStack(spacing: 16) {
            Image(systemName: "stop.circle.fill")
                .font(.system(size: 44)).foregroundStyle(.secondary)
            Text("Engine stopped").font(.title2.weight(.semibold))
            Text("The `kb serve` engine isn't running. Start it to search, chat, and run roundtables.")
                .font(.callout).foregroundStyle(.secondary)
                .multilineTextAlignment(.center).frame(maxWidth: 420)
            Button("Start Engine") { server.start() }
                .buttonStyle(.borderedProminent)
        }
        .padding(40)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(.background)
    }
}

private struct LaunchView: View {
    let status: String
    var body: some View {
        VStack(spacing: 18) {
            Image(systemName: "books.vertical.fill")
                .font(.system(size: 52))
                .foregroundStyle(.tint)
                .symbolEffect(.pulse, options: .repeating)
            Text("KB").font(.largeTitle.weight(.bold))
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text(status).font(.callout).foregroundStyle(.secondary)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(.background)
    }
}

private struct FailureView: View {
    @Environment(ServerController.self) private var server
    let message: String
    var body: some View {
        VStack(spacing: 16) {
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.system(size: 44)).foregroundStyle(.orange)
            Text("Couldn't start the engine").font(.title2.weight(.semibold))
            ScrollView {
                Text(message)
                    .font(.callout.monospaced())
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.leading)
                    .textSelection(.enabled)
                    .frame(maxWidth: 520, alignment: .leading)
            }
            .frame(maxHeight: 200)
            Button("Try Again") { server.start() }
                .buttonStyle(.borderedProminent)
        }
        .padding(40)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(.background)
    }
}

// MARK: - Main window

struct MainView: View {
    let client: KBClient
    @Environment(SpeechController.self) private var speech
    @Environment(ServerController.self) private var server
    @State private var section: AppSection = .search
    @State private var showKeyOnboarding = false
    /// Set by the Problems view to seed a roundtable objective; consumed (and
    /// cleared) by the Roundtable view on appear.
    @State private var roundtableSeed: String?
    /// Set by the Add view to open a just-ingested document; consumed (and
    /// cleared) by the Library view.
    @State private var libraryOpen: LibraryOpen?

    var body: some View {
        NavigationSplitView {
            SidebarView(section: $section)
                .navigationSplitViewColumnWidth(min: 200, ideal: 220, max: 280)
        } detail: {
            Group {
                switch section {
                case .search:  SearchView(client: client)
                case .add:
                    AddView(client: client, onOpen: { result in
                        libraryOpen = LibraryOpen(id: result.id, title: result.title)
                        section = .library
                    })
                case .library: LibraryView(client: client, openRequest: $libraryOpen)
                case .graph:   GraphView(client: client)
                case .chat:    ChatView(client: client)
                case .explore: ExploreView(client: client)
                case .personas: PersonasView()
                case .sparks:  SparksView(client: client)
                case .problems:
                    ProblemsView(client: client, onBrainstorm: { objective in
                        roundtableSeed = objective
                        section = .roundtable
                    })
                case .roundtable:
                    RoundtableView(client: client, seed: $roundtableSeed,
                                   onManagePersonas: { section = .personas })
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .safeAreaInset(edge: .bottom) {
            if speech.isSpeaking {
                SpeechMiniPlayer().transition(.move(edge: .bottom).combined(with: .opacity))
            }
        }
        .animation(.snappy, value: speech.isSpeaking)
        .sheet(isPresented: $showKeyOnboarding) { KeyOnboardingSheet() }
        .task {
            // First-run nudge: if the corpus can't be searched yet, offer to
            // connect OpenAI. Shown once per ready session.
            if !server.hasOpenAIKey { showKeyOnboarding = true }
        }
    }
}

enum AppSection: String, CaseIterable, Identifiable {
    case search, add, library, graph, chat, explore, personas, sparks, problems, roundtable
    var id: String { rawValue }
    var title: String {
        switch self {
        case .search: "Search"
        case .add: "Add"
        case .library: "Library"
        case .graph: "Graph"
        case .chat: "Chat"
        case .explore: "Explore"
        case .personas: "Personas"
        case .sparks: "Sparks"
        case .problems: "Problems"
        case .roundtable: "Roundtable"
        }
    }
    var icon: String {
        switch self {
        case .search: "magnifyingglass"
        case .add: "plus.circle"
        case .library: "books.vertical"
        case .graph: "point.3.connected.trianglepath.dotted"
        case .chat: "bubble.left.and.bubble.right"
        case .explore: "point.3.filled.connected.trianglepath.dotted"
        case .personas: "person.crop.rectangle.stack"
        case .sparks: "sparkles"
        case .problems: "lightbulb.max"
        case .roundtable: "person.3.sequence"
        }
    }
    var subtitle: String {
        switch self {
        case .search: "Find sections across the corpus"
        case .add: "Ingest papers, pages & PDFs"
        case .library: "Browse and read your papers"
        case .graph: "Explore connections visually"
        case .chat: "Ask questions over everything"
        case .explore: "Branch & merge a canvas chat"
        case .personas: "Reusable AI agents"
        case .sparks: "Surprising connections"
        case .problems: "Unsolved gaps worth building"
        case .roundtable: "Agents brainstorm your idea"
        }
    }
}
