// swift-tools-version: 6.0
import PackageDescription
import Foundation

// visionOS spike. SwiftPM can't express a visionOS app target, so bundle.sh
// drives swift-build with an explicit -target triple + xrsimulator SDK and
// assembles the .app by hand (same trick as ../VcadApp/bundle.sh, one level up
// in ambition). Libs/ holds libvcad_ffi.a built for aarch64-apple-visionos-sim
// by build-ffi.sh.
let ffiLibDir = URL(fileURLWithPath: #filePath)
    .deletingLastPathComponent()
    .appendingPathComponent("Libs")
    .path

let package = Package(
    name: "VcadVision",
    platforms: [.visionOS(.v2)],
    targets: [
        .systemLibrary(name: "CVcadFFI", path: "Sources/CVcadFFI"),
        .executableTarget(
            name: "VcadVision",
            dependencies: ["CVcadFFI"],
            path: "Sources/VcadVision",
            linkerSettings: [
                .unsafeFlags(["-L", ffiLibDir]),
                .linkedLibrary("c++"),
            ]
        ),
    ]
)
