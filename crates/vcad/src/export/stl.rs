//! STL export for manufacturing.
//!
//! Thin adapter over the shared `vcad-kernel-export` STL writer — the single
//! source of truth for binary STL serialization.

use crate::{CadError, Part};
use std::io::Write;
use std::path::Path;
use vcad_kernel_export::{build_stl, StlMeshSpec, StlSpec};

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

    let spec = StlSpec {
        name: part.name.clone(),
        meshes: vec![StlMeshSpec {
            positions: [0, vertices.len()],
            indices: [0, indices.len()],
        }],
    };
    build_stl(&spec, &vertices, &indices)
        .map_err(|e| CadError::ExportError(format!("STL build failed: {e}")))
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
