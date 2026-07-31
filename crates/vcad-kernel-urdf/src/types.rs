//! Internal URDF data structures.
//!
//! These types mirror the URDF XML schema for parsing.

use serde::{Deserialize, Deserializer, Serialize};

/// Deserialize an `f64` attribute, tolerating surrounding whitespace
/// (real-world URDFs, e.g. Booster K1, emit values like `" -0.000001"`).
fn ws_f64<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    let s = String::deserialize(d)?;
    s.trim().parse().map_err(serde::de::Error::custom)
}

/// Like [`ws_f64`] but for optional attributes.
fn ws_f64_opt<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
    let s = Option::<String>::deserialize(d)?;
    s.map(|s| s.trim().parse().map_err(serde::de::Error::custom))
        .transpose()
}

/// Root URDF robot element.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename = "robot")]
pub struct Robot {
    /// Robot name.
    #[serde(rename = "@name")]
    pub name: String,

    /// Links in the robot.
    #[serde(rename = "link", default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<Link>,

    /// Joints connecting links.
    #[serde(rename = "joint", default, skip_serializing_if = "Vec::is_empty")]
    pub joints: Vec<Joint>,

    /// Material definitions.
    #[serde(rename = "material", default, skip_serializing_if = "Vec::is_empty")]
    pub materials: Vec<Material>,
}

/// A rigid body in the robot.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Link {
    /// Link name (unique identifier).
    #[serde(rename = "@name")]
    pub name: String,

    /// Visual geometries. URDF allows multiple `<visual>` children per
    /// link (e.g. one per sub-mesh); the importer treats the first as
    /// the primary visual when picking geometry to evaluate.
    #[serde(rename = "visual", default, skip_serializing_if = "Vec::is_empty")]
    pub visuals: Vec<Visual>,

    /// Collision geometries. Same multi-element story as `visuals`.
    #[serde(rename = "collision", default, skip_serializing_if = "Vec::is_empty")]
    pub collisions: Vec<Collision>,

    /// Inertial properties.
    #[serde(rename = "inertial", skip_serializing_if = "Option::is_none")]
    pub inertial: Option<Inertial>,
}

/// Visual representation of a link.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Visual {
    /// Optional name.
    #[serde(rename = "@name", skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Origin transform.
    #[serde(rename = "origin", skip_serializing_if = "Option::is_none")]
    pub origin: Option<Origin>,

    /// Geometry shape.
    #[serde(rename = "geometry")]
    pub geometry: Geometry,

    /// Material references. Standard URDF allows a single `<material>` per
    /// `<visual>`, but Unitree's Go2 URDF (and a handful of other
    /// real-world descriptors) ship multiple — vcad accepts the loose
    /// form and uses the first as the primary.
    #[serde(rename = "material", default, skip_serializing_if = "Vec::is_empty")]
    pub materials: Vec<MaterialRef>,
}

/// Collision geometry of a link.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Collision {
    /// Optional name.
    #[serde(rename = "@name", skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Origin transform.
    #[serde(rename = "origin", skip_serializing_if = "Option::is_none")]
    pub origin: Option<Origin>,

    /// Geometry shape.
    #[serde(rename = "geometry")]
    pub geometry: Geometry,
}

/// Inertial properties of a link.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Inertial {
    /// Origin of the inertial frame.
    #[serde(rename = "origin", skip_serializing_if = "Option::is_none")]
    pub origin: Option<Origin>,

    /// Mass in kg.
    #[serde(rename = "mass")]
    pub mass: Mass,

    /// Inertia tensor.
    #[serde(rename = "inertia")]
    pub inertia: Inertia,
}

/// Mass element.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Mass {
    /// Mass value in kg.
    #[serde(rename = "@value", deserialize_with = "ws_f64")]
    pub value: f64,
}

/// Inertia tensor (symmetric 3x3 matrix).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Inertia {
    /// Ixx component.
    #[serde(rename = "@ixx", deserialize_with = "ws_f64")]
    pub ixx: f64,
    /// Ixy component.
    #[serde(rename = "@ixy", deserialize_with = "ws_f64")]
    pub ixy: f64,
    /// Ixz component.
    #[serde(rename = "@ixz", deserialize_with = "ws_f64")]
    pub ixz: f64,
    /// Iyy component.
    #[serde(rename = "@iyy", deserialize_with = "ws_f64")]
    pub iyy: f64,
    /// Iyz component.
    #[serde(rename = "@iyz", deserialize_with = "ws_f64")]
    pub iyz: f64,
    /// Izz component.
    #[serde(rename = "@izz", deserialize_with = "ws_f64")]
    pub izz: f64,
}

/// Origin transform (position and rotation).
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Origin {
    /// XYZ position.
    #[serde(rename = "@xyz", default, skip_serializing_if = "Option::is_none")]
    pub xyz: Option<String>,

    /// RPY rotation (roll-pitch-yaw in radians).
    #[serde(rename = "@rpy", default, skip_serializing_if = "Option::is_none")]
    pub rpy: Option<String>,
}

impl Origin {
    /// Parse XYZ as [x, y, z] in meters.
    pub fn xyz_vec(&self) -> [f64; 3] {
        self.xyz
            .as_ref()
            .map(|s| parse_xyz(s))
            .unwrap_or([0.0, 0.0, 0.0])
    }

    /// Parse RPY as [roll, pitch, yaw] in radians.
    pub fn rpy_vec(&self) -> [f64; 3] {
        self.rpy
            .as_ref()
            .map(|s| parse_xyz(s))
            .unwrap_or([0.0, 0.0, 0.0])
    }
}

/// Parse a space-separated XYZ/RPY string into an array.
fn parse_xyz(s: &str) -> [f64; 3] {
    let parts: Vec<f64> = s
        .split_whitespace()
        .filter_map(|p| p.parse().ok())
        .collect();
    if parts.len() >= 3 {
        [parts[0], parts[1], parts[2]]
    } else {
        [0.0, 0.0, 0.0]
    }
}

/// Geometry element (one of box, cylinder, sphere, or mesh).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Geometry {
    /// Box geometry.
    #[serde(rename = "box", skip_serializing_if = "Option::is_none")]
    pub box_geom: Option<BoxGeom>,

    /// Cylinder geometry.
    #[serde(rename = "cylinder", skip_serializing_if = "Option::is_none")]
    pub cylinder: Option<CylinderGeom>,

    /// Sphere geometry.
    #[serde(rename = "sphere", skip_serializing_if = "Option::is_none")]
    pub sphere: Option<SphereGeom>,

    /// Mesh geometry.
    #[serde(rename = "mesh", skip_serializing_if = "Option::is_none")]
    pub mesh: Option<MeshGeom>,
}

/// Box geometry.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BoxGeom {
    /// Size as "x y z".
    #[serde(rename = "@size")]
    pub size: String,
}

impl BoxGeom {
    /// Parse size into [x, y, z] dimensions in meters.
    pub fn size_vec(&self) -> [f64; 3] {
        parse_xyz(&self.size)
    }
}

/// Cylinder geometry (along Z axis).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CylinderGeom {
    /// Radius in meters.
    #[serde(rename = "@radius", deserialize_with = "ws_f64")]
    pub radius: f64,

    /// Length (height) in meters.
    #[serde(rename = "@length", deserialize_with = "ws_f64")]
    pub length: f64,
}

/// Sphere geometry.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SphereGeom {
    /// Radius in meters.
    #[serde(rename = "@radius", deserialize_with = "ws_f64")]
    pub radius: f64,
}

/// Mesh geometry reference.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MeshGeom {
    /// Path to mesh file (STL, DAE, etc.).
    #[serde(rename = "@filename")]
    pub filename: String,

    /// Optional scale as "x y z".
    #[serde(rename = "@scale", skip_serializing_if = "Option::is_none")]
    pub scale: Option<String>,
}

/// Material reference or inline definition.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MaterialRef {
    /// Material name (reference to top-level material).
    #[serde(rename = "@name", skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Inline color definition.
    #[serde(rename = "color", skip_serializing_if = "Option::is_none")]
    pub color: Option<Color>,

    /// Inline texture reference.
    #[serde(rename = "texture", skip_serializing_if = "Option::is_none")]
    pub texture: Option<Texture>,
}

/// Top-level material definition.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Material {
    /// Material name.
    #[serde(rename = "@name")]
    pub name: String,

    /// Color.
    #[serde(rename = "color", skip_serializing_if = "Option::is_none")]
    pub color: Option<Color>,

    /// Texture.
    #[serde(rename = "texture", skip_serializing_if = "Option::is_none")]
    pub texture: Option<Texture>,
}

/// RGBA color.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Color {
    /// RGBA as "r g b a" (0.0-1.0).
    #[serde(rename = "@rgba")]
    pub rgba: String,
}

impl Color {
    /// Parse RGBA into [r, g, b, a].
    pub fn rgba_vec(&self) -> [f64; 4] {
        let parts: Vec<f64> = self
            .rgba
            .split_whitespace()
            .filter_map(|p| p.parse().ok())
            .collect();
        if parts.len() >= 4 {
            [parts[0], parts[1], parts[2], parts[3]]
        } else if parts.len() >= 3 {
            [parts[0], parts[1], parts[2], 1.0]
        } else {
            [0.5, 0.5, 0.5, 1.0]
        }
    }
}

/// Texture reference.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Texture {
    /// Path to texture file.
    #[serde(rename = "@filename")]
    pub filename: String,
}

/// Joint connecting two links.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Joint {
    /// Joint name.
    #[serde(rename = "@name")]
    pub name: String,

    /// Joint type: fixed, revolute, continuous, prismatic, floating, planar.
    #[serde(rename = "@type")]
    pub joint_type: String,

    /// Origin transform from parent to joint frame.
    #[serde(rename = "origin", skip_serializing_if = "Option::is_none")]
    pub origin: Option<Origin>,

    /// Parent link reference.
    #[serde(rename = "parent")]
    pub parent: ParentLink,

    /// Child link reference.
    #[serde(rename = "child")]
    pub child: ChildLink,

    /// Joint axis (for revolute/prismatic).
    #[serde(rename = "axis", skip_serializing_if = "Option::is_none")]
    pub axis: Option<Axis>,

    /// Joint limits (for revolute/prismatic).
    #[serde(rename = "limit", skip_serializing_if = "Option::is_none")]
    pub limit: Option<Limit>,

    /// Dynamics (friction, damping).
    #[serde(rename = "dynamics", skip_serializing_if = "Option::is_none")]
    pub dynamics: Option<Dynamics>,
}

/// Parent link reference.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ParentLink {
    /// Link name.
    #[serde(rename = "@link")]
    pub link: String,
}

/// Child link reference.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChildLink {
    /// Link name.
    #[serde(rename = "@link")]
    pub link: String,
}

/// Joint axis.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Axis {
    /// Axis direction as "x y z".
    #[serde(rename = "@xyz")]
    pub xyz: String,
}

impl Axis {
    /// Parse axis into [x, y, z] unit vector.
    pub fn xyz_vec(&self) -> [f64; 3] {
        parse_xyz(&self.xyz)
    }
}

/// Joint limits.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Limit {
    /// Lower limit (radians for revolute, meters for prismatic).
    #[serde(rename = "@lower", default, deserialize_with = "ws_f64_opt", skip_serializing_if = "Option::is_none")]
    pub lower: Option<f64>,

    /// Upper limit.
    #[serde(rename = "@upper", default, deserialize_with = "ws_f64_opt", skip_serializing_if = "Option::is_none")]
    pub upper: Option<f64>,

    /// Maximum effort (Nm for revolute, N for prismatic).
    #[serde(rename = "@effort", default, deserialize_with = "ws_f64_opt", skip_serializing_if = "Option::is_none")]
    pub effort: Option<f64>,

    /// Maximum velocity (rad/s for revolute, m/s for prismatic).
    #[serde(rename = "@velocity", default, deserialize_with = "ws_f64_opt", skip_serializing_if = "Option::is_none")]
    pub velocity: Option<f64>,
}

/// Joint dynamics.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Dynamics {
    /// Viscous damping coefficient.
    #[serde(rename = "@damping", default, deserialize_with = "ws_f64_opt", skip_serializing_if = "Option::is_none")]
    pub damping: Option<f64>,

    /// Static friction (Coulomb).
    #[serde(rename = "@friction", default, deserialize_with = "ws_f64_opt", skip_serializing_if = "Option::is_none")]
    pub friction: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_origin_xyz() {
        let origin = Origin {
            xyz: Some("1.0 2.0 3.0".to_string()),
            rpy: None,
        };
        assert_eq!(origin.xyz_vec(), [1.0, 2.0, 3.0]);
    }

    #[test]
    fn parse_origin_rpy() {
        let origin = Origin {
            xyz: None,
            rpy: Some("0.1 0.2 0.3".to_string()),
        };
        assert_eq!(origin.rpy_vec(), [0.1, 0.2, 0.3]);
    }

    #[test]
    fn parse_color_rgba() {
        let color = Color {
            rgba: "1.0 0.5 0.0 0.8".to_string(),
        };
        assert_eq!(color.rgba_vec(), [1.0, 0.5, 0.0, 0.8]);
    }

    #[test]
    fn parse_color_rgb_default_alpha() {
        let color = Color {
            rgba: "1.0 0.5 0.0".to_string(),
        };
        assert_eq!(color.rgba_vec(), [1.0, 0.5, 0.0, 1.0]);
    }
}
