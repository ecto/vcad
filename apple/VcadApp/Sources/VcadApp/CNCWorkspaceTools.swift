import Foundation

enum CNCInspectorTab: String, CaseIterable, Identifiable {
    case inspector = "Inspector", terminal = "Terminal", gcode = "G-code", macros = "Macros"
    var id: String { rawValue }
}
struct CNCMacro: Identifiable, Codable {
    var id = UUID()
    var name: String
    var command: String
}
