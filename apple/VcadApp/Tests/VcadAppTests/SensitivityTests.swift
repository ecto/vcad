import XCTest
@testable import VcadApp

final class SensitivityTests: XCTestCase {
    /// A plate whose thickness `t` is a named loon parameter: 40 × 20 × t,
    /// compiled by the kernel the way the app opens a .loon file.
    private func plateJSON() throws -> Data {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString + ".loon")
        try "[defparam t 5.0]\n[root [cube 40 20 t] \"aluminum\"]\n".write(to: url, atomically: true, encoding: .utf8)
        defer { try? FileManager.default.removeItem(at: url) }
        return try XCTUnwrap(EditorModel.documentBytes(path: url.path))
    }

    func testPlateThicknessSensitivityFromTheSeam() throws {
        let data = try plateJSON()
        let rows = try XCTUnwrap(ParameterSensitivity.compute(documentJSON: data, densityKgM3: 2700))
        let t = try XCTUnwrap(rows.first { $0.name == "t" })
        XCTAssertNil(t.error, t.error ?? "")
        // V = 40·20·t → dV/dt = 800 mm³/mm; dm/dt = 800 · 2.7e-3 g.
        XCTAssertEqual(t.dVolume, 800, accuracy: 8)
        XCTAssertEqual(t.dMass, 2.16, accuracy: 0.03)
        // The centre of mass rises at half the thickness rate.
        XCTAssertEqual(t.dCentroid.z, 0.5, accuracy: 0.02)
        XCTAssertEqual(t.dCentroid.x, 0, accuracy: 1e-6)
    }

    func testParseCombinesPartsAndKeepsErrors() {
        let json: [String: Any] = ["parameters": [
            ["name": "a", "value": 1, "parts": [
                ["mass": 1, "d_mass": 1, "d_volume": 10, "centroid": [0, 0, 0], "d_centroid": [0, 0, 0]],
                ["mass": 1, "d_mass": 0, "d_volume": 0, "centroid": [2, 0, 0], "d_centroid": [0, 0, 0]],
            ]],
            ["name": "b", "value": 2, "error": "topology changed"],
        ]]
        let rows = ParameterSensitivity.parse(json)
        XCTAssertEqual(rows.map(\.name), ["a", "b"])
        XCTAssertEqual(rows[0].dVolume, 10)
        XCTAssertEqual(rows[0].dMass, 1)
        // C = Σmc/Σm = 1; adding mass at x=0 pulls it toward 0: dC = (0·2 − 2·1)/4 = −0.5.
        XCTAssertEqual(rows[0].dCentroid.x, -0.5, accuracy: 1e-12)
        XCTAssertEqual(rows[1].error, "topology changed")
    }
}
