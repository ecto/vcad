use std::env;

fn main() {
    println!("cargo:rerun-if-changed=clipper");
    if cfg!(feature = "update-bindings") {
        println!("cargo:rerun-if-changed=generated");
    }

    cc::Build::new()
        .cpp(true)
        .opt_level(3)
        .file("clipper/clipper.cpp")
        .file("clipper/wrapper.cpp")
        .compile("clipper");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap();

    match (target_os.as_str(), target_env.as_str()) {
        ("linux", _) | ("windows", "gnu") => println!("cargo:rustc-link-lib=dylib=stdc++"),
        // visionOS/iOS use the same libc++ as macOS (vendored patch for
        // building vcad-ffi against tier-3 Apple targets).
        ("macos", _) | ("ios", _) | ("visionos", _) | ("tvos", _) | ("watchos", _) =>
            println!("cargo:rustc-link-lib=dylib=c++"),
        ("windows", "msvc") => {}
        _ => unimplemented!(
            "target_os: {}, target_env: {}",
            target_os.as_str(),
            target_env.as_str()
        ),
    }

    #[cfg(feature = "generate-bindings")]
    {
        let bindings = bindgen::Builder::default()
            .header("clipper/wrapper.hpp")
            .allowlist_type("Polygons")
            .allowlist_type("ClipType")
            .allowlist_type("JoinType")
            .allowlist_type("EndType")
            .allowlist_type("PolyType")
            .allowlist_type("PolyFillType")
            .allowlist_type("Vertice")
            .allowlist_type("Path")
            .allowlist_type("Polygon")
            .allowlist_function("clean")
            .allowlist_function("simplify")
            .allowlist_function("execute")
            .allowlist_function("offset")
            .allowlist_function("offset_simplify_clean")
            .allowlist_function("free_path")
            .allowlist_function("free_polygon")
            .allowlist_function("free_polygons")
            .size_t_is_usize(true)
            .generate()
            .expect("unable to generate bindings");

        let out_path = if cfg!(feature = "update-bindings") {
            std::path::PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("generated")
        } else {
            std::path::PathBuf::from(env::var("OUT_DIR").unwrap())
        };

        bindings
            .write_to_file(out_path.join("bindings.rs"))
            .expect("couldn't write bindings!");
    }
}
