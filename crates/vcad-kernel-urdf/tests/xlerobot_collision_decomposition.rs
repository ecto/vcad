//! Regression test against the vendored XLeRobot descriptor.
//!
//! `base_link` declares 3 `<visual>` and 8 `<collision>` children, and
//! `Moving_Jaw` 1 and 3 — real convex decompositions, interleaved with the
//! visuals. The importer used to keep only the first `<visual>` per link and
//! drop every `<collision>`, so the physics layer built a cart-sized collider
//! out of one mesh. See `third_party/xlerobot/README.md` for the asset's
//! provenance and its other fidelity caveats.

use std::path::PathBuf;

use vcad_ir::CsgOp;
use vcad_kernel_urdf::read_urdf;

fn xlerobot_urdf() -> PathBuf {
    // CARGO_MANIFEST_DIR is crates/vcad-kernel-urdf.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../third_party/xlerobot/xlerobot.urdf")
}

#[test]
fn base_link_keeps_all_eight_collision_pieces() {
    let doc = read_urdf(xlerobot_urdf()).expect("vendored XLeRobot URDF must import");
    let part_defs = doc.part_defs.as_ref().expect("part defs");

    for (link, expected) in [("base_link", 8), ("Moving_Jaw", 3)] {
        let part = &part_defs[&format!("part_{link}")];
        let colliders = part
            .colliders
            .as_ref()
            .unwrap_or_else(|| panic!("{link} declares <collision> children but authored none"));
        assert_eq!(
            colliders.len(),
            expected,
            "{link} should carry all {expected} <collision> pieces, got {}",
            colliders.len()
        );

        // Distinct, resolvable, and none of them is the rendered root — the
        // link's first <visual> is.
        let unique: std::collections::HashSet<_> = colliders.iter().collect();
        assert_eq!(unique.len(), colliders.len(), "{link}: duplicate roots");
        for root in colliders {
            assert!(doc.nodes.contains_key(root), "{link}: dangling root {root}");
        }
        assert!(
            !colliders.contains(&part.root),
            "{link}: a collision piece is doubling as the render root even \
             though the link has a <visual>"
        );
    }
}

#[test]
fn collision_pieces_resolve_to_their_own_meshes() {
    // XLeRobot's decomposition is `.ply` convex parts distinct from the `.stl`
    // the link renders as — so a piece pointing at the visual mesh would mean
    // the collider silently fell back to <visual>.
    let doc = read_urdf(xlerobot_urdf()).unwrap();
    let part_defs = doc.part_defs.as_ref().unwrap();
    let jaw = &part_defs["part_Moving_Jaw"];

    let mesh_path = |root: &u64| -> String {
        let mut id = *root;
        // Walk down through the origin Translate/Rotate wrappers to the leaf.
        loop {
            match &doc.nodes[&id].op {
                CsgOp::Translate { child, .. } | CsgOp::Rotate { child, .. } => id = *child,
                CsgOp::MeshImport { path, .. } => return path.clone(),
                other => panic!("unexpected leaf op {other:?}"),
            }
        }
    };

    let pieces: Vec<String> = jaw
        .colliders
        .as_ref()
        .unwrap()
        .iter()
        .map(mesh_path)
        .collect();
    assert_eq!(pieces.len(), 3);
    for (i, p) in pieces.iter().enumerate() {
        assert!(
            p.contains("Moving_Jaw_part"),
            "piece {i} should be a decomposition part, got {p:?}"
        );
    }
    // All three are different parts, not the same file three times.
    let unique: std::collections::HashSet<_> = pieces.iter().collect();
    assert_eq!(
        unique.len(),
        3,
        "expected 3 distinct part meshes: {pieces:?}"
    );

    // ...and the rendered root is the whole-jaw STL, not a decomposition part.
    let rendered = mesh_path(&jaw.root);
    assert!(
        rendered.contains("Moving_Jaw.STL") || rendered.contains("Moving_Jaw.stl"),
        "render root should be the visual mesh, got {rendered:?}"
    );
}
