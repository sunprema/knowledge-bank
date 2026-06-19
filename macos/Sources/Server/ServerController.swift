import Foundation
import Observation
import Darwin

// Owns the lifecycle of the bundled `kb serve` child process (LOCAL_UI_PRD §2):
// pick a free loopback port, spawn the engine with a generated API key, poll
// /health until ready, hand back a connected KBClient, and tear the process
// down on quit. The app is race-free on the key because we *inject* it via
// KB_API_KEY rather than reading the file the server would otherwise create.
@MainActor
@Observable
final class ServerController {
    enum Phase {
        case starting(String)         // status line for the launch screen
        case ready(KBClient)
        case failed(String)
    }

    private(set) var phase: Phase = .starting("Locating engine…")

    private var process: Process?
    private var logHandle: FileHandle?

    /// Keychain account holding the user's OpenAI key.
    static let openAIAccount = "openai_api_key"
    /// Keychain account holding the user's Anthropic key (optional — only the
    /// Roundtable's Claude agents need it).
    static let anthropicAccount = "anthropic_api_key"

    /// Whether search/chat/sparks can work — i.e. an OpenAI key is available
    /// (from the parent env, or stored in the Keychain). Drives onboarding UI.
    private(set) var hasOpenAIKey = false
    /// Whether Claude-backed roundtable agents can run (Anthropic key present).
    private(set) var hasAnthropicKey = false

    /// Resolution order for the embedding key: parent process env (dev
    /// convenience) wins, else the Keychain.
    private func resolveOpenAIKey() -> String? {
        resolveKey("OPENAI_API_KEY", account: Self.openAIAccount)
    }

    private func resolveAnthropicKey() -> String? {
        resolveKey("ANTHROPIC_API_KEY", account: Self.anthropicAccount)
    }

    /// Env (dev convenience) wins, else the Keychain.
    private func resolveKey(_ envVar: String, account: String) -> String? {
        if let env = ProcessInfo.processInfo.environment[envVar], !env.isEmpty {
            return env
        }
        if let stored = Keychain.get(account), !stored.isEmpty {
            return stored
        }
        return nil
    }

    /// Store the key and restart the engine so the child picks it up (it only
    /// reads `OPENAI_API_KEY` from its environment at spawn time).
    func setOpenAIKey(_ key: String) {
        let trimmed = key.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        Keychain.set(trimmed, account: Self.openAIAccount)
        restart()
    }

    func clearOpenAIKey() {
        Keychain.delete(Self.openAIAccount)
        restart()
    }

    /// Store the Anthropic key and restart so the engine sees it at spawn time.
    func setAnthropicKey(_ key: String) {
        let trimmed = key.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        Keychain.set(trimmed, account: Self.anthropicAccount)
        restart()
    }

    func clearAnthropicKey() {
        Keychain.delete(Self.anthropicAccount)
        restart()
    }

    static let kbRootDefaultsKey = "kbRoot"

    /// Where the corpus lives. Resolution order: `KB_ROOT` env (dev convenience,
    /// only present when launched from a shell) → a saved preference (set via
    /// Settings, survives Finder launches) → `~/arxiv-kb`.
    var kbRoot: URL {
        if let env = ProcessInfo.processInfo.environment["KB_ROOT"], !env.isEmpty {
            return URL(fileURLWithPath: (env as NSString).expandingTildeInPath)
        }
        if let saved = UserDefaults.standard.string(forKey: Self.kbRootDefaultsKey), !saved.isEmpty {
            return URL(fileURLWithPath: (saved as NSString).expandingTildeInPath)
        }
        return FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("arxiv-kb")
    }

    /// True when `KB_ROOT` is set in the environment — then the saved preference
    /// is ignored, and the Settings picker should say so.
    var kbRootIsFromEnv: Bool {
        !(ProcessInfo.processInfo.environment["KB_ROOT"] ?? "").isEmpty
    }

    /// Point the app at a different corpus folder and relaunch the engine.
    func setKBRoot(_ url: URL) {
        UserDefaults.standard.set(url.path, forKey: Self.kbRootDefaultsKey)
        restart()
    }

    /// Guards against duplicate launches — `WindowGroup` fires `onAppear` once
    /// per window, and macOS may restore several, so `start()` can be called
    /// multiple times. The engine must be spawned exactly once.
    private var isLaunching = false

    func start() {
        Task { await launch() }
    }

    private func launch() async {
        // Idempotent: skip if a launch is in flight or the engine is already up.
        guard !isLaunching else { return }
        if let p = process, p.isRunning { return }
        isLaunching = true
        defer { isLaunching = false }

        // 1. Locate the engine binary: bundled in Resources for a shipped app,
        //    or the dev build tree when running from source.
        guard let binary = locateEngine() else {
            phase = .failed("Couldn't find the `kb` engine binary. Rebuild with build.sh so it gets bundled, or check the project's target/release/kb.")
            return
        }

        guard FileManager.default.fileExists(atPath: kbRoot.path) else {
            phase = .failed("No knowledge bank at \(kbRoot.path). Create one with `kb add <arxiv-id>` first, or set KB_ROOT.")
            return
        }

        // 2. Generate the auth key and a free port; inject both into the child.
        let key = Self.makeKey()
        let port = Self.freePort()
        phase = .starting("Starting engine on 127.0.0.1:\(port)…")

        let proc = Process()
        proc.executableURL = binary
        proc.arguments = ["serve", "--port", String(port)]
        var env = ProcessInfo.processInfo.environment
        env["KB_ROOT"] = kbRoot.path
        env["KB_API_KEY"] = key
        let openAI = resolveOpenAIKey()
        hasOpenAIKey = (openAI != nil)
        if let openAI { env["OPENAI_API_KEY"] = openAI }
        let anthropic = resolveAnthropicKey()
        hasAnthropicKey = (anthropic != nil)
        if let anthropic { env["ANTHROPIC_API_KEY"] = anthropic }
        proc.environment = env

        // Funnel engine stderr/stdout to a log file for diagnostics.
        let logURL = FileManager.default.temporaryDirectory.appendingPathComponent("kb-mac-engine.log")
        FileManager.default.createFile(atPath: logURL.path, contents: nil)
        if let handle = try? FileHandle(forWritingTo: logURL) {
            proc.standardOutput = handle
            proc.standardError = handle
            logHandle = handle
        }

        proc.terminationHandler = { p in
            let status = p.terminationStatus
            Task { @MainActor [weak self] in self?.engineExited(code: status) }
        }

        do {
            try proc.run()
            process = proc
        } catch {
            phase = .failed("Failed to launch the engine: \(error.localizedDescription)")
            return
        }

        // 3. Health-gate. The engine refuses to start on an out-of-sync index
        //    (addendum §7), so a never-ready server usually means it exited —
        //    surface the tail of its log rather than spinning forever.
        let client = KBClient(baseURL: URL(string: "http://127.0.0.1:\(port)")!, apiKey: key)
        let deadline = Date().addingTimeInterval(15)
        while Date() < deadline {
            if proc.isRunning == false {
                phase = .failed("The engine exited during startup.\n\n" + Self.logTail(logURL))
                return
            }
            if (try? await client.health()) == true {
                phase = .ready(client)
                return
            }
            try? await Task.sleep(for: .milliseconds(200))
        }
        phase = .failed("The engine didn't become ready in time.\n\n" + Self.logTail(logURL))
    }

    private func engineExited(code: Int32) {
        if case .ready = phase {
            phase = .failed("The engine stopped unexpectedly (exit \(code)).")
        }
    }

    /// Synchronous teardown for app quit (brief block is acceptable on exit).
    func shutdown() {
        process?.terminationHandler = nil
        process?.interrupt()          // SIGINT → engine's graceful shutdown
        process?.waitUntilExit()
        try? logHandle?.close()
    }

    /// Stop the current engine without blocking the main thread, then relaunch
    /// — used when settings change (e.g. the OpenAI key) require a fresh spawn.
    func restart() {
        Task {
            await stopEngine()
            phase = .starting("Applying changes…")
            await launch()
        }
    }

    private func stopEngine() async {
        guard let p = process else { return }
        p.terminationHandler = nil
        if p.isRunning { p.interrupt() }
        for _ in 0..<30 where p.isRunning {
            try? await Task.sleep(for: .milliseconds(100))
        }
        if p.isRunning { p.terminate() }
        try? logHandle?.close()
        process = nil
        logHandle = nil
    }

    // MARK: - Helpers

    private func locateEngine() -> URL? {
        if let bundled = Bundle.main.url(forResource: "kb", withExtension: nil) {
            return bundled
        }
        // Dev fallback: walk up from the executable to find the cargo build.
        let candidates = [
            "/Volumes/x/kb/target/release/kb",
            "/Volumes/x/kb/target/debug/kb",
        ]
        return candidates.first { FileManager.default.isExecutableFile(atPath: $0) }
            .map { URL(fileURLWithPath: $0) }
    }

    private static func makeKey() -> String {
        var bytes = [UInt8](repeating: 0, count: 32)
        _ = SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes)
        return bytes.map { String(format: "%02x", $0) }.joined()
    }

    /// Bind a socket to port 0 so the OS hands us a free loopback port, then
    /// release it for the engine to claim. A tiny TOCTOU window, acceptable
    /// for a single-user local tool.
    private static func freePort() -> UInt16 {
        let fd = socket(AF_INET, SOCK_STREAM, 0)
        defer { close(fd) }
        var addr = sockaddr_in()
        addr.sin_family = sa_family_t(AF_INET)
        addr.sin_addr.s_addr = inet_addr("127.0.0.1")
        addr.sin_port = 0
        _ = withUnsafePointer(to: &addr) { p in
            p.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                bind(fd, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
            }
        }
        var bound = sockaddr_in()
        var len = socklen_t(MemoryLayout<sockaddr_in>.size)
        _ = withUnsafeMutablePointer(to: &bound) { p in
            p.withMemoryRebound(to: sockaddr.self, capacity: 1) { getsockname(fd, $0, &len) }
        }
        let port = UInt16(bigEndian: bound.sin_port)
        return port == 0 ? 4399 : port
    }

    private static func logTail(_ url: URL, lines: Int = 8) -> String {
        guard let text = try? String(contentsOf: url, encoding: .utf8) else { return "" }
        return text.split(separator: "\n").suffix(lines).joined(separator: "\n")
    }
}
