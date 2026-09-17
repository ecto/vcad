import XCTest
@testable import VcadApp

@MainActor
final class CNCOutlineTests: XCTestCase {
    private let square = """
    0
    SECTION
    2
    ENTITIES
    0
    LWPOLYLINE
    8
    0
    90
    4
    70
    1
    10
    -10
    20
    -5
    10
    30
    20
    -5
    10
    30
    20
    25
    10
    -10
    20
    25
    0
    LWPOLYLINE
    90
    4
    70
    1
    10
    0
    20
    5
    10
    10
    20
    5
    10
    10
    20
    15
    10
    0
    20
    15
    0
    ENDSEC
    0
    EOF
    """

    func testDXFOutlineLandsInTheStockFrameWithHoles() throws {
        let outline = try CNCOutline.parseDXF(square, name: "square.dxf")
        XCTAssertEqual(outline.width, 40, accuracy: 1e-9)
        XCTAssertEqual(outline.height, 30, accuracy: 1e-9)
        XCTAssertEqual(outline.outer.bounds.origin, .zero)
        XCTAssertEqual(outline.holes.count, 1)
        XCTAssertEqual(outline.holes[0].bounds, CGRect(x: 10, y: 10, width: 10, height: 10))
    }

    func testOutlineImportBuildsContourOperationsAndGenerates() async throws {
        let cnc = CNCWorkspace()
        try cnc.importOutline(try CNCOutline.parseDXF(square, name: "square.dxf"))
        XCTAssertEqual(cnc.operations.map(\.setup.operation), ["contour_inside", "contour_outside"])
        XCTAssertEqual(cnc.stockWidth, 40); XCTAssertEqual(cnc.stockHeight, 30)
        XCTAssertEqual(cnc.operations.last?.setup.tabs, 3)
        cnc.generate(all: true)
        let deadline = Date().addingTimeInterval(20)
        while cnc.generating && Date() < deadline { try? await Task.sleep(for: .milliseconds(20)) }
        XCTAssertNil(cnc.error)
        for op in cnc.operations {
            XCTAssertTrue(op.current, "\(op.name) did not generate")
            XCTAssertGreaterThan(op.program?.moves.count ?? 0, 8, op.name)
        }
    }

    /// The real part: the rana stator outline, when `VCAD_STATOR_DXF` points at it.
    func testStatorOutlineGeneratesWithTabs() async throws {
        guard let path = ProcessInfo.processInfo.environment["VCAD_STATOR_DXF"],
              FileManager.default.fileExists(atPath: path) else { throw XCTSkip("stator outline not present") }
        let cnc = CNCWorkspace()
        cnc.stockThickness = 6
        let outline = try CNCOutline.parseDXF(try String(contentsOfFile: path, encoding: .utf8), name: "stator-outline.dxf")
        XCTAssertEqual(outline.holes.count, 4)
        try cnc.importOutline(outline)
        // Bore + slots machinable; three Ø2.5 pilots are smaller than the cutter.
        XCTAssertEqual(cnc.operations.count, 2)
        XCTAssertNotNil(cnc.error)
        let t0 = Date()
        cnc.generate(all: true)
        let deadline = Date().addingTimeInterval(60)
        while cnc.generating && Date() < deadline { try? await Task.sleep(for: .milliseconds(20)) }
        print("STATOR CAM: \(String(format: "%.1f", Date().timeIntervalSince(t0))) s, moves \(cnc.operations.map { $0.program?.moves.count ?? 0 }), est \(CNCWorkspace.durationLabel(cnc.jobDuration))")
        XCTAssertNil(cnc.error)
        XCTAssertTrue(cnc.jobCurrent)
    }
}
