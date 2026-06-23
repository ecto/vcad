// swift-tools-version: 6.0
import PackageDescription

// Absolute path to the directory holding libvcad_ffi.a (built by
// scripts/build-ffi.sh from crates/vcad-ffi). Hardcoded for local dev; a real
// build would resolve this via an xcframework or an env var.
let ffiLibDir = "/Users/cam/Developer/vcad/.claude/worktrees/elated-mclaren-925de7/apple/VcadApp/Libs"

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
