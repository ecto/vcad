import SwiftUI
import CVcadFFI

// vcad on Vision Pro — the full app, not the spike.
//
// The shared sources under Shared/ (symlinked from ../VcadApp) provide the
// whole editor: EditorModel, the document DAG, the kernel FFI bridge, the
// intent engine, and the SwiftUI panels (palette, feature tree, inspector,
// composer, playback). This target supplies what macOS can't share — the
// volumetric viewport (DocumentVolume) and the spatial window layout.
//
// Layout: one volumetric window. The part floats in the center; panels ride
// ornaments around the volume — feature tree leading, inspector trailing,
// palette top, composer/playback bottom. Release-to-desktop has no meaning
// here: the app is *born* released.

@main
struct VcadVisionApp: App {
    @State private var model = EditorModel()
    @State private var intent = IntentEngine()

    var body: some Scene {
        WindowGroup(id: "part") {
            DocumentVolume(model: model, intent: intent)
                .task {
                    let env = ProcessInfo.processInfo.environment
                    if let path = env["VCAD_OPEN"], !path.isEmpty {
                        model.openDocument(URL(fileURLWithPath: path))
                    }
                }
        }
        .windowStyle(.volumetric)
        .defaultSize(width: 0.7, height: 0.55, depth: 0.55, in: .meters)
    }
}
