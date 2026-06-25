// swift-tools-version: 6.0
import PackageDescription
import Foundation

// Directory holding libvcad_ffi.a (built by build-ffi.sh from crates/vcad-ffi).
// Derived from the manifest's own location so it stays correct across worktrees
// instead of pinning to one checkout; a shipping build would resolve this via an
// xcframework or an env var.
let ffiLibDir = URL(fileURLWithPath: #filePath)
    .deletingLastPathComponent()
    .appendingPathComponent("Libs")
    .path

let package = Package(
    name: "VcadApp",
    platforms: [.macOS(.v15)],
    targets: [
        // Header + modulemap exposing the Rust static lib to Swift.
        .systemLibrary(name: "CVcadFFI", path: "Sources/CVcadFFI"),
        .executableTarget(
            name: "VcadApp",
            dependencies: ["CVcadFFI"],
            path: "Sources/VcadApp",
            linkerSettings: [
                .unsafeFlags(["-L", ffiLibDir])
            ]
        ),
    ]
)
