import Foundation

/// What the drawer above the machine bar shows.
enum CNCInspectorTab: String, CaseIterable, Identifiable {
    case terminal = "Terminal", gcode = "G-code", macros = "Macros"
    var id: String { rawValue }
    var symbol: String {
        switch self {
        case .terminal: return "terminal"
        case .gcode: return "doc.text"
        case .macros: return "command"
        }
    }
}
struct CNCMacro: Identifiable, Codable {
    var id = UUID()
    var name: String
    var command: String
}
