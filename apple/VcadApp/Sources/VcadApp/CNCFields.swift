import Foundation

// Every number in the Manufacture workspace, by name.
//
// Friction-log item 51: *"Number fields do not accept accessibility
// value-setting; typing only works with the window frontmost, and the sidebar
// summary is the only confirmation that a value took."*
//
// A `TextField` carries its identifier and its value into the accessibility
// tree, but nothing *outside* the view layer could read or write one — so a
// script, a test or an assistive tool had no way to set a number and confirm
// it landed. This is that way: one name per field, and a getter and setter
// that go through the same model the field's binding does.
//
// It is also the drift guard. `CNCFieldTests` walks this table, sets every
// field through its identifier and reads it back — so a field that is added
// to a panel without a path through here is a test failure, not a discovery
// made on a machine.

/// One named number.
struct CNCField: Identifiable, Sendable {
    /// The accessibility identifier the field carries, e.g. `cnc.stock.width`.
    let id: String
    /// What the panel labels it.
    let label: String
    let unit: String
    /// A value this field will certainly accept, for a round-trip check.
    let sample: Double
}

extension CNCWorkspace {
    /// Every settable number, in the order the panels show them.
    ///
    /// Operation fields address *the selected operation*, which is what the
    /// inspector edits: there is one inspector, and it follows the selection.
    static let fields: [CNCField] = [
        .init(id: "cnc.stock.width", label: "Width · X", unit: "mm", sample: 63.5),
        .init(id: "cnc.stock.height", label: "Length · Y", unit: "mm", sample: 48.25),
        .init(id: "cnc.stock.thickness", label: "Thickness · Z", unit: "mm", sample: 6),
        .init(id: "cnc.stock.margin", label: "Margin round the part", unit: "mm", sample: 9),
        .init(id: "cnc.stock.spoilboard", label: "Spoilboard thickness", unit: "mm", sample: 3),
        .init(id: "cnc.origin.x", label: "CAD X", unit: "mm", sample: -12),
        .init(id: "cnc.origin.y", label: "CAD Y", unit: "mm", sample: 4.5),
        .init(id: "cnc.origin.z", label: "CAD Z", unit: "mm", sample: 17.1),
        .init(id: "cnc.placement.dx", label: "Shift X", unit: "mm", sample: 2.5),
        .init(id: "cnc.placement.dy", label: "Shift Y", unit: "mm", sample: -1.5),
        .init(id: "cnc.placement.rotation", label: "Rotation", unit: "°", sample: 10),
        .init(id: "cnc.tool.diameter", label: "Diameter", unit: "mm", sample: 2),
        .init(id: "cnc.tool.flutes", label: "Flutes", unit: "", sample: 3),
        .init(id: "cnc.tool.fluteLength", label: "Flute length", unit: "mm", sample: 6),
        .init(id: "cnc.tool.stickout", label: "Stickout", unit: "mm", sample: 18),
        .init(id: "cnc.op.depth", label: "Cut depth", unit: "mm", sample: 1),
        .init(id: "cnc.op.stepdown", label: "Roughing stepdown", unit: "mm", sample: 0.17),
        .init(id: "cnc.op.stepover", label: "Stepover", unit: "mm", sample: 0.9),
        .init(id: "cnc.op.clearance", label: "Clearance", unit: "mm", sample: 5),
        .init(id: "cnc.op.feed", label: "Cutting feed", unit: "mm/min", sample: 250),
        .init(id: "cnc.op.plunge", label: "Plunge feed", unit: "mm/min", sample: 40),
        .init(id: "cnc.op.rpm", label: "Spindle", unit: "rpm", sample: 13500),
        .init(id: "cnc.op.stockToLeave", label: "Stock to leave", unit: "mm", sample: 0.2),
        .init(id: "cnc.op.finishFeed", label: "Finishing feed", unit: "mm/min", sample: 180),
        .init(id: "cnc.op.rampAngle", label: "Ramp angle", unit: "°", sample: 2.5),
        .init(id: "cnc.op.bottomAllowance", label: "Leave at the bottom", unit: "mm", sample: 0.15),
        .init(id: "cnc.tabs.count", label: "Tabs", unit: "", sample: 4),
        .init(id: "cnc.tabs.width", label: "Metal left", unit: "mm", sample: 4),
        .init(id: "cnc.tabs.height", label: "Tab height", unit: "mm", sample: 0.42),
        .init(id: "cnc.op.thinSlotTolerance", label: "Allowed wall error", unit: "mm", sample: 0.05),
        .init(id: "cnc.machine.jogStep", label: "Jog step", unit: "mm", sample: 10),
        .init(id: "cnc.machine.jogFeed", label: "Jog feed", unit: "mm/min", sample: 600),
    ]

    /// What a named field reads, or `nil` when that is not a field.
    func fieldValue(_ id: String) -> Double? {
        switch id {
        case "cnc.stock.width": return stockWidth
        case "cnc.stock.height": return stockHeight
        case "cnc.stock.thickness": return stockThickness
        case "cnc.stock.margin": return effectiveMargin
        case "cnc.stock.spoilboard": return underStock.thickness
        case "cnc.origin.x": return origin.x
        case "cnc.origin.y": return origin.y
        case "cnc.origin.z": return origin.z
        case "cnc.placement.dx": return placement.dx
        case "cnc.placement.dy": return placement.dy
        case "cnc.placement.rotation": return placement.rotationDeg
        case "cnc.tool.diameter": return toolDiameter
        case "cnc.tool.flutes": return Double(toolFlutes)
        case "cnc.tool.fluteLength": return toolFluteLength
        case "cnc.tool.stickout": return toolStickout
        case "cnc.op.depth": return setup.depth
        case "cnc.op.stepdown": return setup.stepdown
        case "cnc.op.stepover": return setup.stepover
        case "cnc.op.clearance": return setup.clearance
        case "cnc.op.feed": return setup.feed
        case "cnc.op.plunge": return setup.plunge
        case "cnc.op.rpm": return setup.rpm
        case "cnc.op.stockToLeave": return setup.stockToLeave
        case "cnc.op.finishFeed": return setup.finishFeed
        case "cnc.op.rampAngle": return setup.rampAngle
        case "cnc.op.bottomAllowance": return setup.bottomAllowance
        case "cnc.tabs.count": return Double(setup.tabs)
        case "cnc.tabs.width": return setup.tabWidth
        case "cnc.tabs.height": return setup.tabHeight
        case "cnc.op.thinSlotTolerance": return setup.thinSlotTolerance
        case "cnc.machine.jogStep": return jogStep
        case "cnc.machine.jogFeed": return jogFeed
        default: return nil
        }
    }

    /// Set a named field. Returns false for a name that is not a field, or a
    /// value that is not a number — never silently rounds or clamps, because
    /// a setter that quietly changes what it was given is how a 0.15 mm skin
    /// becomes something else.
    @discardableResult
    func setFieldValue(_ id: String, to value: Double) -> Bool {
        guard value.isFinite, fieldValue(id) != nil else { return false }
        switch id {
        case "cnc.stock.width": stockWidth = value
        case "cnc.stock.height": stockHeight = value
        case "cnc.stock.thickness": stockThickness = value
        case "cnc.stock.margin": stockMargin = value
        case "cnc.stock.spoilboard": underStock = .spoilboard(value)
        case "cnc.origin.x": origin.x = value
        case "cnc.origin.y": origin.y = value
        case "cnc.origin.z": origin.z = value
        case "cnc.placement.dx": placement.dx = value
        case "cnc.placement.dy": placement.dy = value
        case "cnc.placement.rotation": placement.rotationDeg = value
        case "cnc.tool.diameter": toolDiameter = value
        // The stepper's range, honoured here so the two paths to the same
        // number cannot disagree about what it may be.
        case "cnc.tool.flutes": toolFlutes = Int(max(1, min(6, value.rounded())))
        case "cnc.tool.fluteLength": toolFluteLength = value
        case "cnc.tool.stickout": toolStickout = value
        case "cnc.op.depth": setup.depth = value
        case "cnc.op.stepdown": setup.stepdown = value
        case "cnc.op.stepover": setup.stepover = value
        case "cnc.op.clearance": setup.clearance = value
        case "cnc.op.feed": setup.feed = value
        case "cnc.op.plunge": setup.plunge = value
        case "cnc.op.rpm": setup.rpm = value
        case "cnc.op.stockToLeave": setup.stockToLeave = value
        case "cnc.op.finishFeed": setup.finishFeed = value
        case "cnc.op.rampAngle": setup.rampAngle = value
        case "cnc.op.bottomAllowance": setup.bottomAllowance = value
        // Through `setTabCount`, not the raw field: adding a tab has to decide
        // where it goes, and the stepper beside it goes the same way.
        case "cnc.tabs.count": setTabCount(Int(max(0, min(12, value.rounded()))))
        case "cnc.tabs.width": setup.tabWidth = value
        case "cnc.tabs.height": setup.tabHeight = value
        case "cnc.op.thinSlotTolerance": setup.thinSlotTolerance = value
        case "cnc.machine.jogStep": jogStep = value
        case "cnc.machine.jogFeed": jogFeed = value
        default: return false
        }
        return true
    }
}
