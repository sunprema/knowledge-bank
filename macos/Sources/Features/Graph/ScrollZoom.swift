import SwiftUI
import AppKit

// A transparent overlay that turns scroll-wheel / two-finger-scroll events into
// a zoom factor anchored at the cursor — the gesture Mac users reach for first
// to "zoom" a canvas. It stays out of the way of clicks and drags by refusing
// hit-testing and instead listening through a local event monitor, so SwiftUI's
// own drag/tap gestures underneath keep working untouched.
struct ScrollZoom: NSViewRepresentable {
    /// (factor, cursor location in this view's top-left coordinate space).
    var onZoom: (CGFloat, CGPoint) -> Void

    func makeNSView(context: Context) -> Catcher {
        let v = Catcher()
        v.onZoom = onZoom
        return v
    }

    func updateNSView(_ nsView: Catcher, context: Context) {
        nsView.onZoom = onZoom
    }

    final class Catcher: NSView {
        var onZoom: ((CGFloat, CGPoint) -> Void)?
        private var monitor: Any?

        // Pass clicks/drags straight through to the SwiftUI views below.
        override func hitTest(_ point: NSPoint) -> NSView? { nil }
        override var acceptsFirstResponder: Bool { false }

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            removeMonitor()
            guard window != nil else { return }
            monitor = NSEvent.addLocalMonitorForEvents(matching: [.scrollWheel]) { [weak self] event in
                guard let self, let window = self.window, event.window === window else { return event }
                let inView = self.convert(event.locationInWindow, from: nil)
                guard self.bounds.contains(inView) else { return event }
                let dy = event.scrollingDeltaY
                guard dy != 0 else { return event }
                // Flip to SwiftUI's top-left origin.
                let loc = CGPoint(x: inView.x, y: self.bounds.height - inView.y)
                // Trackpads report large pixel deltas; the exp curve keeps zoom
                // smooth and symmetric for both scroll directions.
                let factor = exp(dy * 0.004)
                self.onZoom?(factor, loc)
                return nil   // consume so the page/canvas doesn't also scroll
            }
        }

        private func removeMonitor() {
            if let monitor { NSEvent.removeMonitor(monitor); self.monitor = nil }
        }

        deinit {
            if let monitor { NSEvent.removeMonitor(monitor) }
        }
    }
}
