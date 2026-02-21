//! USD/USDA export for Isaac Sim.
//!
//! Generates ASCII USD files with physics schemas for simulation.
//! Compatible with NVIDIA Isaac Sim and Omniverse.

use super::materials::Material;
use crate::{CadError, Part};
use std::path::Path;

/// Export a single part to USD format with physics.
pub fn export_usd(
    part: &Part,
    material: &Material,
    path: impl AsRef<Path>,
) -> Result<(), CadError> {
    let usda = generate_part_usda(part, material)?;
    std::fs::write(path, usda)?;
    Ok(())
}

/// Physics properties for the robot body in USD export.
#[derive(Debug, Clone)]
pub struct RobotPhysics {
    /// Mass of the robot body in kg
    pub mass: f64,
    /// Center of mass [x, y, z] in meters
    pub center_of_mass: [f64; 3],
}

impl Default for RobotPhysics {
    fn default() -> Self {
        Self {
            mass: 25.0,
            center_of_mass: [0.0, 0.15, 0.0],
        }
    }
}

/// Export a complete robot assembly with articulations.
///
/// - `robot_name`: Root prim name (e.g. `"BVR1"`)
/// - `body_material_key`: Key to look up body material in the materials database
/// - `physics`: Robot body physics (mass, center of mass)
pub fn export_robot_usd(
    body: &Part,
    wheels: &[(Part, WheelConfig)],
    materials: &super::Materials,
    path: impl AsRef<Path>,
    robot_name: &str,
    body_material_key: &str,
    physics: &RobotPhysics,
) -> Result<(), CadError> {
    let usda = generate_robot_usda(
        body,
        wheels,
        materials,
        robot_name,
        body_material_key,
        physics,
    )?;
    std::fs::write(path, usda)?;
    Ok(())
}

/// Configuration for a wheel joint.
#[derive(Debug, Clone)]
pub struct WheelConfig {
    /// Wheel name
    pub name: String,
    /// Position relative to body [x, y, z] in meters
    pub position: [f64; 3],
    /// Rotation axis (typically [1, 0, 0] or [0, 1, 0])
    pub axis: [f64; 3],
    /// Maximum drive velocity (rad/s)
    pub max_velocity: f64,
    /// Maximum drive torque (N·m)
    pub max_torque: f64,
}

impl Default for WheelConfig {
    fn default() -> Self {
        Self {
            name: "wheel".to_string(),
            position: [0.0, 0.0, 0.0],
            axis: [1.0, 0.0, 0.0],
            max_velocity: 100.0,
            max_torque: 50.0,
        }
    }
}

fn generate_part_usda(part: &Part, material: &Material) -> Result<String, CadError> {
    let mesh = part.to_mesh();
    let vertices = mesh.vertices();
    let indices = mesh.indices();

    if vertices.is_empty() || indices.is_empty() {
        return Err(CadError::EmptyGeometry);
    }

    // Convert vertices to USD format (array of Vec3f)
    let vertex_count = vertices.len() / 3;
    let mut points = Vec::with_capacity(vertex_count);
    for i in 0..vertex_count {
        points.push(format!(
            "({}, {}, {})",
            vertices[i * 3],
            vertices[i * 3 + 1],
            vertices[i * 3 + 2]
        ));
    }

    // Convert indices to face vertex counts and indices
    let face_count = indices.len() / 3;
    let face_vertex_counts: Vec<String> = (0..face_count).map(|_| "3".to_string()).collect();
    let face_vertex_indices: Vec<String> = indices.iter().map(|i| i.to_string()).collect();

    // Calculate mass from volume and density
    // Approximate volume using bounding box (simplified)
    let mut min = [f32::MAX; 3];
    let mut max = [f32::MIN; 3];
    for i in 0..vertex_count {
        for j in 0..3 {
            let v = vertices[i * 3 + j];
            min[j] = min[j].min(v);
            max[j] = max[j].max(v);
        }
    }
    let size = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
    // Convert from mm to m for volume calculation
    let volume_m3 = (size[0] * size[1] * size[2]) as f64 / 1e9;
    let mass_kg = volume_m3 * material.density as f64;

    let usda = format!(
        r#"#usda 1.0
(
    defaultPrim = "{name}"
    upAxis = "Z"
    metersPerUnit = 0.001
)

def Xform "{name}" (
    prepend apiSchemas = ["PhysicsRigidBodyAPI", "PhysicsMassAPI"]
)
{{
    float physics:mass = {mass}

    def Mesh "{name}_mesh"
    {{
        float3[] extent = [({min0}, {min1}, {min2}), ({max0}, {max1}, {max2})]
        int[] faceVertexCounts = [{face_counts}]
        int[] faceVertexIndices = [{face_indices}]
        point3f[] points = [{points}]

        rel material:binding = </{name}/Material>
    }}

    def Scope "Material"
    {{
        def Material "PBR"
        {{
            token outputs:surface.connect = <PBR/Shader.outputs:surface>

            def Shader "Shader"
            {{
                uniform token info:id = "UsdPreviewSurface"
                color3f inputs:diffuseColor = ({r}, {g}, {b})
                float inputs:metallic = {metallic}
                float inputs:roughness = {roughness}
                token outputs:surface
            }}
        }}
    }}

    def PhysicsCollisionAPI "collision"
    {{
        rel physics:collisionGroup = </CollisionGroup>
    }}
}}
"#,
        name = sanitize_usd_name(&part.name),
        mass = mass_kg.max(0.001),
        min0 = min[0],
        min1 = min[1],
        min2 = min[2],
        max0 = max[0],
        max1 = max[1],
        max2 = max[2],
        face_counts = face_vertex_counts.join(", "),
        face_indices = face_vertex_indices.join(", "),
        points = points.join(", "),
        r = material.color[0],
        g = material.color[1],
        b = material.color[2],
        metallic = material.metallic,
        roughness = material.roughness,
    );

    Ok(usda)
}

fn generate_robot_usda(
    body: &Part,
    wheels: &[(Part, WheelConfig)],
    materials: &super::Materials,
    robot_name: &str,
    body_material_key: &str,
    physics: &RobotPhysics,
) -> Result<String, CadError> {
    let body_mesh = body.to_mesh();
    let body_vertices = body_mesh.vertices();
    let body_indices = body_mesh.indices();

    if body_vertices.is_empty() {
        return Err(CadError::EmptyGeometry);
    }

    // Format body mesh data
    let body_vertex_count = body_vertices.len() / 3;
    let mut body_points = Vec::with_capacity(body_vertex_count);
    for i in 0..body_vertex_count {
        body_points.push(format!(
            "({}, {}, {})",
            body_vertices[i * 3],
            body_vertices[i * 3 + 1],
            body_vertices[i * 3 + 2]
        ));
    }

    let body_face_count = body_indices.len() / 3;
    let body_face_counts: Vec<String> = (0..body_face_count).map(|_| "3".to_string()).collect();
    let body_face_indices: Vec<String> = body_indices.iter().map(|i| i.to_string()).collect();

    let rn = sanitize_usd_name(robot_name);

    // Get body material
    let body_mat = materials.get_for_part_or_default(body_material_key);

    // Build wheel definitions
    let mut wheel_defs = String::new();
    for (wheel_part, config) in wheels {
        let wheel_usda = generate_wheel_def(wheel_part, config, materials, &rn)?;
        wheel_defs.push_str(&wheel_usda);
    }

    let [com_x, com_y, com_z] = physics.center_of_mass;

    let usda = format!(
        r#"#usda 1.0
(
    defaultPrim = "{rn}"
    upAxis = "Z"
    metersPerUnit = 0.001
    doc = "{rn} Robot - Municipal Robotics"
)

def PhysicsScene "PhysicsScene"
{{
    vector3f physics:gravityDirection = (0, 0, -1)
    float physics:gravityMagnitude = 9.81
}}

def Xform "{rn}" (
    prepend apiSchemas = ["PhysicsArticulationRootAPI"]
)
{{
    def Xform "Body" (
        prepend apiSchemas = ["PhysicsRigidBodyAPI", "PhysicsMassAPI"]
    )
    {{
        float physics:mass = {mass}
        point3f physics:centerOfMass = ({com_x}, {com_y}, {com_z})

        def Mesh "BodyMesh"
        {{
            int[] faceVertexCounts = [{body_face_counts}]
            int[] faceVertexIndices = [{body_face_indices}]
            point3f[] points = [{body_points}]

            rel material:binding = </{rn}/Materials/BodyMaterial>
        }}
    }}

{wheel_defs}

    def Scope "Materials"
    {{
        def Material "BodyMaterial"
        {{
            token outputs:surface.connect = <BodyMaterial/Shader.outputs:surface>

            def Shader "Shader"
            {{
                uniform token info:id = "UsdPreviewSurface"
                color3f inputs:diffuseColor = ({body_r}, {body_g}, {body_b})
                float inputs:metallic = {body_metallic}
                float inputs:roughness = {body_roughness}
                token outputs:surface
            }}
        }}

        def Material "WheelMaterial"
        {{
            token outputs:surface.connect = <WheelMaterial/Shader.outputs:surface>

            def Shader "Shader"
            {{
                uniform token info:id = "UsdPreviewSurface"
                color3f inputs:diffuseColor = (0.1, 0.1, 0.1)
                float inputs:metallic = 0.0
                float inputs:roughness = 0.9
                token outputs:surface
            }}
        }}
    }}
}}
"#,
        rn = rn,
        mass = physics.mass,
        com_x = com_x,
        com_y = com_y,
        com_z = com_z,
        body_face_counts = body_face_counts.join(", "),
        body_face_indices = body_face_indices.join(", "),
        body_points = body_points.join(", "),
        wheel_defs = wheel_defs,
        body_r = body_mat.color[0],
        body_g = body_mat.color[1],
        body_b = body_mat.color[2],
        body_metallic = body_mat.metallic,
        body_roughness = body_mat.roughness,
    );

    Ok(usda)
}

fn generate_wheel_def(
    wheel: &Part,
    config: &WheelConfig,
    _materials: &super::Materials,
    robot_name: &str,
) -> Result<String, CadError> {
    let mesh = wheel.to_mesh();
    let vertices = mesh.vertices();
    let indices = mesh.indices();

    if vertices.is_empty() {
        return Err(CadError::EmptyGeometry);
    }

    let vertex_count = vertices.len() / 3;
    let mut points = Vec::with_capacity(vertex_count);
    for i in 0..vertex_count {
        points.push(format!(
            "({}, {}, {})",
            vertices[i * 3],
            vertices[i * 3 + 1],
            vertices[i * 3 + 2]
        ));
    }

    let face_count = indices.len() / 3;
    let face_counts: Vec<String> = (0..face_count).map(|_| "3".to_string()).collect();
    let face_indices: Vec<String> = indices.iter().map(|i| i.to_string()).collect();

    let name = sanitize_usd_name(&config.name);
    let [px, py, pz] = config.position;
    let [ax, ay, az] = config.axis;

    Ok(format!(
        r#"
    def Xform "{name}" (
        prepend apiSchemas = ["PhysicsRigidBodyAPI", "PhysicsMassAPI"]
    )
    {{
        double3 xformOp:translate = ({px}, {py}, {pz})
        uniform token[] xformOpOrder = ["xformOp:translate"]

        float physics:mass = 2.0

        def PhysicsRevoluteJoint "{name}Joint"
        {{
            rel physics:body0 = </{robot_name}/Body>
            rel physics:body1 = </{robot_name}/{name}>
            float3 physics:localPos0 = ({px}, {py}, {pz})
            float3 physics:localPos1 = (0, 0, 0)
            float3 physics:axis = ({ax}, {ay}, {az})

            float drive:angular:physics:damping = 10.0
            float drive:angular:physics:stiffness = 0.0
            float drive:angular:physics:maxForce = {max_torque}
            token drive:angular:physics:type = "force"
        }}

        def Mesh "{name}Mesh"
        {{
            int[] faceVertexCounts = [{face_counts}]
            int[] faceVertexIndices = [{face_indices}]
            point3f[] points = [{points}]

            rel material:binding = </{robot_name}/Materials/WheelMaterial>
        }}
    }}
"#,
        name = name,
        robot_name = robot_name,
        px = px * 1000.0, // Convert m to mm
        py = py * 1000.0,
        pz = pz * 1000.0,
        ax = ax,
        ay = ay,
        az = az,
        max_torque = config.max_torque,
        face_counts = face_counts.join(", "),
        face_indices = face_indices.join(", "),
        points = points.join(", "),
    ))
}

fn sanitize_usd_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_name() {
        assert_eq!(sanitize_usd_name("my-part.stl"), "my_part_stl");
        assert_eq!(sanitize_usd_name("Part_123"), "Part_123");
    }
}
