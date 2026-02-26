//! STL export for manufacturing.
//!
//! Generates binary STL files suitable for 3D printing and CNC.

use crate::{CadError, Part};
type Vec3 = tang::Vec3<f64>;
use std::io::Write;
use std::path::Path;

/// Export a part to binary STL format.
pub fn export_stl(part: &Part, path: impl AsRef<Path>) -> Result<(), CadError> {
    let stl_data = to_stl_bytes(part)?;
    let mut file = std::fs::File::create(path)?;
    file.write_all(&stl_data)?;
    Ok(())
}

/// Convert a part to binary STL bytes.
pub fn to_stl_bytes(part: &Part) -> Result<Vec<u8>, CadError> {
    let mesh = part.to_mesh();
    let vertices = mesh.vertices();
    let indices = mesh.indices();

    if vertices.is_empty() || indices.is_empty() {
        return Err(CadError::EmptyGeometry);
    }

    let num_triangles = indices.len() / 3;
    let mut buffer = Vec::with_capacity(84 + num_triangles * 50);

    // STL header (80 bytes)
    let header = format!("{:<80}", part.name);
    buffer.extend_from_slice(&header.as_bytes()[..80.min(header.len())]);
    buffer.resize(80, 0);

    // Number of triangles (4 bytes, little endian)
    buffer.extend_from_slice(&(num_triangles as u32).to_le_bytes());

    // Each triangle: normal (12 bytes) + 3 vertices (36 bytes) + attribute (2 bytes)
    for tri in indices.chunks(3) {
        let i0 = tri[0] as usize * 3;
        let i1 = tri[1] as usize * 3;
        let i2 = tri[2] as usize * 3;

        let v0 = Vec3::new(
            vertices[i0] as f64,
            vertices[i0 + 1] as f64,
            vertices[i0 + 2] as f64,
        );
        let v1 = Vec3::new(
            vertices[i1] as f64,
            vertices[i1 + 1] as f64,
            vertices[i1 + 2] as f64,
        );
        let v2 = Vec3::new(
            vertices[i2] as f64,
            vertices[i2 + 1] as f64,
            vertices[i2 + 2] as f64,
        );

        // Calculate normal
        let edge1 = v1 - v0;
        let edge2 = v2 - v0;
        let normal = edge1.cross(&edge2).normalize();

        // Write normal
        buffer.extend_from_slice(&(normal.x as f32).to_le_bytes());
        buffer.extend_from_slice(&(normal.y as f32).to_le_bytes());
        buffer.extend_from_slice(&(normal.z as f32).to_le_bytes());

        // Write vertices
        for v in [v0, v1, v2] {
            buffer.extend_from_slice(&(v.x as f32).to_le_bytes());
            buffer.extend_from_slice(&(v.y as f32).to_le_bytes());
            buffer.extend_from_slice(&(v.z as f32).to_le_bytes());
        }

        // Attribute byte count (0)
        buffer.extend_from_slice(&0u16.to_le_bytes());
    }

    Ok(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stl_export() {
        let cube = Part::cube("test_cube", 10.0, 10.0, 10.0);
        let stl_data = to_stl_bytes(&cube).unwrap();

        // STL header is 80 bytes + 4 bytes for triangle count
        assert!(stl_data.len() >= 84);

        // Check header starts with part name
        assert!(stl_data[0..9] == *b"test_cube");
    }
}
