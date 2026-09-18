import XCTest
@testable import VcadApp

/// Every number in Manufacture, reachable by name.
///
/// Friction-log item 51: a number field could be typed into only with the
/// window frontmost, and nothing outside the view layer could set one or read
/// back whether it took. These walk the registry the panels are built from —
/// existence, then a set through the identifier, then a read back off the
/// model — and then check the registry against the panels themselves, so a
/// field that is added without a path through here fails here rather than on a
/// machine.
@MainActor
final class CNCFieldTests: XCTestCase {

    /// The views' own directory, for the drift check below.
    private var sources: URL {
        URL(fileURLWithPath: #filePath)                       // …/Tests/VcadAppTests/CNCFieldTests.swift
            .deletingLastPathComponent()                      // …/Tests/VcadAppTests
            .deletingLastPathComponent()                      // …/Tests
            .deletingLastPathComponent()                      // …/VcadApp
            .appendingPathComponent("Sources/VcadApp")
    }

    func testEveryFieldCanBeSetByNameAndReadBack() throws {
        for field in CNCWorkspace.fields {
            // A fresh workspace per field: a setter with a side effect on
            // another field (the tool diameter re-plans the operations) must
            // not be able to hide behind one that ran before it.
            let cnc = workspaceShowingEveryField()
            XCTAssertNotNil(cnc.fieldValue(field.id), "\(field.id) does not read")
            XCTAssertTrue(cnc.setFieldValue(field.id, to: field.sample), "\(field.id) does not set")
            let readBack = try XCTUnwrap(cnc.fieldValue(field.id), field.id)
            XCTAssertEqual(readBack, field.sample, accuracy: 1e-9,
                           "\(field.id) did not keep what it was given")
        }
    }

    /// A workspace in the state where every panel is showing: a contour
    /// operation (tabs, ramps, thin slots) with tabs on it, and a spoilboard
    /// declared under the blank. A field that is only on screen in one state
    /// is only settable in that state, and that is the same rule the panel
    /// follows — the stepper for a spoilboard's thickness does not exist while
    /// the blank is on the bare bed either.
    private func workspaceShowingEveryField() -> CNCWorkspace {
        let cnc = CNCWorkspace()
        cnc.addOperation(.contourOutside)
        cnc.setup.tabs = 3
        cnc.underStock = .spoilboard(3)
        return cnc
    }

    /// …and a field whose panel is not showing does not read, rather than
    /// reading a plausible zero.
    func testAFieldThatIsNotOnScreenDoesNotRead() {
        let cnc = CNCWorkspace()
        XCTAssertNil(cnc.underStock.thickness)
        XCTAssertNil(cnc.fieldValue("cnc.stock.spoilboard"),
                     "no spoilboard is declared, so its thickness is not a number to read")
        XCTAssertFalse(cnc.setFieldValue("cnc.stock.spoilboard", to: 3),
                       "…and setting it would be declaring one behind the operator's back")
        cnc.underStock = .spoilboard(3)
        XCTAssertEqual(cnc.fieldValue("cnc.stock.spoilboard"), 3)
    }

    /// The setter refuses what it cannot honour rather than storing it.
    func testUnknownNamesAndNonNumbersAreRefused() {
        let cnc = CNCWorkspace()
        XCTAssertNil(cnc.fieldValue("cnc.stock.colour"))
        XCTAssertFalse(cnc.setFieldValue("cnc.stock.colour", to: 1))
        XCTAssertFalse(cnc.setFieldValue("cnc.stock.width", to: .nan))
        XCTAssertFalse(cnc.setFieldValue("cnc.stock.width", to: .infinity))
        XCTAssertEqual(cnc.fieldValue("cnc.stock.width"), 40, "the default is untouched")
    }

    /// Setting through the name is the same edit the field's binding makes —
    /// including the invalidation that follows it. A path that changed the
    /// number without staling the job would be worse than no path at all.
    func testSettingByNameInvalidatesTheJustAsTypingDoes() async {
        let cnc = CNCWorkspace()
        cnc.build()
        let deadline = Date().addingTimeInterval(60)
        while cnc.generating && Date() < deadline { try? await Task.sleep(for: .milliseconds(20)) }
        cnc.setupConfirmed = true
        XCTAssertTrue(cnc.jobCurrent)

        XCTAssertTrue(cnc.setFieldValue("cnc.stock.thickness", to: 6))
        XCTAssertEqual(cnc.stockThickness, 6)
        XCTAssertFalse(cnc.jobCurrent, "the stock is part of what the job was built from")
        XCTAssertFalse(cnc.setupConfirmed, "and the confirmation is stale with it")
        XCTAssertNil(cnc.jobCode, "a stale job has nothing to send")
    }

    /// Two ranges are honoured by both paths, so the stepper and the name
    /// cannot disagree about what a value may be.
    func testRangesAreTheSameWhicheverWayTheValueArrives() {
        let cnc = CNCWorkspace()
        XCTAssertTrue(cnc.setFieldValue("cnc.tool.flutes", to: 99))
        XCTAssertEqual(cnc.toolFlutes, 6, "the stepper's own upper bound")
        XCTAssertTrue(cnc.setFieldValue("cnc.tool.flutes", to: 0))
        XCTAssertEqual(cnc.toolFlutes, 1)

        cnc.addOperation(.contourOutside)
        XCTAssertTrue(cnc.setFieldValue("cnc.tabs.count", to: 40))
        XCTAssertEqual(cnc.setup.tabs, 12, "the stepper's own upper bound")
        XCTAssertEqual(cnc.setup.tabPositions.count, 12, "…and they were placed, not just counted")
    }

    /// `cnc.tool.<n>.<field>` reaches every tool in the list, and the plain
    /// `cnc.tool.<field>` names still mean the primary end mill.
    func testTheToolFamilyAddressesEveryToolInTheList() throws {
        let cnc = CNCWorkspace()
        cnc.tools = [CNCTool(number: 1, kind: .flatEndMill, diameter: 3.175),
                     CNCTool(number: 2, kind: .drill, diameter: 2.5)]

        for tool in cnc.tools {
            for (field, sample) in [("diameter", 4.5), ("fluteLength", 7.0), ("stickout", 21.0)] {
                let id = "cnc.tool.\(tool.number).\(field)"
                XCTAssertNotNil(cnc.fieldValue(id), "\(id) does not read")
                XCTAssertTrue(cnc.setFieldValue(id, to: sample), "\(id) does not set")
                XCTAssertEqual(try XCTUnwrap(cnc.fieldValue(id)), sample, accuracy: 1e-9, id)
            }
            XCTAssertTrue(cnc.setFieldValue("cnc.tool.\(tool.number).flutes", to: 99))
            XCTAssertEqual(cnc.fieldValue("cnc.tool.\(tool.number).flutes"), 6,
                           "the stepper's own upper bound, whichever way the value arrives")
        }

        // The alias: the plain name is the primary end mill, which is T1.
        let drillBefore = try XCTUnwrap(cnc.fieldValue("cnc.tool.2.diameter"))
        XCTAssertTrue(cnc.setFieldValue("cnc.tool.diameter", to: 6))
        XCTAssertEqual(cnc.fieldValue("cnc.tool.1.diameter"), 6)
        XCTAssertEqual(cnc.fieldValue("cnc.tool.diameter"), 6)
        XCTAssertEqual(cnc.fieldValue("cnc.tool.2.diameter"), drillBefore,
                       "the plain name is the end mill's; the drill is untouched")

        // A tool that is not in the list is not a field, rather than a zero.
        XCTAssertNil(cnc.fieldValue("cnc.tool.7.diameter"))
        XCTAssertFalse(cnc.setFieldValue("cnc.tool.7.diameter", to: 3))
        XCTAssertNil(cnc.fieldValue("cnc.tool.1.colour"))
    }

    // MARK: - drift

    /// Every `CNCNumber` in the panels names itself, and every name it uses is
    /// in the registry. This is the test that fails when a field is added to a
    /// panel and nothing outside the view can reach it.
    func testEveryNumberFieldInThePanelsIsInTheRegistry() throws {
        let known = Set(CNCWorkspace.fields.map(\.id))
        var found: Set<String> = []
        var unnamed: [String] = []

        for url in try swiftFiles() {
            let text = try String(contentsOf: url, encoding: .utf8)
            // A `CNCNumber(` call runs to its closing paren; the identifier is
            // the last argument when there is one.
            for call in calls(named: "CNCNumber(", in: text) {
                guard let id = literalIdentifier(in: call) else {
                    // A dynamic identifier (a clamp's index, a tab's) is a
                    // family, not a field, and is covered by its own tests.
                    if call.contains("identifier: \"cnc.") { continue }
                    unnamed.append(url.lastPathComponent + ": " + call.prefix(70))
                    continue
                }
                found.insert(id)
            }
        }

        XCTAssertTrue(unnamed.isEmpty,
                      "every number field needs a stable identifier, not its label:\n" + unnamed.joined(separator: "\n"))
        XCTAssertFalse(found.isEmpty, "the panels were not found — check the source path")
        // The two steppers are numbers too, and carry their identifiers the
        // way an accessibility modifier does rather than as an argument.
        found.formUnion(["cnc.tool.flutes", "cnc.tabs.count",
                         "cnc.machine.jogStep", "cnc.machine.jogFeed"])
        // The tool panel addresses tools by number now that there is a list of
        // them (`cnc.tool.2.diameter`), which is an interpolated family and so
        // deliberately unmatched above. The plain `cnc.tool.*` names are kept
        // as aliases onto the primary end mill — that is what they always
        // meant, and a script or an assistive tool that already used them goes
        // on working. `testTheToolFamilyAddressesEveryToolInTheList` is what
        // guards them; this list records that they are alias names rather than
        // a panel's own.
        found.formUnion(["cnc.tool.diameter", "cnc.tool.fluteLength", "cnc.tool.stickout"])
        XCTAssertEqual(found.subtracting(known), [],
                       "these fields are on screen but cannot be set by name")
        XCTAssertEqual(known.subtracting(found), [],
                       "these names are in the registry but no panel uses them")
    }

    private func swiftFiles() throws -> [URL] {
        try FileManager.default
            .contentsOfDirectory(at: sources, includingPropertiesForKeys: nil)
            .filter { $0.pathExtension == "swift" }
    }

    /// Every `name(...)` call in `text`, balanced on parentheses so a nested
    /// `Binding(get:set:)` does not cut the call short.
    private func calls(named name: String, in text: String) -> [String] {
        var out: [String] = []
        var search = text.startIndex..<text.endIndex
        while let start = text.range(of: name, range: search) {
            var depth = 0
            var index = text.index(before: start.upperBound)   // the "("
            var end: String.Index?
            while index < text.endIndex {
                let character = text[index]
                if character == "(" { depth += 1 }
                if character == ")" {
                    depth -= 1
                    if depth == 0 { end = text.index(after: index); break }
                }
                index = text.index(after: index)
            }
            guard let end else { break }
            out.append(String(text[start.lowerBound..<end]))
            search = end..<text.endIndex
        }
        return out
    }

    /// `identifier: "cnc.foo.bar"` — a literal one only; an interpolated one
    /// names a family and is deliberately not matched.
    private func literalIdentifier(in call: String) -> String? {
        guard let marker = call.range(of: "identifier: \"") else { return nil }
        let rest = call[marker.upperBound...]
        guard let close = rest.firstIndex(of: "\"") else { return nil }
        let value = String(rest[rest.startIndex..<close])
        return value.contains("\\(") ? nil : value
    }
}
