import CVcadFFI
import Foundation

/// The shared Rust document engine (`vcad_app::DocumentApi`) over the C ABI.
///
/// Speaks the same `DocCommand` vocabulary as the web app's
/// `WasmDocumentEngine.apply`, so a mutation means the same thing on every
/// client. Commands and results are JSON; the materialized IR document comes
/// back as raw JSON for the existing `vcad_scene_from_json` path.
final class DocumentEngine {
    /// A command the engine refused (`CommandError` in Rust).
    struct Refusal: Error, CustomStringConvertible {
        /// `unknown_id`, `invalid`, `nothing_to`, `unsupported` or `batch`.
        let kind: String
        let message: String
        var description: String { message }
    }

    /// Materialized state after a successful command.
    struct Result {
        /// IR `Document` JSON.
        let documentJSON: Data
        /// The same document, parsed — what the editor renders and reads.
        let document: [String: Any]
        /// `PartInfo` per materialized feature (`id`, `kind`, `*NodeId`s).
        let parts: [[String: Any]]
        /// Stable id of the feature the command created, if any.
        let createdFeatureId: String?
        var partCount: Int { parts.count }
        let warnings: [String]

        /// Stable part id (`"replica:seq"`) of a scene root node, via the
        /// document's `partIds` map. Node ids are renumbered on every
        /// materialize; part ids survive, so they are what commands carry.
        func partId(forRootNode node: Int) -> String? {
            (document["partIds"] as? [String: Any])?[String(node)] as? String
        }

        /// The part that owns `node` and the node's role in its transform
        /// chain: `translate`, `rotate`, `scale`, or `primitive` (the leaf —
        /// primitive, boolean, fillet, …).
        func owner(ofNode node: Int) -> (part: [String: Any], role: String)? {
            for part in parts {
                for (key, value) in part where key.hasSuffix("NodeId") {
                    guard (value as? NSNumber)?.intValue == node else { continue }
                    switch key {
                    case "translateNodeId": return (part, "translate")
                    case "rotateNodeId": return (part, "rotate")
                    case "scaleNodeId": return (part, "scale")
                    default: return (part, "primitive")
                    }
                }
            }
            return nil
        }
    }

    private let handle: OpaquePointer

    init(replicaId: UInt64 = UInt64(Date().timeIntervalSince1970 * 1000)) {
        handle = vcad_engine_new(replicaId)
    }

    deinit { vcad_engine_free(handle) }

    /// Apply one `DocCommand`, e.g. `["type": "set_material", "id": id, "material": "steel"]`.
    @discardableResult
    func apply(_ command: [String: Any]) throws -> Result {
        let json = String(decoding: try JSONSerialization.data(withJSONObject: command), as: UTF8.self)
        return try Self.unwrap(json.withCString { vcad_engine_apply(handle, $0) })
    }

    /// Lower and apply an AI tool call (`create`, `translate`, …) in Rust.
    @discardableResult
    func applyTool(_ tool: String, _ args: [String: Any]) throws -> Result {
        let json = String(decoding: try JSONSerialization.data(withJSONObject: args), as: UTF8.self)
        return try Self.unwrap(tool.withCString { t in json.withCString { vcad_engine_apply_tool(handle, t, $0) } })
    }

    /// Answer an agent read tool (`describe_scene`, `inspect`) as text.
    func agentRead(_ tool: String, _ args: [String: Any]) throws -> String {
        let json = String(decoding: try JSONSerialization.data(withJSONObject: args), as: UTF8.self)
        guard let raw = tool.withCString({ t in json.withCString { vcad_engine_agent_read(handle, t, $0) } }) else {
            throw Refusal(kind: "ffi", message: "agent read failed")
        }
        defer { vcad_engine_string_free(raw) }
        let envelope = try JSONSerialization.jsonObject(with: Data(String(cString: raw).utf8)) as? [String: Any] ?? [:]
        if envelope["ok"] as? Bool == true, let text = (envelope["result"] as? [String: Any])?["text"] as? String {
            return text
        }
        let error = envelope["error"] as? [String: Any] ?? [:]
        throw Refusal(kind: error["kind"] as? String ?? "unknown", message: error["message"] as? String ?? "read refused")
    }

    /// The agent's tool manifest + system prompt (`vcad_app::agent`).
    static func agentManifest() -> [String: Any] {
        guard let raw = vcad_agent_manifest() else { return [:] }
        defer { vcad_engine_string_free(raw) }
        return (try? JSONSerialization.jsonObject(with: Data(String(cString: raw).utf8))) as? [String: Any] ?? [:]
    }

    /// The current state (no mutation).
    func state() throws -> Result { try Self.unwrap(vcad_engine_state(handle)) }

    /// Replace the document with an IR `Document` (a `.vcad` file's
    /// contents); the undo history starts fresh. Throws, leaving the engine
    /// untouched, when the JSON isn't an IR document.
    @discardableResult
    func loadIR(_ document: [String: Any]) throws -> Result {
        let json = String(decoding: try JSONSerialization.data(withJSONObject: document), as: UTF8.self)
        return try Self.unwrap(json.withCString { vcad_engine_load_ir(handle, $0) })
    }

    /// Snapshot of document + undo/redo history, for `restore`.
    func save() -> Data {
        var len = 0
        guard let bytes = vcad_engine_save(handle, &len) else { return Data() }
        defer { vcad_engine_bytes_free(bytes, len) }
        return Data(bytes: bytes, count: len)
    }

    /// Put back a `save` snapshot (document and history).
    @discardableResult
    func restore(_ snapshot: Data) throws -> Result {
        try Self.unwrap(snapshot.withUnsafeBytes {
            vcad_engine_restore(handle, $0.bindMemory(to: UInt8.self).baseAddress, snapshot.count)
        })
    }

    var canUndo: Bool { vcad_engine_can_undo(handle) }
    var canRedo: Bool { vcad_engine_can_redo(handle) }

    /// Deterministic fingerprint of the materialized state — equal hashes
    /// mean equal documents (the editor's "unsaved changes" test).
    var stateHash: String {
        guard let raw = vcad_engine_state_hash(handle) else { return "" }
        defer { vcad_engine_string_free(raw) }
        return String(cString: raw)
    }

    private static func unwrap(_ raw: UnsafeMutablePointer<CChar>?) throws -> Result {
        guard let raw else {
            var count = 0
            let message = vcad_last_error(&count).map { String(decoding: UnsafeBufferPointer(start: $0, count: count), as: UTF8.self) }
            throw Refusal(kind: "ffi", message: message ?? "engine call failed")
        }
        defer { vcad_engine_string_free(raw) }
        let envelope = try JSONSerialization.jsonObject(with: Data(String(cString: raw).utf8)) as? [String: Any] ?? [:]
        guard envelope["ok"] as? Bool == true, let result = envelope["result"] as? [String: Any] else {
            let error = envelope["error"] as? [String: Any] ?? [:]
            throw Refusal(kind: error["kind"] as? String ?? "unknown", message: error["message"] as? String ?? "command refused")
        }
        let document = result["document"] as? [String: Any] ?? [:]
        return Result(
            documentJSON: try JSONSerialization.data(withJSONObject: document),
            document: document,
            parts: result["parts"] as? [[String: Any]] ?? [],
            createdFeatureId: result["createdFeatureId"] as? String,
            warnings: result["warnings"] as? [String] ?? []
        )
    }
}
