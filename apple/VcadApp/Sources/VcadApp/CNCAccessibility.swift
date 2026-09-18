import Foundation
import SwiftUI
#if canImport(AppKit)
import AppKit
#endif

// The named-field table, bridged to `NSAccessibility`.
//
// Friction-log item 51, the half that was still open: *"nothing bridges that
// table to `NSAccessibility`. `CNCNumber` exposes its label, identifier and
// value but no setter, and there is no AX element subclass or scripting hook
// anywhere in the app — so an assistive tool or a script still cannot write a
// value; only the tests can."*
//
// Two ways in, one model underneath. Both go through `fieldValue(_:)` and
// `setFieldValue(_:to:)`, which are the same calls the bindings make — so a
// value set from outside stales the job exactly as typing it does. A path that
// changed a number without invalidating what was built from it would be worse
// than no path at all.
//
// 1. **`NSAccessibility`.** `CNCAccessibilityHost` is a real `NSView` mounted
//    in the Manufacture workspace whose accessibility children are one element
//    per named field, each with a role, a label, an identifier, a value — and
//    a *setter*. An assistive client walks them and writes to them like any
//    other text field. This is the part that was missing: SwiftUI's
//    `.accessibilityValue` is read-only, so the number fields on screen
//    announce themselves and cannot be written to.
//
// 2. **`VCAD_SET`.** The debug counterpart of `VCAD_STATUS` (item 31), for
//    driving the app from a script when the window is not frontmost and no
//    accessibility grant is in hand.
//
//    Item 31 was careful that the status dump is *a report, never a control
//    surface*. This is a control surface, so it is fenced: it exists only when
//    `VCAD_SET` names a file, it can only write numbers that are already in
//    the field table, and it refuses every one of them while the machine is
//    streaming. It cannot start a build, move a machine, open a file or change
//    a setting that is not a named number. Everything it did, and everything
//    it refused, is written back beside the request.

/// One named number, as something an assistive client can read and write.
///
/// `NSAccessibilityElement` rather than a hidden `NSControl` per field: the
/// elements are the app's own answer to "what numbers does this window have",
/// and building them from `CNCWorkspace.fields` means the table that the
/// panels are checked against is the same table the bridge publishes. A field
/// that is added to a panel without a path through the registry is already a
/// test failure (`CNCFieldTests`); now it is also missing from the bridge, and
/// for the same reason.
/// Not `@MainActor`: these are AppKit accessibility callbacks, which AppKit
/// calls on the main thread by contract but does not declare as isolated. The
/// isolation is asserted inside each one instead.
final class CNCFieldElement: NSAccessibilityElement {
    let field: CNCField
    /// Weak: the element is owned by the view, the workspace outlives neither
    /// reliably, and an element that outlived its workspace should read as
    /// having no value rather than keep one alive.
    nonisolated(unsafe) private(set) weak var workspace: CNCWorkspace?

    @MainActor
    init(field: CNCField, workspace: CNCWorkspace) {
        self.field = field
        self.workspace = workspace
        super.init()
        setAccessibilityRole(.textField)
        setAccessibilityLabel(field.label)
        setAccessibilityIdentifier(field.id)
        setAccessibilityHelp(field.unit.isEmpty ? field.label : "\(field.label), in \(field.unit)")
    }

    override func isAccessibilityElement() -> Bool { true }

    override func accessibilityValue() -> Any? {
        // A field whose panel is not showing has no value to read, rather than
        // a plausible zero — the same rule `fieldValue(_:)` follows for a
        // spoilboard thickness on a bare bed. The number is fetched as a
        // `Double?` and boxed outside the isolated body, because
        // `assumeIsolated` will only carry a `Sendable` result across.
        // `self` is not `Sendable`, so what the isolated body needs is lifted
        // out first: the identifier is a string and the workspace is a
        // main-actor class, both of which cross fine.
        let id = field.id, model = workspace
        let value: Double? = MainActor.assumeIsolated { model?.fieldValue(id) }
        return value.map(NSNumber.init(value:))
    }

    override func setAccessibilityValue(_ value: Any?) {
        guard let number = Self.number(from: value) else { return }
        let id = field.id, model = workspace
        MainActor.assumeIsolated {
            guard let model, !model.machine.active else { return }
            model.setFieldValue(id, to: number)
        }
    }

    /// Whether this field can be written *right now*. AppKit asks this before
    /// offering the setter, so a client is told "read-only while the machine
    /// is streaming" rather than having a write silently do nothing.
    override func isAccessibilitySelectorAllowed(_ selector: Selector) -> Bool {
        if selector == #selector(setAccessibilityValue(_:)) {
            let id = field.id, model = workspace
            return MainActor.assumeIsolated {
                guard let model else { return false }
                return !model.machine.active && model.fieldValue(id) != nil
            }
        }
        return super.isAccessibilitySelectorAllowed(selector)
    }

    /// What an assistive client might hand over: a number, or the string a
    /// user typed into it. Anything else is refused rather than coerced —
    /// `NSString.doubleValue` reads "six" as 0, and a 0 mm stepdown arriving
    /// from a misheard word is the kind of thing this whole app exists to
    /// refuse.
    static func number(from value: Any?) -> Double? {
        if let n = value as? NSNumber { return n.doubleValue }
        if let d = value as? Double { return d }
        if let i = value as? Int { return Double(i) }
        if let text = value as? String {
            return Double(text.trimmingCharacters(in: .whitespaces))
        }
        return nil
    }
}

/// The view that owns the field elements.
///
/// It draws nothing and takes no clicks; it exists so the elements have a
/// place in the window's accessibility tree. Mounted once in the Manufacture
/// workspace, beside the panels rather than inside any one of them, because
/// the fields it publishes belong to the workspace and not to whichever panel
/// happens to be showing.
final class CNCAccessibilityHost: NSView {
    nonisolated(unsafe) private var elements: [CNCFieldElement] = []

    @MainActor
    init(workspace: CNCWorkspace) {
        super.init(frame: .zero)
        rebuild(for: workspace)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    @MainActor
    func rebuild(for workspace: CNCWorkspace) {
        elements = CNCWorkspace.fields.map { CNCFieldElement(field: $0, workspace: workspace) }
        // Per-tool numbers are a family, not a fixed list, so they are built
        // from the list that is actually loaded (item 19). Without this a
        // second cutter would be on screen and unreachable — the very shape of
        // the gap item 51 was about.
        for tool in workspace.tools {
            for (suffix, label, unit) in [("diameter", "Diameter", "mm"),
                                          ("flutes", "Flutes", ""),
                                          ("fluteLength", "Flute length", "mm"),
                                          ("stickout", "Stickout", "mm")] {
                let field = CNCField(id: "cnc.tool.\(tool.number).\(suffix)",
                                     label: "T\(tool.number) \(label)", unit: unit, sample: 0)
                elements.append(CNCFieldElement(field: field, workspace: workspace))
            }
        }
        for element in elements { element.setAccessibilityParent(self) }
    }

    override func accessibilityChildren() -> [Any]? { elements }
    override func accessibilityRole() -> NSAccessibility.Role? { .group }
    override func accessibilityLabel() -> String? { "Manufacture numbers" }
    override func isAccessibilityElement() -> Bool { true }
    /// Nothing to draw and nothing to click: it is a place in the tree.
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    /// The element for one identifier, which is also what a client's
    /// `AXUIElement` search resolves to.
    func element(named id: String) -> CNCFieldElement? {
        elements.first { $0.field.id == id }
    }
    var fieldIdentifiers: [String] { elements.map(\.field.id) }
}

/// Mounts the host in SwiftUI. Zero-sized on purpose.
struct CNCAccessibilityBridge: NSViewRepresentable {
    var cnc: CNCWorkspace
    /// Rebuilt when the tool list changes, because the per-tool elements are
    /// built from it.
    var toolSignature: [Int]

    func makeNSView(context: Context) -> CNCAccessibilityHost { CNCAccessibilityHost(workspace: cnc) }
    func updateNSView(_ view: CNCAccessibilityHost, context: Context) { view.rebuild(for: cnc) }
}

// MARK: - VCAD_SET

/// The debug write path: `VCAD_SET` names a file of `field=value` lines, and
/// `SIGUSR2` applies it.
///
/// Deliberately narrow. `VcadStatus` is a report and stays one; this is the
/// only thing in the app that changes state from outside, it only changes
/// numbers that are already named in the field table, and it refuses
/// everything while the machine is streaming. It cannot build, run, connect,
/// open or save.
enum VcadFieldWriter {
    /// The request file, or nil when the app was not started with one — which
    /// is the normal case, and means there is no write path at all.
    static func source(environment: [String: String] = ProcessInfo.processInfo.environment) -> URL? {
        guard let path = environment["VCAD_SET"]?.trimmingCharacters(in: .whitespaces),
              !path.isEmpty else { return nil }
        return URL(fileURLWithPath: (path as NSString).expandingTildeInPath)
    }

    /// Where the answer goes: beside the request, as `<name>.result.json`.
    static func resultURL(for source: URL) -> URL {
        source.deletingPathExtension().appendingPathExtension("result.json")
    }

    /// One parsed instruction.
    struct Assignment: Equatable {
        var field: String
        var value: Double
    }

    /// `field = value`, one per line; `#` comments and blank lines ignored.
    /// A line that is not an assignment is reported, not guessed at.
    static func parse(_ text: String) -> (assignments: [Assignment], rejected: [String]) {
        var assignments: [Assignment] = []
        var rejected: [String] = []
        for raw in text.split(separator: "\n", omittingEmptySubsequences: false) {
            let line = raw.trimmingCharacters(in: .whitespaces)
            if line.isEmpty || line.hasPrefix("#") { continue }
            guard let separator = line.firstIndex(of: "=") else {
                rejected.append("\(line): not a field=value assignment")
                continue
            }
            let name = String(line[line.startIndex..<separator]).trimmingCharacters(in: .whitespaces)
            let text = String(line[line.index(after: separator)...]).trimmingCharacters(in: .whitespaces)
            guard let value = Double(text) else {
                rejected.append("\(line): \"\(text)\" is not a number")
                continue
            }
            assignments.append(Assignment(field: name, value: value))
        }
        return (assignments, rejected)
    }

    /// Apply the assignments, and say what happened to each.
    ///
    /// The whole point of reporting per field is that `setFieldValue` already
    /// refuses a name it does not know and a value it cannot honour. A writer
    /// that swallowed those would turn a refusal into a silent no-op, which is
    /// exactly the failure mode the named-field work was for.
    @MainActor
    static func apply(_ assignments: [Assignment], to cnc: CNCWorkspace) -> [[String: Any]] {
        assignments.map { assignment in
            var row: [String: Any] = ["field": assignment.field, "value": assignment.value]
            if cnc.machine.active {
                row["applied"] = false
                row["why"] = "the machine is streaming a job; nothing may be changed under it"
                return row
            }
            let applied = cnc.setFieldValue(assignment.field, to: assignment.value)
            row["applied"] = applied
            if applied {
                row["now"] = cnc.fieldValue(assignment.field) as Any
            } else {
                row["why"] = cnc.fieldValue(assignment.field) == nil
                    ? "not a field, or its panel is not showing"
                    : "the value is not a number this field can take"
            }
            return row
        }
    }

    /// Read the request file, apply it, write the answer. Returns the answer
    /// so the signal handler and a test take the same path.
    @discardableResult
    @MainActor
    static func run(_ cnc: CNCWorkspace, source: URL? = Self.source()) -> [String: Any]? {
        guard let source else { return nil }
        guard let text = try? String(contentsOf: source, encoding: .utf8) else {
            let answer: [String: Any] = ["error": "could not read \(source.path)"]
            write(answer, to: resultURL(for: source))
            return answer
        }
        let (assignments, rejected) = parse(text)
        let answer: [String: Any] = [
            "at": ISO8601DateFormatter().string(from: Date()),
            "source": source.path,
            "results": apply(assignments, to: cnc),
            "unparsed": rejected,
        ]
        write(answer, to: resultURL(for: source))
        return answer
    }

    private static func write(_ answer: [String: Any], to url: URL) {
        guard let data = try? JSONSerialization.data(withJSONObject: answer,
                                                     options: [.prettyPrinted, .sortedKeys]) else { return }
        try? data.write(to: url, options: .atomic)
        FileHandle.standardError.write(Data("[VCAD_SET] \(url.path)\n".utf8))
    }

    // MARK: - The signal

    @MainActor private static var source_: DispatchSourceSignal?

    /// `SIGUSR2` applies the request file, the way `SIGUSR1` writes the dump.
    /// Installed only when `VCAD_SET` names one: with no file there is no
    /// write path, and the signal is left alone.
    @MainActor static func installSignalHandler(_ model: EditorModel) {
        guard source_ == nil, let file = source() else { return }
        signal(SIGUSR2, SIG_IGN)
        let source = DispatchSource.makeSignalSource(signal: SIGUSR2, queue: .main)
        source.setEventHandler {
            MainActor.assumeIsolated { _ = run(model.cnc, source: file) }
        }
        source.resume()
        source_ = source
        FileHandle.standardError.write(Data(
            "[VCAD_SET] kill -USR2 \(ProcessInfo.processInfo.processIdentifier) applies \(file.path)\n".utf8))
    }
}
