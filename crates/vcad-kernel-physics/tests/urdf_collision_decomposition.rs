//! A URDF link's `<collision>` elements are a convex decomposition, and they
//! have to reach the physics layer as *separate* collider shapes.
//!
//! The importer used to collapse every link to a single geometry taken from
//! its first `<visual>`, and the physics layer derived the collider from that
//! one mesh. A cart body approximated by one mesh instead of its 8-piece
//! decomposition gets the wrong contact surface — the pieces exist precisely
//! because the single mesh is a bad collider.
//!
//! The fixture below is shaped like XLeRobot's `base_link` (several visuals,
//! several collisions, interleaved) but built from primitives so it needs no
//! mesh files on disk.

use phyz::Geometry;
use vcad_kernel_physics::PhysicsWorld;
use vcad_kernel_urdf::read_urdf_from_str;

/// `cart`'s visual is one 400×100×100 mm slab; its collision geometry is two
/// 50 mm pads near the ends, with a 250 mm gap between them. Any collapse to a
/// single shape — first visual, union, or convex hull — spans that gap.
/// `plain` has no `<collision>` at all: the overwhelmingly common case, where
/// the part is its own collider.
const CART_URDF: &str = r#"<?xml version="1.0"?>
<robot name="cart">
    <link name="base"/>
    <link name="cart">
        <visual>
            <geometry><box size="0.40 0.10 0.10"/></geometry>
        </visual>
        <collision>
            <origin xyz="-0.15 0 0"/>
            <geometry><box size="0.05 0.05 0.05"/></geometry>
        </collision>
        <visual>
            <origin xyz="0 0 0.10"/>
            <geometry><box size="0.05 0.05 0.05"/></geometry>
        </visual>
        <collision>
            <origin xyz="0.15 0 0"/>
            <geometry><box size="0.05 0.05 0.05"/></geometry>
        </collision>
    </link>
    <link name="plain">
        <visual>
            <geometry><box size="0.20 0.20 0.20"/></geometry>
        </visual>
    </link>
    <joint name="base_to_cart" type="revolute">
        <parent link="base"/>
        <child link="cart"/>
        <origin xyz="0 0 0.5"/>
        <axis xyz="0 0 1"/>
        <limit lower="-1" upper="1" effort="10" velocity="1"/>
    </joint>
    <joint name="cart_to_plain" type="revolute">
        <parent link="cart"/>
        <child link="plain"/>
        <origin xyz="0 0 0.3"/>
        <axis xyz="0 0 1"/>
        <limit lower="-1" upper="1" effort="10" velocity="1"/>
    </joint>
</robot>"#;

/// Axis-aligned extents of a collider shape, in metres.
fn extents(g: &Geometry) -> ([f64; 3], [f64; 3]) {
    let Geometry::Mesh { vertices, .. } = g else {
        panic!("expected a mesh collider, got {g:?}");
    };
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for v in vertices {
        for (i, c) in [v.x, v.y, v.z].into_iter().enumerate() {
            min[i] = min[i].min(c);
            max[i] = max[i].max(c);
        }
    }
    (min, max)
}

fn body<'a>(world: &'a PhysicsWorld, name: &str) -> &'a phyz::Body {
    world
        .model()
        .bodies
        .iter()
        .find(|b| b.name == format!("{name}_inst"))
        .unwrap_or_else(|| panic!("no body for link '{name}'"))
}

#[test]
fn each_collision_element_becomes_its_own_collider_shape() {
    let doc = read_urdf_from_str(CART_URDF).unwrap();
    let world = PhysicsWorld::from_document(&doc).unwrap();
    let cart = body(&world, "cart");

    assert_eq!(
        cart.collisions.len(),
        2,
        "both <collision> pieces must survive as separate shapes; collapsing \
         them to one is what this test exists to catch"
    );

    // Sorted by x so the assertions don't depend on document order.
    let mut shapes: Vec<_> = cart
        .collisions
        .iter()
        .map(|c| extents(&c.geometry))
        .collect();
    shapes.sort_by(|a, b| a.0[0].total_cmp(&b.0[0]));

    for (i, (expect_min_x, expect_max_x)) in [(-0.175, -0.125), (0.125, 0.175)].iter().enumerate() {
        let (min, max) = shapes[i];
        assert!(
            (min[0] - expect_min_x).abs() < 1e-6 && (max[0] - expect_max_x).abs() < 1e-6,
            "piece {i} should span x ∈ [{expect_min_x}, {expect_max_x}], got [{}, {}]",
            min[0],
            max[0]
        );
        assert!(
            max[0] - min[0] < 0.06,
            "piece {i} is {:.3} m wide — it bridges the 250 mm gap, so the \
             decomposition was merged into one shape",
            max[0] - min[0]
        );
    }
}

#[test]
fn collision_geometry_wins_over_visual_for_the_collider() {
    let doc = read_urdf_from_str(CART_URDF).unwrap();
    let world = PhysicsWorld::from_document(&doc).unwrap();
    let cart = body(&world, "cart");

    // The visual slab is 100 mm tall (z ∈ [−0.05, 0.05]); the collision pads
    // are 50 mm (z ∈ [−0.025, 0.025]). Reading z-extent off the collider tells
    // us unambiguously which element it came from.
    for inst in &cart.collisions {
        let (min, max) = extents(&inst.geometry);
        assert!(
            (min[2] - -0.025).abs() < 1e-6 && (max[2] - 0.025).abs() < 1e-6,
            "collider z ∈ [{}, {}] — that is the <visual> slab, not the \
             <collision> pad",
            min[2],
            max[2]
        );
    }

    // `geometry` mirrors the first piece so phyz's single-shape consumers
    // (and its GPU contact pass) still see a collider.
    let primary = cart.geometry.as_ref().expect("primary collider");
    let (min, max) = extents(primary);
    assert!((max[2] - min[2] - 0.05).abs() < 1e-6);
}

#[test]
fn mass_properties_still_come_from_the_parts_own_geometry() {
    // Deliberate: the collision decomposition describes the contact surface,
    // not the material distribution, and its pieces routinely overlap — so
    // summing their volumes would over-count. The 400×100×100 mm slab at the
    // default 1000 kg/m³ is 4 kg; the two 50 mm pads would be 0.25 kg.
    let doc = read_urdf_from_str(CART_URDF).unwrap();
    let world = PhysicsWorld::from_document(&doc).unwrap();
    let mass = body(&world, "cart").inertia.mass;
    assert!(
        (mass - 4.0).abs() < 0.05,
        "expected the visual slab's 4 kg, got {mass} kg"
    );
}

#[test]
fn a_link_without_collisions_is_still_its_own_collider() {
    let doc = read_urdf_from_str(CART_URDF).unwrap();
    let world = PhysicsWorld::from_document(&doc).unwrap();
    let plain = body(&world, "plain");

    assert_eq!(plain.collisions.len(), 1);
    let (min, max) = extents(&plain.collisions[0].geometry);
    for axis in 0..3 {
        assert!(
            (max[axis] - min[axis] - 0.20).abs() < 1e-6,
            "axis {axis}: expected the 200 mm visual cube, got {}",
            max[axis] - min[axis]
        );
    }
}
