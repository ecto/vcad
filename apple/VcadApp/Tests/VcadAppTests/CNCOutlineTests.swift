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
        cnc.toolDiameter = 2
        // A Ø2 cutter's flutes are 6 mm unless it is told otherwise, and the
        // job refuses a 10 mm cut with them — so ask for one it can make.
        cnc.stockThickness = 3
        try cnc.importOutline(try CNCOutline.parseDXF(square, name: "square.dxf"))
        XCTAssertEqual(cnc.operations.map(\.setup.kind), [.contourInside, .contourOutside])
        XCTAssertEqual(cnc.operations.map(\.name), ["Opening 10 × 10", "Outside profile"])
        XCTAssertEqual(cnc.stockWidth, 40); XCTAssertEqual(cnc.stockHeight, 30)
        XCTAssertEqual(cnc.operations.last?.setup.tabs, 3)
        // The stock frame's zero sits where the outline was drawn, so the path
        // lands on the part instead of beside it (item 47).
        XCTAssertEqual(cnc.origin.x, -10); XCTAssertEqual(cnc.origin.y, -5)
        cnc.generate(all: true)
        let deadline = Date().addingTimeInterval(60)
        while cnc.generating && Date() < deadline { try? await Task.sleep(for: .milliseconds(20)) }
        XCTAssertNil(cnc.error)
        XCTAssertTrue(cnc.jobCurrent)
        for op in cnc.operations {
            XCTAssertFalse(op.ranges.isEmpty, "\(op.name) produced no moves")
            XCTAssertGreaterThan(op.ranges.reduce(0) { $0 + ($1.end - $1.start) }, 8, op.name)
        }
    }

    /// Entering the stock thickness after importing the outline must not leave
    /// the contours cutting to the thickness that was there at import.
    func testThroughCutsFollowTheStockThickness() throws {
        let cnc = CNCWorkspace()
        let square = "0\nLWPOLYLINE\n70\n1\n10\n0\n20\n0\n10\n40\n20\n0\n10\n40\n20\n40\n10\n0\n20\n40\n0\nEOF\n"
        try cnc.importOutline(try CNCOutline.parseDXF(square, name: "square.dxf"))
        XCTAssertEqual(cnc.operations.map(\.setup.depth), [10])
        cnc.stockThickness = 6
        XCTAssertEqual(cnc.operations.map(\.setup.depth), [6])
        XCTAssertEqual(cnc.operations.map(\.setup.kind), [.contourOutside])
        // A depth the user set by hand is theirs.
        cnc.setup.depth = 2
        cnc.stockThickness = 8
        XCTAssertEqual(cnc.operations.map(\.setup.depth), [2])
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
        print("STATOR CAM: \(String(format: "%.1f", Date().timeIntervalSince(t0))) s, moves \(cnc.jobMoves.count), est \(CNCWorkspace.durationLabel(cnc.jobDuration)), blocked by \(cnc.policy?.blockedBy ?? [])")
        XCTAssertTrue(cnc.jobCurrent)
        // `VCAD_STATOR_GCODE_OUT` keeps the job so it can be checked outside the app.
        if let out = ProcessInfo.processInfo.environment["VCAD_STATOR_GCODE_OUT"] {
            try XCTUnwrap(cnc.jobCode).write(toFile: out, atomically: true, encoding: .utf8)
        }
    }
}
