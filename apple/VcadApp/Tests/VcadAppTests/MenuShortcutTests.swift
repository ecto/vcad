import XCTest
@testable import VcadApp

/// No two menu items claim the same key (friction-log item 55).
///
/// `CNCIntegrationTests` already pins this for `CNCCommand`, which is how the
/// Manufacture menu's ten shortcuts were picked without a clash. It could not
/// see the rest of the menu bar, and that is where the clash was: View ▸
/// Show/Hide All Panels and Camera ▸ Frame Selection both claimed ⌥⌘0, which
/// AppKit shows on both and fires on one.
///
/// So this reads the panels' own sources, the way `CNCFieldTests` reads them
/// for number fields. A source scan rather than a live menu walk because the
/// menu bar is built by SwiftUI `Commands` that need a running app to
/// instantiate, and the thing worth catching — two literals with the same key
/// and modifiers — is in the text.
final class MenuShortcutTests: XCTestCase {

    private var sources: URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()      // …/Tests/VcadAppTests
            .deletingLastPathComponent()      // …/Tests
            .deletingLastPathComponent()      // …/VcadApp
            .appendingPathComponent("Sources/VcadApp")
    }

    /// One literal `keyboardShortcut` call.
    private struct Shortcut: Hashable {
        var key: String
        var modifiers: Set<String>
        /// ⌘ is implied when no modifiers are given.
        var display: String {
            let order = ["control", "option", "shift", "command"]
            let symbols = ["control": "⌃", "option": "⌥", "shift": "⇧", "command": "⌘"]
            return order.filter(modifiers.contains).compactMap { symbols[$0] }.joined() + key.uppercased()
        }
    }

    /// `⌘K` is claimed twice on purpose: the menu item and the command bar's
    /// invisible accelerator. Both focus the same field, so it is duplication
    /// rather than a conflict (item 55 says so in as many words). Listed here
    /// so that the exception is a decision on the record rather than a hole.
    private let allowed: Set<String> = ["⌘K"]

    func testNoTwoMenuItemsClaimTheSameKey() throws {
        var seen: [Shortcut: [String]] = [:]
        for url in try swiftFiles() {
            let text = try String(contentsOf: url, encoding: .utf8)
            for (shortcut, call) in shortcuts(in: text) {
                seen[shortcut, default: []].append("\(url.lastPathComponent): \(call)")
            }
        }

        let clashes = seen
            .filter { $0.value.count > 1 && !allowed.contains($0.key.display) }
            .map { "\($0.key.display) is claimed \($0.value.count) times:\n    " + $0.value.joined(separator: "\n    ") }
            .sorted()
        XCTAssertTrue(clashes.isEmpty, "two menu items cannot share a key:\n" + clashes.joined(separator: "\n"))

        // …and the scan really found the menu bar, so an empty result can
        // never be the reason this passes.
        XCTAssertGreaterThan(seen.count, 20, "the sources were not read — check the path")
        XCTAssertNotNil(seen.keys.first { $0.display == "⌥⌘0" }, "Show/Hide All Panels")
        XCTAssertNotNil(seen.keys.first { $0.display == "⇧⌘0" }, "Frame Selection, moved off ⌥⌘0")
    }

    /// Every literal `keyboardShortcut("k", modifiers: [...])` in `text`.
    /// Non-literal keys (`ws.keyEquivalent`, `shortcut.key`) and the semantic
    /// `.defaultAction` / `.cancelAction` are skipped: those are a family or a
    /// role, not a key, and `CNCIntegrationTests` covers the one family that
    /// matters.
    private func shortcuts(in text: String) -> [(Shortcut, String)] {
        var out: [(Shortcut, String)] = []
        var search = text.startIndex..<text.endIndex
        while let start = text.range(of: "keyboardShortcut(", range: search) {
            var depth = 0
            var index = text.index(before: start.upperBound)
            var end: String.Index?
            while index < text.endIndex {
                if text[index] == "(" { depth += 1 }
                if text[index] == ")" { depth -= 1; if depth == 0 { end = text.index(after: index); break } }
                index = text.index(after: index)
            }
            guard let end else { break }
            let call = String(text[start.lowerBound..<end])
            search = end..<text.endIndex

            let inner = String(call.dropFirst("keyboardShortcut(".count).dropLast())
            let head = inner.split(separator: ",", maxSplits: 1).first.map {
                $0.trimmingCharacters(in: .whitespaces)
            } ?? ""
            // A quoted character, or one of the named keys spelled `.upArrow`.
            var key: String?
            if head.hasPrefix("\""), head.hasSuffix("\""), head.count >= 3 {
                key = String(head.dropFirst().dropLast())
            } else if head.hasPrefix("."), !["defaultAction", "cancelAction"].contains(String(head.dropFirst())) {
                key = head
            }
            guard let key else { continue }

            var modifiers: Set<String> = ["command"]
            if let range = inner.range(of: "modifiers:") {
                let rest = inner[range.upperBound...]
                modifiers = []
                for name in ["command", "option", "shift", "control"] where rest.contains(".\(name)") {
                    modifiers.insert(name)
                }
                if modifiers.isEmpty { modifiers = ["command"] }
            }
            out.append((Shortcut(key: key, modifiers: modifiers), call))
        }
        return out
    }

    private func swiftFiles() throws -> [URL] {
        try FileManager.default
            .contentsOfDirectory(at: sources, includingPropertiesForKeys: nil)
            .filter { $0.pathExtension == "swift" }
    }
}
