//! STL mesh loading for URDF `<mesh>` references.
//!
//! Supports binary and ASCII STL via the `stl_io` crate. Coordinates are
//! stored in millimetres (matching the rest of the vcad IR), with optional
//! non-uniform scale applied at load time. URDF mesh files are authored in
//! metres; this loader multiplies through by 1000 so the result drops
//! straight into a [`vcad_kernel_tessellate::TriangleMesh`].

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use vcad_ir::Vec3;
use vcad_kernel_tessellate::TriangleMesh;

use crate::error::PhysicsError;

/// Load an STL file into a [`TriangleMesh`], applying URDF-style scale and
/// converting metres → millimetres.
///
/// `scale` is the URDF `<mesh scale="x y z"/>` factor. `None` means 1.0
/// per axis (URDF default).
pub fn load_stl(path: &Path, scale: Option<Vec3>) -> Result<TriangleMesh, PhysicsError> {
    let file = File::open(path)
        .map_err(|e| PhysicsError::Evaluation(format!("STL open {}: {}", path.display(), e)))?;
    let mut reader = BufReader::new(file);
    let stl = stl_io::read_stl(&mut reader)
        .map_err(|e| PhysicsError::Evaluation(format!("STL parse {}: {}", path.display(), e)))?;

    // URDF mesh coords are in metres; vcad IR is in millimetres.
    let m_to_mm = 1000.0_f32;
    let sx = scale.map(|s| s.x as f32).unwrap_or(1.0) * m_to_mm;
    let sy = scale.map(|s| s.y as f32).unwrap_or(1.0) * m_to_mm;
    let sz = scale.map(|s| s.z as f32).unwrap_or(1.0) * m_to_mm;

    // stl_io gives us per-vertex positions and per-face indices already
    // deduplicated. Triangle normals are per-face; expand them to per-vertex
    // (each triangle's three vertices share its face normal — good enough
    // for collider work; viewport-quality smooth normals would need
    // per-vertex averaging).
    let n_verts = stl.vertices.len();
    let n_tris = stl.faces.len();
    let mut vertices = Vec::with_capacity(n_verts * 3);
    let mut normals = vec![0.0_f32; n_verts * 3];
    let mut indices = Vec::with_capacity(n_tris * 3);

    for v in &stl.vertices {
        vertices.push(v[0] * sx);
        vertices.push(v[1] * sy);
        vertices.push(v[2] * sz);
    }

    for tri in &stl.faces {
        let [a, b, c] = tri.vertices;
        indices.push(a as u32);
        indices.push(b as u32);
        indices.push(c as u32);
        // Spread the face normal onto each vertex it touches. Later faces
        // overwrite earlier — fine for physics, blocky for shading but we
        // don't render this mesh in the editor today.
        for &i in &[a, b, c] {
            normals[i * 3] = tri.normal[0];
            normals[i * 3 + 1] = tri.normal[1];
            normals[i * 3 + 2] = tri.normal[2];
        }
    }

    Ok(TriangleMesh {
        vertices,
        indices,
        normals,
        face_kinds: Vec::new(),
    })
}
