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
        WindowGroup("KB") {
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

        Settings {
            SettingsView()
                .environment(server)
        }
    }
}
