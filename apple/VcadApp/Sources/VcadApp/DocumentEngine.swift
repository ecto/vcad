import CVcadFFI
import Foundation

/// The shared Rust document engine (`vcad_app::DocumentApi`) over the C ABI.
///
/// Speaks the same `DocCommand` vocabulary as the web app's
/// `WasmDocumentEngine.apply`, so a mutation means the same thing on every
/// client. Commands, part info and the result envelope are the typed Swift
/// mirrors in `Generated/EngineTypes.swift`, generated from the Rust types
/// (`npm run swift:gen`); the materialized IR document stays raw JSON for the
/// existing `vcad_scene_from_json` path.
final class DocumentEngine {
    /// A command the engine refused (`CommandError` in Rust).
    struct Refusal: Error, CustomStringConvertible {
        /// `unknown_id`, `invalid`, `nothing_to`, `unsupported`, `batch`, or
        /// `ffi` when the call itself failed.
        let kind: String
        let message: String
        /// The structured error, when the engine sent one.
        var error: CommandError? = nil
        var description: String { message }
    }

    /// Materialized state after a successful command.
    struct Result {
        /// IR `Document` JSON.
        let documentJSON: Data
        /// The same document, parsed — what the editor renders and reads.
        let document: [String: Any]
        /// Per materialized feature: its stable id, kind and IR node ids.
        let parts: [EnginePartInfo]
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
        func owner(ofNode node: Int) -> (part: EnginePartInfo, role: String)? {
            for part in parts {
                if part.translateNodeId == node { return (part, "translate") }
                if part.rotateNodeId == node { return (part, "rotate") }
                if part.scaleNodeId == node { return (part, "scale") }
                if part.leafNodeIds.contains(node) { return (part, "primitive") }
            }
            return nil
        }

        /// Material key of the scene root whose part id is `id`.
        func material(ofPart id: String) -> String? {
            let ids = document["partIds"] as? [String: Any] ?? [:]
            let roots = document["roots"] as? [[String: Any]] ?? []
            return roots.first { r in
                (r["root"] as? NSNumber).map { ids[$0.stringValue] as? String == id } ?? false
            }?["material"] as? String
        }

        /// The material new parts get: the document's first defined material,
        /// else the first part's, else `default` (the pre-engine editor rule).
        var defaultMaterial: String {
            if let m = (document["materials"] as? [String: Any])?.keys.sorted().first { return m }
            if let r = (document["roots"] as? [[String: Any]])?.first?["material"] as? String { return r }
            return "default"
        }

        /// Whether the document's `materials` table defines `key`.
        func definesMaterial(_ key: String) -> Bool {
            (document["materials"] as? [String: Any])?[key] != nil
        }
    }

    private let handle: OpaquePointer

    init(replicaId: UInt64 = UInt64(Date().timeIntervalSince1970 * 1000)) {
        handle = vcad_engine_new(replicaId)
    }

    deinit { vcad_engine_free(handle) }

    /// Apply one `DocCommand`, e.g. `.setMaterial(id: id, material: "steel")`.
    @discardableResult
    func apply(_ command: DocCommand) throws -> Result {
        let json = String(decoding: try JSONEncoder().encode(command), as: UTF8.self)
        return try Self.unwrap(json.withCString { vcad_engine_apply(handle, $0) })
    }

    /// Lower and apply an AI tool call (`create`, `translate`, …) in Rust.
    @discardableResult
    func applyTool(_ call: ToolCall) throws -> Result {
        let json = String(decoding: try JSONEncoder().encode(call.args), as: UTF8.self)
        return try Self.unwrap(call.tool.withCString { t in json.withCString { vcad_engine_apply_tool(handle, t, $0) } })
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
        return try result(from: Data(String(cString: raw).utf8))
    }

    /// Decode an `EngineEnvelope`: a `Result`, or the `Refusal` it carries.
    static func result(from envelope: Data) throws -> Result {
        let decoded = try JSONDecoder().decode(EngineEnvelope.self, from: envelope)
        guard decoded.ok, let result = decoded.result else {
            let error = decoded.error
            throw Refusal(kind: error?.commandError.kind ?? "unknown",
                          message: error?.message ?? "command refused",
                          error: error?.commandError)
        }
        let document = result.document.foundationValue as? [String: Any] ?? [:]
        return Result(
            documentJSON: try JSONSerialization.data(withJSONObject: document),
            document: document,
            parts: result.parts,
            createdFeatureId: result.createdFeatureId,
            warnings: result.warnings
        )
    }
}

extension EnginePartInfo {
    /// The IR node ids of this part's leaf (everything but the transform
    /// chain): the primitive, boolean, fillet, sketch, … nodes.
    var leafNodeIds: [Int] {
        guard let payload = Mirror(reflecting: self).children.first?.value else { return [] }
        return Mirror(reflecting: payload).children.flatMap { child -> [Int] in
            guard let label = child.label,
                  label.hasSuffix("NodeId") || label.hasSuffix("NodeIds"),
                  !["translateNodeId", "rotateNodeId", "scaleNodeId"].contains(label) else { return [] }
            return (child.value as? Int).map { [$0] } ?? (child.value as? [Int]) ?? []
        }
    }
}

extension EngineJSON {
    /// The Foundation (`JSONSerialization`) form.
    var foundationValue: Any {
        switch self {
        case .null: return NSNull()
        case .bool(let v): return v
        case .number(let v): return v
        case .string(let v): return v
        case .array(let v): return v.map(\.foundationValue)
        case .object(let v): return v.mapValues(\.foundationValue)
        }
    }
}

extension EngineJSON: ExpressibleByStringLiteral, ExpressibleByFloatLiteral, ExpressibleByIntegerLiteral,
    ExpressibleByBooleanLiteral, ExpressibleByArrayLiteral, ExpressibleByDictionaryLiteral {
    public init(stringLiteral value: String) { self = .string(value) }
    public init(floatLiteral value: Double) { self = .number(value) }
    public init(integerLiteral value: Int) { self = .number(Double(value)) }
    public init(booleanLiteral value: Bool) { self = .bool(value) }
    public init(arrayLiteral elements: EngineJSON...) { self = .array(elements) }
    public init(dictionaryLiteral elements: (String, EngineJSON)...) {
        self = .object(Dictionary(elements, uniquingKeysWith: { _, last in last }))
    }
}
