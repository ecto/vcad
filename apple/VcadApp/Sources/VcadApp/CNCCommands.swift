import Foundation

/// Wire commands are kept independent of the UI so framing and units can be tested.
enum CNCCommands {
    static let workspaces = ["G54", "G55", "G56", "G57", "G58", "G59"]
    static func validManual(_ line: String) -> Bool {
        let value = line.trimmingCharacters(in: .whitespaces)
        guard !value.isEmpty, value.utf8.count <= 79,
              value.utf8.allSatisfy({ $0 >= 32 && $0 < 127 }),
              !value.contains(where: { "?!~".contains($0) }) else { return false }
        return !value.hasPrefix("$") || ["$$", "$G", "$I", "$#"].contains(value.uppercased())
    }
    /// Strip parenthesised comments, counting depth.
    ///
    /// `\([^)]*\)` stopped at the first `)`, which is right by RS-274 — where
    /// comments do not nest — and wrong for what vcad's own post writes:
    /// `(T1 Ø3.175 flat end mill (Ø3.17) at 10000 rpm)` left ` at 10000 rpm)`
    /// behind, and the importer rejected the app's own job as unrecognised
    /// G-code. Counting depth reads that line and is identical on every line
    /// that does not nest.
    static func stripComments(_ line: String) -> String {
        var out = ""
        var depth = 0
        for character in line {
            if character == "(" { depth += 1; continue }
            if character == ")" { depth = max(0, depth - 1); continue }
            if depth == 0 { out.append(character) }
        }
        return out
    }

    static func programLines(_ code: String) throws -> [String] {
        let lines = code.components(separatedBy: .newlines).map {
            stripComments($0).components(separatedBy: ";")[0].trimmingCharacters(in: .whitespaces)
        }.filter { !$0.isEmpty && $0 != "%" }
        guard !lines.isEmpty else { throw CNCError.message("The program contains no commands.") }
        for (index, line) in lines.enumerated() {
            guard validManual(line), !line.hasPrefix("$") else { throw CNCError.message("Invalid G-code at line \(index + 1): realtime characters, system commands and lines over 79 bytes are not allowed.") }
            if index < lines.count - 1 && line.range(of: #"(?i)M0*(2|30)(?![\d.])"#, options: .regularExpression) != nil {
                throw CNCError.message("Program end before the last line. Load a single program.")
            }
        }
        return lines
    }
    static func zero(axes: String, workspace: String) -> String? {
        guard !axes.isEmpty, axes.allSatisfy({ "XYZ".contains($0) }), Set(axes).count == axes.count,
              let index = workspaces.firstIndex(of: workspace) else { return nil }
        return "G10 L20 P\(index + 1) " + axes.map { "\($0)0" }.joined(separator: " ")
    }
    static func jog(x: Double, y: Double, z: Double, feed: Double) -> String? {
        guard [x, y, z, feed].allSatisfy(\.isFinite), [x, y, z].allSatisfy({ abs($0) <= 10 }),
              x != 0 || y != 0 || z != 0, feed > 0, feed <= 3000 else { return nil }
        return String(format: "$J=G91 G21 X%.3f Y%.3f Z%.3f F%.1f", locale: Locale(identifier: "en_US_POSIX"), x, y, z, feed)
    }
    static func overrideByte(spindle: Bool, current: Int, target: Int) -> UInt8? {
        guard (10...200).contains(target), current != target else { return nil }
        if target == 100 { return spindle ? 0x99 : 0x90 }
        let delta = target - current
        if abs(delta) >= 10 { return spindle ? (delta > 0 ? 0x9a : 0x9b) : (delta > 0 ? 0x91 : 0x92) }
        return spindle ? (delta > 0 ? 0x9c : 0x9d) : (delta > 0 ? 0x93 : 0x94)
    }
}
