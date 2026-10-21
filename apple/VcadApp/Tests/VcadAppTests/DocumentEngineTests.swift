import XCTest
@testable import VcadApp

final class DocumentEngineTests: XCTestCase {
    func testCommandsRoundTripThroughRust() throws {
        let engine = DocumentEngine(replicaId: 1)
        let created = try engine.apply(.addFeature(feature: .cube(sizeX: 40, sizeY: 20, sizeZ: 5), name: "Plate"))
        let id = try XCTUnwrap(created.createdFeatureId)
        XCTAssertEqual(created.partCount, 1)
        XCTAssertEqual(created.parts.first?.kind, "cube")
        XCTAssertEqual(created.parts.first?.name, "Plate")
        try engine.apply(.setTranslation(id: id, offset: [10, 0, 0]))
        let filleted = try engine.applyTool(ToolCall(tool: "create", args: [
            "type": "fillet", "parent_part_id": .string(id), "params": ["radius": 1],
        ]))
        XCTAssertEqual(filleted.parts.last?.kind, "fillet")
        try engine.apply(.undo)
    }

    func testRefusalsCarryTheEngineReason() {
        let engine = DocumentEngine(replicaId: 1)
        XCTAssertThrowsError(try engine.apply(.deleteFeature(id: "nope"))) { error in
            let refusal = error as? DocumentEngine.Refusal
            XCTAssertEqual(refusal?.kind, "unknown_id")
            XCTAssertEqual(refusal?.error, .unknownId(id: "nope"))
        }
        XCTAssertThrowsError(try engine.applyTool(ToolCall(tool: "teleport", args: [:]))) { error in
            XCTAssertEqual((error as? DocumentEngine.Refusal)?.kind, "unsupported")
        }
        // `place` lowers in Rust now (#955); without a part it is invalid.
        XCTAssertThrowsError(try engine.applyTool(ToolCall(tool: "place", args: [:]))) { error in
            XCTAssertEqual((error as? DocumentEngine.Refusal)?.kind, "invalid")
        }
        XCTAssertThrowsError(try engine.apply(.batch(commands: [.undo]))) { error in
            guard case .batch(let index, let source)? = (error as? DocumentEngine.Refusal)?.error else {
                return XCTFail("expected a batch error, got \(error)")
            }
            XCTAssertEqual(index, 0)
            XCTAssertEqual(source, .nothingTo(action: "undo"))
        }
    }

    func testDefineMaterialLandsInTheDocument() throws {
        let engine = DocumentEngine(replicaId: 1)
        let id = try XCTUnwrap(try engine.apply(.addFeature(feature: .sphere(radius: 2))).createdFeatureId)
        let r = try engine.apply(.batch(commands: [
            .defineMaterial(key: "brass", definition: MaterialDef(name: "brass", color: [0.8, 0.6, 0.2],
                                                                  metallic: 1, roughness: 0.3)),
            .setMaterial(id: id, material: "brass"),
        ]))
        XCTAssertTrue(r.definesMaterial("brass"))
        XCTAssertEqual(r.material(ofPart: id), "brass")
        XCTAssertEqual(r.defaultMaterial, "brass")
    }
}

/// The generated Swift types (`Generated/EngineTypes.swift`) against JSON the
/// Rust types produced (`Tests/Fixtures/engine-types.json`, kept in sync by
/// vcad-ffi's `swift_codegen_fixture_matches_rust_output`): every value must
/// decode, and re-encode to the same JSON.
final class DocumentEngineCodegenTests: XCTestCase {
    private func fixture() throws -> [String: Any] {
        let url = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Fixtures/engine-types.json")
        return try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any])
    }

    private func assertRoundTrip<T: Codable & Equatable>(_ type: T.Type, _ values: [Any],
                                                          file: StaticString = #filePath, line: UInt = #line) throws -> [T] {
        XCTAssertFalse(values.isEmpty, file: file, line: line)
        return try values.map { value in
            let rust = try JSONSerialization.data(withJSONObject: value)
            let decoded = try JSONDecoder().decode(T.self, from: rust)
            let swift = try JSONEncoder().encode(decoded)
            let again = try JSONSerialization.jsonObject(with: swift)
            XCTAssertEqual(again as? NSObject, value as? NSObject, "\(T.self) round trip", file: file, line: line)
            XCTAssertEqual(try JSONDecoder().decode(T.self, from: swift), decoded, file: file, line: line)
            return decoded
        }
    }

    func testCommandsRoundTrip() throws {
        let commands = try assertRoundTrip(DocCommand.self, try XCTUnwrap(fixture()["commands"] as? [Any]))
        // Every variant is covered.
        let covered = Set(commands.map(\.type))
        XCTAssertEqual(covered.count, 19, "fixture should exercise every DocCommand variant: \(covered.sorted())")
        XCTAssertEqual(commands[0], .addFeature(feature: .cube(sizeX: 1, sizeY: 2, sizeZ: 3), name: "Box"))
    }

    func testEnvelopesRoundTripAndDecodeIntoResults() throws {
        let raw = try XCTUnwrap(fixture()["envelopes"] as? [Any])
        let envelopes = try assertRoundTrip(EngineEnvelope.self, raw)
        let ok = envelopes.compactMap(\.result)
        XCTAssertEqual(Set(ok.flatMap(\.parts).map(\.kind)), ["cube", "cylinder", "boolean", "fillet"])
        let kinds = envelopes.compactMap(\.error).map(\.commandError.kind)
        XCTAssertEqual(kinds, ["unknown_id", "batch", "invalid"])

        // The engine's own decode path agrees.
        let data = try JSONSerialization.data(withJSONObject: raw[0])
        let r = try DocumentEngine.result(from: data)
        XCTAssertEqual(r.createdFeatureId, ok[0].createdFeatureId)
        XCTAssertEqual((r.document["roots"] as? [Any])?.count, 1)
        let cube = try XCTUnwrap(r.parts.first)
        XCTAssertEqual(r.owner(ofNode: cube.translateNodeId)?.role, "translate")
        XCTAssertEqual(cube.leafNodeIds.count, 1)
        XCTAssertEqual(r.owner(ofNode: cube.leafNodeIds[0])?.role, "primitive")
        XCTAssertThrowsError(try DocumentEngine.result(from: JSONSerialization.data(withJSONObject: raw[4])))
    }

    func testDeltaEnvelopesRoundTrip() throws {
        let deltas = try assertRoundTrip(EngineDeltaEnvelope.self, try XCTUnwrap(fixture()["deltas"] as? [Any]))
        let ok = deltas.compactMap(\.result)
        XCTAssertEqual(ok.first?.parts.added.map(\.kind), ["cube"])
        XCTAssertEqual(deltas.compactMap(\.error).map(\.commandError.kind), ["unknown_id", "batch", "invalid"])
    }

    func testToolCallsRoundTrip() throws {
        let calls = try assertRoundTrip(ToolCall.self, try XCTUnwrap(fixture()["toolCalls"] as? [Any]))
        XCTAssertEqual(calls.first?.tool, "create")
    }
}
