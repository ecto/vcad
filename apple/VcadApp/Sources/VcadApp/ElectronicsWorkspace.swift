import SwiftUI
import AppKit
import UniformTypeIdentifiers

typealias ECObject = [String: Any]
func ecRows(_ value: Any?) -> [ECObject] { value as? [ECObject] ?? [] }
func ecNumber(_ value: Any?, _ fallback: Double = 0) -> Double { (value as? NSNumber)?.doubleValue ?? fallback }
func ecPoint(_ value: Any?) -> CGPoint {
    let p = value as? ECObject ?? [:]
    return CGPoint(x: ecNumber(p["x"]), y: ecNumber(p["y"]))
}
func ecJSON(_ point: CGPoint) -> ECObject { ["x": point.x, "y": point.y] }

enum ElectronicsView: String, CaseIterable { case schematic = "Schematic", board = "Board", spatial = "3D" }
enum ElectronicsTool: String, CaseIterable { case select = "Select", place = "Place", wire = "Connect", route = "Route", via = "Via" }

@MainActor @Observable
final class ElectronicsWorkspace {
    var view: ElectronicsView = .board { didSet { routeStart = nil; pendingPin = nil; tool = .select } }
    var tool: ElectronicsTool = .select { didSet { routeStart = nil; pendingPin = nil } }
    var boardID: String?
    var selectedRef: String?
    var selectedTrace: Int?
    var selectedVia: Int?
    var componentKind = "Resistor"
    var activeLayer = "FCu"
    var activeNet = ""
    var traceWidth = 0.25
    var grid = 1.0
    var snap = true
    var zoom = 1.0
    var routeStart: CGPoint?
    var pendingPin: String?
    var message: String?
    var issues: [String] = []
    var checking = false
    let kinds = ["Resistor", "Capacitor", "Connector"]

    func boards(_ model: EditorModel) -> [String] {
        _ = model.undoStack.count; _ = model.redoStack.count; _ = model.documentDirty
        let nodes = model.documentJSON?["nodes"] as? ECObject ?? [:]
        return nodes.keys.filter { ((nodes[$0] as? ECObject)?["op"] as? ECObject)?["type"] as? String == "PcbBoard" }.sorted()
    }
    func board(_ model: EditorModel) -> ECObject {
        let ids = boards(model)
        let id = ids.contains(boardID ?? "") ? boardID : ids.first
        if let id, let node = (model.documentJSON?["nodes"] as? ECObject)?[id] as? ECObject,
           let op = node["op"] as? ECObject, let board = op["board"] as? ECObject { return board }
        return model.documentJSON?["pcb"] as? ECObject ?? [:]
    }
    func schematic(_ model: EditorModel) -> ECObject {
        _ = model.undoStack.count; _ = model.redoStack.count
        return model.documentJSON?["schematic"] as? ECObject ?? [:]
    }
    func mutateBoard(_ model: EditorModel, _ mutate: (inout ECObject) -> Void) {
        let ids = boards(model), chosen = ids.contains(boardID ?? "") ? boardID : ids.first
        model.editElectronics { doc in
            if let chosen {
                var nodes = doc["nodes"] as? ECObject ?? [:], node = nodes[chosen] as? ECObject ?? [:]
                var op = node["op"] as? ECObject ?? [:], b = op["board"] as? ECObject ?? [:]
                mutate(&b); op["board"] = b; node["op"] = op; nodes[chosen] = node; doc["nodes"] = nodes
            } else {
                var b = doc["pcb"] as? ECObject ?? [:]; mutate(&b); doc["pcb"] = b
            }
        }
        issues = []; message = nil
    }
    func createBoard(_ model: EditorModel) {
        guard board(model).isEmpty else { return }
        guard model.source.isSandbox || model.documentJSON != nil else { message = "Open an editable vcad document before adding electronics."; return }
        model.editElectronics { doc in
            var nodes = doc["nodes"] as? ECObject ?? [:]
            let id = String((nodes.keys.compactMap(Int.init).max() ?? -1) + 1)
            nodes[id] = ["id": Int(id)!, "name": "Circuit board", "op": ["type": "PcbBoard", "board": Self.emptyBoard]]
            doc["nodes"] = nodes
            var roots = ecRows(doc["roots"]); roots.append(["root": Int(id)!, "material": "default"]); doc["roots"] = roots
            if doc["schematic"] == nil { doc["schematic"] = ["components": [ECObject](), "wires": [ECObject](), "junctions": [ECObject](), "labels": [ECObject](), "nets": [String: [String]]()] }
            boardID = id
        }
    }
    static var emptyBoard: ECObject {
        ["outline": ["vertices": [["x": 0, "y": 0], ["x": 80, "y": 0], ["x": 80, "y": 50], ["x": 0, "y": 50]], "thickness": 1.6],
         "stackup": ["layers": [["layer": "FCu", "copperThickness": 0.035, "dielectricThickness": 1.53], ["layer": "BCu", "copperThickness": 0.035]]],
         "nets": [ECObject](), "footprints": [ECObject](), "traces": [ECObject](), "vias": [ECObject](), "zones": [ECObject](),
         "rules": ["defaultRules": ["name": "Default", "traceWidth": 0.25, "clearance": 0.2, "viaDiameter": 0.6, "viaDrill": 0.3], "edgeClearance": 0.5, "holeToHole": 0.25, "minAnnularRing": 0.15, "minDrill": 0.3]]
    }
    func snapped(_ p: CGPoint) -> CGPoint {
        guard snap, grid.isFinite, grid > 0 else { return p }
        return CGPoint(x: (p.x / grid).rounded() * grid, y: (p.y / grid).rounded() * grid)
    }
    func place(_ model: EditorModel, at point: CGPoint) {
        let prefix = componentKind == "Resistor" ? "R" : componentKind == "Capacitor" ? "C" : "J"
        let existing = ecRows(board(model)["footprints"]).compactMap { $0["ref"] as? String } + ecRows(schematic(model)["components"]).compactMap { $0["ref"] as? String }
        var index = 1; while existing.contains("\(prefix)\(index)") { index += 1 }
        let undoBefore = model.undoStack
        let before = model.documentJSON.flatMap { DocEdit.serialize($0) }
        let ref = "\(prefix)\(index)", point = snapped(point)
        let pads: [ECObject] = [-2.0, 2.0].enumerated().map { i, x in
            ["number": "\(i + 1)", "padType": "SMD", "shape": ["type": "Rect", "width": 1.5, "height": 2.0], "position": ["x": x, "y": 0], "layers": ["FCu"]]
        }
        let value = componentKind == "Resistor" ? "10k" : componentKind == "Capacitor" ? "100nF" : "2-pin"
        let fp: ECObject = ["ref": ref, "value": value, "footprintName": "vcad:Native_2Pad", "position": ecJSON(point), "rotation": 0, "front": true, "pads": pads, "graphics": [ECObject]()]
        mutateBoard(model) { b in var f = ecRows(b["footprints"]); f.append(fp); b["footprints"] = f }
        // Add schematic and board as one undoable placement.
        model.editElectronics { doc in
            var sch = doc["schematic"] as? ECObject ?? [:], components = ecRows(sch["components"])
            let pins: [ECObject] = [-2.0, 2.0].enumerated().map { i, x in ["number": "\(i + 1)", "name": "\(i + 1)", "pin_type": "Passive", "position": ["x": x, "y": 0]] }
            components.append(["ref": ref, "value": value, "footprintId": "vcad:Native_2Pad", "position": ecJSON(point), "pins": pins, "pads": pads])
            sch["components"] = components; sch["wires"] = sch["wires"] ?? [ECObject](); sch["junctions"] = sch["junctions"] ?? [ECObject](); sch["labels"] = sch["labels"] ?? [ECObject](); doc["schematic"] = sch
        }
        if let before { model.undoStack = Array((undoBefore + [before]).suffix(64)) }
        selectedRef = ref; tool = .select
    }
    func editComponent(_ model: EditorModel, key: String, value: Any) {
        guard let ref = selectedRef else { return }
        if key == "value" {
            let undoBefore = model.undoStack
            let before = model.documentJSON.flatMap { DocEdit.serialize($0) }
            mutateBoard(model) { b in
                var rows = ecRows(b["footprints"])
                if let i = rows.firstIndex(where: { $0["ref"] as? String == ref }) { rows[i][key] = value }
                b["footprints"] = rows
            }
            model.editElectronics { doc in
                var sch = doc["schematic"] as? ECObject ?? [:], rows = ecRows(sch["components"])
                if let i = rows.firstIndex(where: { $0["ref"] as? String == ref }) { rows[i][key] = value }
                sch["components"] = rows; doc["schematic"] = sch
            }
            if let before { model.undoStack = Array((undoBefore + [before]).suffix(64)) }
            return
        }
        if view == .schematic {
            model.editElectronics { doc in
                var sch = doc["schematic"] as? ECObject ?? [:], rows = ecRows(sch["components"])
                if let i = rows.firstIndex(where: { $0["ref"] as? String == ref }) { rows[i][key] = value }
                sch["components"] = rows; doc["schematic"] = sch
            }
        } else {
            mutateBoard(model) { b in var rows = ecRows(b["footprints"]); if let i = rows.firstIndex(where: { $0["ref"] as? String == ref }) { rows[i][key] = value }; b["footprints"] = rows }
        }
    }
    func addTrace(_ model: EditorModel, to point: CGPoint, snapToGrid: Bool = true) {
        guard !activeNet.isEmpty, ecRows(board(model)["nets"]).contains(where: { $0["id"] as? String == activeNet }), (0.1...5).contains(traceWidth) else { message = "Choose a net and a trace width between 0.1 and 5 mm."; return }
        let end = snapToGrid ? snapped(point) : point
        guard let start = routeStart else { routeStart = end; return }
        guard start != end else { return }
        mutateBoard(model) { b in var traces = ecRows(b["traces"]); traces.append(["start": ecJSON(start), "end": ecJSON(end), "width": traceWidth, "layer": activeLayer, "net": activeNet, "source": "manual"]); b["traces"] = traces }
        routeStart = end
    }
    func addVia(_ model: EditorModel, at p: CGPoint) {
        guard !activeNet.isEmpty else { message = "Choose a net before placing a via."; return }
        mutateBoard(model) { b in var vias = ecRows(b["vias"]); vias.append(["position": ecJSON(snapped(p)), "diameter": 0.6, "drill": 0.3, "startLayer": "FCu", "endLayer": "BCu", "net": activeNet, "source": "manual"]); b["vias"] = vias }
    }
    func connect(_ model: EditorModel, pin: String) {
        guard let start = pendingPin else { pendingPin = pin; return }
        guard start != pin else { return }
        let undoBefore = model.undoStack
        let before = model.documentJSON.flatMap { DocEdit.serialize($0) }
        var nets = schematic(model)["nets"] as? [String: [String]] ?? [:]
        let matching = nets.keys.filter { nets[$0]!.contains(start) || nets[$0]!.contains(pin) }
        var next = 1
        while nets["N\(next)"] != nil { next += 1 }
        let name = matching.sorted().first ?? "N\(next)"
        var refs = Set([start, pin]); for key in matching { refs.formUnion(nets.removeValue(forKey: key) ?? []) }; nets[name] = refs.sorted()
        model.editElectronics { doc in var sch = doc["schematic"] as? ECObject ?? [:]; sch["nets"] = nets; doc["schematic"] = sch }
        // Net assignment updates are committed with the schematic by syncBoardNets.
        syncBoardNets(model, nets: nets, merged: matching, target: name)
        if let before { model.undoStack = Array((undoBefore + [before]).suffix(64)) }
        pendingPin = nil; activeNet = name
    }
    private func syncBoardNets(_ model: EditorModel, nets: [String: [String]], merged: [String], target: String) {
        mutateBoard(model) { b in
            var rows = ecRows(b["nets"]).filter { !merged.contains($0["id"] as? String ?? "") && $0["id"] as? String != target }
            rows.append(["id": target, "name": target]); b["nets"] = rows
            var footprints = ecRows(b["footprints"])
            for i in footprints.indices {
                var pads = ecRows(footprints[i]["pads"])
                for j in pads.indices {
                    let ref = "\(footprints[i]["ref"] as? String ?? "").\(pads[j]["number"] as? String ?? "")"
                    if let net = nets.first(where: { $0.value.contains(ref) }) { pads[j]["net"] = net.key }
                }
                footprints[i]["pads"] = pads
            }
            b["footprints"] = footprints
            for key in ["traces", "vias", "traceArcs", "zones"] {
                if b[key] != nil { b[key] = ecRows(b[key]).map { row in var row = row; if merged.contains(row["net"] as? String ?? "") { row["net"] = target }; return row } }
            }
        }
    }
    func removeSelection(_ model: EditorModel) {
        if let i = selectedTrace {
            mutateBoard(model) { b in var rows = ecRows(b["traces"]); if rows.indices.contains(i) { rows.remove(at: i) }; b["traces"] = rows }; selectedTrace = nil
        } else if let i = selectedVia {
            mutateBoard(model) { b in var rows = ecRows(b["vias"]); if rows.indices.contains(i) { rows.remove(at: i) }; b["vias"] = rows }; selectedVia = nil
        }
    }
    func check(_ model: EditorModel) {
        issues = []
        let b = board(model), footprints = ecRows(b["footprints"]), nets = ecRows(b["nets"]).compactMap { $0["id"] as? String }
        var seen = Set<String>()
        for f in footprints {
            let ref = f["ref"] as? String ?? "?"
            if !seen.insert(ref).inserted { issues.append("Duplicate reference \(ref)") }
            for pad in ecRows(f["pads"]) where (pad["net"] as? String ?? "").isEmpty { issues.append("\(ref).\(pad["number"] as? String ?? "?") has no net") }
        }
        for (i, trace) in ecRows(b["traces"]).enumerated() {
            if !nets.contains(trace["net"] as? String ?? "") { issues.append("Trace \(i + 1) references an unknown net") }
            if ecNumber(trace["width"]) < 0.1 { issues.append("Trace \(i + 1) is narrower than 0.1 mm") }
        }
        checking = true
    }
}
