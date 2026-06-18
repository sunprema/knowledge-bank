import SwiftUI

// Persistent transport for speech readout, docked at the bottom of the window
// whenever something is being read aloud.
struct SpeechMiniPlayer: View {
    @Environment(SpeechController.self) private var speech

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: "waveform")
                .font(.title3)
                .foregroundStyle(.tint)
                .symbolEffect(.variableColor.iterative, isActive: !speech.isPaused)

            VStack(alignment: .leading, spacing: 1) {
                Text(speech.isPaused ? "Paused" : "Reading aloud")
                    .font(.caption2).foregroundStyle(.secondary)
                Text(speech.nowReading.isEmpty ? "Knowledge Bank" : speech.nowReading)
                    .font(.callout.weight(.medium))
                    .lineLimit(1)
            }

            Spacer()

            Button {
                speech.togglePauseResume()
            } label: {
                Image(systemName: speech.isPaused ? "play.fill" : "pause.fill")
                    .font(.title3)
            }
            .buttonStyle(.borderless)
            .help(speech.isPaused ? "Resume" : "Pause")

            Button {
                speech.stop()
            } label: {
                Image(systemName: "stop.fill").font(.title3)
            }
            .buttonStyle(.borderless)
            .help("Stop")
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        .background(.bar)
        .overlay(alignment: .top) { Divider() }
    }
}

// A reusable "Read aloud" button that toggles playback of its own text:
// click to start, click again to stop. Reflects whether it's the active source.
@MainActor
struct ReadAloudButton: View {
    @Environment(SpeechController.self) private var speech
    let text: String
    var title: String
    var compact: Bool = false

    private var active: Bool { speech.isReading(text) }

    var body: some View {
        Button {
            if active { speech.stop() } else { speech.speak(text, title: title) }
        } label: {
            let label = active ? "Stop" : "Read Aloud"
            let icon = active ? "stop.fill" : "speaker.wave.2.fill"
            if compact {
                Label(label, systemImage: icon).labelStyle(.iconOnly)
            } else {
                Label(label, systemImage: icon).labelStyle(.titleAndIcon)
            }
        }
        .help(active ? "Stop reading" : "Read this aloud")
        .disabled(text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
    }
}
