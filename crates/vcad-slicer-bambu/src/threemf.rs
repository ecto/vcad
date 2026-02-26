//! 3MF file generation for Bambu printers.

use std::io::{Cursor, Write};

use zip::write::SimpleFileOptions;
use zip::ZipWriter;

use crate::error::{BambuError, Result};

/// 3MF model for export.
pub struct ThreeMfModel {
    /// Model name.
    pub name: String,
    /// Mesh vertices (x, y, z triplets).
    pub vertices: Vec<f32>,
    /// Triangle indices.
    pub indices: Vec<u32>,
    /// Plate configuration.
    pub plate: PlateConfig,
    /// Print settings.
    pub settings: PrintSettings,
}

/// Plate (build plate) configuration.
#[derive(Debug, Clone)]
pub struct PlateConfig {
    /// Plate index.
    pub index: u32,
    /// Plate name.
    pub name: String,
}

impl Default for PlateConfig {
    fn default() -> Self {
        Self {
            index: 0,
            name: "Plate 1".into(),
        }
    }
}

/// Print settings embedded in 3MF.
#[derive(Debug, Clone)]
pub struct PrintSettings {
    /// Layer height (mm).
    pub layer_height: f64,
    /// First layer height (mm).
    pub first_layer_height: f64,
    /// Wall count.
    pub wall_count: u32,
    /// Infill density (0-1).
    pub infill_density: f64,
    /// Print temperature.
    pub print_temp: u32,
    /// Bed temperature.
    pub bed_temp: u32,
    /// Filament type.
    pub filament_type: String,
}

impl Default for PrintSettings {
    fn default() -> Self {
        Self {
            layer_height: 0.2,
            first_layer_height: 0.25,
            wall_count: 3,
            infill_density: 0.15,
            print_temp: 220,
            bed_temp: 55,
            filament_type: "PLA".into(),
        }
    }
}

impl ThreeMfModel {
    /// Create a new 3MF model from mesh data.
    pub fn new(name: String, vertices: Vec<f32>, indices: Vec<u32>) -> Self {
        Self {
            name,
            vertices,
            indices,
            plate: PlateConfig::default(),
            settings: PrintSettings::default(),
        }
    }

    /// Generate 3MF file as bytes.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut buffer = Cursor::new(Vec::new());
        let mut zip = ZipWriter::new(&mut buffer);
        let options = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .compression_level(Some(6));

        // [Content_Types].xml
        zip.start_file("[Content_Types].xml", options)
            .map_err(|e| BambuError::ThreeMfError(e.to_string()))?;
        zip.write_all(self.content_types_xml().as_bytes())
            .map_err(|e| BambuError::ThreeMfError(e.to_string()))?;

        // _rels/.rels
        zip.start_file("_rels/.rels", options)
            .map_err(|e| BambuError::ThreeMfError(e.to_string()))?;
        zip.write_all(self.rels_xml().as_bytes())
            .map_err(|e| BambuError::ThreeMfError(e.to_string()))?;

        // 3D/3dmodel.model
        zip.start_file("3D/3dmodel.model", options)
            .map_err(|e| BambuError::ThreeMfError(e.to_string()))?;
        zip.write_all(self.model_xml().as_bytes())
            .map_err(|e| BambuError::ThreeMfError(e.to_string()))?;

        // Metadata/plate_1.json (Bambu specific)
        zip.start_file("Metadata/plate_1.json", options)
            .map_err(|e| BambuError::ThreeMfError(e.to_string()))?;
        zip.write_all(self.plate_json().as_bytes())
            .map_err(|e| BambuError::ThreeMfError(e.to_string()))?;

        // Metadata/model_settings.config (Bambu specific)
        zip.start_file("Metadata/model_settings.config", options)
            .map_err(|e| BambuError::ThreeMfError(e.to_string()))?;
        zip.write_all(self.model_settings_xml().as_bytes())
            .map_err(|e| BambuError::ThreeMfError(e.to_string()))?;

        // Metadata/slice_info.config
        zip.start_file("Metadata/slice_info.config", options)
            .map_err(|e| BambuError::ThreeMfError(e.to_string()))?;
        zip.write_all(self.slice_info_xml().as_bytes())
            .map_err(|e| BambuError::ThreeMfError(e.to_string()))?;

        zip.finish()
            .map_err(|e| BambuError::ThreeMfError(e.to_string()))?;

        Ok(buffer.into_inner())
    }

    fn content_types_xml(&self) -> String {
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
    <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
    <Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/>
    <Default Extension="json" ContentType="application/json"/>
    <Default Extension="config" ContentType="text/xml"/>
</Types>"#.to_string()
    }

    fn rels_xml(&self) -> String {
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
    <Relationship Target="/3D/3dmodel.model" Id="rel-1" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
</Relationships>"#.to_string()
    }

    fn model_xml(&self) -> String {
        let mut vertices_xml = String::new();
        for i in 0..(self.vertices.len() / 3) {
            let x = self.vertices[i * 3];
            let y = self.vertices[i * 3 + 1];
            let z = self.vertices[i * 3 + 2];
            vertices_xml.push_str(&format!(
                "                <vertex x=\"{:.6}\" y=\"{:.6}\" z=\"{:.6}\"/>\n",
                x, y, z
            ));
        }

        let mut triangles_xml = String::new();
        for i in 0..(self.indices.len() / 3) {
            let v1 = self.indices[i * 3];
            let v2 = self.indices[i * 3 + 1];
            let v3 = self.indices[i * 3 + 2];
            triangles_xml.push_str(&format!(
                "                <triangle v1=\"{}\" v2=\"{}\" v3=\"{}\"/>\n",
                v1, v2, v3
            ));
        }

        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xml:lang="en-US" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" xmlns:p="http://schemas.microsoft.com/3dmanufacturing/production/2015/06">
    <metadata name="Application">vcad</metadata>
    <resources>
        <object id="1" type="model">
            <mesh>
                <vertices>
{vertices_xml}                </vertices>
                <triangles>
{triangles_xml}                </triangles>
            </mesh>
        </object>
    </resources>
    <build p:UUID="{}">
        <item objectid="1" transform="1 0 0 0 1 0 0 0 1 0 0 0" p:UUID="{}"/>
    </build>
</model>"#,
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4()
        )
    }

    fn plate_json(&self) -> String {
        serde_json::json!({
            "plate_index": self.plate.index,
            "plate_name": self.plate.name,
            "objects": [{
                "identify_id": "1",
                "object_id": 1,
                "name": self.name,
                "extruder": 1
            }]
        })
        .to_string()
    }

    fn model_settings_xml(&self) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<config>
    <object id="1">
        <part id="1" subtype="normal_part">
            <metadata key="name" value="{}"/>
        </part>
    </object>
</config>"#,
            self.name
        )
    }

    fn slice_info_xml(&self) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<config>
    <plate>
        <metadata key="index" value="{}"/>
        <metadata key="layer_height" value="{}"/>
        <metadata key="initial_layer_print_height" value="{}"/>
        <metadata key="wall_loops" value="{}"/>
        <metadata key="sparse_infill_density" value="{}%"/>
        <metadata key="nozzle_temperature" value="{}"/>
        <metadata key="bed_temperature" value="{}"/>
        <metadata key="filament_type" value="{}"/>
    </plate>
</config>"#,
            self.plate.index,
            self.settings.layer_height,
            self.settings.first_layer_height,
            self.settings.wall_count,
            (self.settings.infill_density * 100.0) as u32,
            self.settings.print_temp,
            self.settings.bed_temp,
            self.settings.filament_type
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_threemf_generation() {
        // Simple triangle
        let vertices = vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.5, 1.0, 0.0];
        let indices = vec![0, 1, 2];

        let model = ThreeMfModel::new("test".into(), vertices, indices);
        let bytes = model.to_bytes().unwrap();

        // Should be a valid ZIP file
        assert!(bytes.len() > 100);
        assert_eq!(&bytes[0..2], b"PK"); // ZIP magic number
    }
}
