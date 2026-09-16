import SwiftUI
import AppKit
import UniformTypeIdentifiers

struct NativeElectronicsView: View {
    @Bindable var model: EditorModel
    private var ec: ElectronicsWorkspace { model.electronics }
    @State private var fileError: String?
    @State private var search = ""
    @State private var navigatorTab = "Parts"
    var body: some View {
        @Bindable var ec = ec
        VStack(spacing: 0) {
            commands.padding(.horizontal, 16).padding(.vertical, 8).background(.bar)
            Divider()
            if ec.board(model).isEmpty {
                ContentUnavailableView {
                    Label("Electronics", systemImage: "cpu")
                } description: {
                    Text(ec.message ?? "Create a circuit board in this document, or open a vcad document containing a PCB.")
                } actions: {
                    Button("Create circuit board") { ec.createBoard(model) }.buttonStyle(.borderedProminent)
                    Button("Open document…") { open() }
                }
            } else {
                GeometryReader { geometry in
                    ZStack(alignment: .top) {
                        Group {
                            if ec.view == .spatial { ElectronicsBoardPreview(board: ec.board(model)) }
                            else { ElectronicsCanvas(model: model) }
                        }
                        .padding(.horizontal, 270)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                        HStack(alignment: .top, spacing: 16) {
                            navigator.padding(16).frame(width: 236)
                                .frame(maxHeight: min(470, geometry.size.height - 32))
                                .cncFloatingPanel()
                            Spacer(minLength: 0)
                            inspector.padding(16).frame(width: 268)
                                .frame(maxHeight: min(600, geometry.size.height - 32))
                                .cncFloatingPanel()
                        }.padding(16)
                    }
                }
                Divider()
                HStack {
                    Text(ec.message ?? (ec.view == .spatial ? "3D layout preview · simplified component bodies" : ec.tool == .wire ? "Select two pins to connect their net" : ec.tool == .route ? "Click to route · Escape ends the route" : ec.tool == .place ? "Click to place \(ec.componentKind.lowercased())" : "Select a component to edit its properties"))
                        .lineLimit(1)
                    Spacer()
                    Text("mm").foregroundStyle(.secondary)
                }.font(.caption).padding(10)
            }
        }
        .background(Color(nsColor: .windowBackgroundColor))
        .onChange(of: ec.boardID) { _, _ in ec.selectedRef = nil; ec.selectedTrace = nil; ec.selectedVia = nil; ec.routeStart = nil }
        .onChange(of: model.source) { _, _ in ec.selectedRef = nil; ec.selectedTrace = nil; ec.routeStart = nil; ec.pendingPin = nil }
        .sheet(isPresented: $ec.checking) {
            VStack(alignment: .leading, spacing: 14) {
                Text("Connectivity checks").font(.title2)
                Text("Checks duplicate references, missing net assignments and trace metadata. This is not a clearance DRC or electrical simulation.").font(.callout).foregroundStyle(.secondary)
                List(ec.issues.isEmpty ? ["No issues found by these checks."] : ec.issues, id: \.self) { Text($0) }.frame(height: 240)
                Button("Done") { ec.checking = false }.keyboardShortcut(.defaultAction)
            }.padding(24).frame(width: 500)
        }
        .alert("Document error", isPresented: Binding(get: { fileError != nil }, set: { if !$0 { fileError = nil } })) { Button("OK") { fileError = nil } } message: { Text(fileError ?? "") }
        .background {
            Button("") { ec.routeStart = nil; ec.pendingPin = nil; ec.tool = .select }.keyboardShortcut(.cancelAction).frame(width: 0, height: 0).opacity(0).accessibilityHidden(true)
        }
    }
    private var commands: some View {
        @Bindable var ec = ec
        return HStack(spacing: 12) {
            Picker("Electronics view", selection: $ec.view) { ForEach(ElectronicsView.allCases, id: \.self) { Text($0.rawValue).tag($0) } }.pickerStyle(.segmented).frame(width: 235)
            if ec.view != .spatial {
                Picker("Tool", selection: $ec.tool) {
                    Text("Select").tag(ElectronicsTool.select)
                    Text("Place").tag(ElectronicsTool.place)
                    if ec.view == .schematic { Text("Connect pins").tag(ElectronicsTool.wire) }
                    else { Text("Route").tag(ElectronicsTool.route); Text("Via").tag(ElectronicsTool.via) }
                }.pickerStyle(.segmented).labelsHidden().frame(width: 245)
            }
            Spacer(minLength: 0)
            Button("Undo", systemImage: "arrow.uturn.backward") { model.undo() }.labelStyle(.iconOnly).disabled(!model.canUndo)
            Button("Redo", systemImage: "arrow.uturn.forward") { model.redo() }.labelStyle(.iconOnly).disabled(!model.canRedo)
            Button("Check") { ec.check(model) }.disabled(ec.board(model).isEmpty)
            Menu("Document") {
                Button("Open…") { open() }
                Button("Save…") { save() }.disabled(model.documentJSON == nil)
            }.fixedSize()
        }.controlSize(.small)
    }
    private var navigator: some View {
        @Bindable var ec = ec
        return VStack(alignment: .leading, spacing: 12) {
            Label("Circuit", systemImage: "cpu").font(.headline)
            TextField("Search parts or nets", text: $search).textFieldStyle(.roundedBorder)
            Picker("Navigator", selection: $navigatorTab) {
                ForEach(["Parts", "Nets", "Layers"], id: \.self) { Text($0) }
            }.pickerStyle(.segmented).labelsHidden()
            Divider()
            ScrollView {
                VStack(alignment: .leading, spacing: 4) {
                    if navigatorTab == "Parts" {
                        ForEach(ecRows(ec.board(model)["footprints"]).indices, id: \.self) { i in
                            let row = ecRows(ec.board(model)["footprints"])[i]
                            let ref = row["ref"] as? String ?? "?"
                            let value = row["value"] as? String ?? ""
                            if search.isEmpty || (ref + value).localizedCaseInsensitiveContains(search) {
                                Button { ec.selectedRef = ref; ec.selectedTrace = nil; ec.selectedVia = nil; ec.tool = .select } label: {
                                    HStack { Image(systemName: "cpu"); Text(ref); Spacer(); Text(value).foregroundStyle(.secondary) }
                                        .padding(8).background(ec.selectedRef == ref ? Color.accentColor.opacity(0.25) : .clear, in: RoundedRectangle(cornerRadius: 7))
                                }.buttonStyle(.plain)
                            }
                        }
                    } else if navigatorTab == "Nets" {
                        ForEach(ecRows(ec.board(model)["nets"]), id: \.ecID) { row in
                            let name = row["name"] as? String ?? row.ecID
                            if search.isEmpty || name.localizedCaseInsensitiveContains(search) {
                                Button { ec.activeNet = row.ecID; ec.routeStart = nil } label: {
                                    HStack { Image(systemName: "point.3.connected.trianglepath.dotted"); Text(name); Spacer() }
                                        .padding(8).background(ec.activeNet == row.ecID ? Color.accentColor.opacity(0.25) : .clear, in: RoundedRectangle(cornerRadius: 7))
                                }.buttonStyle(.plain)
                            }
                        }
                        if ecRows(ec.board(model)["nets"]).isEmpty { Text("Connect schematic pins to create a net.").foregroundStyle(.secondary) }
                    } else {
                        Picker("Active copper", selection: $ec.activeLayer) {
                            Text("Front copper").tag("FCu"); Text("Back copper").tag("BCu")
                        }.pickerStyle(.radioGroup)
                    }
                }.font(.callout)
            }
            Divider()
            Button(ec.view == .schematic ? "Show in Board" : "Show in Schematic", systemImage: "arrow.left.arrow.right") {
                ec.view = ec.view == .schematic ? .board : .schematic
            }
            Text("\(ecRows(ec.board(model)["footprints"]).count) components · \(ecRows(ec.board(model)["traces"]).count) traces")
                .font(.caption).foregroundStyle(.secondary)
        }.controlSize(.small)
    }
    private var inspector: some View {
        @Bindable var ec = ec
        return ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                Text(ec.tool == .route ? "Route" : ec.tool == .place ? "Place component" : ec.selectedRef ?? "Circuit board").font(.headline)
                if ec.tool == .select, let ref = ec.selectedRef,
                   let component = ecRows((ec.view == .schematic ? ec.schematic(model)["components"] : ec.board(model)["footprints"])).first(where: { $0["ref"] as? String == ref }) {
                    HStack {
                        Text("Value")
                        TextField("Component value", text: Binding(get: { component["value"] as? String ?? "" }, set: { ec.editComponent(model, key: "value", value: $0) })).textFieldStyle(.roundedBorder)
                    }
                    HStack {
                        Text("Rotation")
                        TextField("Rotation", value: Binding(get: { ecNumber(component["rotation"]) }, set: { v in if v.isFinite { ec.editComponent(model, key: "rotation", value: v.truncatingRemainder(dividingBy: 360)) } }), format: .number).textFieldStyle(.roundedBorder)
                        Text("°")
                    }
                    Text(component["footprintName"] as? String ?? component["footprintId"] as? String ?? "Component").font(.caption).foregroundStyle(.secondary)
                    ForEach(["x", "y"], id: \.self) { axis in
                        HStack {
                            Text(axis.uppercased())
                            TextField(axis.uppercased(), value: Binding(get: { ecNumber((component["position"] as? ECObject)?[axis]) }, set: { v in
                                guard v.isFinite, abs(v) < 100_000 else { return }
                                var pos = component["position"] as? ECObject ?? [:]; pos[axis] = v; ec.editComponent(model, key: "position", value: pos)
                            }), format: .number).textFieldStyle(.roundedBorder)
                            Text("mm").foregroundStyle(.secondary)
                        }
                    }
                    Text("Move coordinates independently in Schematic and Board views.").font(.caption).foregroundStyle(.secondary)
                    Divider()
                }
                if ec.tool == .place {
                Picker("Component", selection: $ec.componentKind) { ForEach(ec.kinds, id: \.self) { Text($0) } }
                Text("Native starter parts use explicit two-pad SMD geometry; verify the footprint against your part before fabrication.").font(.caption).foregroundStyle(.secondary)
                }
                if ec.view == .board && (ec.tool == .route || ec.tool == .via) {
                Picker("Net", selection: $ec.activeNet) {
                    Text("Choose net").tag("")
                    ForEach(ecRows(ec.board(model)["nets"]), id: \.ecID) { row in Text(row["name"] as? String ?? row.ecID).tag(row.ecID) }
                }
                Picker("Copper layer", selection: $ec.activeLayer) { Text("Front copper").tag("FCu"); Text("Back copper").tag("BCu") }
                HStack { Text("Trace width"); TextField("Trace width", value: $ec.traceWidth, format: .number).textFieldStyle(.roundedBorder); Text("mm") }
                Text(ec.routeStart == nil ? "Choose a pad to begin routing." : "Preview · click to commit").font(.caption).foregroundStyle(.secondary)
                Divider()
                Label("Linked schematic", systemImage: "point.3.connected.trianglepath.dotted").font(.headline)
                let pins = (ec.schematic(model)["nets"] as? [String: [String]])?[ec.activeNet] ?? []
                ElectronicsNetPreview(pins: pins, net: ec.activeNet).frame(height: 120)
                Button("Show in Schematic") { ec.view = .schematic }
                }
                Divider()
                Picker("Grid", selection: $ec.grid) { Text("0.25 mm").tag(0.25); Text("0.5 mm").tag(0.5); Text("1 mm").tag(1.0); Text("2.54 mm").tag(2.54) }
                Toggle("Snap to grid", isOn: $ec.snap)
                HStack { Text("Zoom"); Slider(value: $ec.zoom, in: 0.5...3) }
                Button("Delete selected copper", role: .destructive) { ec.removeSelection(model) }.disabled(ec.selectedTrace == nil && ec.selectedVia == nil)
                DisclosureGroup("Editor capabilities") {
                    Text("Imported zones, arcs and custom graphics are preserved but not displayed. Routing previews do not perform clearance checks.").font(.caption).foregroundStyle(.secondary)
                }
                Text("Escape ends routing or pin connection.").font(.caption).foregroundStyle(.secondary)
            }.controlSize(.small)
        }
    }
    private func open() {
        let panel = NSOpenPanel(); panel.allowedContentTypes = [UTType(filenameExtension: "vcad") ?? .json, .json]
        guard panel.runModal() == .OK, let url = panel.url else { return }
        do {
            let data = try Data(contentsOf: url)
            guard data.count < 32_000_000, let doc = try JSONSerialization.jsonObject(with: data) as? ECObject,
                  doc["nodes"] is ECObject else { throw CocoaError(.fileReadCorruptFile) }
            // AppInstance preserves unsaved work in the current document.
            AppInstance.opening(url, from: model)
        } catch { fileError = error.localizedDescription }
    }
    private func save() {
        guard let doc = model.documentJSON else { return }
        let panel = NSSavePanel(); panel.allowedContentTypes = [UTType(filenameExtension: "vcad") ?? .json]; panel.nameFieldStringValue = model.source.label + ".vcad"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        do {
            let data = try JSONSerialization.data(withJSONObject: doc, options: [.prettyPrinted, .sortedKeys])
            try data.write(to: url, options: .atomic); model.openDocument(url)
        } catch { fileError = error.localizedDescription }
    }
}

extension Dictionary where Key == String, Value == Any {
    var ecID: String { self["id"] as? String ?? "" }
}

struct ElectronicsCanvas: View {
    @Bindable var model: EditorModel
    private var ec: ElectronicsWorkspace { model.electronics }
    @State private var pointer: CGPoint?
    private func transform(_ size: CGSize) -> (CGFloat, CGPoint) {
        let vertices = ecRows((ec.board(model)["outline"] as? ECObject)?["vertices"]).map { ecPoint($0) }
        let minX = min(0, vertices.map(\.x).min() ?? 0), maxX = max(80, vertices.map(\.x).max() ?? 80)
        let minY = min(0, vertices.map(\.y).min() ?? 0), maxY = max(50, vertices.map(\.y).max() ?? 50)
        let scale = max(0.1, min((size.width - 60) / (maxX - minX), (size.height - 60) / (maxY - minY))) * ec.zoom
        return (scale, CGPoint(x: size.width / 2 - (minX + maxX) / 2 * scale, y: size.height / 2 - (minY + maxY) / 2 * scale))
    }
    var body: some View {
        GeometryReader { geo in
            let (scale, offset) = transform(geo.size)
            Canvas { context, size in
                let spatial = ec.view == .spatial
                func screen(_ p: CGPoint) -> CGPoint {
                    if spatial { return CGPoint(x: size.width / 2 + (p.x - 40 - (p.y - 25) * 0.45) * scale * 0.8, y: size.height / 2 + (p.y - 25) * scale * 0.65) }
                    return CGPoint(x: offset.x + p.x * scale, y: offset.y + p.y * scale)
                }
                func line(_ a: CGPoint, _ b: CGPoint, _ color: Color, _ width: CGFloat = 1) {
                    var path = Path(); path.move(to: screen(a)); path.addLine(to: screen(b)); context.stroke(path, with: .color(color), lineWidth: width)
                }
                let board = ec.board(model)
                let spacing = max(12, ec.grid * scale)
                for x in stride(from: offset.x.truncatingRemainder(dividingBy: spacing), to: size.width, by: spacing) {
                    for y in stride(from: offset.y.truncatingRemainder(dividingBy: spacing), to: size.height, by: spacing) {
                        context.fill(Path(ellipseIn: CGRect(x: x, y: y, width: 1, height: 1)), with: .color(.secondary.opacity(0.22)))
                    }
                }
                if ec.tool == .route, let start = ec.routeStart, let pointer {
                    line(start, ec.snapped(pointer), .yellow.opacity(0.16), max(5, (ec.traceWidth + 0.4) * scale))
                    line(start, ec.snapped(pointer), .yellow, max(1.5, ec.traceWidth * scale))
                }
                if ec.view != .schematic {
                    let vertices = ecRows((board["outline"] as? ECObject)?["vertices"]).map { screen(ecPoint($0)) }
                    var path = Path(); path.addLines(vertices); path.closeSubpath()
                    context.fill(path, with: .color(.green.opacity(0.15)))
                    context.stroke(path, with: .color(.green), lineWidth: 2)
                    for (i, trace) in ecRows(board["traces"]).enumerated() {
                        let layer = trace["layer"] as? String ?? "FCu"
                        line(ecPoint(trace["start"]), ecPoint(trace["end"]), i == ec.selectedTrace ? .yellow : layer == "FCu" ? .orange : .cyan, max(1.5, ecNumber(trace["width"], 0.25) * scale))
                    }
                    for (i, via) in ecRows(board["vias"]).enumerated() {
                        let p = screen(ecPoint(via["position"])), r = max(3, ecNumber(via["diameter"], 0.6) * scale / 2)
                        context.stroke(Path(ellipseIn: CGRect(x: p.x - r, y: p.y - r, width: r * 2, height: r * 2)), with: .color(i == ec.selectedVia ? .yellow : .cyan), lineWidth: 2)
                    }
                } else {
                    for wire in ecRows(ec.schematic(model)["wires"]) { line(ecPoint(wire["start"]), ecPoint(wire["end"]), .green) }
                    let nets = ec.schematic(model)["nets"] as? [String: [String]] ?? [:]
                    for (name, refs) in nets {
                        let points = refs.compactMap { pinPosition($0) }
                        if let first = points.first { for next in points.dropFirst() { line(first, next, name == ec.activeNet ? .orange : .green) } }
                    }
                }
                let components = ecRows(ec.view == .schematic ? ec.schematic(model)["components"] : board["footprints"])
                for f in components {
                    let ref = f["ref"] as? String ?? "?", p = ecPoint(f["position"]), center = screen(p)
                    let rect = CGRect(x: center.x - 3 * scale, y: center.y - 1.5 * scale, width: 6 * scale, height: 3 * scale)
                    context.fill(Path(roundedRect: rect, cornerRadius: 2), with: .color(ref == ec.selectedRef ? .accentColor.opacity(0.4) : .secondary.opacity(0.25)))
                    context.stroke(Path(roundedRect: rect, cornerRadius: 2), with: .color(ref == ec.selectedRef ? .accentColor : .primary), lineWidth: 1)
                    context.draw(Text(ref + " · " + (f["value"] as? String ?? "")).font(.system(size: 11)), at: CGPoint(x: center.x, y: center.y - 2.5 * scale))
                    let pads = ecRows(f[ec.view == .schematic ? "pins" : "pads"])
                    for pad in pads {
                        let pp = screen(Self.world(pad, in: f)), r = max(3, scale * 0.6)
                        context.fill(Path(ellipseIn: CGRect(x: pp.x - r, y: pp.y - r, width: 2 * r, height: 2 * r)), with: .color(ec.view == .schematic ? .green : .orange))
                    }
                }
                if let start = ec.routeStart { let p = screen(start); context.stroke(Path(ellipseIn: CGRect(x: p.x - 5, y: p.y - 5, width: 10, height: 10)), with: .color(.accentColor), lineWidth: 2) }
            }
            .onContinuousHover { phase in
                switch phase {
                case .active(let location): pointer = CGPoint(x: (location.x - offset.x) / scale, y: (location.y - offset.y) / scale)
                case .ended: pointer = nil
                }
            }
            .contentShape(Rectangle())
            .gesture(SpatialTapGesture().onEnded { event in
                guard ec.view != .spatial else { return }
                let p = CGPoint(x: (event.location.x - offset.x) / scale, y: (event.location.y - offset.y) / scale)
                switch ec.tool {
                case .place: ec.place(model, at: p)
                case .route:
                    var target = p
                    var onPad = false
                    for component in ecRows(ec.board(model)["footprints"]) {
                        for pad in ecRows(component["pads"]) {
                            let q = Self.world(pad, in: component)
                            if hypot(p.x - q.x, p.y - q.y) < 10 / scale {
                                if ec.routeStart == nil { ec.activeNet = pad["net"] as? String ?? "" }
                                else if pad["net"] as? String != ec.activeNet { ec.message = "This pad belongs to a different net."; return }
                                target = q; onPad = true
                            }
                        }
                    }
                    ec.addTrace(model, to: target, snapToGrid: !onPad)
                case .via: ec.addVia(model, at: p)
                case .wire:
                    if let pin = nearestPin(p, distance: 8 / scale) { ec.connect(model, pin: pin) }
                    else { ec.message = "Choose a component pin." }
                case .select: select(p, tolerance: 8 / scale)
                }
            })
            .simultaneousGesture(DragGesture(minimumDistance: 4).onEnded { event in
                guard ec.view != .spatial, ec.tool == .select else { return }
                let start = CGPoint(x: (event.startLocation.x - offset.x) / scale, y: (event.startLocation.y - offset.y) / scale)
                select(start, tolerance: 8 / scale)
                guard ec.selectedRef != nil else { return }
                let end = ec.snapped(CGPoint(x: (event.location.x - offset.x) / scale, y: (event.location.y - offset.y) / scale))
                ec.editComponent(model, key: "position", value: ecJSON(end))
            })
            .accessibilityLabel(ec.view == .schematic ? "Schematic canvas" : "PCB layout canvas")
        }
    }
    static func world(_ item: ECObject, in component: ECObject) -> CGPoint {
        let p = ecPoint(item["position"]), origin = ecPoint(component["position"]), radians = ecNumber(component["rotation"]) * .pi / 180
        let x = component["mirror"] as? Bool == true ? -p.x : p.x
        return CGPoint(x: origin.x + x * cos(radians) - p.y * sin(radians), y: origin.y + x * sin(radians) + p.y * cos(radians))
    }
    private func pinPosition(_ ref: String) -> CGPoint? {
        for component in ecRows(ec.schematic(model)["components"]) {
            for pin in ecRows(component["pins"]) where "\(component["ref"] as? String ?? "").\(pin["number"] as? String ?? "")" == ref { return Self.world(pin, in: component) }
        }
        return nil
    }
    private func nearestPin(_ p: CGPoint, distance: CGFloat) -> String? {
        for component in ecRows(ec.schematic(model)["components"]) {
            for pin in ecRows(component["pins"]) {
                let q = Self.world(pin, in: component)
                if hypot(p.x - q.x, p.y - q.y) < distance { return "\(component["ref"] as? String ?? "").\(pin["number"] as? String ?? "")" }
            }
        }
        return nil
    }
    private func select(_ p: CGPoint, tolerance: CGFloat) {
        ec.selectedRef = nil; ec.selectedTrace = nil; ec.selectedVia = nil
        for f in ecRows(ec.view == .schematic ? ec.schematic(model)["components"] : ec.board(model)["footprints"]) {
            let q = ecPoint(f["position"])
            if hypot(p.x - q.x, p.y - q.y) < max(4, tolerance) { ec.selectedRef = f["ref"] as? String; return }
        }
        guard ec.view == .board else { return }
        for (i, via) in ecRows(ec.board(model)["vias"]).enumerated() { let q = ecPoint(via["position"]); if hypot(p.x - q.x, p.y - q.y) < tolerance { ec.selectedVia = i; return } }
        for (i, t) in ecRows(ec.board(model)["traces"]).enumerated() {
            let a = ecPoint(t["start"]), b = ecPoint(t["end"]), dx = b.x - a.x, dy = b.y - a.y
            let u = max(0, min(1, ((p.x - a.x) * dx + (p.y - a.y) * dy) / max(0.0001, dx * dx + dy * dy)))
            if hypot(p.x - a.x - u * dx, p.y - a.y - u * dy) < tolerance { ec.selectedTrace = i; return }
        }
    }
}

/// Connectivity inset uses document net membership; it does not imply physical routing.
private struct ElectronicsNetPreview: View {
    let pins: [String]
    let net: String
    var body: some View {
        Canvas { context, size in
            guard !pins.isEmpty else {
                context.draw(Text("Select a net to inspect its pins").font(.caption).foregroundStyle(.secondary), at: CGPoint(x: size.width / 2, y: size.height / 2))
                return
            }
            let visible = Array(pins.prefix(6))
            let step = size.height / CGFloat(visible.count + 1)
            var bus = Path()
            bus.move(to: CGPoint(x: size.width - 35, y: step))
            bus.addLine(to: CGPoint(x: size.width - 35, y: step * CGFloat(visible.count)))
            context.stroke(bus, with: .color(.orange), lineWidth: 1.5)
            for (index, pin) in visible.enumerated() {
                let y = step * CGFloat(index + 1)
                context.draw(Text(pin).font(.system(size: 11, design: .monospaced)), at: CGPoint(x: 8, y: y), anchor: .leading)
                var wire = Path(); wire.move(to: CGPoint(x: 80, y: y)); wire.addLine(to: CGPoint(x: size.width - 35, y: y))
                context.stroke(wire, with: .color(.orange), lineWidth: 1.5)
                context.fill(Path(ellipseIn: CGRect(x: size.width - 38, y: y - 3, width: 6, height: 6)), with: .color(.orange))
            }
        }.accessibilityLabel("Net \(net): \(pins.joined(separator: ", "))")
    }
}
