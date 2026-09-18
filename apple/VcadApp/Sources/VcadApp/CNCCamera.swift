import AppKit
import Foundation
import Observation

// The camera tile.
//
// On 2026-09-17 the operator watched the cut on a UniFi camera in another
// window while polling ncSender in a third. This puts the frame next to the
// progress bar. It is deliberately *stills*, not video: macOS AVFoundation
// does not play RTSP, and VLCKit or a bundled ffmpeg would be a large
// dependency for something whose whole job is "is the cutter still there".
//
// So: one JPEG every few seconds, from whichever of two backends the URL and
// the machine support, with the frame's age always on screen — a frozen tile
// that looks live is worse than no tile.

// MARK: - The URL is a secret

/// A string the app holds but does not say.
///
/// RTSP URLs carry `user:pass@`, so the camera URL is treated as a credential:
/// it is masked in the UI, scrubbed out of anything a backend reports, and
/// hidden from reflection — `dump`, `String(describing:)` and `String(reflecting:)`
/// all see the mask, so it cannot reach a log through a debug print.
struct CNCSecret: Equatable, Sendable, CustomStringConvertible, CustomDebugStringConvertible, CustomReflectable {
    private let value: String

    init(_ value: String) { self.value = value.trimmingCharacters(in: .whitespacesAndNewlines) }

    var isEmpty: Bool { value.isEmpty }
    /// The scheme, and how much was typed. Enough to confirm a paste landed.
    var masked: String {
        guard !value.isEmpty else { return "not set" }
        let scheme = value.components(separatedBy: "://").first.map { $0 + "://" } ?? ""
        return scheme + String(repeating: "•", count: 8) + " (\(value.count) characters)"
    }
    var description: String { masked }
    var debugDescription: String { masked }
    var customMirror: Mirror { Mirror(self, children: ["masked": masked]) }

    /// The one way out, named so that a reviewer sees every use.
    func reveal() -> String { value }

    /// Replace the secret wherever it appears in text a backend produced.
    /// ffmpeg puts the input URL in its own error messages.
    func scrub(_ text: String) -> String {
        guard !value.isEmpty else { return text }
        return text.replacingOccurrences(of: value, with: masked)
    }
}

// MARK: - Snapshot sources

/// One still frame, on demand.
protocol CNCCameraSnapshotSource: Sendable {
    /// Encoded image bytes (JPEG or PNG). Throws, never returns empty.
    func grab() async throws -> Data
    /// What the tile calls this backend.
    var label: String { get }
}

enum CNCCameraError: LocalizedError, Equatable {
    case noURL
    case noBackend(String)
    case failed(String)
    case notAnImage
    case timedOut

    var errorDescription: String? {
        switch self {
        case .noURL: return "No camera URL set."
        case .noBackend(let detail): return detail
        case .failed(let detail): return detail
        case .notAnImage: return "The camera answered, but not with an image."
        case .timedOut: return "The camera did not produce a frame in time."
        }
    }
}

/// `ffmpeg` on PATH, one frame at a time.
///
/// `ffmpeg -rtsp_transport tcp -i <url> -frames:v 1 -q:v 3 -f image2 pipe:1`.
/// Everything runs off the main thread; cancelling the task terminates the
/// process rather than leaving it holding the stream open.
struct FFmpegSnapshotSource: CNCCameraSnapshotSource {
    let executable: String
    let url: CNCSecret
    var timeout: TimeInterval = 12

    var label: String { "ffmpeg" }

    /// The usual installs. A GUI app's PATH is not the shell's, so these are
    /// checked even when PATH does not mention them.
    static let wellKnownPaths = ["/opt/homebrew/bin/ffmpeg", "/usr/local/bin/ffmpeg", "/usr/bin/ffmpeg"]

    /// Where `ffmpeg` is, or nil.
    static func locate(environment: [String: String] = ProcessInfo.processInfo.environment,
                       wellKnown: [String] = wellKnownPaths) -> String? {
        let manager = FileManager.default
        let fromPath = (environment["PATH"] ?? "").split(separator: ":").map { "\($0)/ffmpeg" }
        return (fromPath + wellKnown).first { manager.isExecutableFile(atPath: $0) }
    }

    func grab() async throws -> Data {
        let box = ProcessBox()
        let executable = self.executable
        let target = url.reveal()
        let secret = url
        let timeout = self.timeout
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Data, Error>) in
                let process = Process()
                process.executableURL = URL(fileURLWithPath: executable)
                process.arguments = [
                    "-nostdin", "-loglevel", "error",
                    "-rtsp_transport", "tcp",
                    "-i", target,
                    "-frames:v", "1", "-q:v", "3",
                    "-f", "image2", "pipe:1",
                ]
                let out = Pipe(), err = Pipe()
                process.standardOutput = out
                process.standardError = err
                process.standardInput = FileHandle.nullDevice
                guard box.adopt(process) else {
                    continuation.resume(throwing: CNCCameraError.failed("Cancelled before start."))
                    return
                }
                do { try process.run() } catch {
                    continuation.resume(throwing: CNCCameraError.failed(secret.scrub(error.localizedDescription)))
                    return
                }
                // Terminate on the deadline: an unreachable camera otherwise
                // leaves ffmpeg waiting on the socket until the app quits.
                DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + timeout) { box.terminate() }
                DispatchQueue.global(qos: .utility).async {
                    let errors = (try? err.fileHandleForReading.readToEnd()) ?? Data()
                    box.note(String(decoding: errors.prefix(4096), as: UTF8.self))
                }
                DispatchQueue.global(qos: .utility).async {
                    let data = (try? out.fileHandleForReading.readToEnd()) ?? Data()
                    process.waitUntilExit()
                    let status = process.terminationStatus
                    box.finish()
                    guard status == 0, !data.isEmpty else {
                        let detail = secret.scrub(box.notes).trimmingCharacters(in: .whitespacesAndNewlines)
                        continuation.resume(throwing: box.cancelled ? CNCCameraError.timedOut
                            : CNCCameraError.failed(detail.isEmpty ? "ffmpeg exited \(status) without a frame." : detail))
                        return
                    }
                    continuation.resume(returning: data)
                }
            }
        } onCancel: {
            box.terminate()
        }
    }

    /// Holds the running process across the cancellation boundary. `Process`
    /// is not `Sendable`; this is the one place that is acknowledged.
    private final class ProcessBox: @unchecked Sendable {
        private let lock = NSLock()
        private var process: Process?
        private var done = false
        private var killed = false
        private var stderrText = ""

        var cancelled: Bool { lock.withLock { killed } }
        var notes: String { lock.withLock { stderrText } }

        func adopt(_ process: Process) -> Bool {
            lock.withLock {
                guard !killed else { return false }
                self.process = process
                return true
            }
        }
        func note(_ text: String) { lock.withLock { stderrText += text } }
        func finish() { lock.withLock { done = true; process = nil } }
        func terminate() {
            let victim: Process? = lock.withLock {
                killed = true
                guard !done, let process, process.isRunning else { return nil }
                return process
            }
            victim?.terminate()
        }
    }
}

/// A plain HTTP(S) snapshot URL. Most NVRs and many cameras expose one, and it
/// needs no external binary at all.
struct HTTPSnapshotSource: CNCCameraSnapshotSource {
    let url: CNCSecret
    let session: URLSession

    init(url: CNCSecret, session: URLSession = .shared) {
        self.url = url
        self.session = session
    }

    var label: String { "HTTP snapshot" }

    func grab() async throws -> Data {
        guard let target = URL(string: url.reveal()) else { throw CNCCameraError.noURL }
        var request = URLRequest(url: target)
        request.timeoutInterval = 10
        let data: Data, response: URLResponse
        do { (data, response) = try await session.data(for: request) }
        catch { throw CNCCameraError.failed(url.scrub(error.localizedDescription)) }
        if let http = response as? HTTPURLResponse, !(200..<300).contains(http.statusCode) {
            throw CNCCameraError.failed("The camera answered \(http.statusCode).")
        }
        guard !data.isEmpty else { throw CNCCameraError.notAnImage }
        return data
    }
}

/// Which backend a URL can use on this machine.
enum CNCCameraBackend {
    /// Nil when nothing can serve this URL here — the tile says so rather than
    /// showing an empty frame forever.
    static func source(for url: CNCSecret,
                       ffmpegPath: String? = FFmpegSnapshotSource.locate(),
                       session: URLSession = .shared) -> Result<any CNCCameraSnapshotSource, CNCCameraError> {
        guard !url.isEmpty else { return .failure(.noURL) }
        let scheme = url.reveal().components(separatedBy: "://").first?.lowercased() ?? ""
        switch scheme {
        case "http", "https":
            return .success(HTTPSnapshotSource(url: url, session: session))
        case "rtsp", "rtsps":
            guard let ffmpegPath else {
                return .failure(.noBackend("macOS cannot play RTSP on its own and ffmpeg was not found on PATH. Install ffmpeg, or use the camera's HTTP snapshot URL instead."))
            }
            return .success(FFmpegSnapshotSource(executable: ffmpegPath, url: url))
        default:
            return .failure(.noBackend("“\(scheme.isEmpty ? "that" : scheme)” is not a camera URL this tile can read. Use rtsp://, rtsps://, or an HTTP snapshot URL."))
        }
    }
}

// MARK: - The model

/// The camera tile's state: the last frame, when it arrived, and why there is
/// none. Never holds the URL where anything can print it.
@MainActor @Observable
final class CNCCameraModel: @MainActor CustomStringConvertible, @MainActor CustomDebugStringConvertible,
                            @MainActor CustomReflectable {
    enum State: Equatable, Sendable {
        case off
        case waiting
        case live
        case failed(String)
    }

    private(set) var frame: NSImage?
    private(set) var frameAt: Date?
    private(set) var state: State = .off
    /// What the tile has to say, in order. Never contains the URL.
    private(set) var log: [String] = []
    private(set) var grabbing = false
    /// Bumped on every frame and every tick so the age label refreshes.
    private(set) var tick = 0

    private(set) var url: CNCSecret
    private(set) var interval: Double
    private let bounds: ClosedRange<Double>
    private var override: (any CNCCameraSnapshotSource)?
    private var loop: Task<Void, Never>?

    /// The UI's initialiser: 2–5 s, from `Prefs`.
    convenience init() {
        self.init(url: CNCSecret(NcSenderPrefs.cameraURL), interval: NcSenderPrefs.cameraInterval)
    }

    /// `source` overrides backend selection — that is how the tests drive it,
    /// and `bounds` is relaxed there so a test does not sleep for seconds.
    init(url: CNCSecret, interval: Double, bounds: ClosedRange<Double> = 2...5,
         source: (any CNCCameraSnapshotSource)? = nil) {
        self.url = url
        self.bounds = bounds
        self.interval = min(bounds.upperBound, max(bounds.lowerBound, interval))
        self.override = source
    }

    var description: String { "CNCCameraModel(camera: \(url.masked), \(state))" }
    var debugDescription: String { description }
    var customMirror: Mirror {
        Mirror(self, children: ["camera": url.masked, "state": state, "hasFrame": frame != nil])
    }

    var maskedURL: String { url.masked }
    var hasURL: Bool { !url.isEmpty }

    /// "3 s ago". The tile always shows one: a still frame with no age is the
    /// failure this whole tile exists to avoid.
    var ageLabel: String {
        _ = tick
        guard let frameAt else { return "no frame" }
        let age = max(0, Date().timeIntervalSince(frameAt))
        if age < 1 { return "just now" }
        if age < 90 { return "\(Int(age.rounded())) s ago" }
        return "\(Int((age / 60).rounded())) min ago"
    }

    func setURL(_ text: String) {
        let wasRunning = loop != nil
        stop()
        url = CNCSecret(text)
        NcSenderPrefs.cameraURL = url.reveal()
        frame = nil; frameAt = nil; state = .off; tick += 1
        if wasRunning, hasURL { start() }
    }

    func setInterval(_ seconds: Double) {
        interval = min(bounds.upperBound, max(bounds.lowerBound, seconds))
        NcSenderPrefs.cameraInterval = interval
    }

    private func makeSource() -> Result<any CNCCameraSnapshotSource, CNCCameraError> {
        if let override { return .success(override) }
        return CNCCameraBackend.source(for: url)
    }

    func start() {
        guard loop == nil else { return }
        switch makeSource() {
        case .failure(let error):
            state = .failed(error.localizedDescription); note(error.localizedDescription); return
        case .success(let source):
            state = .waiting
            note("Watching via \(source.label), one frame every \(NcSenderJobPlan.mm(interval, 1)) s.")
            loop = Task { [weak self] in
                while !Task.isCancelled {
                    guard let self else { return }
                    await self.grabOnce(source)
                    let wait = self.interval
                    try? await Task.sleep(for: .milliseconds(Int(wait * 1000)))
                }
            }
        }
    }

    func stop() {
        loop?.cancel(); loop = nil
        if state != .off { note("Stopped.") }
        if case .failed = state {} else { state = .off }
        tick += 1
    }

    var running: Bool { loop != nil }

    /// "Grab frame now" — one frame, outside the interval.
    func grabNow() async {
        switch makeSource() {
        case .failure(let error): state = .failed(error.localizedDescription); note(error.localizedDescription)
        case .success(let source): await grabOnce(source)
        }
    }

    private func grabOnce(_ source: any CNCCameraSnapshotSource) async {
        guard !grabbing else { return }
        grabbing = true
        defer { grabbing = false }
        do {
            let data = try await source.grab()
            guard let image = NSImage(data: data) else { throw CNCCameraError.notAnImage }
            frame = image
            frameAt = Date()
            state = .live
            tick += 1
        } catch is CancellationError {
            // Cancelling is not a failure; `stop()` has already said so.
        } catch {
            // Backends scrub the URL out of their own messages; this is the
            // last gate before anything reaches `log`.
            let detail = url.scrub((error as? LocalizedError)?.errorDescription ?? error.localizedDescription)
            state = .failed(detail)
            note(detail)
            tick += 1
        }
    }

    private func note(_ text: String) {
        log.append(url.scrub(text))
        if log.count > 50 { log.removeFirst(log.count - 50) }
    }
}
