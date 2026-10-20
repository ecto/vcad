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
        /// Stable id of the feature the command created, if any.
        let createdFeatureId: String?
        let partCount: Int
        let warnings: [String]
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
        return Result(
            documentJSON: try JSONSerialization.data(withJSONObject: result["document"] ?? [:]),
            createdFeatureId: result["createdFeatureId"] as? String,
            partCount: (result["parts"] as? [Any])?.count ?? 0,
            warnings: result["warnings"] as? [String] ?? []
        )
    }
}
