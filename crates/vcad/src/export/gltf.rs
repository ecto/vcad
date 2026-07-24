//! glTF/GLB export for visualization.
//!
//! Thin adapter over the shared `vcad-kernel-export` GLB writer — the single
//! source of truth for GLB byte serialization.

use super::materials::Material;
use super::Materials;
use crate::{CadError, Part, Scene};
use std::path::Path;
use vcad_kernel_export::{build_glb, GlbMeshSpec, GlbSpec};

fn mesh_spec_for(
    name: &str,
    material: &Material,
    f32_data: &mut Vec<f32>,
    u32_data: &mut Vec<u32>,
    vertices: &[f32],
    indices: &[u32],
) -> GlbMeshSpec {
    let pos_off = f32_data.len();
    f32_data.extend_from_slice(vertices);
    let idx_off = u32_data.len();
    u32_data.extend_from_slice(indices);
    GlbMeshSpec {
        name: name.to_string(),
        positions: [pos_off, vertices.len()],
        indices: [idx_off, indices.len()],
        normals: None,
        color: [
            material.color[0] as f64,
            material.color[1] as f64,
            material.color[2] as f64,
        ],
        metallic: material.metallic as f64,
        roughness: material.roughness as f64,
        emissive: None,
        emissive_strength: None,
        clearcoat: None,
        clearcoat_roughness: None,
        alpha: None,
        transform: None,
        mesh_key: None,
    }
}

/// Export a part to binary GLB format with PBR material.
pub fn export_glb(
    part: &Part,
    material: &Material,
    path: impl AsRef<Path>,
) -> Result<(), CadError> {
    let glb_data = to_glb_bytes(part, material)?;
    std::fs::write(path, glb_data)?;
    Ok(())
}

/// Convert a part to binary GLB bytes.
pub fn to_glb_bytes(part: &Part, material: &Material) -> Result<Vec<u8>, CadError> {
    let mesh = part.to_mesh();
    let vertices = mesh.vertices();
    let indices = mesh.indices();

    if vertices.is_empty() || indices.is_empty() {
        return Err(CadError::EmptyGeometry);
    }

    let mut f32_data = Vec::new();
    let mut u32_data = Vec::new();
    let spec = GlbSpec {
        name: part.name.clone(),
        meshes: vec![mesh_spec_for(
            &part.name,
            material,
            &mut f32_data,
            &mut u32_data,
            &vertices,
            &indices,
        )],
        animation: None,
    };
    build_glb(&spec, &f32_data, &u32_data)
        .map_err(|e| CadError::ExportError(format!("GLB build failed: {e}")))
}

/// Export a scene with multiple parts and materials to GLB
pub fn export_scene_glb(
    scene: &Scene,
    materials_db: &Materials,
    path: impl AsRef<Path>,
) -> Result<(), CadError> {
    let glb_data = scene_to_glb_bytes(scene, materials_db)?;
    std::fs::write(path, glb_data)?;
    Ok(())
}

/// Convert a scene to GLB bytes with multiple meshes and materials
pub fn scene_to_glb_bytes(scene: &Scene, materials_db: &Materials) -> Result<Vec<u8>, CadError> {
    if scene.is_empty() {
        return Err(CadError::EmptyGeometry);
    }

    let mut f32_data = Vec::new();
    let mut u32_data = Vec::new();
    let mut meshes = Vec::new();
    for node in &scene.nodes {
        let mesh = node.part.to_mesh();
        let vertices = mesh.vertices();
        let indices = mesh.indices();
        if vertices.is_empty() || indices.is_empty() {
            continue;
        }
        let material = materials_db.get_for_part_or_default(&node.material_key);
        meshes.push(mesh_spec_for(
            &node.part.name,
            &material,
            &mut f32_data,
            &mut u32_data,
            &vertices,
            &indices,
        ));
    }

    if meshes.is_empty() {
        return Err(CadError::EmptyGeometry);
    }

    let spec = GlbSpec {
        name: scene.name.clone(),
        meshes,
        animation: None,
    };
    build_glb(&spec, &f32_data, &u32_data)
        .map_err(|e| CadError::ExportError(format!("GLB build failed: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glb_export() {
        let cube = Part::cube("test_cube", 10.0, 10.0, 10.0);
        let material = Material::default();
        let glb_data = to_glb_bytes(&cube, &material).unwrap();

        // Check GLB magic
        assert_eq!(&glb_data[0..4], b"glTF");

        // Check version
        let version = u32::from_le_bytes([glb_data[4], glb_data[5], glb_data[6], glb_data[7]]);
        assert_eq!(version, 2);
    }

    #[test]
    fn test_scene_glb_export() {
        let mut scene = Scene::new("test_scene");
        scene.add(Part::cube("cube1", 10.0, 10.0, 10.0), "aluminum_6061");
        scene.add(
            Part::cube("cube2", 5.0, 5.0, 5.0).translate(20.0, 0.0, 0.0),
            "aluminum_powder_orange",
        );

        let materials = Materials::parse(
            r#"
            [materials.aluminum_6061]
            color = [0.85, 0.85, 0.88]
            metallic = 0.95
            roughness = 0.35
            [materials.aluminum_powder_orange]
            color = [1.0, 0.4, 0.0]
            metallic = 0.3
            roughness = 0.6
        "#,
        )
        .unwrap();

        let glb_data = scene_to_glb_bytes(&scene, &materials).unwrap();

        // Check GLB magic
        assert_eq!(&glb_data[0..4], b"glTF");
    }
}
