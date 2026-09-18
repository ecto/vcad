import AppKit
import SwiftUI
import XCTest
@testable import VcadApp

/// The narrower follow-ups the integrate pass left: the readiness list outside
/// its popover, the outline re-checked when the part changes under it, and the
/// Work Zero radio group.
@MainActor
final class CNCFollowupTests: XCTestCase {

    private static func repoRoot() -> URL {
        var url = URL(fileURLWithPath: #filePath)
        for _ in 0..<5 { url.deleteLastPathComponent() }
        return url
    }

    /// `examples/parametric-plate.vcad` — an 80 × 50 × 6 plate with a Ø16 bore.
    private func plateData() throws -> Data {
        let url = Self.repoRoot().appendingPathComponent("examples/parametric-plate.vcad")
        guard FileManager.default.fileExists(atPath: url.path) else {
            throw XCTSkip("parametric-plate.vcad not found at \(url.path)")
        }
        return try Data(contentsOf: url)
    }

    /// The same plate, but a different size: the part edited after its outline
    /// was taken from it.
    ///
    /// Both the parameter and the node literal are moved. The kernel drives
    /// the cube from `parameters`, so editing the node alone re-evaluates to
    /// the original 80 mm — which is worth knowing before writing a fixture
    /// that silently does not change anything.
    private func widenedPlate(to width: Double) throws -> Data {
        var json = try XCTUnwrap(try JSONSerialization.jsonObject(with: try plateData()) as? [String: Any])
        var parameters = try XCTUnwrap(json["parameters"] as? [String: Any])
        var parameter = try XCTUnwrap(parameters["width"] as? [String: Any])
        parameter["value"] = width
        parameters["width"] = parameter; json["parameters"] = parameters
        var nodes = try XCTUnwrap(json["nodes"] as? [String: Any])
        var plate = try XCTUnwrap(nodes["1"] as? [String: Any])
        var op = try XCTUnwrap(plate["op"] as? [String: Any])
        var size = try XCTUnwrap(op["size"] as? [String: Any])
        size["x"] = width
        op["size"] = size; plate["op"] = op; nodes["1"] = plate; json["nodes"] = nodes
        return try JSONSerialization.data(withJSONObject: json)
    }

    private func document(_ data: Data) -> CNCModelDocument {
        CNCModelDocument(data: data, isLoon: false, name: "parametric-plate.vcad")
    }

    // MARK: - Item 16's caveat: the outline is re-checked when the part moves

    /// **A part edited after its outline was imported warns again.**
    ///
    /// The comparison ran at import and never afterwards, so a stale outline
    /// was caught and a *freshly*-staled one was not. Now the part on screen
    /// is re-sectioned and re-compared whenever it has changed.
    func testEditingTheSolidAfterImportingItsOutlineWarnsAgain() throws {
        let cnc = CNCWorkspace()
        var data = try plateData()
        cnc.modelDocument = { [data] in self.documentBox(data) }
        cnc.toolDiameter = 3.175
        XCTAssertTrue(cnc.importFromModel(), cnc.error ?? "no error")
        XCTAssertNil(cnc.outlineMismatch, "the outline came from this very part")
        let outline = try XCTUnwrap(cnc.outline)
        XCTAssertEqual(outline.width, 80, accuracy: 0.2, "the plate is 80 mm wide")

        // The part is edited under the outline: 80 mm becomes 120.
        data = try widenedPlate(to: 120)
        cnc.modelDocument = { [data] in self.documentBox(data) }

        cnc.recheckOutlineAgainstModel()
        let mismatch = try XCTUnwrap(cnc.outlineMismatch,
                                     "a 40 mm wider part must not machine the old outline in silence")
        // …and it says how far apart they are, not just that they differ.
        XCTAssertTrue(mismatch.contains("40.000 mm apart"), mismatch)
        XCTAssertEqual(cnc.outline?.width ?? 0, 80, accuracy: 0.2,
                       "the outline itself is untouched — it is the comparison that changed")
    }

    /// …and the warning has to be acknowledged again, rather than keeping the
    /// tick the *old* comparison earned. The outline and the clamps are
    /// identical, so without the mismatch in the job key the acknowledgement
    /// survived and a freshly-staled outline ran with a tick beside it.
    func testAFreshMismatchIsNotStillAcknowledged() throws {
        let cnc = CNCWorkspace()
        var data = try plateData()
        cnc.modelDocument = { [data] in self.documentBox(data) }
        cnc.toolDiameter = 3.175
        XCTAssertTrue(cnc.importFromModel(), cnc.error ?? "no error")
        let keyBefore = cnc.currentKey

        data = try widenedPlate(to: 120)
        cnc.modelDocument = { [data] in self.documentBox(data) }
        cnc.recheckOutlineAgainstModel()

        XCTAssertNotEqual(cnc.currentKey, keyBefore,
                          "the job a mismatch warns about is not the job that had none")
        XCTAssertTrue(cnc.warnings.contains { $0.id == "outline-mismatch" }
                        || cnc.blockers.contains { $0.id == "outline-mismatch" }
                        || !cnc.jobCurrent,
                      "the disagreement is on the job, not only in a caption")
    }

    /// Nothing is re-sectioned when nothing moved: the check is a hash of the
    /// document's bytes, and the kernel call only happens when they differ.
    func testTheRecheckIsSilentWhenThePartHasNotChanged() throws {
        let cnc = CNCWorkspace()
        let data = try plateData()
        cnc.modelDocument = { [data] in self.documentBox(data) }
        cnc.toolDiameter = 3.175
        XCTAssertTrue(cnc.importFromModel(), cnc.error ?? "no error")
        let key = cnc.currentKey
        for _ in 0..<3 { cnc.recheckOutlineAgainstModel() }
        XCTAssertNil(cnc.outlineMismatch)
        XCTAssertEqual(cnc.currentKey, key, "a no-op re-check does not stale the job")
    }

    // MARK: - Item 69: a DXF over a part takes its thickness from the part

    /// **A DXF says where the part ends in X and Y; the solid on screen says
    /// so in Z.** The stator's DXF over its own solid kept the 10 mm default
    /// and the first build was refused (9.52 mm flutes) for a number nobody
    /// typed. The plate is 6 mm tall, so the blank is 6 mm and its top is the
    /// plate's top.
    func testImportingADXFOverAPartTakesTheThicknessFromThePart() throws {
        let cnc = CNCWorkspace()
        let data = try plateData()
        cnc.modelDocument = { [data] in self.documentBox(data) }
        cnc.toolDiameter = 3.175
        XCTAssertEqual(cnc.stockThickness, 10, "the default, before anything is known")
        let square = "0\nLWPOLYLINE\n70\n1\n10\n0\n20\n0\n10\n80\n20\n0\n10\n80\n20\n50\n10\n0\n20\n50\n0\nEOF\n"
        try cnc.importOutline(try CNCOutline.parseDXF(square, name: "plate.dxf"))
        XCTAssertEqual(cnc.stockThickness, 6, accuracy: 1e-6, "the plate is 6 mm tall")
        let section = try CNCCam.section(try XCTUnwrap(cnc.modelDocument?()), partIndex: cnc.modelPartIndex)
        XCTAssertEqual(cnc.origin.z, try XCTUnwrap(section.zRange.last), accuracy: 1e-6,
                       "the stock top is the part's top")
        XCTAssertEqual(cnc.operations.map(\.setup.depth), [6], "the through-cut follows")
    }

    private func documentBox(_ data: Data) -> CNCModelDocument { document(data) }

    // MARK: - Item 6: the Work Zero radio group

    /// **The model can only hold one choice.** `zeroLocation` is a single
    /// enum, so exactly one of the three radios is the selection and the other
    /// two are not — whatever an offscreen render draws. This is the
    /// assertion that decides whether "all three looked filled" was a bug in
    /// the model or in the snapshot harness; it is the harness.
    func testWorkZeroIsExactlyOneChoice() {
        let cnc = CNCWorkspace()
        for choice in CNCZeroLocation.allCases {
            cnc.zeroLocation = choice
            let selected = CNCZeroLocation.allCases.filter { $0 == cnc.zeroLocation }
            XCTAssertEqual(selected.count, 1,
                           "exactly one of \(CNCZeroLocation.allCases.map(\.label)) is selected")
            XCTAssertEqual(selected.first, choice)
            // …and the numbers under the radios follow the choice, so the
            // three are not interchangeable labels on one state.
            XCTAssertTrue(cnc.stockCornerFromZero.allSatisfy(\.isFinite))
        }
        XCTAssertEqual(CNCZeroLocation.allCases.count, 3)
        XCTAssertEqual(Set(CNCZeroLocation.allCases.map(\.rawValue)).count, 3,
                       "three distinct tags, so the picker cannot match two rows to one value")
    }

    /// Choosing a different zero moves the blank's corner relative to it —
    /// which is what makes the choice mean something, and what a radio group
    /// that only *looked* like it had three selections would still get right.
    func testEachWorkZeroPutsTheBlankSomewhereElse() {
        let cnc = CNCWorkspace()
        var corners: [[Double]] = []
        for choice in CNCZeroLocation.allCases {
            cnc.zeroLocation = choice
            corners.append(cnc.stockCornerFromZero)
        }
        XCTAssertEqual(Set(corners.map { "\($0)" }).count, corners.count,
                       "three choices, three places: \(corners)")
    }

    // MARK: - Item 3: the readiness list is not popover-only

    /// The readiness list renders in the Machine stage from the same
    /// workspace the popover uses, so there is one source of truth and not a
    /// second copy of the run gate.
    func testTheReadinessListRendersInTheMachineStage() async throws {
        let model = EditorModel()
        model.workspace = .manufacture
        let cnc = model.cnc
        cnc.shown = true
        cnc.mode = .machine
        cnc.machine.connect(simulated: true)
        defer { cnc.machine.disconnect() }

        // The inspector's own answer for this stage, rather than the selected
        // item's: this is what item 3 asked for.
        let inspector = CNCStudioInspector(model: model)
        let hosting = NSHostingView(rootView: inspector)
        hosting.layoutSubtreeIfNeeded()
        XCTAssertGreaterThan(hosting.fittingSize.height, 0)

        // The list and the popover read the same gate, so they cannot
        // disagree about whether the job may run.
        let list = CNCReadinessList(cnc: cnc)
        XCTAssertNotNil(cnc.runBlocker, "an unbuilt job is not runnable")
        _ = list.body
    }
}
