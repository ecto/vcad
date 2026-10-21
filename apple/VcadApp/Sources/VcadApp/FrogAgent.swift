import Foundation

// The built-in agent's turn loop: stream a completion, run every tool call it
// asks for against the open document (edits through the Rust tool lowering,
// reads answered by the engine), send the results back, repeat until the
// model answers in plain text. Tool failures go back to the model as tool
// errors; nothing here throws on a bad tool call.

/// The manifest shipped in the engine (`crates/vcad-app/agent/manifest.json`).
struct FrogManifest {
    var system: String
    var cacheKey: String
    var tools: [[String: Any]]
    var visionTools: [[String: Any]]

    init(_ json: [String: Any]) {
        system = json["system"] as? String ?? ""
        cacheKey = json["cache_key"] as? String ?? "vcad-agent"
        tools = json["tools"] as? [[String: Any]] ?? []
        visionTools = json["vision_tools"] as? [[String: Any]] ?? []
    }

    static func load() -> FrogManifest { FrogManifest(DocumentEngine.agentManifest()) }

    /// Tools offered to an endpoint; vision tools only when it takes images.
    func offered(supportsImages: Bool) -> [[String: Any]] {
        supportsImages ? tools + visionTools : tools
    }
}

/// Runs tool calls against a `DocumentEngine`.
@MainActor
final class FrogToolExecutor {
    /// What one tool call produced.
    struct Outcome {
        var name: String
        var ok: Bool
        /// Text for the `role: tool` message.
        var content: String
        /// A PNG viewport snapshot to attach (vision endpoints only).
        var image: Data?
        /// The engine result when the call changed the document.
        var mutation: DocumentEngine.Result?
    }

    static let readTools: Set<String> = ["describe_scene", "inspect"]

    let engine: DocumentEngine
    var supportsImages: Bool
    /// Renders the viewport as PNG for `render_view` (view name → bytes).
    var snapshot: ((String) async -> Data?)?

    init(engine: DocumentEngine, supportsImages: Bool = false, snapshot: ((String) async -> Data?)? = nil) {
        self.engine = engine
        self.supportsImages = supportsImages
        self.snapshot = snapshot
    }

    func execute(_ call: FrogToolCall) async -> Outcome {
        let raw = call.arguments.trimmingCharacters(in: .whitespacesAndNewlines)
        let parsed = raw.isEmpty ? [:] : (try? JSONSerialization.jsonObject(with: Data(raw.utf8))) as? [String: Any]
        guard let args = parsed else {
            return Outcome(name: call.name, ok: false, content: "error: arguments are not a JSON object: \(raw.prefix(200))")
        }
        if call.name == "render_view" { return await renderView(args) }
        if Self.readTools.contains(call.name) {
            do {
                return Outcome(name: call.name, ok: true, content: try engine.agentRead(call.name, args))
            } catch {
                return Outcome(name: call.name, ok: false, content: "error: \(error)")
            }
        }
        do {
            let r = try engine.applyTool(call.name, args)
            var text = "ok"
            if let id = r.createdFeatureId { text += "; created part \(id)" }
            text += "; document has \(counted(r.partCount, "part"))"
            if !r.warnings.isEmpty { text += "; warnings: " + r.warnings.joined(separator: "; ") }
            return Outcome(name: call.name, ok: true, content: text, mutation: r)
        } catch let refusal as DocumentEngine.Refusal {
            let hint = refusal.kind == "unsupported" ? " (not a tool you can call)" : ""
            return Outcome(name: call.name, ok: false, content: "error (\(refusal.kind))\(hint): \(refusal.message)")
        } catch {
            return Outcome(name: call.name, ok: false, content: "error: \(error)")
        }
    }

    /// With a vision endpoint and a renderer: an image. Otherwise the text
    /// scene description, so the model still learns what is there.
    private func renderView(_ args: [String: Any]) async -> Outcome {
        let view = args["view"] as? String ?? "current"
        if supportsImages, let png = await snapshot?(view) {
            return Outcome(name: "render_view", ok: true,
                           content: "Rendered the \(view) view; the image is attached in the next message.", image: png)
        }
        let scene = (try? engine.agentRead("describe_scene", [:])) ?? "scene unavailable"
        return Outcome(name: "render_view", ok: true,
                       content: "Images are not available on this endpoint; text scene description instead:\n\(scene)")
    }
}

/// What the UI hears during a turn.
enum FrogTurnEvent {
    case text(String)
    case toolStarted(id: String, name: String, arguments: String)
    case toolFinished(id: String, outcome: FrogToolExecutor.Outcome)
}

/// One agent conversation over one document.
@MainActor
final class FrogAgent {
    let client: FrogClient
    let manifest: FrogManifest
    let executor: FrogToolExecutor
    /// The conversation so far (system message first).
    private(set) var messages: [[String: Any]]
    var maxRounds = 16
    /// Stable for the session so the shared prefix stays in the KV cache.
    let cacheKey: String

    init(client: FrogClient, manifest: FrogManifest, executor: FrogToolExecutor) {
        self.client = client
        self.manifest = manifest
        self.executor = executor
        executor.supportsImages = client.config.supportsImages
        messages = [["role": "system", "content": manifest.system]]
        cacheKey = "\(manifest.cacheKey)-\(UUID().uuidString.prefix(8))"
    }

    /// Run one user turn to completion. Returns true when the document changed.
    @discardableResult
    func send(_ text: String, onEvent: (FrogTurnEvent) -> Void) async throws -> Bool {
        messages.append(["role": "user", "content": text])
        let tools = manifest.offered(supportsImages: client.config.supportsImages)
        var mutated = false
        for _ in 0..<maxRounds {
            try Task.checkCancellation()
            let acc = try await client.complete(messages: messages, tools: tools, cacheKey: cacheKey) { event in
                if case .text(let t) = event { onEvent(.text(t)) }
            }
            messages.append(acc.assistantMessage)
            let calls = acc.toolCalls
            if calls.isEmpty { return mutated }
            var images: [Data] = []
            for call in calls {
                if Task.isCancelled {
                    // Keep the transcript well-formed: every call gets a result.
                    messages.append(["role": "tool", "tool_call_id": call.id, "content": "cancelled by the user"])
                    continue
                }
                onEvent(.toolStarted(id: call.id, name: call.name, arguments: call.arguments))
                let outcome = await executor.execute(call)
                if outcome.mutation != nil { mutated = true }
                onEvent(.toolFinished(id: call.id, outcome: outcome))
                messages.append(["role": "tool", "tool_call_id": call.id, "content": outcome.content])
                if let png = outcome.image { images.append(png) }
            }
            if !images.isEmpty {
                messages.append(["role": "user", "content": images.map { png -> [String: Any] in
                    ["type": "image_url", "image_url": ["url": "data:image/png;base64,\(png.base64EncodedString())"]]
                }])
            }
            try Task.checkCancellation()
        }
        messages.append(["role": "user", "content": "(Stopped: too many tool rounds in one turn.)"])
        return mutated
    }
}
