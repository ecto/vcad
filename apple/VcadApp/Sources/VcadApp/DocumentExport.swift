#if os(macOS)
import AppKit
import CVcadFFI
import Foundation
import simd
import SwiftUI
import UniformTypeIdentifiers

// File ▸ Export (STL / STEP / GLB) and File ▸ Render Photoreal…, both through
// vcad-ffi: the same writers as `vcad export` and the same renderer as
// `vcad render --photoreal`, linked into the app (no vcad-render binary).

/// A document export format, as `vcad_export_document` numbers it.
enum DocumentExportFormat: CaseIterable, Sendable {
    case stl, step, glb

    var label: String {
        switch self { case .stl: "STL"; case .step: "STEP"; case .glb: "GLB" }
    }
    var fileExtension: String {
        switch self { case .stl: "stl"; case .step: "step"; case .glb: "glb" }
    }
    var contentType: UTType {
        UTType(filenameExtension: fileExtension) ?? .data
    }
    var ffiCode: UInt32 {
        switch self { case .stl: UInt32(VCAD_EXPORT_STL); case .step: UInt32(VCAD_EXPORT_STEP); case .glb: UInt32(VCAD_EXPORT_GLB) }
    }
}

/// A kernel call's failure: `vcad_last_error`'s message.
struct KernelError: Error, Equatable, CustomStringConvertible {
    let message: String
    init(_ message: String) { self.message = message }
    var description: String { message }
    /// The render was cancelled through its progress callback.
    var isCancelled: Bool { message == "cancelled" }
}

/// This thread's `vcad_last_error`, or `fallback`.
func vcadLastError(_ fallback: String) -> String {
    var count = 0
    guard let p = vcad_last_error(&count), count > 0 else { return fallback }
    return String(decoding: UnsafeBufferPointer(start: p, count: count), as: UTF8.self)
}

enum KernelExport {
    /// Write `json` to `url` as `format`. Blocking; returns parts written.
    nonisolated static func export(json: Data, baseDir: String?, to url: URL,
                                   format: DocumentExportFormat) -> Result<Int, KernelError> {
        let base = Array((baseDir ?? "").utf8)
        let out = Array(url.path.utf8)
        let n: Int32 = json.withUnsafeBytes { raw in
            let doc = raw.bindMemory(to: UInt8.self)
            return base.withUnsafeBufferPointer { b in
                out.withUnsafeBufferPointer { o in
                    vcad_export_document(doc.baseAddress, doc.count,
                                         b.count > 0 ? b.baseAddress : nil, b.count,
                                         o.baseAddress, o.count, format.ffiCode)
                }
            }
        }
        return n > 0 ? .success(Int(n)) : .failure(KernelError(vcadLastError("\(format.label) export failed")))
    }
}

/// One photoreal render: progress in, cancel out. The C progress callback
/// reaches it through an unretained pointer for the duration of the call.
final class PhotorealJob: @unchecked Sendable {
    private let lock = NSLock()
    private var cancelRequested = false
    private let onProgress: @Sendable (_ done: UInt32, _ total: UInt32) -> Void

    init(onProgress: @escaping @Sendable (_ done: UInt32, _ total: UInt32) -> Void = { _, _ in }) {
        self.onProgress = onProgress
    }

    func cancel() { lock.lock(); cancelRequested = true; lock.unlock() }
    var isCancelled: Bool { lock.lock(); defer { lock.unlock() }; return cancelRequested }

    /// Blocking render to PNG bytes. Call off the main thread.
    nonisolated func render(json: Data, baseDir: String?, width: Int, height: Int, samples: Int,
                            azimuth: Double, elevation: Double, forceCPU: Bool = false) -> Result<Data, KernelError> {
        var params = VcadPhotorealParams(width: UInt32(clamping: width), height: UInt32(clamping: height),
                                         samples: UInt32(clamping: samples), azimuth_deg: azimuth,
                                         elevation_deg: elevation, force_cpu: forceCPU ? 1 : 0)
        let base = Array((baseDir ?? "").utf8)
        let ctx = Unmanaged.passUnretained(self).toOpaque()
        let progress: VcadRenderProgress = { ctx, done, total in
            guard let ctx else { return 0 }
            let job = Unmanaged<PhotorealJob>.fromOpaque(ctx).takeUnretainedValue()
            job.onProgress(done, total)
            return job.isCancelled ? 1 : 0
        }
        let bytes: OpaquePointer? = json.withUnsafeBytes { raw in
            let doc = raw.bindMemory(to: UInt8.self)
            return base.withUnsafeBufferPointer { b in
                vcad_render_photoreal_png(doc.baseAddress, doc.count,
                                          b.count > 0 ? b.baseAddress : nil, b.count,
                                          &params, progress, ctx)
            }
        }
        withExtendedLifetime(self) {}
        guard let bytes else { return .failure(KernelError(vcadLastError("photoreal render failed"))) }
        defer { vcad_bytes_free(bytes) }
        let view = vcad_bytes_view(bytes)
        guard let data = view.data, view.len > 0 else { return .failure(KernelError("photoreal render returned no image")) }
        return .success(Data(bytes: data, count: view.len))
    }
}

extension EditorModel {
    /// The document to export or render, as `.vcad` JSON: the on-screen pose
    /// baked in when there is one, else the edited document, else the file.
    /// Nil for sources with no document (the sandbox, a generated preview).
    func exportableDocumentData() -> Data? {
        if let posed = posedDocumentJSON() { return posed }
        if let json = documentJSON { return DocEdit.serialize(json) }
        if case let .document(path, _) = source { return Self.documentBytes(path: path) }
        return nil
    }

    /// Directory relative mesh imports resolve against.
    var exportBaseDir: String? { documentURL?.deletingLastPathComponent().path }

    /// Cheap enough for menu validation (no serialization).
    var canExportDocument: Bool {
        guard canExport else { return false }
        if documentJSON != nil { return true }
        if case .document = source { return true }
        return false
    }

    /// Camera direction in the kernel frame: azimuth CCW from +X, elevation
    /// above XY, degrees. The renderer frames the scene itself.
    func photorealCameraAngles() -> (azimuth: Double, elevation: Double) {
        var az = 35.0, el = 20.0
        if let (cam, target) = kernelCamera() {
            let d = cam - target
            let len = simd_length(d)
            if len > 1e-9 {
                az = atan2(d.y, d.x) * 180 / .pi
                el = asin(d.z / len) * 180 / .pi
            }
        }
        return (az, el)
    }

    /// File ▸ Export as `format`, off the main thread. A source with no
    /// document (the sandbox) still exports STL from the on-screen meshes.
    func exportDocument(as format: DocumentExportFormat, to url: URL) {
        guard let json = exportableDocumentData() else {
            if format == .stl, exportSTL(to: url) { return }
            presentExportError("Nothing to export as \(format.label).")
            return
        }
        let base = exportBaseDir
        Task.detached(priority: .userInitiated) {
            let result = KernelExport.export(json: json, baseDir: base, to: url, format: format)
            await MainActor.run {
                if case let .failure(error) = result { presentExportError(error.message) }
            }
        }
    }
}

@MainActor private func presentExportError(_ message: String) {
    let alert = NSAlert()
    alert.alertStyle = .warning
    alert.messageText = "Export failed"
    alert.informativeText = message
    if let host = ReleaseWindowController.shared.hostWindow, host.styleMask.contains(.titled) {
        alert.beginSheetModal(for: host)
    } else {
        alert.runModal()
    }
}

@MainActor func exportDocumentPanel(_ model: EditorModel, format: DocumentExportFormat) {
    let panel = NSSavePanel()
    panel.allowedContentTypes = [format.contentType]
    panel.nameFieldStringValue = "\(model.documentName).\(format.fileExtension)"
    panel.prompt = "Export"
    panel.canCreateDirectories = true
    present(panel) { if let url = panel.url { model.exportDocument(as: format, to: url) } }
}

// MARK: - Render Photoreal… sheet

/// State for File ▸ Render Photoreal….
@MainActor @Observable
final class PhotorealRenderController {
    static let shared = PhotorealRenderController()

    var width = 1600
    var height = 1200
    var samples = 128
    private(set) var running = false
    private(set) var progress = 0.0
    private(set) var status: String?
    private var job: PhotorealJob?
    private var window: NSPanel?

    func show(for model: EditorModel) {
        if window == nil {
            let host = NSHostingController(rootView: PhotorealRenderSheet(controller: self, model: model))
            let panel = NSPanel(contentViewController: host)
            panel.title = "Render Photoreal"
            panel.styleMask = [.titled, .closable, .utilityWindow]
            panel.isReleasedWhenClosed = false
            window = panel
        }
        window?.center()
        window?.makeKeyAndOrderFront(nil)
    }

    func start(model: EditorModel) {
        guard !running else { return }
        guard let json = model.exportableDocumentData() else {
            status = "Open a document to render."; return
        }
        let (az, el) = model.photorealCameraAngles()
        let (w, h, spp) = (width, height, samples)
        let base = model.exportBaseDir
        let name = model.documentName
        let job = PhotorealJob { [weak self] done, total in
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    guard let self, self.running else { return }
                    self.progress = total > 0 ? Double(done) / Double(total) : 0
                    self.status = "Path tracing… \(done)/\(total) samples"
                }
            }
        }
        self.job = job
        running = true
        progress = 0
        status = "Preparing scene…"
        let started = Date()
        Task.detached(priority: .userInitiated) {
            let result = job.render(json: json, baseDir: base, width: w, height: h, samples: spp,
                                    azimuth: az, elevation: el)
            await MainActor.run { [weak self] in
                self?.finish(result, name: name, seconds: Date().timeIntervalSince(started))
            }
        }
    }

    func cancel() {
        job?.cancel()
        status = "Cancelling…"
    }

    private func finish(_ result: Result<Data, KernelError>, name: String, seconds: Double) {
        running = false
        job = nil
        switch result {
        case .failure(let error):
            progress = 0
            status = error.isCancelled ? "Cancelled." : error.message
        case .success(let png):
            progress = 1
            status = String(format: "Rendered in %.0f s", seconds)
            let panel = NSSavePanel()
            panel.allowedContentTypes = [.png]
            panel.nameFieldStringValue = "\(name).png"
            panel.prompt = "Save"
            panel.canCreateDirectories = true
            let save = { [weak self] in
                guard let url = panel.url else { return }
                do {
                    try png.write(to: url, options: .atomic)
                    self?.status = "Saved \(url.lastPathComponent)"
                } catch {
                    self?.status = "Could not save: \(error.localizedDescription)"
                }
            }
            if let window {
                panel.beginSheetModal(for: window) { if $0 == .OK { save() } }
            } else if panel.runModal() == .OK {
                save()
            }
        }
    }
}

struct PhotorealRenderSheet: View {
    @Bindable var controller: PhotorealRenderController
    let model: EditorModel

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Form {
                HStack {
                    TextField("Width", value: $controller.width, format: .number).frame(width: 80)
                    Text("×")
                    TextField("Height", value: $controller.height, format: .number).frame(width: 80)
                    Text("px").foregroundStyle(.secondary)
                }
                Stepper(value: $controller.samples, in: 1...4096, step: 16) {
                    Text("Samples: \(controller.samples)")
                }
            }
            .disabled(controller.running)
            Text("Renders from the current camera.").font(.caption).foregroundStyle(.secondary)
            if controller.running || controller.progress > 0 {
                ProgressView(value: controller.progress)
            }
            if let status = controller.status {
                Text(status).font(.caption).foregroundStyle(.secondary).lineLimit(3)
            }
            HStack {
                Spacer()
                if controller.running {
                    Button("Cancel") { controller.cancel() }.keyboardShortcut(.cancelAction)
                } else {
                    Button("Render…") { controller.start(model: model) }
                        .keyboardShortcut(.defaultAction)
                        .disabled(controller.width < 1 || controller.height < 1
                                  || controller.width > 16384 || controller.height > 16384)
                }
            }
        }
        .padding(20)
        .frame(width: 340)
    }
}
#endif
