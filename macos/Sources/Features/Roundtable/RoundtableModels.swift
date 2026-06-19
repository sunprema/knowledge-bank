import SwiftUI

// The "Roundtable": a panel of specialist AI agents that brainstorm a startup
// objective with each other, grounded in the knowledge bank, and converge on a
// synthesis. This file holds the value types — all Codable so personas and
// completed debates persist across launches (PersonaStore / RoundtableStore).

// MARK: - Models the agents can run on

/// An LLM an agent speaks through. `provider` lets the UI badge who's Anthropic
/// vs OpenAI — the whole point of the mixed-provider roundtable.
struct LLMModel: Identifiable, Hashable {
    enum Provider: String { case anthropic = "Anthropic", openai = "OpenAI" }
    let id: String          // wire model id, e.g. "claude-opus-4-8"
    let label: String       // human label, e.g. "Claude Opus 4.8"
    let provider: Provider

    var providerGlyph: String {
        switch provider {
        case .anthropic: "sparkle"
        case .openai:    "circle.hexagongrid"
        }
    }
    var providerColor: Color {
        switch provider {
        case .anthropic: Color(red: 0.79, green: 0.45, blue: 0.30) // warm clay
        case .openai:    .teal
        }
    }

    static let opus     = LLMModel(id: "claude-opus-4-8",   label: "Claude Opus 4.8",   provider: .anthropic)
    static let sonnet   = LLMModel(id: "claude-sonnet-4-6", label: "Claude Sonnet 4.6", provider: .anthropic)
    static let haiku    = LLMModel(id: "claude-haiku-4-5",  label: "Claude Haiku 4.5",  provider: .anthropic)
    static let gpt4o    = LLMModel(id: "gpt-4o",            label: "GPT-4o",            provider: .openai)
    static let gpt4mini = LLMModel(id: "gpt-4o-mini",       label: "GPT-4o mini",       provider: .openai)

    /// Every model the UI offers for assignment.
    static let all: [LLMModel] = [.opus, .sonnet, .haiku, .gpt4o, .gpt4mini]

    /// Resolve a stored wire id back to a model (falls back to a labeled stub so
    /// an unknown id still renders and routes by prefix on the engine).
    static func byId(_ id: String) -> LLMModel {
        all.first { $0.id == id }
            ?? LLMModel(id: id, label: id, provider: id.hasPrefix("claude") ? .anthropic : .openai)
    }
}

// MARK: - Persona appearance vocabulary

/// Named colors a persona can wear (Color isn't Codable, so we store the name).
enum PersonaPalette {
    static let options: [(name: String, color: Color)] = [
        ("purple", .purple), ("green", .green), ("orange", .orange), ("blue", .blue),
        ("pink", .pink), ("teal", .teal), ("red", .red), ("indigo", .indigo),
        ("brown", .brown), ("accent", .accentColor),
    ]
    static func color(_ name: String) -> Color {
        options.first { $0.name == name }?.color ?? .accentColor
    }
}

/// Curated SF Symbols for persona avatars.
enum PersonaIcons {
    static let options = [
        "cpu", "chart.line.uptrend.xyaxis", "exclamationmark.shield", "wand.and.stars",
        "brain", "lightbulb", "scale.3d", "person.fill.questionmark", "flask",
        "megaphone", "shield.lefthalf.filled", "paintbrush.pointed", "books.vertical",
        "globe", "gearshape.2", "dollarsign.circle", "heart.text.square", "bolt.horizontal",
    ]
}

// MARK: - Personas

/// One seat at the table: a specialist with a distinct lens, icon, color, and
/// (assignable) model. The synthesizer is flagged `isSynth`, which the engine
/// runs last. Codable so the panel persists and is snapshotted into saved runs.
struct Persona: Identifiable, Codable, Hashable {
    var id: String
    var name: String
    var role: String
    /// Free-form markdown instructions that describe this persona to the LLM —
    /// the heart of a persona. Becomes the model's system prompt (composed with a
    /// little roundtable/chat framing). Empty ⇒ the engine falls back to a
    /// role-templated prompt, so older personas still work.
    var prompt: String
    var icon: String
    var colorName: String
    var modelId: String
    var isSynth: Bool
    /// Verifies the panel's claims against the corpus (runs after the debaters).
    var isFactChecker: Bool
    /// Pulls grounding from the corpus before speaking.
    var queriesKB: Bool
    /// Grants live tool access: the persona can search the corpus and fetch
    /// papers mid-turn via the agent harness. Only takes effect on Claude models
    /// (tool-use is Anthropic-only); ignored for OpenAI personas.
    var tools: Bool

    init(id: String = UUID().uuidString,
         name: String, role: String, prompt: String = "", icon: String,
         colorName: String, modelId: String,
         isSynth: Bool = false, isFactChecker: Bool = false, queriesKB: Bool = true,
         tools: Bool = false) {
        self.id = id
        self.name = name
        self.role = role
        self.prompt = prompt
        self.icon = icon
        self.colorName = colorName
        self.modelId = modelId
        self.isSynth = isSynth
        self.isFactChecker = isFactChecker
        self.queriesKB = queriesKB
        self.tools = tools
    }

    // Tolerant decode so panels/records saved before a field existed still load.
    enum CodingKeys: String, CodingKey {
        case id, name, role, prompt, icon, colorName, modelId, isSynth, isFactChecker, queriesKB, tools
    }
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        name = try c.decode(String.self, forKey: .name)
        role = try c.decode(String.self, forKey: .role)
        prompt = try c.decodeIfPresent(String.self, forKey: .prompt) ?? ""
        icon = try c.decode(String.self, forKey: .icon)
        colorName = try c.decode(String.self, forKey: .colorName)
        modelId = try c.decode(String.self, forKey: .modelId)
        isSynth = try c.decodeIfPresent(Bool.self, forKey: .isSynth) ?? false
        isFactChecker = try c.decodeIfPresent(Bool.self, forKey: .isFactChecker) ?? false
        queriesKB = try c.decodeIfPresent(Bool.self, forKey: .queriesKB) ?? true
        tools = try c.decodeIfPresent(Bool.self, forKey: .tools) ?? false
    }

    var color: Color { PersonaPalette.color(colorName) }
    var model: LLMModel { LLMModel.byId(modelId) }

    /// The human at the table. Used only to render the user's own directed
    /// messages (`@mention` chat) in the transcript — it's never part of a panel
    /// and never sent to the engine. Its reserved id resolves via
    /// `RoundtableSession.persona(_:)`.
    static let you = Persona(id: "__you__", name: "You", role: "you",
                             icon: "person.crop.circle.fill", colorName: "blue",
                             modelId: "", isSynth: false, isFactChecker: false, queriesKB: false)

    static let defaultPanel: [Persona] = [
        Persona(id: "tech",    name: "Aria",  role: "Technologist",
                prompt: "You are a senior systems technologist. Reason about feasibility, architecture, and what's actually buildable today versus speculative. Be concrete about the tech stack and the hardest engineering problem.",
                icon: "cpu", colorName: "purple", modelId: LLMModel.opus.id, queriesKB: true, tools: true),
        Persona(id: "biz",     name: "Mateo", role: "Business & GTM",
                prompt: "You are a pragmatic business and go-to-market lead. Focus on the customer, the wedge, pricing, distribution, and how this becomes a real business. Push for a sharp ICP and a first revenue motion.",
                icon: "chart.line.uptrend.xyaxis", colorName: "green", modelId: LLMModel.gpt4o.id, queriesKB: true),
        Persona(id: "skeptic", name: "Nadia", role: "Skeptic / Risk",
                prompt: "You are the resident skeptic. Stress-test the idea: name the strongest reasons it fails, the riskiest assumptions, and the competition. Be sharp but fair — the goal is to make the idea stronger.",
                icon: "exclamationmark.shield", colorName: "orange", modelId: LLMModel.sonnet.id, queriesKB: false),
        Persona(id: "factcheck", name: "Vera", role: "Fact-checker",
                icon: "checkmark.seal", colorName: "teal", modelId: LLMModel.gpt4o.id, isFactChecker: true, queriesKB: true),
        Persona(id: "synth",   name: "Sol",   role: "Synthesizer",
                icon: "wand.and.stars", colorName: "accent", modelId: LLMModel.opus.id, isSynth: true, queriesKB: true),
    ]

    /// Wire payload the engine expects for one persona.
    var wirePayload: [String: Any] {
        ["id": id, "name": name, "role": role, "prompt": prompt, "model": modelId,
         "is_synth": isSynth, "is_fact_checker": isFactChecker, "queries_kb": queriesKB,
         "tools": tools]
    }
}

// MARK: - Turns & citations

/// A knowledge-bank hit an agent pulled mid-thought.
struct AgentCitation: Identifiable, Codable, Hashable {
    var id = UUID()
    let title: String
    let sectionType: String
    var page: Int? = nil
}

/// A single agent's contribution in one round. `text` grows as the (revealed)
/// stream arrives; `status` drives the canvas node's state machine.
struct AgentTurn: Identifiable, Codable {
    enum Status: String, Codable { case thinking, queryingKB, streaming, done }
    var id = UUID()
    let personaId: String
    let round: Int
    var text: String = ""
    var citations: [AgentCitation] = []
    var status: Status = .thinking
}

/// A guiding idea the user injects while the debate runs.
struct Interjection: Identifiable {
    let id = UUID()
    let text: String
    let atRound: Int
}

// MARK: - Saved research

/// A persisted roundtable — its objective, the panel that ran it, every turn,
/// and the synthesis — so research can be reopened and continued anytime.
struct RoundtableRecord: Codable, Identifiable {
    let id: UUID
    var objective: String
    var createdAt: Date
    var updatedAt: Date
    var personas: [Persona]
    var turns: [AgentTurn]
    var scores: [AgentScore]

    init(id: UUID, objective: String, createdAt: Date, updatedAt: Date,
         personas: [Persona], turns: [AgentTurn], scores: [AgentScore]) {
        self.id = id; self.objective = objective; self.createdAt = createdAt
        self.updatedAt = updatedAt; self.personas = personas; self.turns = turns
        self.scores = scores
    }

    enum CodingKeys: String, CodingKey {
        case id, objective, createdAt, updatedAt, personas, turns, scores
    }
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(UUID.self, forKey: .id)
        objective = try c.decode(String.self, forKey: .objective)
        createdAt = try c.decode(Date.self, forKey: .createdAt)
        updatedAt = try c.decode(Date.self, forKey: .updatedAt)
        personas = try c.decode([Persona].self, forKey: .personas)
        turns = try c.decode([AgentTurn].self, forKey: .turns)
        scores = try c.decodeIfPresent([AgentScore].self, forKey: .scores) ?? []
    }

    /// The synthesizer's last contribution, if any.
    var synthesis: String? {
        turns.last { p in personas.first { $0.id == p.personaId }?.isSynth == true }?.text
    }
    var title: String {
        let t = objective.trimmingCharacters(in: .whitespacesAndNewlines)
        return t.isEmpty ? "Untitled research" : String(t.prefix(70))
    }
}

// MARK: - Activity log

/// One line in the live activity log.
struct LogEntry: Identifiable {
    enum Level { case info, active, ok, warn, error }
    let id = UUID()
    let time = Date()
    let level: Level
    let text: String

    var color: Color {
        switch level {
        case .info:   return .secondary
        case .active: return .blue
        case .ok:     return .green
        case .warn:   return .orange
        case .error:  return .red
        }
    }
    var glyph: String {
        switch level {
        case .info:   return "circle.fill"
        case .active: return "arrow.triangle.2.circlepath"
        case .ok:     return "checkmark.circle.fill"
        case .warn:   return "exclamationmark.circle.fill"
        case .error:  return "xmark.octagon.fill"
        }
    }
}

// MARK: - Wire events

/// One event from the engine's `/brainstorm` SSE stream. The engine tags each
/// event with `type`; the fields are a flat union (only those relevant to a
/// given `type` are populated). `KBClient.decoder` converts snake_case keys.
struct RoundtableEvent: Decodable, Sendable {
    let type: String
    var objective: String?
    var rounds: Int?
    var personaId: String?
    var round: Int?
    var query: String?
    var title: String?
    var sectionType: String?
    var page: Int?
    var snippet: String?
    var deepLink: String?
    var text: String?
    var message: String?
    var attempt: Int?
    var feasibility: Double?
    var market: Double?
    var defensibility: Double?
    var timing: Double?
    var rationale: String?
    var similarity: Double?
    var name: String?
    var role: String?
    var model: String?
    var reason: String?
}

/// One agent's rating of the idea across the four scorecard dimensions.
struct AgentScore: Codable, Identifiable {
    var id: String { personaId }
    let personaId: String
    let feasibility: Double
    let market: Double
    let defensibility: Double
    let timing: Double
    var rationale: String = ""

    /// Values in dimension order (matches `ScoreDimensions.labels`).
    var values: [Double] { [feasibility, market, defensibility, timing] }
}

enum ScoreDimensions {
    static let labels = ["Feasibility", "Market", "Defensibility", "Timing"]
    static let count = labels.count
    static let maxValue = 10.0

    /// Average per dimension across all agents (nil if no scores).
    static func averages(_ scores: [AgentScore]) -> [Double]? {
        guard !scores.isEmpty else { return nil }
        return (0..<count).map { i in
            scores.map { $0.values[i] }.reduce(0, +) / Double(scores.count)
        }
    }
}
