import AppKit
import XCTest
@testable import VcadApp

/// The accessibility bridge (friction-log item 51's second half).
///
/// **Which route these drive, and why.** They call the `NSAccessibility`
/// protocol methods — `accessibilityValue()`, `setAccessibilityValue(_:)`,
/// `isAccessibilitySelectorAllowed(_:)`, `accessibilityChildren()` — on the
/// real elements the app mounts, which is exactly the surface AppKit's
/// accessibility server calls when a client asks. What is *not* driven here is
/// the client side: `AXUIElementSetAttributeValue` needs a running,
/// front-most, accessibility-trusted app, and this suite is offscreen and
/// untrusted, so a client round trip would test the grant rather than the
/// bridge. `VCAD_SET` is covered end to end below, and is the route a script
/// on this machine can actually use without a grant.
@MainActor
final class CNCAccessibilityTests: XCTestCase {

    private func host(_ cnc: CNCWorkspace) -> CNCAccessibilityHost {
        CNCAccessibilityHost(workspace: cnc)
    }

    /// A workspace with every panel showing, so every field in the registry
    /// has a value — the same state `CNCFieldTests` uses.
    private func fullWorkspace() -> CNCWorkspace {
        let cnc = CNCWorkspace()
        cnc.addOperation(.contourOutside)
        cnc.setup.tabs = 3
        cnc.underStock = .spoilboard(3)
        return cnc
    }

    // MARK: - NSAccessibility

    /// Every named number is in the tree, with a role, a label, an identifier
    /// and a value.
    func testEveryNamedNumberIsAnAccessibilityElement() throws {
        let cnc = fullWorkspace()
        let view = host(cnc)
        let children = try XCTUnwrap(view.accessibilityChildren()) as? [CNCFieldElement]
        let elements = try XCTUnwrap(children)

        for field in CNCWorkspace.fields {
            let element = try XCTUnwrap(elements.first { $0.field.id == field.id },
                                        "\(field.id) is not in the accessibility tree")
            XCTAssertEqual(element.accessibilityRole(), .textField)
            XCTAssertEqual(element.accessibilityLabel(), field.label)
            XCTAssertEqual(element.accessibilityIdentifier(), field.id)
            XCTAssertTrue(element.isAccessibilityElement())
            let value = try XCTUnwrap(element.accessibilityValue() as? NSNumber, field.id)
            XCTAssertEqual(value.doubleValue, try XCTUnwrap(cnc.fieldValue(field.id)), accuracy: 1e-9)
        }
        XCTAssertEqual(view.accessibilityRole(), .group)
    }

    /// **The thing item 51 asked for: a client can set a number and the model
    /// changes.** Through `setAccessibilityValue`, which is the call an
    /// assistive client's write lands on.
    func testAnAccessibilityClientCanSetANumberAndTheModelChanges() throws {
        let cnc = fullWorkspace()
        let view = host(cnc)

        let stepdown = try XCTUnwrap(view.element(named: "cnc.op.stepdown"))
        XCTAssertTrue(stepdown.isAccessibilitySelectorAllowed(#selector(NSAccessibilityElement.setAccessibilityValue(_:))),
                      "the field advertises itself as settable")
        stepdown.setAccessibilityValue(NSNumber(value: 0.17))
        XCTAssertEqual(cnc.setup.stepdown, 0.17, accuracy: 1e-9,
                       "the model changed, not just the element")
        XCTAssertEqual((stepdown.accessibilityValue() as? NSNumber)?.doubleValue, 0.17)

        // A client that hands over the string a user typed works too.
        let thickness = try XCTUnwrap(view.element(named: "cnc.stock.thickness"))
        thickness.setAccessibilityValue("6.35")
        XCTAssertEqual(cnc.stockThickness, 6.35, accuracy: 1e-9)

        // …and one that hands over a word does not become a zero. A 0 mm
        // stepdown arriving from a misheard dictation is exactly what this
        // whole app exists to refuse.
        stepdown.setAccessibilityValue("nought point one")
        XCTAssertEqual(cnc.setup.stepdown, 0.17, accuracy: 1e-9, "unchanged, not zeroed")
    }

    /// A write through the bridge is the same edit the field's binding makes —
    /// including the invalidation. A path that changed a number without
    /// staling the job would be worse than no path.
    func testAWriteThroughTheBridgeStalesTheJob() async throws {
        let cnc = CNCWorkspace()
        cnc.build()
        let deadline = Date().addingTimeInterval(60)
        while cnc.generating && Date() < deadline { try? await Task.sleep(for: .milliseconds(20)) }
        cnc.setupConfirmed = true
        XCTAssertTrue(cnc.jobCurrent)

        let view = host(cnc)
        try XCTUnwrap(view.element(named: "cnc.stock.thickness")).setAccessibilityValue(NSNumber(value: 6))
        XCTAssertEqual(cnc.stockThickness, 6)
        XCTAssertFalse(cnc.jobCurrent, "the stock is part of what the job was built from")
        XCTAssertFalse(cnc.setupConfirmed)
        XCTAssertNil(cnc.jobCode)
    }

    /// The per-tool numbers are in the tree too, built from the list that is
    /// loaded — otherwise a second cutter would be on screen and unreachable,
    /// which is the shape of the gap item 51 was about.
    func testThePerToolNumbersAreInTheTree() throws {
        let cnc = fullWorkspace()
        cnc.tools = [CNCTool(number: 1, kind: .flatEndMill, diameter: 3.175),
                     CNCTool(number: 2, kind: .drill, diameter: 2.5)]
        let view = host(cnc)

        let drill = try XCTUnwrap(view.element(named: "cnc.tool.2.diameter"),
                                  "identifiers: \(view.fieldIdentifiers)")
        XCTAssertEqual(drill.accessibilityLabel(), "T2 Diameter")
        XCTAssertEqual((drill.accessibilityValue() as? NSNumber)?.doubleValue, 2.5)
        drill.setAccessibilityValue(NSNumber(value: 3.0))
        XCTAssertEqual(cnc.tool(number: 2)?.diameter, 3.0)
        XCTAssertEqual(cnc.toolDiameter, 3.175, "the end mill is untouched")

        // A tool that leaves takes its elements with it.
        XCTAssertTrue(cnc.removeTool(number: 2))
        view.rebuild(for: cnc)
        XCTAssertNil(view.element(named: "cnc.tool.2.diameter"))
    }

    /// A workspace with a job on the machine, streaming — the same route
    /// `CNCStudioTests` takes, because there is no back door into the stream
    /// and there should not be one.
    private func streaming() async -> CNCWorkspace {
        let cnc = CNCWorkspace()
        cnc.machine.connect(simulated: true)
        cnc.build()
        let deadline = Date().addingTimeInterval(120)
        while cnc.generating && Date() < deadline { try? await Task.sleep(for: .milliseconds(25)) }
        for warning in cnc.warnings { cnc.acknowledge(warning.id, on: true) }
        cnc.setupConfirmed = true
        cnc.startJob()
        return cnc
    }

    /// Nothing may be changed under a job that is streaming, and the element
    /// says so rather than accepting a write that does nothing.
    func testNothingIsSettableWhileTheMachineIsStreaming() async throws {
        let cnc = await streaming()
        defer { cnc.machine.disconnect() }
        XCTAssertTrue(cnc.machine.active)
        let view = host(cnc)
        let field = try XCTUnwrap(view.element(named: "cnc.stock.thickness"))
        let before = cnc.stockThickness

        XCTAssertFalse(field.isAccessibilitySelectorAllowed(#selector(NSAccessibilityElement.setAccessibilityValue(_:))),
                       "a client is told it is read-only rather than finding out by being ignored")
        field.setAccessibilityValue(NSNumber(value: 99))
        XCTAssertEqual(cnc.stockThickness, before, "and the write did not land either")
    }

    /// A field whose panel is not showing has no value and is not settable,
    /// rather than reading a plausible zero — the same rule the model follows.
    func testAFieldThatIsNotOnScreenHasNoAccessibleValue() throws {
        let cnc = CNCWorkspace()              // no spoilboard declared
        let view = host(cnc)
        let field = try XCTUnwrap(view.element(named: "cnc.stock.spoilboard"))
        XCTAssertNil(field.accessibilityValue())
        XCTAssertFalse(field.isAccessibilitySelectorAllowed(#selector(NSAccessibilityElement.setAccessibilityValue(_:))))
    }

    // MARK: - VCAD_SET

    func testVcadSetAppliesNamedNumbersAndReportsEachOne() throws {
        let cnc = fullWorkspace()
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("vcad-set-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let request = directory.appendingPathComponent("set.txt")
        try """
        # the first cut's numbers
        cnc.op.stepdown = 0.17
        cnc.op.feed=250
        cnc.stock.colour = 3
        cnc.stock.width = wide
        not an assignment
        """.write(to: request, atomically: true, encoding: .utf8)

        let answer = try XCTUnwrap(VcadFieldWriter.run(cnc, source: request))
        XCTAssertEqual(cnc.setup.stepdown, 0.17, accuracy: 1e-9)
        XCTAssertEqual(cnc.setup.feed, 250, accuracy: 1e-9)

        let results = try XCTUnwrap(answer["results"] as? [[String: Any]])
        XCTAssertEqual(results.count, 3, "three assignments parsed; two lines were not assignments")
        func row(_ name: String) throws -> [String: Any] {
            try XCTUnwrap(results.first { $0["field"] as? String == name }, name)
        }
        XCTAssertEqual(try row("cnc.op.stepdown")["applied"] as? Bool, true)
        XCTAssertEqual(try row("cnc.op.stepdown")["now"] as? Double, 0.17)
        // A name that is not a field is refused *by name*, not swallowed.
        XCTAssertEqual(try row("cnc.stock.colour")["applied"] as? Bool, false)
        XCTAssertNotNil(try row("cnc.stock.colour")["why"])

        let unparsed = try XCTUnwrap(answer["unparsed"] as? [String])
        XCTAssertEqual(unparsed.count, 2, "\(unparsed)")
        XCTAssertTrue(unparsed.contains { $0.contains("not a number") })
        XCTAssertTrue(unparsed.contains { $0.contains("not a field=value") })

        // The answer is written beside the request, where a script reads it.
        let result = VcadFieldWriter.resultURL(for: request)
        XCTAssertTrue(FileManager.default.fileExists(atPath: result.path), result.path)
    }

    /// `VCAD_SET` is a control surface, so it is fenced: no file, no write
    /// path at all.
    func testVcadSetDoesNothingWithoutARequestFile() {
        let cnc = fullWorkspace()
        XCTAssertNil(VcadFieldWriter.source(environment: [:]),
                     "no VCAD_SET means there is no write path")
        XCTAssertNil(VcadFieldWriter.run(cnc, source: nil))
        XCTAssertEqual(VcadFieldWriter.source(environment: ["VCAD_SET": "/tmp/x.txt"])?.path, "/tmp/x.txt")
    }

    /// …and it refuses every field while the machine is streaming, saying so
    /// per field rather than silently doing nothing.
    func testVcadSetRefusesWhileTheMachineIsStreaming() async throws {
        let cnc = await streaming()
        defer { cnc.machine.disconnect() }
        XCTAssertTrue(cnc.machine.active)
        let before = cnc.setup.stepdown

        let results = VcadFieldWriter.apply([.init(field: "cnc.op.stepdown", value: 0.17)], to: cnc)
        XCTAssertEqual(results.first?["applied"] as? Bool, false)
        XCTAssertEqual(results.first?["why"] as? String,
                       "the machine is streaming a job; nothing may be changed under it")
        XCTAssertEqual(cnc.setup.stepdown, before)
    }

    func testVcadSetParsesTheLinesItAccepts() {
        let (assignments, rejected) = VcadFieldWriter.parse("""

        # a comment
        a = 1
        b=2.5
        c
        d = x
        """)
        XCTAssertEqual(assignments, [.init(field: "a", value: 1), .init(field: "b", value: 2.5)])
        XCTAssertEqual(rejected.count, 2)
    }
}
