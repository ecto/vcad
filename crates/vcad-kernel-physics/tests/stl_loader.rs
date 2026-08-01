//! Round-trip a synthetic STL through the loader to make sure the unit
//! conversion (metres → millimetres) and scale handling are correct.

use std::io::Write;

use vcad_ir::Vec3;
use vcad_kernel_physics::stl::load_stl;

/// Write a minimal binary STL containing a single triangle whose vertices
/// span 1 metre along each axis. Returns the temp file path. The `tag`
/// argument keeps parallel-running tests from racing on the same path.
fn write_unit_triangle_stl(tag: &str) -> std::path::PathBuf {
    let mut buf: Vec<u8> = Vec::new();
    // 80-byte header
    buf.extend_from_slice(&[0u8; 80]);
    // u32 triangle count
    buf.write_all(&1u32.to_le_bytes()).unwrap();
    // normal + 3 vertices + attribute byte count
    let normal = [0.0f32, 0.0, 1.0];
    let v0 = [0.0f32, 0.0, 0.0];
    let v1 = [1.0f32, 0.0, 0.0];
    let v2 = [0.0f32, 1.0, 0.0];
    for v in [normal, v0, v1, v2] {
        for c in v {
            buf.write_all(&c.to_le_bytes()).unwrap();
        }
    }
    buf.write_all(&0u16.to_le_bytes()).unwrap();

    let mut path = std::env::temp_dir();
    path.push(format!("vcad-stl-test-{}-{tag}.stl", std::process::id()));
    std::fs::write(&path, &buf).unwrap();
    path
}

#[test]
fn load_stl_metres_to_millimetres() {
    let path = write_unit_triangle_stl("mm");
    let mesh = load_stl(&path, None).expect("load STL");
    std::fs::remove_file(&path).ok();

    assert_eq!(mesh.num_triangles(), 1, "expected 1 triangle");
    assert_eq!(mesh.num_vertices(), 3, "expected 3 vertices");
    // 1 metre in URDF should land at 1000 mm in IR.
    let max_x = mesh
        .vertices
        .chunks(3)
        .map(|v| v[0])
        .fold(f32::NEG_INFINITY, f32::max);
    assert!(
        (max_x - 1000.0).abs() < 1e-3,
        "max x should be 1000mm, got {max_x}"
    );
}

#[test]
fn load_stl_applies_scale() {
    let path = write_unit_triangle_stl("scale");
    let mesh = load_stl(&path, Some(Vec3::new(2.0, 1.0, 1.0))).expect("load STL");
    std::fs::remove_file(&path).ok();

    // 1 metre × 2.0 scale × 1000 mm/m = 2000 mm
    let max_x = mesh
        .vertices
        .chunks(3)
        .map(|v| v[0])
        .fold(f32::NEG_INFINITY, f32::max);
    assert!(
        (max_x - 2000.0).abs() < 1e-3,
        "scaled x should be 2000mm, got {max_x}"
    );
}

/// End-to-end: a URDF with `package://demo/foo.stl` resolves through the
/// reader and the resulting CsgOp::MeshImport carries an absolute path.
#[test]
fn urdf_resolves_package_uri() {
    use vcad_kernel_urdf::{read_urdf_from_str_with_options, UrdfReadOptions};

    let tmp = std::env::temp_dir().join(format!("vcad-pkg-test-{}", std::process::id()));
    let pkg_dir = tmp.join("demo");
    std::fs::create_dir_all(&pkg_dir).unwrap();
    let stl_path = pkg_dir.join("foo.stl");
    std::fs::write(
        &stl_path,
        std::fs::read(write_unit_triangle_stl("pkg")).unwrap(),
    )
    .unwrap();

    let urdf = r#"<?xml version="1.0"?>
<robot name="t">
  <link name="root">
    <visual>
      <geometry><mesh filename="package://demo/foo.stl"/></geometry>
    </visual>
  </link>
</robot>"#;

    let opts = UrdfReadOptions {
        package_roots: vec![tmp.clone()],
        urdf_dir: None,
        ..UrdfReadOptions::default()
    };
    let doc = read_urdf_from_str_with_options(urdf, &opts).expect("parse URDF");

    let part_defs = doc.part_defs.expect("has part_defs");
    let part = part_defs.get("part_root").expect("part_root exists");
    let nodes = doc.nodes;
    let mesh_node = nodes
        .values()
        .find(|n| matches!(n.op, vcad_ir::CsgOp::MeshImport { .. }))
        .expect("MeshImport node not found");

    if let vcad_ir::CsgOp::MeshImport { path, .. } = &mesh_node.op {
        assert!(
            path.contains("foo.stl"),
            "resolved path should reference foo.stl, got {path}"
        );
        assert!(
            std::path::Path::new(path).is_absolute(),
            "path must be absolute, got {path}"
        );
    }
    let _ = part; // silence unused warning when assertions trim out

    std::fs::remove_dir_all(&tmp).ok();
}
