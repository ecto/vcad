import AVFoundation

// vcad's "solve chime" — a short synthesized tone fired when geometry resolves,
// pitched by the verdict so you HEAR the difference between solved and
// solved-but-violated. A clean metallic "set" on success, a touch flat on a
// warning, a dry low knock on failure. Tones are synthesized on the fly (no
// asset files); the engine starts lazily on the first play.
@MainActor
final class Chime {
    enum Kind { case solved, warning, failed }

    private let engine = AVAudioEngine()
    private let player = AVAudioPlayerNode()
    private let format = AVAudioFormat(standardFormatWithSampleRate: 44_100, channels: 1)!
    private var started = false

    init() {
        engine.attach(player)
        engine.connect(player, to: engine.mainMixerNode, format: format)
    }

    func play(_ kind: Kind) {
        if !started {
            do {
                try engine.start()
                player.play()
                started = true
            } catch {
                return
            }
        }
        player.scheduleBuffer(buffer(for: kind), at: nil, options: [], completionHandler: nil)
    }

    private func buffer(for kind: Kind) -> AVAudioPCMBuffer {
        let sr = 44_100.0
        let duration = kind == .failed ? 0.10 : 0.17
        let frames = AVAudioFrameCount(sr * duration)
        let buf = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: frames)!
        buf.frameLength = frames
        let out = buf.floatChannelData![0]

        let f0: Double
        switch kind {
        case .solved: f0 = 880      // bright A5
        case .warning: f0 = 760     // a semitone-ish flat
        case .failed: f0 = 170      // a dry low knock
        }
        let decay = kind == .failed ? 38.0 : 15.0

        for i in 0..<Int(frames) {
            let t = Double(i) / sr
            let env = exp(-t * decay)
            var s = sin(2 * .pi * f0 * t)
            if kind != .failed {
                // inharmonic partials → a metallic, "machined" timbre.
                s += 0.42 * sin(2 * .pi * f0 * 2.01 * t)
                s += 0.18 * sin(2 * .pi * f0 * 3.0 * t)
            }
            out[i] = Float(s * env * 0.16)
        }
        return buf
    }
}
