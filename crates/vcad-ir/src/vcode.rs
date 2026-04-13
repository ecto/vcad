//! VCode text-based IR format for cad0 model training and inference.
//!
//! This format is designed to be:
//! - Token-efficient (short opcodes, minimal punctuation)
//! - Unambiguous (line-based, implicit node IDs)
//! - Easy to parse and generate
//!
//! # Format v0.2
//!
//! ## Header
//! ```text
//! # vcad 0.2
//! ```
//!
//! ## Materials
//! ```text
//! M name r g b metallic roughness [density] [friction]
//! ```
//!
//! ## Geometry (line number = node ID, optional quoted name at end)
//! ```text
//! C sx sy sz ["name"]           # Cube
//! Y r h ["name"]                # Cylinder
//! S r ["name"]                  # Sphere
//! K rb rt h ["name"]            # Cone
//! U a b ["name"]                # Union
//! D a b ["name"]                # Difference
//! I a b ["name"]                # Intersection
//! T n dx dy dz ["name"]         # Translate
//! R n rx ry rz ["name"]         # Rotate (degrees)
//! X n sx sy sz ["name"]         # Scale
//! LP n dx dy dz count spacing ["name"]  # Linear pattern
//! CP n ox oy oz ax ay az count angle ["name"]  # Circular pattern
//! SH n thickness ["name"]       # Shell
//! FI n radius ["name"]          # Fillet
//! CH n distance ["name"]        # Chamfer
//! ```
//!
//! ## Sketch (block)
//! ```text
//! SK ox oy oz  xx xy xz  yx yy yz ["name"]
//! L x1 y1 x2 y2                 # Line segment
//! A x1 y1 x2 y2 cx cy ccw       # Arc (ccw: 0 or 1)
//! END
//! E sk dx dy dz ["name"]        # Extrude
//! V sk ox oy oz ax ay az angle ["name"]  # Revolve
//! ```
//!
//! ## Scene roots
//! ```text
//! ROOT nodeId material [hidden]
//! ```
//!
//! ## Assembly
//! ```text
//! PDEF id "name" rootNodeId [material]
//! INST id partDefId "name" tx ty tz rx ry rz sx sy sz [material]
//! JFIX id parentInst childInst px py pz cx cy cz
//! JREV id parentInst childInst px py pz cx cy cz ax ay az [min max]
//! JSLD id parentInst childInst px py pz cx cy cz ax ay az [min max]
//! JCYL id parentInst childInst px py pz cx cy cz ax ay az
//! JBAL id parentInst childInst px py pz cx cy cz
//! GROUND instanceId
//! ```
//!
//! ## Scene settings
//! ```text
//! ENV preset intensity                # Environment preset
//! ENV url intensity                   # Custom HDR
//! BG solid r g b                      # Solid background
//! BG gradient r1 g1 b1 r2 g2 b2      # Gradient background
//! BG env                              # Environment background
//! BG transparent                      # Transparent background
//! LDIR id r g b intensity dx dy dz [shadow]    # Directional light
//! LPNT id r g b intensity px py pz [distance]  # Point light
//! LSPT id r g b intensity px py pz dx dy dz [angle] [penumbra]  # Spot light
//! LAREA id r g b intensity px py pz dx dy dz w h  # Area light
//! AO enabled intensity radius         # Ambient occlusion
//! BLOOM enabled intensity threshold   # Bloom effect
//! VIG enabled offset darkness         # Vignette effect
//! TONE mapping                        # Tone mapping
//! EXP value                           # Exposure
//! CAM id px py pz tx ty tz [fov] ["name"]  # Camera preset
//! ```
//!
//! # Example
//!
//! A 50x30x5mm plate with a 10mm diameter hole in the center:
//!
//! ```text
//! # vcad 0.2
//! M default 0.8 0.8 0.8 0 0.5
//! C 50 30 5 "Base Plate"
//! Y 5 10 "Hole"
//! T 1 25 15 0
//! D 0 2 "Plate with Hole"
//! ROOT 3 default
//! ```

use crate::{
    AmbientOcclusion, Background, Bloom, CameraPreset, CsgOp, Document, Environment,
    EnvironmentPreset, Instance, Joint, JointKind, Light, LightKind, MaterialDef, Node, PartDef,
    PostProcessing, SceneEntry, SceneSettings, SketchSegment2D, ToneMapping, Transform3D, Vec2,
    Vec3, Vignette,
};
use std::collections::HashMap;
use std::fmt::{self, Write as FmtWrite};

/// Current VCode format version.
pub const VCODE_VERSION: &str = "0.2";

/// Error type for VCode parsing.
#[derive(Debug, Clone, PartialEq)]
pub struct VCodeParseError {
    /// Line number where the error occurred (0-indexed).
    pub line: usize,
    /// Description of the error.
    pub message: String,
}

impl fmt::Display for VCodeParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for VCodeParseError {}

/// Convert a Document to VCode format.
///
/// The document must have a simple DAG structure where node IDs can be
/// mapped to sequential line numbers. Nodes are sorted topologically
/// so dependencies appear before their dependents.
pub fn to_vcode(doc: &Document) -> Result<String, VCodeParseError> {
    let mut output = String::new();

    // Header
    writeln!(output, "# vcad {}", VCODE_VERSION).unwrap();
    writeln!(output).unwrap();

    // Materials section
    if !doc.materials.is_empty() {
        writeln!(output, "# Materials").unwrap();
        let mut mat_names: Vec<_> = doc.materials.keys().collect();
        mat_names.sort();
        for name in mat_names {
            let mat = &doc.materials[name];
            write!(
                output,
                "M {} {} {} {} {} {}",
                escape_id(&mat.name),
                mat.color[0],
                mat.color[1],
                mat.color[2],
                mat.metallic,
                mat.roughness
            )
            .unwrap();
            if let Some(density) = mat.density {
                write!(output, " {}", density).unwrap();
                if let Some(friction) = mat.friction {
                    write!(output, " {}", friction).unwrap();
                }
            }
            writeln!(output).unwrap();
        }
        writeln!(output).unwrap();
    }

    // Geometry section
    if !doc.nodes.is_empty() {
        writeln!(output, "# Geometry").unwrap();

        // Find all root nodes (nodes not referenced by any other node)
        let referenced: std::collections::HashSet<u64> = doc
            .nodes
            .values()
            .flat_map(|n| get_children(&n.op))
            .collect();

        let roots: Vec<u64> = doc
            .nodes
            .keys()
            .filter(|id| !referenced.contains(id))
            .copied()
            .collect();

        // Topological sort: dependencies before dependents
        let sorted = topological_sort(doc, &roots)?;

        // Create ID mapping: original NodeId -> line number
        let id_map: HashMap<u64, usize> =
            sorted.iter().enumerate().map(|(i, &id)| (id, i)).collect();

        for &node_id in &sorted {
            let node = &doc.nodes[&node_id];
            let line = format_op(&node.op, &id_map, node.name.as_deref())?;
            writeln!(output, "{}", line).unwrap();
        }
        writeln!(output).unwrap();

        // Scene roots section
        if !doc.roots.is_empty() {
            writeln!(output, "# Scene").unwrap();
            for entry in &doc.roots {
                let mapped_id = id_map.get(&entry.root).ok_or_else(|| VCodeParseError {
                    line: 0,
                    message: format!("unknown root node {}", entry.root),
                })?;
                write!(output, "ROOT {} {}", mapped_id, escape_id(&entry.material)).unwrap();
                if entry.visible == Some(false) {
                    write!(output, " hidden").unwrap();
                }
                writeln!(output).unwrap();
            }
            writeln!(output).unwrap();
        }
    }

    // Part definitions section
    if let Some(ref part_defs) = doc.part_defs {
        if !part_defs.is_empty() {
            writeln!(output, "# Parts").unwrap();

            // Get id_map for node references
            let referenced: std::collections::HashSet<u64> = doc
                .nodes
                .values()
                .flat_map(|n| get_children(&n.op))
                .collect();
            let roots: Vec<u64> = doc
                .nodes
                .keys()
                .filter(|id| !referenced.contains(id))
                .copied()
                .collect();
            let sorted = topological_sort(doc, &roots)?;
            let id_map: HashMap<u64, usize> =
                sorted.iter().enumerate().map(|(i, &id)| (id, i)).collect();

            let mut pdef_ids: Vec<_> = part_defs.keys().collect();
            pdef_ids.sort();
            for id in pdef_ids {
                let pdef = &part_defs[id];
                let mapped_root = id_map.get(&pdef.root).ok_or_else(|| VCodeParseError {
                    line: 0,
                    message: format!("unknown part def root {}", pdef.root),
                })?;
                write!(
                    output,
                    "PDEF {} {} {}",
                    escape_id(&pdef.id),
                    format_quoted_string(pdef.name.as_deref().unwrap_or(&pdef.id)),
                    mapped_root
                )
                .unwrap();
                if let Some(ref mat) = pdef.default_material {
                    write!(output, " {}", escape_id(mat)).unwrap();
                }
                writeln!(output).unwrap();
            }
            writeln!(output).unwrap();
        }
    }

    // Instances section
    if let Some(ref instances) = doc.instances {
        if !instances.is_empty() {
            writeln!(output, "# Instances").unwrap();
            for inst in instances {
                let tf = inst.transform.as_ref().cloned().unwrap_or_default();
                write!(
                    output,
                    "INST {} {} {} {} {} {} {} {} {} {} {} {}",
                    escape_id(&inst.id),
                    escape_id(&inst.part_def_id),
                    format_quoted_string(inst.name.as_deref().unwrap_or(&inst.id)),
                    tf.translation.x,
                    tf.translation.y,
                    tf.translation.z,
                    tf.rotation.x,
                    tf.rotation.y,
                    tf.rotation.z,
                    tf.scale.x,
                    tf.scale.y,
                    tf.scale.z
                )
                .unwrap();
                if let Some(ref mat) = inst.material {
                    write!(output, " {}", escape_id(mat)).unwrap();
                }
                writeln!(output).unwrap();
            }
            writeln!(output).unwrap();
        }
    }

    // Joints section
    if let Some(ref joints) = doc.joints {
        if !joints.is_empty() {
            writeln!(output, "# Joints").unwrap();
            for joint in joints {
                format_joint(&mut output, joint);
            }
            writeln!(output).unwrap();
        }
    }

    // Ground instance
    if let Some(ref ground_id) = doc.ground_instance_id {
        writeln!(output, "GROUND {}", escape_id(ground_id)).unwrap();
        writeln!(output).unwrap();
    }

    // Scene settings
    if let Some(ref scene) = doc.scene {
        format_scene_settings(&mut output, scene);
    }

    // Remove trailing newlines
    while output.ends_with('\n') {
        output.pop();
    }

    Ok(output)
}

/// Format a joint to VCode format.
fn format_joint(output: &mut String, joint: &Joint) {
    let parent = joint
        .parent_instance_id
        .as_deref()
        .map(escape_id)
        .unwrap_or_else(|| "_".to_string());
    let child = escape_id(&joint.child_instance_id);
    let pa = &joint.parent_anchor;
    let ca = &joint.child_anchor;

    match &joint.kind {
        JointKind::Fixed => {
            writeln!(
                output,
                "JFIX {} {} {} {} {} {} {} {} {}",
                escape_id(&joint.id),
                parent,
                child,
                pa.x,
                pa.y,
                pa.z,
                ca.x,
                ca.y,
                ca.z
            )
            .unwrap();
        }
        JointKind::Revolute { axis, limits } => {
            write!(
                output,
                "JREV {} {} {} {} {} {} {} {} {} {} {} {}",
                escape_id(&joint.id),
                parent,
                child,
                pa.x,
                pa.y,
                pa.z,
                ca.x,
                ca.y,
                ca.z,
                axis.x,
                axis.y,
                axis.z
            )
            .unwrap();
            if let Some((min, max)) = limits {
                write!(output, " {} {}", min, max).unwrap();
            }
            writeln!(output).unwrap();
        }
        JointKind::Slider { axis, limits } => {
            write!(
                output,
                "JSLD {} {} {} {} {} {} {} {} {} {} {} {}",
                escape_id(&joint.id),
                parent,
                child,
                pa.x,
                pa.y,
                pa.z,
                ca.x,
                ca.y,
                ca.z,
                axis.x,
                axis.y,
                axis.z
            )
            .unwrap();
            if let Some((min, max)) = limits {
                write!(output, " {} {}", min, max).unwrap();
            }
            writeln!(output).unwrap();
        }
        JointKind::Cylindrical { axis } => {
            writeln!(
                output,
                "JCYL {} {} {} {} {} {} {} {} {} {} {} {}",
                escape_id(&joint.id),
                parent,
                child,
                pa.x,
                pa.y,
                pa.z,
                ca.x,
                ca.y,
                ca.z,
                axis.x,
                axis.y,
                axis.z
            )
            .unwrap();
        }
        JointKind::Ball => {
            writeln!(
                output,
                "JBAL {} {} {} {} {} {} {} {} {}",
                escape_id(&joint.id),
                parent,
                child,
                pa.x,
                pa.y,
                pa.z,
                ca.x,
                ca.y,
                ca.z
            )
            .unwrap();
        }
    }
}

/// Format scene settings to VCode format.
fn format_scene_settings(output: &mut String, scene: &SceneSettings) {
    writeln!(output, "# Scene Settings").unwrap();

    // Environment
    if let Some(ref env) = scene.environment {
        match env {
            Environment::Preset { preset, intensity } => {
                let name = match preset {
                    EnvironmentPreset::Studio => "studio",
                    EnvironmentPreset::Warehouse => "warehouse",
                    EnvironmentPreset::Apartment => "apartment",
                    EnvironmentPreset::Park => "park",
                    EnvironmentPreset::City => "city",
                    EnvironmentPreset::Dawn => "dawn",
                    EnvironmentPreset::Night => "night",
                    EnvironmentPreset::Sunset => "sunset",
                    EnvironmentPreset::Forest => "forest",
                    EnvironmentPreset::Neutral => "neutral",
                };
                writeln!(output, "ENV {} {}", name, intensity.unwrap_or(1.0)).unwrap();
            }
            Environment::Custom { url, intensity } => {
                writeln!(
                    output,
                    "ENV {} {}",
                    format_quoted_string(url),
                    intensity.unwrap_or(1.0)
                )
                .unwrap();
            }
            Environment::None => {
                writeln!(output, "ENV none").unwrap();
            }
        }
    }

    // Background
    if let Some(ref bg) = scene.background {
        match bg {
            Background::Solid { color } => {
                writeln!(output, "BG solid {} {} {}", color[0], color[1], color[2]).unwrap();
            }
            Background::Gradient { top, bottom } => {
                writeln!(
                    output,
                    "BG gradient {} {} {} {} {} {}",
                    top[0], top[1], top[2], bottom[0], bottom[1], bottom[2]
                )
                .unwrap();
            }
            Background::Environment => {
                writeln!(output, "BG env").unwrap();
            }
            Background::Transparent => {
                writeln!(output, "BG transparent").unwrap();
            }
        }
    }

    // Lights
    if let Some(ref lights) = scene.lights {
        for light in lights {
            let c = &light.color;
            let i = light.intensity;
            let shadow = if light.cast_shadow == Some(true) {
                " shadow"
            } else {
                ""
            };

            match &light.kind {
                LightKind::Directional { direction } => {
                    writeln!(
                        output,
                        "LDIR {} {} {} {} {} {} {} {}{}",
                        escape_id(&light.id),
                        c[0],
                        c[1],
                        c[2],
                        i,
                        direction.x,
                        direction.y,
                        direction.z,
                        shadow
                    )
                    .unwrap();
                }
                LightKind::Point { position, distance } => {
                    write!(
                        output,
                        "LPNT {} {} {} {} {} {} {} {}",
                        escape_id(&light.id),
                        c[0],
                        c[1],
                        c[2],
                        i,
                        position.x,
                        position.y,
                        position.z
                    )
                    .unwrap();
                    if let Some(d) = distance {
                        write!(output, " {}", d).unwrap();
                    }
                    writeln!(output).unwrap();
                }
                LightKind::Spot {
                    position,
                    direction,
                    angle,
                    penumbra,
                } => {
                    write!(
                        output,
                        "LSPT {} {} {} {} {} {} {} {} {} {} {}",
                        escape_id(&light.id),
                        c[0],
                        c[1],
                        c[2],
                        i,
                        position.x,
                        position.y,
                        position.z,
                        direction.x,
                        direction.y,
                        direction.z
                    )
                    .unwrap();
                    if let Some(a) = angle {
                        write!(output, " {}", a).unwrap();
                        if let Some(p) = penumbra {
                            write!(output, " {}", p).unwrap();
                        }
                    }
                    writeln!(output).unwrap();
                }
                LightKind::Area {
                    position,
                    direction,
                    width,
                    height,
                } => {
                    writeln!(
                        output,
                        "LAREA {} {} {} {} {} {} {} {} {} {} {} {} {}",
                        escape_id(&light.id),
                        c[0],
                        c[1],
                        c[2],
                        i,
                        position.x,
                        position.y,
                        position.z,
                        direction.x,
                        direction.y,
                        direction.z,
                        width,
                        height
                    )
                    .unwrap();
                }
            }
        }
    }

    // Post-processing
    if let Some(ref pp) = scene.post_processing {
        if let Some(ref ao) = pp.ambient_occlusion {
            writeln!(
                output,
                "AO {} {} {}",
                if ao.enabled { 1 } else { 0 },
                ao.intensity.unwrap_or(1.0),
                ao.radius.unwrap_or(1.0)
            )
            .unwrap();
        }
        if let Some(ref bloom) = pp.bloom {
            writeln!(
                output,
                "BLOOM {} {} {}",
                if bloom.enabled { 1 } else { 0 },
                bloom.intensity.unwrap_or(0.5),
                bloom.threshold.unwrap_or(0.8)
            )
            .unwrap();
        }
        if let Some(ref vig) = pp.vignette {
            writeln!(
                output,
                "VIG {} {} {}",
                if vig.enabled { 1 } else { 0 },
                vig.offset.unwrap_or(0.3),
                vig.darkness.unwrap_or(0.5)
            )
            .unwrap();
        }
        if let Some(ref tm) = pp.tone_mapping {
            let name = match tm {
                ToneMapping::None => "none",
                ToneMapping::Reinhard => "reinhard",
                ToneMapping::Cineon => "cineon",
                ToneMapping::AcesFilmic => "acesFilmic",
                ToneMapping::AgX => "agX",
                ToneMapping::Neutral => "neutral",
            };
            writeln!(output, "TONE {}", name).unwrap();
        }
        if let Some(exp) = pp.exposure {
            writeln!(output, "EXP {}", exp).unwrap();
        }
    }

    // Camera presets
    if let Some(ref cams) = scene.camera_presets {
        for cam in cams {
            write!(
                output,
                "CAM {} {} {} {} {} {} {}",
                escape_id(&cam.id),
                cam.position.x,
                cam.position.y,
                cam.position.z,
                cam.target.x,
                cam.target.y,
                cam.target.z
            )
            .unwrap();
            if let Some(fov) = cam.fov {
                write!(output, " {}", fov).unwrap();
            }
            if let Some(ref name) = cam.name {
                write!(output, " {}", format_quoted_string(name)).unwrap();
            }
            writeln!(output).unwrap();
        }
    }
}

/// Escape an identifier - if it contains spaces or special chars, quote it.
fn escape_id(s: &str) -> String {
    if s.contains(char::is_whitespace)
        || s.contains('"')
        || s.is_empty()
        || s.chars().next().is_some_and(|c| c.is_ascii_digit())
    {
        format_quoted_string(s)
    } else {
        s.to_string()
    }
}

/// Format a string with quotes.
fn format_quoted_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Parse VCode format into a Document.
pub fn from_vcode(s: &str) -> Result<Document, VCodeParseError> {
    let mut doc = Document::new();
    let mut current_line = 0;
    let mut lines = s.lines().peekable();

    // Track the first geometry line number for node ID mapping
    let mut geometry_line_offset: Option<usize> = None;
    let mut geometry_node_count = 0;

    while let Some(line) = lines.next() {
        let trimmed = line.trim();

        // Skip empty lines and comments
        if trimmed.is_empty() || trimmed.starts_with('#') {
            current_line += 1;
            continue;
        }

        // Parse the line based on its opcode
        let parts: Vec<&str> = split_line_respecting_quotes(trimmed);
        if parts.is_empty() {
            current_line += 1;
            continue;
        }

        let opcode = parts[0];

        match opcode {
            // Material definition
            "M" => {
                parse_material(&mut doc, &parts, current_line)?;
            }

            // Scene root
            "ROOT" => {
                parse_root(&mut doc, &parts, current_line)?;
            }

            // Part definition
            "PDEF" => {
                parse_part_def(&mut doc, &parts, current_line)?;
            }

            // Instance
            "INST" => {
                parse_instance(&mut doc, &parts, current_line)?;
            }

            // Joints
            "JFIX" | "JREV" | "JSLD" | "JCYL" | "JBAL" => {
                parse_joint(&mut doc, opcode, &parts, current_line)?;
            }

            // Ground instance
            "GROUND" => {
                if parts.len() != 2 {
                    return Err(VCodeParseError {
                        line: current_line,
                        message: format!("GROUND requires 1 arg, got {}", parts.len() - 1),
                    });
                }
                doc.ground_instance_id = Some(parse_string_arg(parts[1]));
            }

            // Environment
            "ENV" => {
                parse_environment(&mut doc, &parts, current_line)?;
            }

            // Background
            "BG" => {
                parse_background(&mut doc, &parts, current_line)?;
            }

            // Lights
            "LDIR" | "LPNT" | "LSPT" | "LAREA" => {
                parse_light(&mut doc, opcode, &parts, current_line)?;
            }

            // Post-processing
            "AO" => {
                parse_ao(&mut doc, &parts, current_line)?;
            }
            "BLOOM" => {
                parse_bloom(&mut doc, &parts, current_line)?;
            }
            "VIG" => {
                parse_vignette(&mut doc, &parts, current_line)?;
            }
            "TONE" => {
                parse_tone_mapping(&mut doc, &parts, current_line)?;
            }
            "EXP" => {
                parse_exposure(&mut doc, &parts, current_line)?;
            }

            // Camera preset
            "CAM" => {
                parse_camera(&mut doc, &parts, current_line)?;
            }

            // Geometry opcodes - these create nodes
            _ => {
                // Track geometry section start
                if geometry_line_offset.is_none() {
                    geometry_line_offset = Some(current_line);
                }

                let node_id = geometry_node_count as u64;
                let (op, name) =
                    parse_geometry_line(trimmed, current_line, &mut lines, &mut current_line)?;

                doc.nodes.insert(
                    node_id,
                    Node {
                        id: node_id,
                        name,
                        op,
                    },
                );

                geometry_node_count += 1;
            }
        }

        current_line += 1;
    }

    // If no explicit ROOTs were defined, add a default one
    if doc.roots.is_empty() && !doc.nodes.is_empty() {
        let referenced: std::collections::HashSet<u64> = doc
            .nodes
            .values()
            .flat_map(|n| get_children(&n.op))
            .collect();

        let root_id = doc
            .nodes
            .keys()
            .filter(|id| !referenced.contains(id))
            .max()
            .copied()
            .unwrap_or(0);

        // Add default material if none exists
        if doc.materials.is_empty() {
            doc.materials.insert(
                "default".to_string(),
                MaterialDef {
                    name: "default".to_string(),
                    color: [0.8, 0.8, 0.8],
                    metallic: 0.0,
                    roughness: 0.5,
                    density: None,
                    friction: None,
                },
            );
        }

        doc.roots.push(SceneEntry {
            root: root_id,
            material: doc
                .materials
                .keys()
                .next()
                .cloned()
                .unwrap_or_else(|| "default".to_string()),
            visible: None,
        });
    }

    Ok(doc)
}

/// Split a line by whitespace, but keep quoted strings together.
fn split_line_respecting_quotes(line: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut in_quotes = false;
    let chars: Vec<char> = line.chars().collect();

    for (i, &c) in chars.iter().enumerate() {
        if c == '"' && (i == 0 || chars[i - 1] != '\\') {
            if in_quotes {
                // End of quoted string - include the quote
                parts.push(&line[start..=i]);
                start = i + 1;
                in_quotes = false;
            } else {
                // Start of quoted string
                if start < i {
                    // Push any whitespace-separated parts before the quote
                    for part in line[start..i].split_whitespace() {
                        parts.push(part);
                    }
                }
                start = i;
                in_quotes = true;
            }
        }
    }

    // Handle remaining content
    if start < line.len() {
        if in_quotes {
            // Unterminated quote - include as-is
            parts.push(&line[start..]);
        } else {
            for part in line[start..].split_whitespace() {
                parts.push(part);
            }
        }
    }

    parts
}

/// Parse a potentially quoted string argument.
fn parse_string_arg(s: &str) -> String {
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        let inner = &s[1..s.len() - 1];
        inner.replace("\\\"", "\"").replace("\\\\", "\\")
    } else {
        s.to_string()
    }
}

/// Parse a material definition.
fn parse_material(doc: &mut Document, parts: &[&str], line: usize) -> Result<(), VCodeParseError> {
    if parts.len() < 7 {
        return Err(VCodeParseError {
            line,
            message: format!("M requires at least 6 args, got {}", parts.len() - 1),
        });
    }

    let name = parse_string_arg(parts[1]);
    let color = [
        parse_f64(parts[2], line)?,
        parse_f64(parts[3], line)?,
        parse_f64(parts[4], line)?,
    ];
    let metallic = parse_f64(parts[5], line)?;
    let roughness = parse_f64(parts[6], line)?;
    let density = parts.get(7).map(|s| parse_f64(s, line)).transpose()?;
    let friction = parts.get(8).map(|s| parse_f64(s, line)).transpose()?;

    doc.materials.insert(
        name.clone(),
        MaterialDef {
            name,
            color,
            metallic,
            roughness,
            density,
            friction,
        },
    );

    Ok(())
}

/// Parse a ROOT entry.
fn parse_root(doc: &mut Document, parts: &[&str], line: usize) -> Result<(), VCodeParseError> {
    if parts.len() < 3 {
        return Err(VCodeParseError {
            line,
            message: format!("ROOT requires at least 2 args, got {}", parts.len() - 1),
        });
    }

    let root = parse_u64(parts[1], line)?;
    let material = parse_string_arg(parts[2]);
    let visible = if parts.get(3) == Some(&"hidden") {
        Some(false)
    } else {
        None
    };

    doc.roots.push(SceneEntry {
        root,
        material,
        visible,
    });

    Ok(())
}

/// Parse a part definition.
fn parse_part_def(doc: &mut Document, parts: &[&str], line: usize) -> Result<(), VCodeParseError> {
    if parts.len() < 4 {
        return Err(VCodeParseError {
            line,
            message: format!("PDEF requires at least 3 args, got {}", parts.len() - 1),
        });
    }

    let id = parse_string_arg(parts[1]);
    let name = parse_string_arg(parts[2]);
    let root = parse_u64(parts[3], line)?;
    let default_material = parts.get(4).map(|s| parse_string_arg(s));

    let part_defs = doc.part_defs.get_or_insert_with(HashMap::new);
    part_defs.insert(
        id.clone(),
        PartDef {
            id,
            name: Some(name),
            root,
            default_material,
        },
    );

    Ok(())
}

/// Parse an instance.
fn parse_instance(doc: &mut Document, parts: &[&str], line: usize) -> Result<(), VCodeParseError> {
    if parts.len() < 13 {
        return Err(VCodeParseError {
            line,
            message: format!("INST requires at least 12 args, got {}", parts.len() - 1),
        });
    }

    let id = parse_string_arg(parts[1]);
    let part_def_id = parse_string_arg(parts[2]);
    let name = parse_string_arg(parts[3]);
    let transform = Transform3D {
        translation: Vec3::new(
            parse_f64(parts[4], line)?,
            parse_f64(parts[5], line)?,
            parse_f64(parts[6], line)?,
        ),
        rotation: Vec3::new(
            parse_f64(parts[7], line)?,
            parse_f64(parts[8], line)?,
            parse_f64(parts[9], line)?,
        ),
        scale: Vec3::new(
            parse_f64(parts[10], line)?,
            parse_f64(parts[11], line)?,
            parse_f64(parts[12], line)?,
        ),
    };
    let material = parts.get(13).map(|s| parse_string_arg(s));

    let instances = doc.instances.get_or_insert_with(Vec::new);
    instances.push(Instance {
        id,
        part_def_id,
        name: Some(name),
        transform: Some(transform),
        material,
    });

    Ok(())
}

/// Parse a joint.
fn parse_joint(
    doc: &mut Document,
    opcode: &str,
    parts: &[&str],
    line: usize,
) -> Result<(), VCodeParseError> {
    let joints = doc.joints.get_or_insert_with(Vec::new);

    match opcode {
        "JFIX" => {
            if parts.len() < 10 {
                return Err(VCodeParseError {
                    line,
                    message: format!("JFIX requires 9 args, got {}", parts.len() - 1),
                });
            }
            joints.push(Joint {
                id: parse_string_arg(parts[1]),
                name: None,
                parent_instance_id: parse_optional_parent(parts[2]),
                child_instance_id: parse_string_arg(parts[3]),
                parent_anchor: Vec3::new(
                    parse_f64(parts[4], line)?,
                    parse_f64(parts[5], line)?,
                    parse_f64(parts[6], line)?,
                ),
                child_anchor: Vec3::new(
                    parse_f64(parts[7], line)?,
                    parse_f64(parts[8], line)?,
                    parse_f64(parts[9], line)?,
                ),
                kind: JointKind::Fixed,
                state: 0.0,
            });
        }
        "JREV" => {
            if parts.len() < 13 {
                return Err(VCodeParseError {
                    line,
                    message: format!("JREV requires at least 12 args, got {}", parts.len() - 1),
                });
            }
            let limits = if parts.len() >= 15 {
                Some((parse_f64(parts[13], line)?, parse_f64(parts[14], line)?))
            } else {
                None
            };
            joints.push(Joint {
                id: parse_string_arg(parts[1]),
                name: None,
                parent_instance_id: parse_optional_parent(parts[2]),
                child_instance_id: parse_string_arg(parts[3]),
                parent_anchor: Vec3::new(
                    parse_f64(parts[4], line)?,
                    parse_f64(parts[5], line)?,
                    parse_f64(parts[6], line)?,
                ),
                child_anchor: Vec3::new(
                    parse_f64(parts[7], line)?,
                    parse_f64(parts[8], line)?,
                    parse_f64(parts[9], line)?,
                ),
                kind: JointKind::Revolute {
                    axis: Vec3::new(
                        parse_f64(parts[10], line)?,
                        parse_f64(parts[11], line)?,
                        parse_f64(parts[12], line)?,
                    ),
                    limits,
                },
                state: 0.0,
            });
        }
        "JSLD" => {
            if parts.len() < 13 {
                return Err(VCodeParseError {
                    line,
                    message: format!("JSLD requires at least 12 args, got {}", parts.len() - 1),
                });
            }
            let limits = if parts.len() >= 15 {
                Some((parse_f64(parts[13], line)?, parse_f64(parts[14], line)?))
            } else {
                None
            };
            joints.push(Joint {
                id: parse_string_arg(parts[1]),
                name: None,
                parent_instance_id: parse_optional_parent(parts[2]),
                child_instance_id: parse_string_arg(parts[3]),
                parent_anchor: Vec3::new(
                    parse_f64(parts[4], line)?,
                    parse_f64(parts[5], line)?,
                    parse_f64(parts[6], line)?,
                ),
                child_anchor: Vec3::new(
                    parse_f64(parts[7], line)?,
                    parse_f64(parts[8], line)?,
                    parse_f64(parts[9], line)?,
                ),
                kind: JointKind::Slider {
                    axis: Vec3::new(
                        parse_f64(parts[10], line)?,
                        parse_f64(parts[11], line)?,
                        parse_f64(parts[12], line)?,
                    ),
                    limits,
                },
                state: 0.0,
            });
        }
        "JCYL" => {
            if parts.len() < 13 {
                return Err(VCodeParseError {
                    line,
                    message: format!("JCYL requires 12 args, got {}", parts.len() - 1),
                });
            }
            joints.push(Joint {
                id: parse_string_arg(parts[1]),
                name: None,
                parent_instance_id: parse_optional_parent(parts[2]),
                child_instance_id: parse_string_arg(parts[3]),
                parent_anchor: Vec3::new(
                    parse_f64(parts[4], line)?,
                    parse_f64(parts[5], line)?,
                    parse_f64(parts[6], line)?,
                ),
                child_anchor: Vec3::new(
                    parse_f64(parts[7], line)?,
                    parse_f64(parts[8], line)?,
                    parse_f64(parts[9], line)?,
                ),
                kind: JointKind::Cylindrical {
                    axis: Vec3::new(
                        parse_f64(parts[10], line)?,
                        parse_f64(parts[11], line)?,
                        parse_f64(parts[12], line)?,
                    ),
                },
                state: 0.0,
            });
        }
        "JBAL" => {
            if parts.len() < 10 {
                return Err(VCodeParseError {
                    line,
                    message: format!("JBAL requires 9 args, got {}", parts.len() - 1),
                });
            }
            joints.push(Joint {
                id: parse_string_arg(parts[1]),
                name: None,
                parent_instance_id: parse_optional_parent(parts[2]),
                child_instance_id: parse_string_arg(parts[3]),
                parent_anchor: Vec3::new(
                    parse_f64(parts[4], line)?,
                    parse_f64(parts[5], line)?,
                    parse_f64(parts[6], line)?,
                ),
                child_anchor: Vec3::new(
                    parse_f64(parts[7], line)?,
                    parse_f64(parts[8], line)?,
                    parse_f64(parts[9], line)?,
                ),
                kind: JointKind::Ball,
                state: 0.0,
            });
        }
        _ => unreachable!(),
    }

    Ok(())
}

/// Parse optional parent instance ID ("_" means None).
fn parse_optional_parent(s: &str) -> Option<String> {
    if s == "_" {
        None
    } else {
        Some(parse_string_arg(s))
    }
}

/// Parse environment setting.
fn parse_environment(
    doc: &mut Document,
    parts: &[&str],
    line: usize,
) -> Result<(), VCodeParseError> {
    if parts.len() < 2 {
        return Err(VCodeParseError {
            line,
            message: format!("ENV requires at least 1 arg, got {}", parts.len() - 1),
        });
    }

    let scene = doc.scene.get_or_insert_with(SceneSettings::default);
    let preset_or_url = parse_string_arg(parts[1]);

    if preset_or_url == "none" {
        scene.environment = Some(Environment::None);
        return Ok(());
    }

    if parts.len() < 3 {
        return Err(VCodeParseError {
            line,
            message: format!("ENV requires 2 args, got {}", parts.len() - 1),
        });
    }

    let intensity = Some(parse_f64(parts[2], line)?);

    let env = match preset_or_url.as_str() {
        "studio" => Environment::Preset {
            preset: EnvironmentPreset::Studio,
            intensity,
        },
        "warehouse" => Environment::Preset {
            preset: EnvironmentPreset::Warehouse,
            intensity,
        },
        "apartment" => Environment::Preset {
            preset: EnvironmentPreset::Apartment,
            intensity,
        },
        "park" => Environment::Preset {
            preset: EnvironmentPreset::Park,
            intensity,
        },
        "city" => Environment::Preset {
            preset: EnvironmentPreset::City,
            intensity,
        },
        "dawn" => Environment::Preset {
            preset: EnvironmentPreset::Dawn,
            intensity,
        },
        "night" => Environment::Preset {
            preset: EnvironmentPreset::Night,
            intensity,
        },
        "sunset" => Environment::Preset {
            preset: EnvironmentPreset::Sunset,
            intensity,
        },
        "forest" => Environment::Preset {
            preset: EnvironmentPreset::Forest,
            intensity,
        },
        "neutral" => Environment::Preset {
            preset: EnvironmentPreset::Neutral,
            intensity,
        },
        url => Environment::Custom {
            url: url.to_string(),
            intensity,
        },
    };

    scene.environment = Some(env);
    Ok(())
}

/// Parse background setting.
fn parse_background(
    doc: &mut Document,
    parts: &[&str],
    line: usize,
) -> Result<(), VCodeParseError> {
    if parts.len() < 2 {
        return Err(VCodeParseError {
            line,
            message: "BG requires at least 1 arg".to_string(),
        });
    }

    let scene = doc.scene.get_or_insert_with(SceneSettings::default);

    let bg = match parts[1] {
        "solid" => {
            if parts.len() < 5 {
                return Err(VCodeParseError {
                    line,
                    message: "BG solid requires 3 color values".to_string(),
                });
            }
            Background::Solid {
                color: [
                    parse_f64(parts[2], line)?,
                    parse_f64(parts[3], line)?,
                    parse_f64(parts[4], line)?,
                ],
            }
        }
        "gradient" => {
            if parts.len() < 8 {
                return Err(VCodeParseError {
                    line,
                    message: "BG gradient requires 6 color values".to_string(),
                });
            }
            Background::Gradient {
                top: [
                    parse_f64(parts[2], line)?,
                    parse_f64(parts[3], line)?,
                    parse_f64(parts[4], line)?,
                ],
                bottom: [
                    parse_f64(parts[5], line)?,
                    parse_f64(parts[6], line)?,
                    parse_f64(parts[7], line)?,
                ],
            }
        }
        "env" => Background::Environment,
        "transparent" => Background::Transparent,
        _ => {
            return Err(VCodeParseError {
                line,
                message: format!("unknown BG type: {}", parts[1]),
            });
        }
    };

    scene.background = Some(bg);
    Ok(())
}

/// Parse light.
fn parse_light(
    doc: &mut Document,
    opcode: &str,
    parts: &[&str],
    line: usize,
) -> Result<(), VCodeParseError> {
    let scene = doc.scene.get_or_insert_with(SceneSettings::default);
    let lights = scene.lights.get_or_insert_with(Vec::new);

    match opcode {
        "LDIR" => {
            if parts.len() < 9 {
                return Err(VCodeParseError {
                    line,
                    message: format!("LDIR requires at least 8 args, got {}", parts.len() - 1),
                });
            }
            let cast_shadow = parts.get(9) == Some(&"shadow");
            lights.push(Light {
                id: parse_string_arg(parts[1]),
                color: [
                    parse_f64(parts[2], line)?,
                    parse_f64(parts[3], line)?,
                    parse_f64(parts[4], line)?,
                ],
                intensity: parse_f64(parts[5], line)?,
                kind: LightKind::Directional {
                    direction: Vec3::new(
                        parse_f64(parts[6], line)?,
                        parse_f64(parts[7], line)?,
                        parse_f64(parts[8], line)?,
                    ),
                },
                enabled: Some(true),
                cast_shadow: if cast_shadow { Some(true) } else { None },
            });
        }
        "LPNT" => {
            if parts.len() < 9 {
                return Err(VCodeParseError {
                    line,
                    message: format!("LPNT requires at least 8 args, got {}", parts.len() - 1),
                });
            }
            let distance = parts.get(9).map(|s| parse_f64(s, line)).transpose()?;
            lights.push(Light {
                id: parse_string_arg(parts[1]),
                color: [
                    parse_f64(parts[2], line)?,
                    parse_f64(parts[3], line)?,
                    parse_f64(parts[4], line)?,
                ],
                intensity: parse_f64(parts[5], line)?,
                kind: LightKind::Point {
                    position: Vec3::new(
                        parse_f64(parts[6], line)?,
                        parse_f64(parts[7], line)?,
                        parse_f64(parts[8], line)?,
                    ),
                    distance,
                },
                enabled: Some(true),
                cast_shadow: None,
            });
        }
        "LSPT" => {
            if parts.len() < 12 {
                return Err(VCodeParseError {
                    line,
                    message: format!("LSPT requires at least 11 args, got {}", parts.len() - 1),
                });
            }
            let angle = parts.get(12).map(|s| parse_f64(s, line)).transpose()?;
            let penumbra = parts.get(13).map(|s| parse_f64(s, line)).transpose()?;
            lights.push(Light {
                id: parse_string_arg(parts[1]),
                color: [
                    parse_f64(parts[2], line)?,
                    parse_f64(parts[3], line)?,
                    parse_f64(parts[4], line)?,
                ],
                intensity: parse_f64(parts[5], line)?,
                kind: LightKind::Spot {
                    position: Vec3::new(
                        parse_f64(parts[6], line)?,
                        parse_f64(parts[7], line)?,
                        parse_f64(parts[8], line)?,
                    ),
                    direction: Vec3::new(
                        parse_f64(parts[9], line)?,
                        parse_f64(parts[10], line)?,
                        parse_f64(parts[11], line)?,
                    ),
                    angle,
                    penumbra,
                },
                enabled: Some(true),
                cast_shadow: None,
            });
        }
        "LAREA" => {
            if parts.len() < 14 {
                return Err(VCodeParseError {
                    line,
                    message: format!("LAREA requires 13 args, got {}", parts.len() - 1),
                });
            }
            lights.push(Light {
                id: parse_string_arg(parts[1]),
                color: [
                    parse_f64(parts[2], line)?,
                    parse_f64(parts[3], line)?,
                    parse_f64(parts[4], line)?,
                ],
                intensity: parse_f64(parts[5], line)?,
                kind: LightKind::Area {
                    position: Vec3::new(
                        parse_f64(parts[6], line)?,
                        parse_f64(parts[7], line)?,
                        parse_f64(parts[8], line)?,
                    ),
                    direction: Vec3::new(
                        parse_f64(parts[9], line)?,
                        parse_f64(parts[10], line)?,
                        parse_f64(parts[11], line)?,
                    ),
                    width: parse_f64(parts[12], line)?,
                    height: parse_f64(parts[13], line)?,
                },
                enabled: Some(true),
                cast_shadow: None,
            });
        }
        _ => unreachable!(),
    }

    Ok(())
}

/// Parse AO settings.
fn parse_ao(doc: &mut Document, parts: &[&str], line: usize) -> Result<(), VCodeParseError> {
    if parts.len() < 4 {
        return Err(VCodeParseError {
            line,
            message: format!("AO requires 3 args, got {}", parts.len() - 1),
        });
    }

    let scene = doc.scene.get_or_insert_with(SceneSettings::default);
    let pp = scene
        .post_processing
        .get_or_insert_with(PostProcessing::default);

    pp.ambient_occlusion = Some(AmbientOcclusion {
        enabled: parse_u32(parts[1], line)? != 0,
        intensity: Some(parse_f64(parts[2], line)?),
        radius: Some(parse_f64(parts[3], line)?),
    });

    Ok(())
}

/// Parse bloom settings.
fn parse_bloom(doc: &mut Document, parts: &[&str], line: usize) -> Result<(), VCodeParseError> {
    if parts.len() < 4 {
        return Err(VCodeParseError {
            line,
            message: format!("BLOOM requires 3 args, got {}", parts.len() - 1),
        });
    }

    let scene = doc.scene.get_or_insert_with(SceneSettings::default);
    let pp = scene
        .post_processing
        .get_or_insert_with(PostProcessing::default);

    pp.bloom = Some(Bloom {
        enabled: parse_u32(parts[1], line)? != 0,
        intensity: Some(parse_f64(parts[2], line)?),
        threshold: Some(parse_f64(parts[3], line)?),
    });

    Ok(())
}

/// Parse vignette settings.
fn parse_vignette(doc: &mut Document, parts: &[&str], line: usize) -> Result<(), VCodeParseError> {
    if parts.len() < 4 {
        return Err(VCodeParseError {
            line,
            message: format!("VIG requires 3 args, got {}", parts.len() - 1),
        });
    }

    let scene = doc.scene.get_or_insert_with(SceneSettings::default);
    let pp = scene
        .post_processing
        .get_or_insert_with(PostProcessing::default);

    pp.vignette = Some(Vignette {
        enabled: parse_u32(parts[1], line)? != 0,
        offset: Some(parse_f64(parts[2], line)?),
        darkness: Some(parse_f64(parts[3], line)?),
    });

    Ok(())
}

/// Parse tone mapping.
fn parse_tone_mapping(
    doc: &mut Document,
    parts: &[&str],
    line: usize,
) -> Result<(), VCodeParseError> {
    if parts.len() < 2 {
        return Err(VCodeParseError {
            line,
            message: "TONE requires 1 arg".to_string(),
        });
    }

    let scene = doc.scene.get_or_insert_with(SceneSettings::default);
    let pp = scene
        .post_processing
        .get_or_insert_with(PostProcessing::default);

    pp.tone_mapping = Some(match parts[1] {
        "none" => ToneMapping::None,
        "reinhard" => ToneMapping::Reinhard,
        "cineon" => ToneMapping::Cineon,
        "acesFilmic" => ToneMapping::AcesFilmic,
        "agX" => ToneMapping::AgX,
        "neutral" => ToneMapping::Neutral,
        other => {
            return Err(VCodeParseError {
                line,
                message: format!("unknown tone mapping: {}", other),
            });
        }
    });

    Ok(())
}

/// Parse exposure.
fn parse_exposure(doc: &mut Document, parts: &[&str], line: usize) -> Result<(), VCodeParseError> {
    if parts.len() < 2 {
        return Err(VCodeParseError {
            line,
            message: "EXP requires 1 arg".to_string(),
        });
    }

    let scene = doc.scene.get_or_insert_with(SceneSettings::default);
    let pp = scene
        .post_processing
        .get_or_insert_with(PostProcessing::default);
    pp.exposure = Some(parse_f64(parts[1], line)?);

    Ok(())
}

/// Parse camera preset.
fn parse_camera(doc: &mut Document, parts: &[&str], line: usize) -> Result<(), VCodeParseError> {
    if parts.len() < 8 {
        return Err(VCodeParseError {
            line,
            message: format!("CAM requires at least 7 args, got {}", parts.len() - 1),
        });
    }

    let scene = doc.scene.get_or_insert_with(SceneSettings::default);
    let cams = scene.camera_presets.get_or_insert_with(Vec::new);

    let fov = parts.get(8).map(|s| parse_f64(s, line)).transpose()?;
    let name = parts.get(9).map(|s| parse_string_arg(s));

    cams.push(CameraPreset {
        id: parse_string_arg(parts[1]),
        position: Vec3::new(
            parse_f64(parts[2], line)?,
            parse_f64(parts[3], line)?,
            parse_f64(parts[4], line)?,
        ),
        target: Vec3::new(
            parse_f64(parts[5], line)?,
            parse_f64(parts[6], line)?,
            parse_f64(parts[7], line)?,
        ),
        fov,
        name,
    });

    Ok(())
}

/// Parse a geometry line (returns op and optional name).
fn parse_geometry_line<'a, I>(
    line: &str,
    line_num: usize,
    lines: &mut std::iter::Peekable<I>,
    current_line: &mut usize,
) -> Result<(CsgOp, Option<String>), VCodeParseError>
where
    I: Iterator<Item = &'a str>,
{
    // Use the quoted-aware split
    let parts = split_line_respecting_quotes(line);
    if parts.is_empty() {
        return Err(VCodeParseError {
            line: line_num,
            message: "empty line".to_string(),
        });
    }

    let opcode = parts[0];

    // Check for trailing quoted name
    let (args, name) = extract_trailing_name(&parts);

    // Now parse based on opcode
    let op = parse_geometry_opcode(opcode, &args, line_num, lines, current_line)?;

    Ok((op, name))
}

/// Extract trailing quoted name from parts if present.
fn extract_trailing_name<'a>(parts: &[&'a str]) -> (Vec<&'a str>, Option<String>) {
    if let Some(last) = parts.last() {
        if last.starts_with('"') && last.ends_with('"') {
            let name = parse_string_arg(last);
            let args = parts[..parts.len() - 1].to_vec();
            return (args, Some(name));
        }
    }
    (parts.to_vec(), None)
}

/// Parse a geometry opcode into a CsgOp.
fn parse_geometry_opcode<'a, I>(
    opcode: &str,
    parts: &[&str],
    line_num: usize,
    lines: &mut std::iter::Peekable<I>,
    current_line: &mut usize,
) -> Result<CsgOp, VCodeParseError>
where
    I: Iterator<Item = &'a str>,
{
    match opcode {
        "C" => {
            if parts.len() != 4 {
                return Err(VCodeParseError {
                    line: line_num,
                    message: format!("C requires 3 args, got {}", parts.len() - 1),
                });
            }
            Ok(CsgOp::Cube {
                size: Vec3::new(
                    parse_f64(parts[1], line_num)?,
                    parse_f64(parts[2], line_num)?,
                    parse_f64(parts[3], line_num)?,
                ),
            })
        }

        "Y" => {
            if parts.len() != 3 {
                return Err(VCodeParseError {
                    line: line_num,
                    message: format!("Y requires 2 args, got {}", parts.len() - 1),
                });
            }
            Ok(CsgOp::Cylinder {
                radius: parse_f64(parts[1], line_num)?,
                height: parse_f64(parts[2], line_num)?,
                segments: 0,
            })
        }

        "S" => {
            if parts.len() != 2 {
                return Err(VCodeParseError {
                    line: line_num,
                    message: format!("S requires 1 arg, got {}", parts.len() - 1),
                });
            }
            Ok(CsgOp::Sphere {
                radius: parse_f64(parts[1], line_num)?,
                segments: 0,
            })
        }

        "K" => {
            if parts.len() != 4 {
                return Err(VCodeParseError {
                    line: line_num,
                    message: format!("K requires 3 args, got {}", parts.len() - 1),
                });
            }
            Ok(CsgOp::Cone {
                radius_bottom: parse_f64(parts[1], line_num)?,
                radius_top: parse_f64(parts[2], line_num)?,
                height: parse_f64(parts[3], line_num)?,
                segments: 0,
            })
        }

        "U" => {
            if parts.len() != 3 {
                return Err(VCodeParseError {
                    line: line_num,
                    message: format!("U requires 2 args, got {}", parts.len() - 1),
                });
            }
            Ok(CsgOp::Union {
                left: parse_u64(parts[1], line_num)?,
                right: parse_u64(parts[2], line_num)?,
            })
        }

        "D" => {
            if parts.len() != 3 {
                return Err(VCodeParseError {
                    line: line_num,
                    message: format!("D requires 2 args, got {}", parts.len() - 1),
                });
            }
            Ok(CsgOp::Difference {
                left: parse_u64(parts[1], line_num)?,
                right: parse_u64(parts[2], line_num)?,
            })
        }

        "I" => {
            if parts.len() != 3 {
                return Err(VCodeParseError {
                    line: line_num,
                    message: format!("I requires 2 args, got {}", parts.len() - 1),
                });
            }
            Ok(CsgOp::Intersection {
                left: parse_u64(parts[1], line_num)?,
                right: parse_u64(parts[2], line_num)?,
            })
        }

        "T" => {
            if parts.len() != 5 {
                return Err(VCodeParseError {
                    line: line_num,
                    message: format!("T requires 4 args, got {}", parts.len() - 1),
                });
            }
            Ok(CsgOp::Translate {
                child: parse_u64(parts[1], line_num)?,
                offset: Vec3::new(
                    parse_f64(parts[2], line_num)?,
                    parse_f64(parts[3], line_num)?,
                    parse_f64(parts[4], line_num)?,
                ),
            })
        }

        "R" => {
            if parts.len() != 5 {
                return Err(VCodeParseError {
                    line: line_num,
                    message: format!("R requires 4 args, got {}", parts.len() - 1),
                });
            }
            Ok(CsgOp::Rotate {
                child: parse_u64(parts[1], line_num)?,
                angles: Vec3::new(
                    parse_f64(parts[2], line_num)?,
                    parse_f64(parts[3], line_num)?,
                    parse_f64(parts[4], line_num)?,
                ),
            })
        }

        "X" => {
            if parts.len() != 5 {
                return Err(VCodeParseError {
                    line: line_num,
                    message: format!("X requires 4 args, got {}", parts.len() - 1),
                });
            }
            Ok(CsgOp::Scale {
                child: parse_u64(parts[1], line_num)?,
                factor: Vec3::new(
                    parse_f64(parts[2], line_num)?,
                    parse_f64(parts[3], line_num)?,
                    parse_f64(parts[4], line_num)?,
                ),
            })
        }

        "LP" => {
            if parts.len() != 7 {
                return Err(VCodeParseError {
                    line: line_num,
                    message: format!("LP requires 6 args, got {}", parts.len() - 1),
                });
            }
            Ok(CsgOp::LinearPattern {
                child: parse_u64(parts[1], line_num)?,
                direction: Vec3::new(
                    parse_f64(parts[2], line_num)?,
                    parse_f64(parts[3], line_num)?,
                    parse_f64(parts[4], line_num)?,
                ),
                count: parse_u32(parts[5], line_num)?,
                spacing: parse_f64(parts[6], line_num)?,
            })
        }

        "CP" => {
            if parts.len() != 10 {
                return Err(VCodeParseError {
                    line: line_num,
                    message: format!("CP requires 9 args, got {}", parts.len() - 1),
                });
            }
            Ok(CsgOp::CircularPattern {
                child: parse_u64(parts[1], line_num)?,
                axis_origin: Vec3::new(
                    parse_f64(parts[2], line_num)?,
                    parse_f64(parts[3], line_num)?,
                    parse_f64(parts[4], line_num)?,
                ),
                axis_dir: Vec3::new(
                    parse_f64(parts[5], line_num)?,
                    parse_f64(parts[6], line_num)?,
                    parse_f64(parts[7], line_num)?,
                ),
                count: parse_u32(parts[8], line_num)?,
                angle_deg: parse_f64(parts[9], line_num)?,
            })
        }

        "SH" => {
            if parts.len() != 3 {
                return Err(VCodeParseError {
                    line: line_num,
                    message: format!("SH requires 2 args, got {}", parts.len() - 1),
                });
            }
            Ok(CsgOp::Shell {
                child: parse_u64(parts[1], line_num)?,
                thickness: parse_f64(parts[2], line_num)?,
            })
        }

        "FI" => {
            if parts.len() != 3 {
                return Err(VCodeParseError {
                    line: line_num,
                    message: format!("FI requires 2 args, got {}", parts.len() - 1),
                });
            }
            Ok(CsgOp::Fillet {
                child: parse_u64(parts[1], line_num)?,
                radius: parse_f64(parts[2], line_num)?,
            })
        }

        "CH" => {
            if parts.len() != 3 {
                return Err(VCodeParseError {
                    line: line_num,
                    message: format!("CH requires 2 args, got {}", parts.len() - 1),
                });
            }
            Ok(CsgOp::Chamfer {
                child: parse_u64(parts[1], line_num)?,
                distance: parse_f64(parts[2], line_num)?,
            })
        }

        "SK" => {
            if parts.len() != 10 {
                return Err(VCodeParseError {
                    line: line_num,
                    message: format!("SK requires 9 args, got {}", parts.len() - 1),
                });
            }

            let origin = Vec3::new(
                parse_f64(parts[1], line_num)?,
                parse_f64(parts[2], line_num)?,
                parse_f64(parts[3], line_num)?,
            );
            let x_dir = Vec3::new(
                parse_f64(parts[4], line_num)?,
                parse_f64(parts[5], line_num)?,
                parse_f64(parts[6], line_num)?,
            );
            let y_dir = Vec3::new(
                parse_f64(parts[7], line_num)?,
                parse_f64(parts[8], line_num)?,
                parse_f64(parts[9], line_num)?,
            );

            let mut segments = Vec::new();

            // Parse sketch segments until END
            loop {
                *current_line += 1;
                let seg_line = lines.next().ok_or_else(|| VCodeParseError {
                    line: *current_line,
                    message: "unexpected end of sketch block".to_string(),
                })?;

                let seg_trimmed = seg_line.trim();
                if seg_trimmed == "END" {
                    break;
                }

                let seg_parts: Vec<&str> = seg_trimmed.split_whitespace().collect();
                if seg_parts.is_empty() {
                    continue; // Skip empty lines in sketch
                }

                match seg_parts[0] {
                    "L" => {
                        if seg_parts.len() != 5 {
                            return Err(VCodeParseError {
                                line: *current_line,
                                message: format!("L requires 4 args, got {}", seg_parts.len() - 1),
                            });
                        }
                        segments.push(SketchSegment2D::Line {
                            start: Vec2::new(
                                parse_f64(seg_parts[1], *current_line)?,
                                parse_f64(seg_parts[2], *current_line)?,
                            ),
                            end: Vec2::new(
                                parse_f64(seg_parts[3], *current_line)?,
                                parse_f64(seg_parts[4], *current_line)?,
                            ),
                        });
                    }
                    "A" => {
                        if seg_parts.len() != 8 {
                            return Err(VCodeParseError {
                                line: *current_line,
                                message: format!("A requires 7 args, got {}", seg_parts.len() - 1),
                            });
                        }
                        segments.push(SketchSegment2D::Arc {
                            start: Vec2::new(
                                parse_f64(seg_parts[1], *current_line)?,
                                parse_f64(seg_parts[2], *current_line)?,
                            ),
                            end: Vec2::new(
                                parse_f64(seg_parts[3], *current_line)?,
                                parse_f64(seg_parts[4], *current_line)?,
                            ),
                            center: Vec2::new(
                                parse_f64(seg_parts[5], *current_line)?,
                                parse_f64(seg_parts[6], *current_line)?,
                            ),
                            ccw: parse_u32(seg_parts[7], *current_line)? != 0,
                        });
                    }
                    _ => {
                        return Err(VCodeParseError {
                            line: *current_line,
                            message: format!("unknown sketch segment opcode: {}", seg_parts[0]),
                        });
                    }
                }
            }

            Ok(CsgOp::Sketch2D {
                origin,
                x_dir,
                y_dir,
                segments,
            })
        }

        "E" => {
            if parts.len() != 5 {
                return Err(VCodeParseError {
                    line: line_num,
                    message: format!("E requires 4 args, got {}", parts.len() - 1),
                });
            }
            Ok(CsgOp::Extrude {
                sketch: parse_u64(parts[1], line_num)?,
                direction: Vec3::new(
                    parse_f64(parts[2], line_num)?,
                    parse_f64(parts[3], line_num)?,
                    parse_f64(parts[4], line_num)?,
                ),
                twist_angle: None,
                scale_end: None,
            })
        }

        "V" => {
            if parts.len() != 9 {
                return Err(VCodeParseError {
                    line: line_num,
                    message: format!("V requires 8 args, got {}", parts.len() - 1),
                });
            }
            Ok(CsgOp::Revolve {
                sketch: parse_u64(parts[1], line_num)?,
                axis_origin: Vec3::new(
                    parse_f64(parts[2], line_num)?,
                    parse_f64(parts[3], line_num)?,
                    parse_f64(parts[4], line_num)?,
                ),
                axis_dir: Vec3::new(
                    parse_f64(parts[5], line_num)?,
                    parse_f64(parts[6], line_num)?,
                    parse_f64(parts[7], line_num)?,
                ),
                angle_deg: parse_f64(parts[8], line_num)?,
            })
        }

        _ => Err(VCodeParseError {
            line: line_num,
            message: format!("unknown opcode: {}", opcode),
        }),
    }
}

/// Get child node IDs from an operation.
fn get_children(op: &CsgOp) -> Vec<u64> {
    match op {
        CsgOp::Union { left, right }
        | CsgOp::Difference { left, right }
        | CsgOp::Intersection { left, right } => vec![*left, *right],
        CsgOp::Translate { child, .. }
        | CsgOp::Rotate { child, .. }
        | CsgOp::Scale { child, .. }
        | CsgOp::LinearPattern { child, .. }
        | CsgOp::CircularPattern { child, .. }
        | CsgOp::Shell { child, .. }
        | CsgOp::Fillet { child, .. }
        | CsgOp::Chamfer { child, .. } => vec![*child],
        CsgOp::Extrude { sketch, .. }
        | CsgOp::Revolve { sketch, .. }
        | CsgOp::Sweep { sketch, .. } => vec![*sketch],
        CsgOp::Loft { sketches, .. } => sketches.clone(),
        _ => vec![],
    }
}

/// Topological sort of nodes.
fn topological_sort(doc: &Document, roots: &[u64]) -> Result<Vec<u64>, VCodeParseError> {
    let mut result = Vec::new();
    let mut visited = std::collections::HashSet::new();
    let mut temp_visited = std::collections::HashSet::new();

    fn visit(
        node_id: u64,
        doc: &Document,
        visited: &mut std::collections::HashSet<u64>,
        temp_visited: &mut std::collections::HashSet<u64>,
        result: &mut Vec<u64>,
    ) -> Result<(), VCodeParseError> {
        if visited.contains(&node_id) {
            return Ok(());
        }
        if temp_visited.contains(&node_id) {
            return Err(VCodeParseError {
                line: 0,
                message: format!("cycle detected at node {}", node_id),
            });
        }

        temp_visited.insert(node_id);

        if let Some(node) = doc.nodes.get(&node_id) {
            for child_id in get_children(&node.op) {
                visit(child_id, doc, visited, temp_visited, result)?;
            }
        }

        temp_visited.remove(&node_id);
        visited.insert(node_id);
        result.push(node_id);
        Ok(())
    }

    for &root_id in roots {
        visit(root_id, doc, &mut visited, &mut temp_visited, &mut result)?;
    }

    // Also visit any orphan nodes
    let all_ids: Vec<u64> = doc.nodes.keys().copied().collect();
    for id in all_ids {
        if !visited.contains(&id) {
            visit(id, doc, &mut visited, &mut temp_visited, &mut result)?;
        }
    }

    Ok(result)
}

/// Format a CsgOp as a VCode line with optional name suffix.
fn format_op(
    op: &CsgOp,
    id_map: &HashMap<u64, usize>,
    name: Option<&str>,
) -> Result<String, VCodeParseError> {
    let name_suffix = name
        .map(|n| format!(" {}", format_quoted_string(n)))
        .unwrap_or_default();

    match op {
        CsgOp::Cube { size } => Ok(format!("C {} {} {}{}", size.x, size.y, size.z, name_suffix)),

        CsgOp::Cylinder { radius, height, .. } => {
            Ok(format!("Y {} {}{}", radius, height, name_suffix))
        }

        CsgOp::Sphere { radius, .. } => Ok(format!("S {}{}", radius, name_suffix)),

        CsgOp::Cone {
            radius_bottom,
            radius_top,
            height,
            ..
        } => Ok(format!(
            "K {} {} {}{}",
            radius_bottom, radius_top, height, name_suffix
        )),

        CsgOp::Empty => Ok(format!("C 0 0 0{}", name_suffix)),

        CsgOp::Union { left, right } => {
            let l = id_map.get(left).ok_or_else(|| VCodeParseError {
                line: 0,
                message: format!("unknown node {}", left),
            })?;
            let r = id_map.get(right).ok_or_else(|| VCodeParseError {
                line: 0,
                message: format!("unknown node {}", right),
            })?;
            Ok(format!("U {} {}{}", l, r, name_suffix))
        }

        CsgOp::Difference { left, right } => {
            let l = id_map.get(left).ok_or_else(|| VCodeParseError {
                line: 0,
                message: format!("unknown node {}", left),
            })?;
            let r = id_map.get(right).ok_or_else(|| VCodeParseError {
                line: 0,
                message: format!("unknown node {}", right),
            })?;
            Ok(format!("D {} {}{}", l, r, name_suffix))
        }

        CsgOp::Intersection { left, right } => {
            let l = id_map.get(left).ok_or_else(|| VCodeParseError {
                line: 0,
                message: format!("unknown node {}", left),
            })?;
            let r = id_map.get(right).ok_or_else(|| VCodeParseError {
                line: 0,
                message: format!("unknown node {}", right),
            })?;
            Ok(format!("I {} {}{}", l, r, name_suffix))
        }

        CsgOp::Translate { child, offset } => {
            let c = id_map.get(child).ok_or_else(|| VCodeParseError {
                line: 0,
                message: format!("unknown node {}", child),
            })?;
            Ok(format!(
                "T {} {} {} {}{}",
                c, offset.x, offset.y, offset.z, name_suffix
            ))
        }

        CsgOp::Rotate { child, angles } => {
            let c = id_map.get(child).ok_or_else(|| VCodeParseError {
                line: 0,
                message: format!("unknown node {}", child),
            })?;
            Ok(format!(
                "R {} {} {} {}{}",
                c, angles.x, angles.y, angles.z, name_suffix
            ))
        }

        CsgOp::Scale { child, factor } => {
            let c = id_map.get(child).ok_or_else(|| VCodeParseError {
                line: 0,
                message: format!("unknown node {}", child),
            })?;
            Ok(format!(
                "X {} {} {} {}{}",
                c, factor.x, factor.y, factor.z, name_suffix
            ))
        }

        CsgOp::LinearPattern {
            child,
            direction,
            count,
            spacing,
        } => {
            let c = id_map.get(child).ok_or_else(|| VCodeParseError {
                line: 0,
                message: format!("unknown node {}", child),
            })?;
            Ok(format!(
                "LP {} {} {} {} {} {}{}",
                c, direction.x, direction.y, direction.z, count, spacing, name_suffix
            ))
        }

        CsgOp::CircularPattern {
            child,
            axis_origin,
            axis_dir,
            count,
            angle_deg,
        } => {
            let c = id_map.get(child).ok_or_else(|| VCodeParseError {
                line: 0,
                message: format!("unknown node {}", child),
            })?;
            Ok(format!(
                "CP {} {} {} {} {} {} {} {} {}{}",
                c,
                axis_origin.x,
                axis_origin.y,
                axis_origin.z,
                axis_dir.x,
                axis_dir.y,
                axis_dir.z,
                count,
                angle_deg,
                name_suffix
            ))
        }

        CsgOp::Shell { child, thickness } => {
            let c = id_map.get(child).ok_or_else(|| VCodeParseError {
                line: 0,
                message: format!("unknown node {}", child),
            })?;
            Ok(format!("SH {} {}{}", c, thickness, name_suffix))
        }

        CsgOp::Fillet { child, radius } => {
            let c = id_map.get(child).ok_or_else(|| VCodeParseError {
                line: 0,
                message: format!("unknown node {}", child),
            })?;
            Ok(format!("FI {} {}{}", c, radius, name_suffix))
        }

        CsgOp::Chamfer { child, distance } => {
            let c = id_map.get(child).ok_or_else(|| VCodeParseError {
                line: 0,
                message: format!("unknown node {}", child),
            })?;
            Ok(format!("CH {} {}{}", c, distance, name_suffix))
        }

        CsgOp::Sketch2D {
            origin,
            x_dir,
            y_dir,
            segments,
        } => {
            let mut lines = vec![format!(
                "SK {} {} {}  {} {} {}  {} {} {}{}",
                origin.x,
                origin.y,
                origin.z,
                x_dir.x,
                x_dir.y,
                x_dir.z,
                y_dir.x,
                y_dir.y,
                y_dir.z,
                name_suffix
            )];

            for seg in segments {
                match seg {
                    SketchSegment2D::Line { start, end } => {
                        lines.push(format!("L {} {} {} {}", start.x, start.y, end.x, end.y));
                    }
                    SketchSegment2D::Arc {
                        start,
                        end,
                        center,
                        ccw,
                    } => {
                        lines.push(format!(
                            "A {} {} {} {} {} {} {}",
                            start.x,
                            start.y,
                            end.x,
                            end.y,
                            center.x,
                            center.y,
                            if *ccw { 1 } else { 0 }
                        ));
                    }
                }
            }

            lines.push("END".to_string());
            Ok(lines.join("\n"))
        }

        CsgOp::Extrude {
            sketch, direction, ..
        } => {
            let sk = id_map.get(sketch).ok_or_else(|| VCodeParseError {
                line: 0,
                message: format!("unknown node {}", sketch),
            })?;
            // Note: twist_angle and scale_end are not serialized to VCode format
            Ok(format!(
                "E {} {} {} {}{}",
                sk, direction.x, direction.y, direction.z, name_suffix
            ))
        }

        CsgOp::Revolve {
            sketch,
            axis_origin,
            axis_dir,
            angle_deg,
        } => {
            let sk = id_map.get(sketch).ok_or_else(|| VCodeParseError {
                line: 0,
                message: format!("unknown node {}", sketch),
            })?;
            Ok(format!(
                "V {} {} {} {} {} {} {} {}{}",
                sk,
                axis_origin.x,
                axis_origin.y,
                axis_origin.z,
                axis_dir.x,
                axis_dir.y,
                axis_dir.z,
                angle_deg,
                name_suffix
            ))
        }

        CsgOp::StepImport { .. } => Err(VCodeParseError {
            line: 0,
            message: "STEP import not supported in VCode format".to_string(),
        }),

        CsgOp::Text2D { .. } => Err(VCodeParseError {
            line: 0,
            message: "Text2D not supported in VCode format".to_string(),
        }),

        CsgOp::Sweep { .. } => Err(VCodeParseError {
            line: 0,
            message: "Sweep not supported in VCode format".to_string(),
        }),

        CsgOp::Loft { .. } => Err(VCodeParseError {
            line: 0,
            message: "Loft not supported in VCode format".to_string(),
        }),

        CsgOp::ImportedMesh { .. } => Err(VCodeParseError {
            line: 0,
            message: "ImportedMesh not supported in VCode format".to_string(),
        }),

        CsgOp::PcbBoard { .. } => Err(VCodeParseError {
            line: 0,
            message: "PcbBoard not supported in VCode format".to_string(),
        }),

        CsgOp::EmbroideryPattern { .. } => Err(VCodeParseError {
            line: 0,
            message: "EmbroideryPattern not supported in VCode format".to_string(),
        }),
    }
}

fn parse_f64(s: &str, line: usize) -> Result<f64, VCodeParseError> {
    s.parse().map_err(|_| VCodeParseError {
        line,
        message: format!("invalid number: {}", s),
    })
}

fn parse_u64(s: &str, line: usize) -> Result<u64, VCodeParseError> {
    s.parse().map_err(|_| VCodeParseError {
        line,
        message: format!("invalid node id: {}", s),
    })
}

fn parse_u32(s: &str, line: usize) -> Result<u32, VCodeParseError> {
    s.parse().map_err(|_| VCodeParseError {
        line,
        message: format!("invalid integer: {}", s),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_cube() {
        let compact = "C 50 30 5";
        let doc = from_vcode(compact).unwrap();

        assert_eq!(doc.nodes.len(), 1);
        let node = &doc.nodes[&0];
        match &node.op {
            CsgOp::Cube { size } => {
                assert_eq!(size.x, 50.0);
                assert_eq!(size.y, 30.0);
                assert_eq!(size.z, 5.0);
            }
            _ => panic!("expected Cube"),
        }
    }

    #[test]
    fn test_plate_with_hole() {
        let compact = "C 50 30 5\nY 5 10\nT 1 25 15 0\nD 0 2";
        let doc = from_vcode(compact).unwrap();

        assert_eq!(doc.nodes.len(), 4);

        // Node 0: Cube
        match &doc.nodes[&0].op {
            CsgOp::Cube { size } => {
                assert_eq!(size.x, 50.0);
                assert_eq!(size.y, 30.0);
                assert_eq!(size.z, 5.0);
            }
            _ => panic!("expected Cube at node 0"),
        }

        // Node 1: Cylinder
        match &doc.nodes[&1].op {
            CsgOp::Cylinder { radius, height, .. } => {
                assert_eq!(*radius, 5.0);
                assert_eq!(*height, 10.0);
            }
            _ => panic!("expected Cylinder at node 1"),
        }

        // Node 2: Translate
        match &doc.nodes[&2].op {
            CsgOp::Translate { child, offset } => {
                assert_eq!(*child, 1);
                assert_eq!(offset.x, 25.0);
                assert_eq!(offset.y, 15.0);
                assert_eq!(offset.z, 0.0);
            }
            _ => panic!("expected Translate at node 2"),
        }

        // Node 3: Difference
        match &doc.nodes[&3].op {
            CsgOp::Difference { left, right } => {
                assert_eq!(*left, 0);
                assert_eq!(*right, 2);
            }
            _ => panic!("expected Difference at node 3"),
        }

        // Root should be node 3
        assert_eq!(doc.roots[0].root, 3);
    }

    #[test]
    fn test_roundtrip_cube() {
        let mut doc = Document::new();
        doc.materials.insert(
            "default".to_string(),
            MaterialDef {
                name: "default".to_string(),
                color: [0.8, 0.8, 0.8],
                metallic: 0.0,
                roughness: 0.5,
                density: None,
                friction: None,
            },
        );
        doc.nodes.insert(
            0,
            Node {
                id: 0,
                name: None,
                op: CsgOp::Cube {
                    size: Vec3::new(10.0, 20.0, 30.0),
                },
            },
        );
        doc.roots.push(SceneEntry {
            root: 0,
            material: "default".to_string(),
            visible: None,
        });

        let compact = to_vcode(&doc).unwrap();
        // New format includes header, materials, and scene sections
        assert!(compact.contains("C 10 20 30"));
        assert!(compact.contains("ROOT 0 default"));

        let restored = from_vcode(&compact).unwrap();
        match &restored.nodes[&0].op {
            CsgOp::Cube { size } => {
                assert_eq!(size.x, 10.0);
                assert_eq!(size.y, 20.0);
                assert_eq!(size.z, 30.0);
            }
            _ => panic!("expected Cube"),
        }
    }

    #[test]
    fn test_roundtrip_plate_with_hole() {
        let mut doc = Document::new();

        // Cube
        doc.nodes.insert(
            0,
            Node {
                id: 0,
                name: None,
                op: CsgOp::Cube {
                    size: Vec3::new(50.0, 30.0, 5.0),
                },
            },
        );

        // Cylinder
        doc.nodes.insert(
            1,
            Node {
                id: 1,
                name: None,
                op: CsgOp::Cylinder {
                    radius: 5.0,
                    height: 10.0,
                    segments: 0,
                },
            },
        );

        // Translate
        doc.nodes.insert(
            2,
            Node {
                id: 2,
                name: None,
                op: CsgOp::Translate {
                    child: 1,
                    offset: Vec3::new(25.0, 15.0, 0.0),
                },
            },
        );

        // Difference
        doc.nodes.insert(
            3,
            Node {
                id: 3,
                name: None,
                op: CsgOp::Difference { left: 0, right: 2 },
            },
        );

        doc.roots.push(SceneEntry {
            root: 3,
            material: "default".to_string(),
            visible: None,
        });

        let compact = to_vcode(&doc).unwrap();
        // Check that geometry section contains expected ops
        assert!(compact.contains("C 50 30 5"));
        assert!(compact.contains("Y 5 10"));
        assert!(compact.contains("T 1 25 15 0"));
        assert!(compact.contains("D 0 2"));
    }

    #[test]
    fn test_all_primitives() {
        let compact = "C 10 20 30\nY 5 15\nS 8\nK 5 2 20";
        let doc = from_vcode(compact).unwrap();

        assert_eq!(doc.nodes.len(), 4);

        match &doc.nodes[&0].op {
            CsgOp::Cube { size } => assert_eq!(*size, Vec3::new(10.0, 20.0, 30.0)),
            _ => panic!("expected Cube"),
        }

        match &doc.nodes[&1].op {
            CsgOp::Cylinder { radius, height, .. } => {
                assert_eq!(*radius, 5.0);
                assert_eq!(*height, 15.0);
            }
            _ => panic!("expected Cylinder"),
        }

        match &doc.nodes[&2].op {
            CsgOp::Sphere { radius, .. } => assert_eq!(*radius, 8.0),
            _ => panic!("expected Sphere"),
        }

        match &doc.nodes[&3].op {
            CsgOp::Cone {
                radius_bottom,
                radius_top,
                height,
                ..
            } => {
                assert_eq!(*radius_bottom, 5.0);
                assert_eq!(*radius_top, 2.0);
                assert_eq!(*height, 20.0);
            }
            _ => panic!("expected Cone"),
        }
    }

    #[test]
    fn test_all_booleans() {
        let compact = "C 10 10 10\nC 5 5 5\nU 0 1\nD 0 1\nI 0 1";
        let doc = from_vcode(compact).unwrap();

        match &doc.nodes[&2].op {
            CsgOp::Union { left, right } => {
                assert_eq!(*left, 0);
                assert_eq!(*right, 1);
            }
            _ => panic!("expected Union"),
        }

        match &doc.nodes[&3].op {
            CsgOp::Difference { left, right } => {
                assert_eq!(*left, 0);
                assert_eq!(*right, 1);
            }
            _ => panic!("expected Difference"),
        }

        match &doc.nodes[&4].op {
            CsgOp::Intersection { left, right } => {
                assert_eq!(*left, 0);
                assert_eq!(*right, 1);
            }
            _ => panic!("expected Intersection"),
        }
    }

    #[test]
    fn test_all_transforms() {
        let compact = "C 10 10 10\nT 0 5 10 15\nR 1 45 0 90\nX 2 2 2 2";
        let doc = from_vcode(compact).unwrap();

        match &doc.nodes[&1].op {
            CsgOp::Translate { child, offset } => {
                assert_eq!(*child, 0);
                assert_eq!(*offset, Vec3::new(5.0, 10.0, 15.0));
            }
            _ => panic!("expected Translate"),
        }

        match &doc.nodes[&2].op {
            CsgOp::Rotate { child, angles } => {
                assert_eq!(*child, 1);
                assert_eq!(*angles, Vec3::new(45.0, 0.0, 90.0));
            }
            _ => panic!("expected Rotate"),
        }

        match &doc.nodes[&3].op {
            CsgOp::Scale { child, factor } => {
                assert_eq!(*child, 2);
                assert_eq!(*factor, Vec3::new(2.0, 2.0, 2.0));
            }
            _ => panic!("expected Scale"),
        }
    }

    #[test]
    fn test_linear_pattern() {
        let compact = "C 10 10 5\nLP 0 1 0 0 5 20";
        let doc = from_vcode(compact).unwrap();

        match &doc.nodes[&1].op {
            CsgOp::LinearPattern {
                child,
                direction,
                count,
                spacing,
            } => {
                assert_eq!(*child, 0);
                assert_eq!(*direction, Vec3::new(1.0, 0.0, 0.0));
                assert_eq!(*count, 5);
                assert_eq!(*spacing, 20.0);
            }
            _ => panic!("expected LinearPattern"),
        }
    }

    #[test]
    fn test_circular_pattern() {
        let compact = "Y 3 10\nCP 0 0 0 0 0 0 1 6 360";
        let doc = from_vcode(compact).unwrap();

        match &doc.nodes[&1].op {
            CsgOp::CircularPattern {
                child,
                axis_origin,
                axis_dir,
                count,
                angle_deg,
            } => {
                assert_eq!(*child, 0);
                assert_eq!(*axis_origin, Vec3::new(0.0, 0.0, 0.0));
                assert_eq!(*axis_dir, Vec3::new(0.0, 0.0, 1.0));
                assert_eq!(*count, 6);
                assert_eq!(*angle_deg, 360.0);
            }
            _ => panic!("expected CircularPattern"),
        }
    }

    #[test]
    fn test_shell() {
        let compact = "C 50 50 50\nSH 0 2";
        let doc = from_vcode(compact).unwrap();

        match &doc.nodes[&1].op {
            CsgOp::Shell { child, thickness } => {
                assert_eq!(*child, 0);
                assert_eq!(*thickness, 2.0);
            }
            _ => panic!("expected Shell"),
        }
    }

    #[test]
    fn test_sketch_extrude() {
        let compact = "SK 0 0 0  1 0 0  0 1 0\nL 0 0 10 0\nL 10 0 10 5\nL 10 5 0 5\nL 0 5 0 0\nEND\nE 0 0 0 20";
        let doc = from_vcode(compact).unwrap();

        // Node IDs are sequential: Sketch is 0, Extrude is 1
        match &doc.nodes[&0].op {
            CsgOp::Sketch2D {
                origin,
                x_dir,
                y_dir,
                segments,
            } => {
                assert_eq!(*origin, Vec3::new(0.0, 0.0, 0.0));
                assert_eq!(*x_dir, Vec3::new(1.0, 0.0, 0.0));
                assert_eq!(*y_dir, Vec3::new(0.0, 1.0, 0.0));
                assert_eq!(segments.len(), 4);
            }
            _ => panic!("expected Sketch2D"),
        }

        // Extrude is node 1 (sequential)
        match &doc.nodes[&1].op {
            CsgOp::Extrude {
                sketch, direction, ..
            } => {
                assert_eq!(*sketch, 0);
                assert_eq!(*direction, Vec3::new(0.0, 0.0, 20.0));
            }
            _ => panic!("expected Extrude"),
        }
    }

    #[test]
    fn test_sketch_revolve() {
        let compact =
            "SK 0 0 0  1 0 0  0 1 0\nL 5 0 10 0\nL 10 0 10 20\nL 10 20 5 20\nL 5 20 5 0\nEND\nV 0 0 0 0 0 1 0 360";
        let doc = from_vcode(compact).unwrap();

        // Revolve is node 1 (sequential)
        match &doc.nodes[&1].op {
            CsgOp::Revolve {
                sketch,
                axis_origin,
                axis_dir,
                angle_deg,
            } => {
                assert_eq!(*sketch, 0);
                assert_eq!(*axis_origin, Vec3::new(0.0, 0.0, 0.0));
                assert_eq!(*axis_dir, Vec3::new(0.0, 1.0, 0.0));
                assert_eq!(*angle_deg, 360.0);
            }
            _ => panic!("expected Revolve"),
        }
    }

    #[test]
    fn test_sketch_with_arc() {
        let compact = "SK 0 0 0  1 0 0  0 1 0\nL 0 0 10 0\nA 10 0 10 10 10 5 1\nL 10 10 0 10\nL 0 10 0 0\nEND";
        let doc = from_vcode(compact).unwrap();

        match &doc.nodes[&0].op {
            CsgOp::Sketch2D { segments, .. } => {
                assert_eq!(segments.len(), 4);
                match &segments[1] {
                    SketchSegment2D::Arc {
                        start,
                        end,
                        center,
                        ccw,
                    } => {
                        assert_eq!(*start, Vec2::new(10.0, 0.0));
                        assert_eq!(*end, Vec2::new(10.0, 10.0));
                        assert_eq!(*center, Vec2::new(10.0, 5.0));
                        assert!(*ccw);
                    }
                    _ => panic!("expected Arc"),
                }
            }
            _ => panic!("expected Sketch2D"),
        }
    }

    #[test]
    fn test_comments_and_empty_lines() {
        let compact = "# This is a comment\nC 10 10 10\n\n# Another comment\nY 5 10";
        let doc = from_vcode(compact).unwrap();

        // Comments and empty lines are skipped
        // Node IDs are assigned sequentially for geometry ops (0, 1, ...)
        assert_eq!(doc.nodes.len(), 2);
        assert!(doc.nodes.contains_key(&0));
        assert!(doc.nodes.contains_key(&1));
    }

    #[test]
    fn test_parse_error_invalid_opcode() {
        let compact = "Z 10 10 10";
        let result = from_vcode(compact);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.line, 0);
        assert!(err.message.contains("unknown opcode"));
    }

    #[test]
    fn test_parse_error_wrong_arg_count() {
        let compact = "C 10 10"; // Missing third arg
        let result = from_vcode(compact);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("requires 3 args"));
    }

    #[test]
    fn test_parse_error_invalid_number() {
        let compact = "C 10 abc 10";
        let result = from_vcode(compact);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("invalid number"));
    }

    #[test]
    fn test_negative_numbers() {
        let compact = "C 10 10 10\nT 0 -5 -10 -15";
        let doc = from_vcode(compact).unwrap();

        match &doc.nodes[&1].op {
            CsgOp::Translate { offset, .. } => {
                assert_eq!(offset.x, -5.0);
                assert_eq!(offset.y, -10.0);
                assert_eq!(offset.z, -15.0);
            }
            _ => panic!("expected Translate"),
        }
    }

    #[test]
    fn test_floating_point() {
        let compact = "C 10.5 20.25 30.125";
        let doc = from_vcode(compact).unwrap();

        match &doc.nodes[&0].op {
            CsgOp::Cube { size } => {
                assert_eq!(size.x, 10.5);
                assert_eq!(size.y, 20.25);
                assert_eq!(size.z, 30.125);
            }
            _ => panic!("expected Cube"),
        }
    }

    #[test]
    fn test_empty_input() {
        let compact = "";
        let doc = from_vcode(compact).unwrap();
        assert!(doc.nodes.is_empty());
        assert!(doc.roots.is_empty());
    }

    #[test]
    fn test_complex_model() {
        // Flange with 6 bolt holes
        let compact = r#"Y 25 5
Y 3 10
T 1 15 0 0
CP 2 0 0 0 0 0 1 6 360
D 0 3"#;

        let doc = from_vcode(compact).unwrap();
        assert_eq!(doc.nodes.len(), 5);

        // Final difference should be root
        assert_eq!(doc.roots[0].root, 4);
    }

    #[test]
    fn test_material_roundtrip() {
        let compact = r#"# vcad 0.2

# Materials
M default 0.8 0.8 0.8 0 0.5
M aluminum 0.9 0.91 0.92 0.9 0.3 2700 0.6

# Geometry
C 50 30 5

# Scene
ROOT 0 aluminum"#;

        let doc = from_vcode(compact).unwrap();
        assert_eq!(doc.materials.len(), 2);

        let default = &doc.materials["default"];
        assert_eq!(default.color, [0.8, 0.8, 0.8]);
        assert_eq!(default.metallic, 0.0);
        assert_eq!(default.roughness, 0.5);
        assert!(default.density.is_none());

        let aluminum = &doc.materials["aluminum"];
        assert_eq!(aluminum.color, [0.9, 0.91, 0.92]);
        assert_eq!(aluminum.metallic, 0.9);
        assert_eq!(aluminum.roughness, 0.3);
        assert_eq!(aluminum.density, Some(2700.0));
        assert_eq!(aluminum.friction, Some(0.6));

        assert_eq!(doc.roots[0].material, "aluminum");
    }

    #[test]
    fn test_node_names() {
        let compact = r#"C 50 30 5 "Base Plate"
Y 5 10 "Hole"
T 1 25 15 0
D 0 2 "Plate with Hole""#;

        let doc = from_vcode(compact).unwrap();
        assert_eq!(doc.nodes[&0].name, Some("Base Plate".to_string()));
        assert_eq!(doc.nodes[&1].name, Some("Hole".to_string()));
        assert_eq!(doc.nodes[&2].name, None);
        assert_eq!(doc.nodes[&3].name, Some("Plate with Hole".to_string()));
    }

    #[test]
    fn test_scene_root_hidden() {
        let compact = r#"M default 0.8 0.8 0.8 0 0.5
C 50 30 5
ROOT 0 default hidden"#;

        let doc = from_vcode(compact).unwrap();
        assert_eq!(doc.roots[0].visible, Some(false));
    }

    #[test]
    fn test_assembly_part_def() {
        let compact = r#"C 50 30 5
PDEF base "Base Part" 0 aluminum"#;

        let doc = from_vcode(compact).unwrap();
        let part_defs = doc.part_defs.as_ref().unwrap();
        let pdef = &part_defs["base"];
        assert_eq!(pdef.id, "base");
        assert_eq!(pdef.name, Some("Base Part".to_string()));
        assert_eq!(pdef.root, 0);
        assert_eq!(pdef.default_material, Some("aluminum".to_string()));
    }

    #[test]
    fn test_assembly_instance() {
        let compact = r#"C 50 30 5
PDEF base "Base Part" 0
INST i1 base "Instance 1" 0 0 0 0 0 0 1 1 1
INST i2 base "Instance 2" 100 0 0 45 0 0 1 1 1 steel"#;

        let doc = from_vcode(compact).unwrap();
        let instances = doc.instances.as_ref().unwrap();
        assert_eq!(instances.len(), 2);

        let i1 = &instances[0];
        assert_eq!(i1.id, "i1");
        assert_eq!(i1.part_def_id, "base");
        assert_eq!(i1.name, Some("Instance 1".to_string()));
        let tf1 = i1.transform.as_ref().unwrap();
        assert_eq!(tf1.translation, Vec3::new(0.0, 0.0, 0.0));
        assert!(i1.material.is_none());

        let i2 = &instances[1];
        assert_eq!(i2.id, "i2");
        let tf2 = i2.transform.as_ref().unwrap();
        assert_eq!(tf2.translation, Vec3::new(100.0, 0.0, 0.0));
        assert_eq!(tf2.rotation, Vec3::new(45.0, 0.0, 0.0));
        assert_eq!(i2.material, Some("steel".to_string()));
    }

    #[test]
    fn test_joints() {
        let compact = r#"C 50 30 5
PDEF base "Base" 0
INST i1 base "I1" 0 0 0 0 0 0 1 1 1
INST i2 base "I2" 100 0 0 0 0 0 1 1 1
JFIX j1 i1 i2 50 0 0 0 0 0
JREV j2 i1 i2 50 0 0 0 0 0 0 0 1
JREV j3 _ i2 0 0 0 0 0 0 0 0 1 -90 90
JSLD j4 i1 i2 0 0 0 0 0 0 1 0 0 0 100
JCYL j5 i1 i2 50 0 0 0 0 0 0 0 1
JBAL j6 i1 i2 50 0 0 0 0 0
GROUND i1"#;

        let doc = from_vcode(compact).unwrap();
        let joints = doc.joints.as_ref().unwrap();
        assert_eq!(joints.len(), 6);

        // Fixed joint
        assert_eq!(joints[0].id, "j1");
        assert!(matches!(joints[0].kind, JointKind::Fixed));

        // Revolute without limits
        assert_eq!(joints[1].id, "j2");
        match &joints[1].kind {
            JointKind::Revolute { axis, limits } => {
                assert_eq!(*axis, Vec3::new(0.0, 0.0, 1.0));
                assert!(limits.is_none());
            }
            _ => panic!("expected Revolute"),
        }

        // Revolute with limits and no parent
        assert_eq!(joints[2].id, "j3");
        assert!(joints[2].parent_instance_id.is_none());
        match &joints[2].kind {
            JointKind::Revolute { limits, .. } => {
                assert_eq!(*limits, Some((-90.0, 90.0)));
            }
            _ => panic!("expected Revolute"),
        }

        // Slider with limits
        match &joints[3].kind {
            JointKind::Slider { axis, limits } => {
                assert_eq!(*axis, Vec3::new(1.0, 0.0, 0.0));
                assert_eq!(*limits, Some((0.0, 100.0)));
            }
            _ => panic!("expected Slider"),
        }

        // Cylindrical
        assert!(matches!(joints[4].kind, JointKind::Cylindrical { .. }));

        // Ball
        assert!(matches!(joints[5].kind, JointKind::Ball));

        // Ground
        assert_eq!(doc.ground_instance_id, Some("i1".to_string()));
    }

    #[test]
    fn test_environment_preset() {
        let compact = r#"C 10 10 10
ENV studio 1.5"#;

        let doc = from_vcode(compact).unwrap();
        let scene = doc.scene.as_ref().unwrap();
        match &scene.environment {
            Some(Environment::Preset { preset, intensity }) => {
                assert!(matches!(preset, EnvironmentPreset::Studio));
                assert_eq!(*intensity, Some(1.5));
            }
            _ => panic!("expected Preset environment"),
        }
    }

    #[test]
    fn test_background_solid() {
        let compact = r#"C 10 10 10
BG solid 0.1 0.1 0.1"#;

        let doc = from_vcode(compact).unwrap();
        let scene = doc.scene.as_ref().unwrap();
        match &scene.background {
            Some(Background::Solid { color }) => {
                assert_eq!(*color, [0.1, 0.1, 0.1]);
            }
            _ => panic!("expected Solid background"),
        }
    }

    #[test]
    fn test_background_gradient() {
        let compact = r#"C 10 10 10
BG gradient 0.1 0.1 0.2 0.9 0.9 1.0"#;

        let doc = from_vcode(compact).unwrap();
        let scene = doc.scene.as_ref().unwrap();
        match &scene.background {
            Some(Background::Gradient { top, bottom }) => {
                assert_eq!(*top, [0.1, 0.1, 0.2]);
                assert_eq!(*bottom, [0.9, 0.9, 1.0]);
            }
            _ => panic!("expected Gradient background"),
        }
    }

    #[test]
    fn test_lights() {
        let compact = r#"C 10 10 10
LDIR light1 1 1 1 1.5 0 -1 0 shadow
LPNT light2 1 0.9 0.8 2.0 10 20 30 100
LSPT light3 1 1 1 1 0 10 0 0 -1 0 45 0.1"#;

        let doc = from_vcode(compact).unwrap();
        let lights = doc.scene.as_ref().unwrap().lights.as_ref().unwrap();
        assert_eq!(lights.len(), 3);

        // Directional with shadow
        match &lights[0].kind {
            LightKind::Directional { direction } => {
                assert_eq!(*direction, Vec3::new(0.0, -1.0, 0.0));
            }
            _ => panic!("expected Directional"),
        }
        assert_eq!(lights[0].cast_shadow, Some(true));

        // Point with distance
        match &lights[1].kind {
            LightKind::Point { position, distance } => {
                assert_eq!(*position, Vec3::new(10.0, 20.0, 30.0));
                assert_eq!(*distance, Some(100.0));
            }
            _ => panic!("expected Point"),
        }

        // Spot with angle and penumbra
        match &lights[2].kind {
            LightKind::Spot {
                position,
                direction,
                angle,
                penumbra,
            } => {
                assert_eq!(*position, Vec3::new(0.0, 10.0, 0.0));
                assert_eq!(*direction, Vec3::new(0.0, -1.0, 0.0));
                assert_eq!(*angle, Some(45.0));
                assert_eq!(*penumbra, Some(0.1));
            }
            _ => panic!("expected Spot"),
        }
    }

    #[test]
    fn test_post_processing() {
        let compact = r#"C 10 10 10
AO 1 0.5 1.0
BLOOM 1 0.6 0.8
VIG 1 0.3 0.5
TONE acesFilmic
EXP 0.5"#;

        let doc = from_vcode(compact).unwrap();
        let pp = doc
            .scene
            .as_ref()
            .unwrap()
            .post_processing
            .as_ref()
            .unwrap();

        let ao = pp.ambient_occlusion.as_ref().unwrap();
        assert!(ao.enabled);
        assert_eq!(ao.intensity, Some(0.5));
        assert_eq!(ao.radius, Some(1.0));

        let bloom = pp.bloom.as_ref().unwrap();
        assert!(bloom.enabled);
        assert_eq!(bloom.intensity, Some(0.6));
        assert_eq!(bloom.threshold, Some(0.8));

        let vig = pp.vignette.as_ref().unwrap();
        assert!(vig.enabled);
        assert_eq!(vig.offset, Some(0.3));
        assert_eq!(vig.darkness, Some(0.5));

        assert!(matches!(pp.tone_mapping, Some(ToneMapping::AcesFilmic)));
        assert_eq!(pp.exposure, Some(0.5));
    }

    #[test]
    fn test_camera_presets() {
        let compact = r#"C 10 10 10
CAM cam1 100 100 100 0 0 0 60 "Front View"
CAM cam2 0 100 0 0 0 0"#;

        let doc = from_vcode(compact).unwrap();
        let cams = doc.scene.as_ref().unwrap().camera_presets.as_ref().unwrap();
        assert_eq!(cams.len(), 2);

        assert_eq!(cams[0].id, "cam1");
        assert_eq!(cams[0].position, Vec3::new(100.0, 100.0, 100.0));
        assert_eq!(cams[0].target, Vec3::new(0.0, 0.0, 0.0));
        assert_eq!(cams[0].fov, Some(60.0));
        assert_eq!(cams[0].name, Some("Front View".to_string()));

        assert_eq!(cams[1].id, "cam2");
        assert!(cams[1].fov.is_none());
        assert!(cams[1].name.is_none());
    }

    #[test]
    fn test_full_document_roundtrip() {
        // Create a document with all features
        let mut doc = Document::new();

        // Materials
        doc.materials.insert(
            "aluminum".to_string(),
            MaterialDef {
                name: "aluminum".to_string(),
                color: [0.9, 0.91, 0.92],
                metallic: 0.9,
                roughness: 0.3,
                density: Some(2700.0),
                friction: Some(0.6),
            },
        );

        // Nodes
        doc.nodes.insert(
            0,
            Node {
                id: 0,
                name: Some("Cube".to_string()),
                op: CsgOp::Cube {
                    size: Vec3::new(10.0, 10.0, 10.0),
                },
            },
        );

        // Scene roots
        doc.roots.push(SceneEntry {
            root: 0,
            material: "aluminum".to_string(),
            visible: None,
        });

        // Part defs
        let mut part_defs = HashMap::new();
        part_defs.insert(
            "part1".to_string(),
            PartDef {
                id: "part1".to_string(),
                name: Some("Part 1".to_string()),
                root: 0,
                default_material: Some("aluminum".to_string()),
            },
        );
        doc.part_defs = Some(part_defs);

        // Instances
        doc.instances = Some(vec![
            Instance {
                id: "inst1".to_string(),
                part_def_id: "part1".to_string(),
                name: Some("Instance 1".to_string()),
                transform: Some(Transform3D::default()),
                material: None,
            },
            Instance {
                id: "inst2".to_string(),
                part_def_id: "part1".to_string(),
                name: Some("Instance 2".to_string()),
                transform: Some(Transform3D {
                    translation: Vec3::new(50.0, 0.0, 0.0),
                    rotation: Vec3::new(0.0, 0.0, 0.0),
                    scale: Vec3::new(1.0, 1.0, 1.0),
                }),
                material: None,
            },
        ]);

        // Joints
        doc.joints = Some(vec![Joint {
            id: "joint1".to_string(),
            name: None,
            parent_instance_id: Some("inst1".to_string()),
            child_instance_id: "inst2".to_string(),
            parent_anchor: Vec3::new(5.0, 0.0, 0.0),
            child_anchor: Vec3::new(0.0, 0.0, 0.0),
            kind: JointKind::Revolute {
                axis: Vec3::new(0.0, 0.0, 1.0),
                limits: Some((-90.0, 90.0)),
            },
            state: 0.0,
        }]);

        // Ground
        doc.ground_instance_id = Some("inst1".to_string());

        // Scene settings
        doc.scene = Some(SceneSettings {
            environment: Some(Environment::Preset {
                preset: EnvironmentPreset::Studio,
                intensity: Some(1.0),
            }),
            background: Some(Background::Solid {
                color: [0.1, 0.1, 0.1],
            }),
            lights: None,
            post_processing: None,
            camera_presets: None,
        });

        // Serialize
        let compact = to_vcode(&doc).unwrap();

        // Parse back
        let restored = from_vcode(&compact).unwrap();

        // Verify materials
        assert_eq!(restored.materials.len(), 1);
        let mat = &restored.materials["aluminum"];
        assert_eq!(mat.density, Some(2700.0));

        // Verify nodes
        assert_eq!(restored.nodes.len(), 1);
        assert_eq!(restored.nodes[&0].name, Some("Cube".to_string()));

        // Verify roots
        assert_eq!(restored.roots.len(), 1);
        assert_eq!(restored.roots[0].material, "aluminum");

        // Verify assembly
        let pdefs = restored.part_defs.unwrap();
        assert_eq!(pdefs.len(), 1);
        assert_eq!(pdefs["part1"].name, Some("Part 1".to_string()));

        let insts = restored.instances.unwrap();
        assert_eq!(insts.len(), 2);

        let joints = restored.joints.unwrap();
        assert_eq!(joints.len(), 1);
        match &joints[0].kind {
            JointKind::Revolute { limits, .. } => {
                assert_eq!(*limits, Some((-90.0, 90.0)));
            }
            _ => panic!("expected Revolute"),
        }

        assert_eq!(restored.ground_instance_id, Some("inst1".to_string()));

        // Verify scene settings
        let scene = restored.scene.unwrap();
        assert!(matches!(
            scene.environment,
            Some(Environment::Preset { .. })
        ));
        assert!(matches!(scene.background, Some(Background::Solid { .. })));
    }
}
