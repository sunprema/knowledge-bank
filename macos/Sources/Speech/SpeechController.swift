import AVFoundation
import Observation

// The signature native feature (LOCAL_UI_PRD §4.1): read any text aloud with a
// proper transport. Wraps AVSpeechSynthesizer and publishes observable state so
// a mini-player can drive play/pause/stop and reflect what's being read.
@MainActor
@Observable
final class SpeechController: NSObject, AVSpeechSynthesizerDelegate {
    private let synth = AVSpeechSynthesizer()

    private(set) var isSpeaking = false
    private(set) var isPaused = false
    /// What's currently being read — shown in the mini-player.
    private(set) var nowReading: String = ""
    /// The exact text being read, so a Read-Aloud button can tell whether it's
    /// the active one and toggle stop.
    private(set) var currentText: String = ""

    /// Whether `text` is what's currently playing (paused counts as playing).
    func isReading(_ text: String) -> Bool {
        isSpeaking && currentText == text
    }

    var rate: Float = AVSpeechUtteranceDefaultSpeechRate
    /// Identifier of the preferred voice; nil = system default.
    var voiceIdentifier: String?

    override init() {
        super.init()
        synth.delegate = self
    }

    static var voices: [AVSpeechSynthesisVoice] {
        AVSpeechSynthesisVoice.speechVoices()
            .filter { $0.language.hasPrefix("en") }
            .sorted { $0.name < $1.name }
    }

    /// Speak `text`, labeled by `title` in the transport. Replaces anything
    /// currently playing.
    func speak(_ text: String, title: String) {
        let clean = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !clean.isEmpty else { return }
        if synth.isSpeaking { synth.stopSpeaking(at: .immediate) }

        let utterance = AVSpeechUtterance(string: clean)
        utterance.rate = rate
        if let id = voiceIdentifier, let v = AVSpeechSynthesisVoice(identifier: id) {
            utterance.voice = v
        }
        nowReading = title
        currentText = text
        synth.speak(utterance)
    }

    func togglePauseResume() {
        if synth.isPaused {
            synth.continueSpeaking()
        } else if synth.isSpeaking {
            synth.pauseSpeaking(at: .word)
        }
    }

    func stop() {
        synth.stopSpeaking(at: .immediate)
    }

    // MARK: AVSpeechSynthesizerDelegate

    nonisolated func speechSynthesizer(_ s: AVSpeechSynthesizer, didStart u: AVSpeechUtterance) {
        Task { @MainActor in isSpeaking = true; isPaused = false }
    }
    nonisolated func speechSynthesizer(_ s: AVSpeechSynthesizer, didPause u: AVSpeechUtterance) {
        Task { @MainActor in isPaused = true }
    }
    nonisolated func speechSynthesizer(_ s: AVSpeechSynthesizer, didContinue u: AVSpeechUtterance) {
        Task { @MainActor in isPaused = false }
    }
    nonisolated func speechSynthesizer(_ s: AVSpeechSynthesizer, didFinish u: AVSpeechUtterance) {
        Task { @MainActor in isSpeaking = false; isPaused = false; nowReading = ""; currentText = "" }
    }
    nonisolated func speechSynthesizer(_ s: AVSpeechSynthesizer, didCancel u: AVSpeechUtterance) {
        Task { @MainActor in isSpeaking = false; isPaused = false; nowReading = ""; currentText = "" }
    }
}
