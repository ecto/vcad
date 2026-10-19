#if canImport(AppKit)
import AppKit
#endif
import Foundation
import Network
import RealityKit
import simd
import SwiftUI

// Agent bridge: the app serves MCP on localhost so a Claude session and the
// user share ONE view — the release-to-desktop viewport. The agent reads what
// the user is pointing at (selection / hover), drives pose, explode and
// camera, lights parts and pins labels the user sees, and takes snapshots of
// exactly what is on screen.
//
// Read/drive only: nothing here edits the document. A generated `.loon` stays
// owned by its generator; regenerates arrive through the file watch.
//
// Transport: MCP streamable HTTP, JSON responses only (no SSE), at
// http://127.0.0.1:<port>/mcp. The port and pid land in ~/.vcad/app.json for
// the stdio shim (apple/VcadApp/scripts/vcad-app-mcp.mjs). Loopback only.

/// A label an agent pinned to an instance.
struct AgentAnnotation: Identifiable, Equatable {
    let id: String
    let instance: Int
    var text: String
}

@MainActor
final class AgentBridge {
    static let shared = AgentBridge()
    static let basePort: UInt16 = 47_361

    private weak var model: EditorModel?
    private var listener: NWListener?
    private(set) var port: UInt16?

    func start(model: EditorModel) {
        self.model = model
        guard listener == nil else { return }
        guard ProcessInfo.processInfo.environment["VCAD_AGENT_BRIDGE"] != "0" else { return }
        listen(on: Self.basePort, attemptsLeft: 8)
    }

    private func listen(on p: UInt16, attemptsLeft: Int) {
        let params = NWParameters.tcp
        params.requiredInterfaceType = .loopback
        params.allowLocalEndpointReuse = true
        guard let port = NWEndpoint.Port(rawValue: p),
              let l = try? NWListener(using: params, on: port) else {
            if attemptsLeft > 0 { listen(on: p + 1, attemptsLeft: attemptsLeft - 1) }
            return
        }
        l.newConnectionHandler = { conn in
            MainActor.assumeIsolated { AgentBridge.shared.accept(conn) }
        }
        l.stateUpdateHandler = { state in
            MainActor.assumeIsolated {
                switch state {
                case .ready:
                    AgentBridge.shared.port = p
                    AgentBridge.shared.writeDiscovery(port: p)
                case .failed:
                    AgentBridge.shared.listener?.cancel()
                    AgentBridge.shared.listener = nil
                    if attemptsLeft > 0 { AgentBridge.shared.listen(on: p + 1, attemptsLeft: attemptsLeft - 1) }
                default: break
                }
            }
        }
        listener = l
        l.start(queue: .main)
    }

    /// ~/.vcad/app.json — how the stdio shim (and anything else) finds us.
    private func writeDiscovery(port: UInt16) {
        let dir = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".vcad")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let info: [String: Any] = ["port": Int(port), "pid": Int(ProcessInfo.processInfo.processIdentifier),
                                   "url": "http://127.0.0.1:\(port)/mcp",
                                   "started": ISO8601DateFormatter().string(from: Date())]
        if let data = try? JSONSerialization.data(withJSONObject: info, options: [.prettyPrinted, .sortedKeys]) {
            try? data.write(to: dir.appendingPathComponent("app.json"), options: .atomic)
        }
        FileHandle.standardError.write(Data("[vcad] agent bridge on http://127.0.0.1:\(port)/mcp\n".utf8))
    }

    // MARK: HTTP (just enough: one request per connection, Content-Length bodies)

    private func accept(_ conn: NWConnection) {
        conn.start(queue: .main)
        receive(conn, buffer: Data())
    }

    private func receive(_ conn: NWConnection, buffer: Data) {
        conn.receive(minimumIncompleteLength: 1, maximumLength: 1 << 20) { data, _, done, error in
            MainActor.assumeIsolated {
                var buf = buffer
                if let data { buf.append(data) }
                if let req = HTTPRequest.parse(buf) {
                    AgentBridge.shared.handle(req, conn)
                } else if done || error != nil || buf.count > 32 << 20 {
                    conn.cancel()
                } else {
                    AgentBridge.shared.receive(conn, buffer: buf)
                }
            }
        }
    }

    private func respond(_ conn: NWConnection, status: String, body: Data = Data(),
                         contentType: String = "application/json") {
        var head = "HTTP/1.1 \(status)\r\nContent-Type: \(contentType)\r\nContent-Length: \(body.count)\r\nConnection: close\r\n"
        head += "Mcp-Session-Id: vcad-app\r\n\r\n"
        var out = Data(head.utf8)
        out.append(body)
        conn.send(content: out, completion: .contentProcessed { _ in conn.cancel() })
    }

    private func handle(_ req: HTTPRequest, _ conn: NWConnection) {
        // A browser page must not drive the app: refuse any request carrying a
        // non-loopback Origin (DNS-rebinding guard, per the MCP transport spec).
        if let origin = req.headers["origin"], !origin.contains("127.0.0.1"), !origin.contains("localhost") {
            respond(conn, status: "403 Forbidden"); return
        }
        guard req.path.hasPrefix("/mcp") else { respond(conn, status: "404 Not Found"); return }
        guard req.method == "POST" else { respond(conn, status: "405 Method Not Allowed"); return }
        guard let msg = (try? JSONSerialization.jsonObject(with: req.body)) as? [String: Any] else {
            respond(conn, status: "400 Bad Request", body: Self.rpcError(nil, -32700, "parse error")); return
        }
        guard let id = msg["id"] else { respond(conn, status: "202 Accepted"); return }   // notification
        let method = msg["method"] as? String ?? ""
        let params = msg["params"] as? [String: Any] ?? [:]
        Task { @MainActor in
            let body: Data
            switch method {
            case "initialize":
                body = Self.rpcResult(id, [
                    "protocolVersion": (params["protocolVersion"] as? String) ?? "2025-06-18",
                    "capabilities": ["tools": ["listChanged": false]],
                    "serverInfo": ["name": "vcad-app", "version": "0.1"],
                    "instructions": Self.instructions,
                ])
            case "ping":
                body = Self.rpcResult(id, [:])
            case "tools/list":
                body = Self.rpcResult(id, ["tools": AgentTools.definitions])
            case "tools/call":
                let name = params["name"] as? String ?? ""
                let args = params["arguments"] as? [String: Any] ?? [:]
                let result = await self.callTool(name, args)
                body = Self.rpcResult(id, result)
            default:
                body = Self.rpcError(id, -32601, "method not found: \(method)")
            }
            self.respond(conn, status: "200 OK", body: body)
        }
    }

    static let instructions = """
    You are sharing the user's live vcad desktop viewport. Call app_state first: it says what is \
    open, the joint pose, explode, camera, and what the user has selected or is hovering — when \
    the user says "this part", that is it. Everything you change (pose, explode, camera, \
    highlights, labels) is visible to the user immediately. Use snapshot to see the view. \
    The document itself is read-only here.
    """

    static func rpcResult(_ id: Any, _ result: [String: Any]) -> Data {
        (try? JSONSerialization.data(withJSONObject: ["jsonrpc": "2.0", "id": id, "result": result])) ?? Data()
    }
    static func rpcError(_ id: Any?, _ code: Int, _ message: String) -> Data {
        (try? JSONSerialization.data(withJSONObject: ["jsonrpc": "2.0", "id": id ?? NSNull(),
                                                      "error": ["code": code, "message": message]])) ?? Data()
    }

    // MARK: tools

    private func callTool(_ name: String, _ args: [String: Any]) async -> [String: Any] {
        guard let model else { return AgentTools.error("The app has no window yet.") }
        do {
            return try await AgentTools.call(name, args, model: model)
        } catch let e as AgentTools.Failure {
            return AgentTools.error(e.message)
        } catch {
            return AgentTools.error("\(error)")
        }
    }
}

/// One parsed HTTP request (nil until the whole body has arrived).
struct HTTPRequest {
    let method: String
    let path: String
    let headers: [String: String]
    let body: Data

    static func parse(_ buf: Data) -> HTTPRequest? {
        guard let sep = buf.range(of: Data("\r\n\r\n".utf8)) else { return nil }
        let head = String(decoding: buf[..<sep.lowerBound], as: UTF8.self)
        var lines = head.components(separatedBy: "\r\n")
        let start = lines.removeFirst().split(separator: " ")
        guard start.count >= 2 else { return nil }
        var headers: [String: String] = [:]
        for l in lines {
            guard let c = l.firstIndex(of: ":") else { continue }
            headers[l[..<c].lowercased()] = l[l.index(after: c)...].trimmingCharacters(in: .whitespaces)
        }
        let len = Int(headers["content-length"] ?? "0") ?? 0
        let bodyStart = sep.upperBound
        guard buf.count - bodyStart >= len else { return nil }
        return HTTPRequest(method: String(start[0]), path: String(start[1]), headers: headers,
                           body: buf[bodyStart..<(bodyStart + len)])
    }
}

@MainActor
enum AgentTools {
    struct Failure: Error { let message: String }

    static func text(_ s: String) -> [String: Any] { ["content": [["type": "text", "text": s]]] }
    static func error(_ s: String) -> [String: Any] { ["content": [["type": "text", "text": s]], "isError": true] }
    static func json(_ obj: Any) -> [String: Any] {
        let data = (try? JSONSerialization.data(withJSONObject: obj, options: [.prettyPrinted, .sortedKeys])) ?? Data()
        return text(String(decoding: data, as: UTF8.self))
    }

    private static func schema(_ props: [String: Any], required: [String] = []) -> [String: Any] {
        ["type": "object", "properties": props, "required": required]
    }
    private static let partsProp: [String: Any] = [
        "type": "array", "items": ["type": "string"],
        "description": "Instance ids or name fragments (e.g. \"cover-knee_ll\", \"knee_ll\"), or a joint name (\"l_knee_pitch\") for the link it drives.",
    ]

    static let definitions: [[String: Any]] = [
        ["name": "app_state",
         "description": "What the user is looking at: open document, every driveable joint (value + limits), explode, camera, the user's selection and hover, and your current highlights/labels. Call this first, and again when the user says \"this\".",
         "inputSchema": schema([:])],
        ["name": "list_parts",
         "description": "All instance ids in the open assembly, with their material.",
         "inputSchema": schema([:])],
        ["name": "open_document",
         "description": "Open a .vcad or .loon file in the shared view (read-only; .loon files hot-reload when regenerated).",
         "inputSchema": schema(["path": ["type": "string"]], required: ["path"])],
        ["name": "set_pose",
         "description": "Set joint angles in degrees (clamped to limits), by joint name, e.g. {\"l_knee_pitch\": 60}. reset:true first returns every joint to its authored value.",
         "inputSchema": schema(["joints": ["type": "object", "additionalProperties": ["type": "number"]],
                                "reset": ["type": "boolean"]])],
        ["name": "set_explode",
         "description": "Explode amount 0 (assembled) … 1 (fully exploded).",
         "inputSchema": schema(["amount": ["type": "number"]], required: ["amount"])],
        ["name": "set_camera",
         "description": "Orbit the shared camera. azimuth/elevation in degrees (Z-up; azimuth 0 looks from the front), distance in view units (1.5 ≈ whole model).",
         "inputSchema": schema(["azimuth": ["type": "number"], "elevation": ["type": "number"],
                                "distance": ["type": "number"]])],
        ["name": "frame_part",
         "description": "Point the camera at a part and pull in to fit it.",
         "inputSchema": schema(["part": ["type": "string"]], required: ["part"])],
        ["name": "highlight_parts",
         "description": "Light parts in brand orange so the user sees what you mean. Replaces the previous highlight unless add:true; parts:[] clears.",
         "inputSchema": schema(["parts": partsProp, "add": ["type": "boolean"]], required: ["parts"])],
        ["name": "annotate",
         "description": "Pin a text label to a part, drawn in the user's viewport with a leader to the part. Same part again replaces its label.",
         "inputSchema": schema(["part": ["type": "string"], "text": ["type": "string"]], required: ["part", "text"])],
        ["name": "clear_annotations",
         "description": "Remove your labels (and highlights, with highlights:true).",
         "inputSchema": schema(["highlights": ["type": "boolean"]])],
        ["name": "snapshot",
         "description": "PNG of the shared viewport exactly as the user sees it (geometry, highlights; labels are an overlay and not included).",
         "inputSchema": schema([:])],
        ["name": "photoreal",
         "description": "Path-trace the current pose/explode from the current camera (vcad-render --photoreal, ~30-60 s). Returns the image and its saved path.",
         "inputSchema": schema(["spp": ["type": "integer", "description": "samples per pixel, default 64"]])],
    ]

    static func call(_ name: String, _ a: [String: Any], model m: EditorModel) async throws -> [String: Any] {
        switch name {
        case "app_state": return json(state(m))
        case "list_parts":
            return json(m.assemblyInstanceIDs.enumerated().map { i, id in
                ["id": id, "index": i, "highlighted": m.agentHighlights.contains(i)] as [String: Any]
            })
        case "open_document":
            guard let path = a["path"] as? String else { throw Failure(message: "path is required") }
            let url = URL(fileURLWithPath: (path as NSString).expandingTildeInPath)
            guard FileManager.default.fileExists(atPath: url.path) else { throw Failure(message: "no file at \(url.path)") }
            m.openDocument(url)
            return text("Opened \(url.lastPathComponent). It solves in the background; call app_state to see when instances appear.")
        case "set_pose":
            guard m.hasPoseStudio else { throw Failure(message: "The open document has no poseable assembly.") }
            if a["reset"] as? Bool == true { m.jointPose = [:] }
            var unknown: [String] = [], applied: [String: Double] = [:]
            for (k, v) in (a["joints"] as? [String: Any]) ?? [:] {
                guard let deg = (v as? NSNumber)?.doubleValue else { continue }
                guard let j = m.drivenJoints.first(where: { EditorModel.poseKey($0) == k || $0.id == k }) else {
                    unknown.append(k); continue
                }
                let clamped = j.limits.map { min(max(deg, $0.lowerBound), $0.upperBound) } ?? deg
                m.jointPose[EditorModel.poseKey(j)] = clamped
                applied[EditorModel.poseKey(j)] = clamped
            }
            m.applyStudioPose()
            var msg = "Pose applied: \(applied.map { "\($0.key)=\(String(format: "%.1f", $0.value))°" }.sorted().joined(separator: ", "))"
            if !unknown.isEmpty {
                msg += ". Unknown joints: \(unknown.joined(separator: ", ")) — names are \(m.drivenJoints.map(EditorModel.poseKey).joined(separator: ", "))"
            }
            return text(msg)
        case "set_explode":
            guard let t = (a["amount"] as? NSNumber)?.doubleValue else { throw Failure(message: "amount is required") }
            m.setExplode(t)
            return text("Explode \(Int(m.explode * 100))%")
        case "set_camera":
            m.stopSpin()
            withAnimation(.smooth(duration: 0.4)) {
                if let az = (a["azimuth"] as? NSNumber)?.floatValue { m.azimuth = az * .pi / 180 }
                if let el = (a["elevation"] as? NSNumber)?.floatValue { m.elevation = max(-1.5, min(1.5, el * .pi / 180)) }
                if let d = (a["distance"] as? NSNumber)?.floatValue { m.distance = max(0.2, min(8, d)) }
            }
            m.pinchBaseline = m.distance
            return json(camera(m))
        case "frame_part":
            let idx = try resolve([a["part"] as? String ?? ""], m)
            guard let i = idx.first else { throw Failure(message: "no part matched") }
            ReleaseWindowController.shared.frame(entityNamed: "inst\(i)")
            return text("Framed \(m.assemblyInstanceIDs[i]).")
        case "highlight_parts":
            let names = (a["parts"] as? [String]) ?? []
            let idx = names.isEmpty ? [] : try resolve(names, m)
            m.agentHighlights = a["add"] as? Bool == true ? m.agentHighlights.union(idx) : Set(idx)
            ReleaseWindowController.shared.repaintAll()
            return text(idx.isEmpty ? "Highlights cleared."
                        : "Highlighted \(idx.count): \(idx.map { m.assemblyInstanceIDs[$0] }.joined(separator: ", "))")
        case "annotate":
            let part = a["part"] as? String ?? ""
            let note = a["text"] as? String ?? ""
            guard let i = try resolve([part], m).first else { throw Failure(message: "no part matched") }
            let id = m.assemblyInstanceIDs[i]
            m.agentAnnotations.removeAll { $0.instance == i }
            if !note.isEmpty { m.agentAnnotations.append(AgentAnnotation(id: id, instance: i, text: note)) }
            return text("Label on \(id): \(note)")
        case "clear_annotations":
            m.agentAnnotations = []
            if a["highlights"] as? Bool == true {
                m.agentHighlights = []
                ReleaseWindowController.shared.repaintAll()
            }
            return text("Cleared.")
        case "snapshot":
            guard let png = await snapshotPNG() else { throw Failure(message: "The viewport is not on screen.") }
            return ["content": [["type": "image", "data": png.base64EncodedString(), "mimeType": "image/png"],
                                ["type": "text", "text": "Viewport \(m.documentName); labels: \(m.agentAnnotations.map { "\($0.id): \($0.text)" })"]]]
        case "photoreal":
            let spp = (a["spp"] as? NSNumber)?.intValue ?? 64
            let (path, err): (String?, String?) = await withCheckedContinuation { cont in
                m.photorealSnapshot(spp: spp) { p, e in cont.resume(returning: (p, e)) }
            }
            guard let path else { throw Failure(message: err ?? "photoreal failed") }
            var content: [[String: Any]] = [["type": "text", "text": "Saved \(path)"]]
            if let img = NSImage(contentsOfFile: path), let png = downscaledPNG(img, maxSide: 1000) {
                content.insert(["type": "image", "data": png.base64EncodedString(), "mimeType": "image/png"], at: 0)
            }
            return ["content": content]
        default:
            throw Failure(message: "unknown tool \(name)")
        }
    }

    // MARK: state

    static func state(_ m: EditorModel) -> [String: Any] {
        let ids = m.assemblyInstanceIDs
        func name(_ i: Int?) -> Any { i.flatMap { $0 < ids.count ? ids[$0] : nil } ?? NSNull() }
        var s: [String: Any] = [
            "document": ["name": m.documentName, "path": m.documentURL?.path ?? NSNull(),
                         "readOnlyGenerated": m.isLoonDocument,
                         "solving": m.solving, "instances": m.assemblyInstanceCount] as [String: Any],
            "camera": camera(m),
            "explode": m.explode,
            "user": ["selected": m.selectedInstances.sorted().map { ids.indices.contains($0) ? ids[$0] : "\($0)" },
                     "hovered": name(m.hoveredInstanceIndex)] as [String: Any],
            "agent": ["highlights": m.agentHighlights.sorted().map { ids.indices.contains($0) ? ids[$0] : "\($0)" },
                      "labels": m.agentAnnotations.map { ["part": $0.id, "text": $0.text] }] as [String: Any],
            "mode": m.isWindowed ? "windowed" : "released-to-desktop",
        ]
        if let t = m.lastReload { s["lastReload"] = ISO8601DateFormatter().string(from: t) }
        s["joints"] = m.drivenJoints.map { j -> [String: Any] in
            var o: [String: Any] = ["name": EditorModel.poseKey(j), "value": m.jointValue(j), "kind": j.kind,
                                    "child": j.childInstanceId]
            if let l = j.limits { o["min"] = l.lowerBound; o["max"] = l.upperBound }
            return o
        }
        return s
    }

    static func camera(_ m: EditorModel) -> [String: Any] {
        ["azimuth": Double(m.azimuth) * 180 / .pi, "elevation": Double(m.elevation) * 180 / .pi,
         "distance": Double(m.distance)]
    }

    /// Names → instance indices. Exact id, id without the `_i` suffix, a joint
    /// name (its child link), then substring. Throws listing near misses.
    static func resolve(_ names: [String], _ m: EditorModel) throws -> [Int] {
        let ids = m.assemblyInstanceIDs
        guard !ids.isEmpty else { throw Failure(message: "No assembly is open.") }
        var out: [Int] = []
        for raw in names {
            let n = raw.trimmingCharacters(in: .whitespaces)
            if let i = ids.firstIndex(where: { $0 == n || $0 == n + "_i" }) { out.append(i); continue }
            if let j = m.drivenJoints.first(where: { EditorModel.poseKey($0) == n }),
               let i = ids.firstIndex(of: j.childInstanceId) { out.append(i); continue }
            let hits = ids.indices.filter { ids[$0].localizedCaseInsensitiveContains(n) }
            guard !hits.isEmpty else {
                throw Failure(message: "No part matches \"\(n)\". Call list_parts for ids.")
            }
            out += hits
        }
        return Array(Set(out)).sorted()
    }

    // MARK: images

    static func snapshotPNG() async -> Data? {
        guard let ar = ReleaseWindowController.shared.arView else { return nil }
        let cg: CGImage? = await withCheckedContinuation { cont in
            ar.snapshot(saveToHDR: false) { image in
                cont.resume(returning: image?.cgImage(forProposedRect: nil, context: nil, hints: nil))
            }
        }
        guard let cg else { return nil }
        return downscaledPNG(NSImage(cgImage: cg, size: .zero), maxSide: 1400)
    }

    static func downscaledPNG(_ img: NSImage, maxSide: CGFloat) -> Data? {
        guard let cg = img.cgImage(forProposedRect: nil, context: nil, hints: nil) else { return nil }
        let w = CGFloat(cg.width), h = CGFloat(cg.height)
        let s = min(1, maxSide / max(w, h))
        let W = Int(w * s), H = Int(h * s)
        guard let ctx = CGContext(data: nil, width: W, height: H, bitsPerComponent: 8, bytesPerRow: 0,
                                  space: CGColorSpaceCreateDeviceRGB(),
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return nil }
        // Released mode renders on a clear background; give the agent a solid one.
        ctx.setFillColor(CGColor(gray: 0.13, alpha: 1))
        ctx.fill(CGRect(x: 0, y: 0, width: W, height: H))
        ctx.interpolationQuality = .high
        ctx.draw(cg, in: CGRect(x: 0, y: 0, width: W, height: H))
        guard let out = ctx.makeImage() else { return nil }
        return NSBitmapImageRep(cgImage: out).representation(using: .png, properties: [:])
    }
}

// MARK: - overlay

/// The agent's labels, drawn over the viewport with a leader to each part.
/// Re-projected every frame so they follow pose, explode and camera.
struct AgentAnnotationOverlay: View {
    let model: EditorModel

    var body: some View {
        if model.agentAnnotations.isEmpty {
            EmptyView()
        } else {
            TimelineView(.animation) { _ in
                GeometryReader { _ in
                    ZStack(alignment: .topLeading) {
                        ForEach(model.agentAnnotations) { a in
                            if let p = Self.anchor(a.instance) {
                                AnnotationPin(text: a.text, at: p)
                            }
                        }
                    }
                }
            }
            .allowsHitTesting(false)
        }
    }

    /// Screen point of an instance's visual centre, in overlay coordinates.
    static func anchor(_ instance: Int) -> CGPoint? {
        let rw = ReleaseWindowController.shared
        guard let ar = rw.arView,
              let e = rw.centeringEntity?.findEntity(named: "inst\(instance)") else { return nil }
        let c = e.visualBounds(relativeTo: nil).center
        guard let p = ar.project(c) else { return nil }
        // ARView on macOS projects in AppKit view coordinates (origin bottom-left).
        return CGPoint(x: p.x, y: ar.isFlipped ? p.y : ar.bounds.height - p.y)
    }
}

private struct AnnotationPin: View {
    let text: String
    let at: CGPoint
    private let offset = CGSize(width: 70, height: -54)

    var body: some View {
        let label = CGPoint(x: at.x + offset.width, y: at.y + offset.height)
        ZStack(alignment: .topLeading) {
            Path { p in
                p.move(to: at)
                p.addLine(to: CGPoint(x: label.x, y: label.y + 12))
            }
            .stroke(Color.orange, lineWidth: 1.5)
            Circle().fill(Color.orange).frame(width: 8, height: 8)
                .position(at)
            HStack(spacing: 6) {
                Image(systemName: "sparkle").font(.caption2)
                Text(text).font(.callout.weight(.medium)).fixedSize()
            }
            .padding(.horizontal, 10).padding(.vertical, 6)
            .background(.ultraThinMaterial, in: Capsule())
            .overlay(Capsule().strokeBorder(Color.orange.opacity(0.8), lineWidth: 1))
            .fixedSize()
            .position(x: label.x, y: label.y)
            .alignmentGuide(.leading) { $0[.leading] }
        }
    }
}
