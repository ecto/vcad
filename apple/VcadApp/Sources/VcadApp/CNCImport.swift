import Foundation

/// Import deliberately accepts the GRBL subset we can also preview accurately.
/// Canned cycles and tool-length compensation need controller offsets we do
/// not have in a static file, and are rejected.
///
/// The subset was drawn too tightly to admit **vcad's own output**: the job
/// assembler emits `M0` between tools (this machine has no changer, so a tool
/// change is an operator stop and a re-probe) and `T1 M6` when one is
/// configured, and neither was on the accepted M list — so a program the app
/// had just exported could not be imported back into it. `G4 P…`, the spin-up
/// dwell that covers a relay-switched router, was accepted; `G55`–`G59` were
/// not, though only `G54` was ever emitted. `CNCImportTests` closes the loop
/// by exporting a job and importing it again.
enum CNCImport {
    /// Work coordinate systems. Every one of them previews identically: the
    /// program is read in whichever work frame it selects, and the offset
    /// between them lives on the controller, not in the file.
    private static let workOffsets: Set<Double> = [54, 55, 56, 57, 58, 59]
    /// Modal words that change nothing this preview models: plane selection
    /// (XY, which is the only plane previewable anyway), feed-per-minute,
    /// compensation cancels, canned-cycle cancel, and path blending.
    private static let inert: Set<Double> = [17, 21, 40, 49, 61, 64, 80, 94]
    /// M words that are safe to replay: program stops and ends, spindle and
    /// coolant. `M0`/`M1` are *pauses*, not ends —
    /// `CNCCommands.programLines` is what refuses an `M2`/`M30` before the
    /// last line.
    ///
    /// `M6` is deliberately not here. This machine has no changer, so the
    /// controller would ignore an `M6` silently and carry on cutting with
    /// whatever is in the collet — which is why the job assembler emits `M0`
    /// and a re-probe instead. Refusing `M6` on the way in keeps the import
    /// honest about the same thing.
    private static let mCodes: Set<Double> = [0, 1, 2, 3, 4, 5, 7, 8, 9, 30]

    static func parse(_ source: String) throws -> CNCProgram {
        let lines = try CNCCommands.programLines(source)
        var p = [0.0, 0, 0], absolute = true, scale = 1.0, motion = 0, feed = 400.0
        var moves: [CNCMove] = []
        let regex = try NSRegularExpression(pattern: #"([A-Z])\s*([-+]?(?:\d+\.?\d*|\.\d+))"#)
        for (index, line) in lines.enumerated() {
            let text = line.uppercased()
            let matches = regex.matches(in: text, range: NSRange(text.startIndex..., in: text))
            var words: [String: Double] = [:]
            var dwell = false
            var machineCoordinates = false
            for match in matches {
                let letter = String(text[Range(match.range(at: 1), in: text)!])
                let number = Double(text[Range(match.range(at: 2), in: text)!])!
                guard number.isFinite, abs(number) < 1_000_000 else { throw CNCError.message("Invalid number on line \(index + 1).") }
                if letter == "G" {
                    switch number {
                    case 0, 1, 2, 3: motion = Int(number)
                    // A dwell — the spin-up that covers a relay-switched
                    // router — takes time but no distance, so it is read and
                    // then skipped rather than becoming a zero-length move.
                    case 4: dwell = true
                    case 53: machineCoordinates = true
                    case 20: scale = 25.4
                    case 21: scale = 1
                    case 90: absolute = true
                    case 91: absolute = false
                    case let g where Self.inert.contains(g): break
                    case let g where Self.workOffsets.contains(g): break
                    default:
                        throw CNCError.message("G\(number.formatted()) on line \(index + 1) cannot be previewed. Import supports G0–G3, G4 dwells, G17, G20/21, G40, G49, G53–G59, G61/G64, G80, G90/91 and G94.")
                    }
                } else {
                    guard ["X", "Y", "Z", "I", "J", "R", "F", "S", "M", "N", "T", "P"].contains(letter) else {
                        throw CNCError.message("Unsupported \(letter) word on line \(index + 1).")
                    }
                    if letter == "T", number > 1 { throw CNCError.message("Import requires one manually installed tool (T0 or T1).") }
                    if letter == "M", !Self.mCodes.contains(number) { throw CNCError.message("Unsupported M code on line \(index + 1).") }
                    words[letter] = number
                }
            }
            let residual = regex.stringByReplacingMatches(in: text, range: NSRange(text.startIndex..., in: text), withTemplate: "").trimmingCharacters(in: .whitespaces)
            guard residual.isEmpty else { throw CNCError.message("Unrecognized G-code on line \(index + 1).") }
            if let value = words["F"] { guard value > 0 else { throw CNCError.message("Feed must be positive.") }; feed = value * scale }
            if dwell { continue }
            guard ["X", "Y", "Z", "I", "J", "R"].contains(where: { words[$0] != nil }) else { continue }
            // `G53` on its own is inert; `G53` *with* motion asks for a move in
            // machine coordinates, and where that lands depends on the work
            // offset the controller holds and this file does not. Refusing is
            // the only honest answer — a previewed position that is silently
            // one work offset out is worse than no preview.
            guard !machineCoordinates else {
                throw CNCError.message("G53 with motion on line \(index + 1) moves in machine coordinates. Where that lands depends on the controller's work offset, which is not in this file, so it cannot be previewed or verified.")
            }
            var end = p
            for (i, axis) in ["X", "Y", "Z"].enumerated() { if let v = words[axis] { end[i] = v * scale + (absolute ? 0 : p[i]) } }
            if moves.isEmpty { moves.append(CNCMove(to: p, rapid: true)) }
            if motion < 2 {
                moves.append(CNCMove(to: end, rapid: motion == 0, feed: feed))
            } else {
                var cx = p[0] + (words["I"] ?? 0) * scale, cy = p[1] + (words["J"] ?? 0) * scale
                if let r = words["R"] {
                    let radius = abs(r * scale), dx = end[0] - p[0], dy = end[1] - p[1], chord = hypot(dx, dy)
                    guard chord > 0, radius >= chord / 2 else { throw CNCError.message("Invalid radius arc on line \(index + 1).") }
                    let h = sqrt(max(0, radius * radius - chord * chord / 4)) * (motion == 2 ? -1.0 : 1.0) * (r < 0 ? -1.0 : 1.0)
                    cx = (p[0] + end[0]) / 2 - dy / chord * h; cy = (p[1] + end[1]) / 2 + dx / chord * h
                } else if words["I"] == nil && words["J"] == nil { throw CNCError.message("Arc needs I/J or R on line \(index + 1).") }
                let radius = hypot(p[0] - cx, p[1] - cy)
                guard radius > 0, abs(hypot(end[0] - cx, end[1] - cy) - radius) < max(0.005, radius * 0.001) else { throw CNCError.message("Arc endpoints do not share a radius on line \(index + 1).") }
                let start = atan2(p[1] - cy, p[0] - cx)
                var sweep = atan2(end[1] - cy, end[0] - cx) - start
                if motion == 2 { if sweep >= 0 { sweep -= 2 * .pi } } else { if sweep <= 0 { sweep += 2 * .pi } }
                let count = max(8, min(4096, Int(ceil(abs(sweep) / 0.035))))
                for i in 1...count {
                    let t = Double(i) / Double(count), angle = start + sweep * t
                    moves.append(CNCMove(to: [cx + radius * cos(angle), cy + radius * sin(angle), p[2] + (end[2] - p[2]) * t], rapid: false, feed: feed))
                }
            }
            p = end
            guard moves.count <= 250_000 else { throw CNCError.message("Preview exceeds 250,000 segments.") }
        }
        guard !moves.isEmpty else { throw CNCError.message("No previewable motion in this file.") }
        // Establish the same initial modal state used by the preview.
        return CNCProgram(gcode: "G21 G90 G17 G54 G94 F400\n" + lines.joined(separator: "\n"), moves: moves)
    }
}
