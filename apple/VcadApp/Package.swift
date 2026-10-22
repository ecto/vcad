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
    dependencies: [
        // Auto-update (EdDSA-signed appcast at https://vcad.io/desktop/appcast.xml).
        // bundle.sh embeds Sparkle.framework into Contents/Frameworks.
        .package(url: "https://github.com/sparkle-project/Sparkle", from: "2.6.0"),
    ],
    targets: [
        // Pure updater configuration (feed URL + EdDSA key validation). Kept
        // free of the FFI so its rules are unit-testable on their own.
        .target(name: "VcadUpdate", path: "Sources/VcadUpdate"),
        // Header + modulemap exposing the Rust static lib to Swift.
        .systemLibrary(name: "CVcadFFI", path: "Sources/CVcadFFI"),
        // irys's C types (no link: the dylib is loaded at runtime when present).
        .systemLibrary(name: "CIrys", path: "Sources/CIrys"),
        // tang-compress field decoder (C ABI staticlib, built by build-tc-ffi.sh
        // into Libs/ from a tang-compress checkout).
        .systemLibrary(name: "CTangCompress", path: "Sources/CTangCompress"),
        .executableTarget(
            name: "VcadApp",
            dependencies: [
                "CVcadFFI", "CIrys", "CTangCompress", "VcadUpdate",
                .product(name: "Sparkle", package: "Sparkle"),
            ],
            path: "Sources/VcadApp",
            linkerSettings: [
                .unsafeFlags(["-L", ffiLibDir]),
                // Sparkle.framework lives in vcad.app/Contents/Frameworks.
                .unsafeFlags(["-Xlinker", "-rpath", "-Xlinker", "@executable_path/../Frameworks"]),
                .linkedLibrary("c++"),
                .linkedLibrary("lzma"),
            ]
        ),
        // Cross-language parity for the simulation ABI. Reads the SAME golden
        // fixtures the Rust `gym_parity` test records, so a Swift-only failure
        // localizes the bug to the marshalling layer rather than the kernel.
        .testTarget(
            name: "VcadAppTests",
            dependencies: ["VcadApp", "VcadUpdate", "CVcadFFI", "CTangCompress"],
            path: "Tests/VcadAppTests",
            linkerSettings: [
                .unsafeFlags(["-L", ffiLibDir]),
                .linkedLibrary("c++"),
                .linkedLibrary("lzma"),
            ]
        ),
    ]
)
