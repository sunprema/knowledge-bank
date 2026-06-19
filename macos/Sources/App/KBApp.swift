import SwiftUI

// KB — a native macOS client for the personal knowledge bank. The engine
// (`kb serve`) runs as a managed child process; every screen is a view over its
// loopback API. See LOCAL_UI_PRD.md.
@main
@MainActor
struct KBApp: App {
    @State private var server = ServerController()
    @State private var speech = SpeechController()

    var body: some Scene {
        // Single identified window so the menu-bar item can reliably focus or
        // reopen it via openWindow(id: "main").
        Window("KB", id: "main") {
            RootView()
                .environment(server)
                .environment(speech)
                .frame(minWidth: 900, idealWidth: 1180, maxWidth: .infinity,
                       minHeight: 600, idealHeight: 760, maxHeight: .infinity)
                .onAppear { server.start() }
        }
        .windowStyle(.titleBar)
        .windowResizability(.contentSize)
        .defaultSize(width: 1180, height: 760)

        // Menu bar icon (top-right of the screen): engine status + quick access
        // to the main window. Keeps KB reachable even when its window is closed.
        MenuBarExtra("KB", systemImage: "books.vertical.fill") {
            MenuBarContent()
                .environment(server)
        }

        Settings {
            SettingsView()
                .environment(server)
        }
    }
}

// Pull-down from KB's menu bar icon: live engine status, a shortcut to bring the
// main window forward (reopening it if it was closed), and Quit.
@MainActor
private struct MenuBarContent: View {
    @Environment(\.openWindow) private var openWindow
    @Environment(ServerController.self) private var server

    var body: some View {
        Text(statusLine)

        Button("Open KB Window") {
            NSApp.activate(ignoringOtherApps: true)
            openWindow(id: "main")
        }
        .keyboardShortcut("o")

        Divider()

        // Engine controls — `kb serve` runs as a child process. Show Stop while
        // it's up, Start while it's down, and always offer a Restart.
        switch server.phase {
        case .ready, .starting:
            Button("Stop Engine") { server.stop() }
        case .stopped, .failed:
            Button("Start Engine") { server.start() }
        }
        Button("Restart Engine") { server.restart() }

        Divider()

        Button("Quit KB") { NSApplication.shared.terminate(nil) }
            .keyboardShortcut("q")
    }

    private var statusLine: String {
        switch server.phase {
        case .starting(let msg): return "Engine: \(msg)"
        case .ready:             return "Engine: running"
        case .failed(let msg):   return "Engine: failed — \(msg)"
        case .stopped:           return "Engine: stopped"
        }
    }
}
