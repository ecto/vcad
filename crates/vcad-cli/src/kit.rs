//! `vcad kit` — a plate layout of many parts, written as a Bambu/Prusa 3MF.
//!
//! This replaces a hand-rolled project script (rana's `tools/make-kit-60c.py`)
//! that built the zip archive, the per-object `.model` parts, the
//! `Metadata/model_settings.config` plate blocks and the collision check by
//! hand. The structure below is that script's, kept deliberately faithful
//! because it is the version that is known to open in Bambu Studio — in
//! particular the two-level object indirection (a `3D/Objects/object_N.model`
//! holding the mesh, referenced by a component object in `3D/3dmodel.model`)
//! and the `<assemble>` block, neither of which the single-object exporter in
//! `vcad-slicer-bambu` emits.
//!
//! Three things it does that the script did not:
//!
//! * **Deterministic output.** Fixed zip timestamps, no compression-level
//!   drift, and UUIDs derived from the object index rather than random. Two
//!   runs over the same spec produce byte-identical files, so `cmp` is a
//!   usable "did anything actually change?" check. Bambu *resaves*
//!   restructure the file completely, which is exactly why a stable baseline
//!   is worth having.
//! * **A producer marker.** `Metadata/vcad_kit.json` plus a `vcad:producer`
//!   metadata entry, so a file found on disk months later can be identified
//!   as ours (or, if the marker is gone, identified as Bambu-resaved).
//! * **Collision and bed bounds are errors, not warnings.** The script
//!   printed `CLASH` and carried on to write the file anyway.

use std::collections::BTreeMap;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

/// Default Bambu-style bed, mm. An A1/P1S/X1 256 x 256 plate.
const DEFAULT_BED: f64 = 256.0;
/// Default keep-out from the bed edge, mm.
const DEFAULT_MARGIN: f64 = 3.0;
/// Default minimum gap between two parts' bounding cylinders, mm.
const DEFAULT_GAP: f64 = 6.0;
/// World-space X offset applied per plate index. Bambu lays plates out along
/// X in one coordinate system; 320 is its default plate pitch.
const DEFAULT_PLATE_PITCH: f64 = 320.0;

/// Arguments for [`run`].
pub struct KitArgs {
    /// Path to the kit spec JSON.
    pub spec: PathBuf,
    /// Output `.3mf`. Defaults to the spec path with a `.3mf` extension.
    pub output: Option<PathBuf>,
}

/// A kit spec: parts, their plate assignments, and their XY placements.
#[derive(Debug, Deserialize)]
pub struct KitSpec {
    /// Kit title, written into the 3MF metadata.
    #[serde(default)]
    pub name: Option<String>,
    /// Bed size and keep-out.
    #[serde(default)]
    pub bed: Bed,
    /// Minimum gap between two parts' bounding cylinders, mm.
    #[serde(default = "default_gap")]
    pub gap: f64,
    /// World X offset per plate index.
    #[serde(default = "default_plate_pitch")]
    pub plate_pitch: f64,
    /// Optional per-plate names, keyed by plate index.
    #[serde(default)]
    pub plates: Vec<PlateSpec>,
    /// The parts to lay out.
    pub parts: Vec<PartSpec>,
}

/// Bed extent and edge keep-out, mm.
#[derive(Debug, Deserialize)]
pub struct Bed {
    /// Bed width (X), mm.
    #[serde(default = "default_bed")]
    pub width: f64,
    /// Bed depth (Y), mm.
    #[serde(default = "default_bed")]
    pub depth: f64,
    /// Keep-out from every bed edge, mm.
    #[serde(default = "default_margin")]
    pub margin: f64,
}

impl Default for Bed {
    fn default() -> Self {
        Self {
            width: DEFAULT_BED,
            depth: DEFAULT_BED,
            margin: DEFAULT_MARGIN,
        }
    }
}

/// A named plate.
#[derive(Debug, Deserialize)]
pub struct PlateSpec {
    /// Plate index (1-based, matching Bambu's plater ids).
    pub index: u32,
    /// Human-readable plate name.
    #[serde(default)]
    pub name: Option<String>,
}

/// One placed part.
#[derive(Debug, Deserialize)]
pub struct PartSpec {
    /// Path to the part's mesh, relative to the spec file.
    pub mesh: PathBuf,
    /// Display name. Defaults to the mesh file stem.
    #[serde(default)]
    pub name: Option<String>,
    /// Plate index (1-based).
    #[serde(default = "default_plate")]
    pub plate: u32,
    /// Placement of the part's XY centre on the plate, mm.
    pub x: f64,
    /// Placement of the part's XY centre on the plate, mm.
    pub y: f64,
}

fn default_bed() -> f64 {
    DEFAULT_BED
}
fn default_margin() -> f64 {
    DEFAULT_MARGIN
}
fn default_gap() -> f64 {
    DEFAULT_GAP
}
fn default_plate_pitch() -> f64 {
    DEFAULT_PLATE_PITCH
}
fn default_plate() -> u32 {
    1
}

/// A mesh recentred on its own XY centroid-of-bounds and dropped to z = 0.
struct Placed {
    name: String,
    plate: u32,
    x: f64,
    y: f64,
    /// Half-height: the z the part's centre must sit at for its underside to
    /// touch the plate, given the mesh is stored centred on z.
    z_centre: f64,
    /// Bounding-cylinder radius about the part's XY centre.
    radius: f64,
    /// Deduplicated vertices, mm, centred in XY and on z.
    vertices: Vec<[f64; 3]>,
    /// Triangle indices into `vertices`.
    triangles: Vec<[usize; 3]>,
}

/// A welded, recentred part mesh: vertices, triangles, XY bounding radius
/// about the centre, and half-height in z.
type LoadedPart = (Vec<[f64; 3]>, Vec<[usize; 3]>, f64, f64);

/// Read an STL and recentre it: XY on its bounding-box centre, z likewise, so
/// placement is "put the part's centre here" and the z-drop is a single add.
fn load_part(path: &Path) -> Result<LoadedPart> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("opening part mesh {}", path.display()))?;
    let mut reader = std::io::BufReader::new(file);
    let stl =
        stl_io::read_stl(&mut reader).with_context(|| format!("reading STL {}", path.display()))?;

    if stl.faces.is_empty() {
        bail!("{} contains no triangles", path.display());
    }

    let mut lo = [f64::MAX; 3];
    let mut hi = [f64::MIN; 3];
    for v in &stl.vertices {
        for k in 0..3 {
            lo[k] = lo[k].min(v[k] as f64);
            hi[k] = hi[k].max(v[k] as f64);
        }
    }
    let centre = [
        (lo[0] + hi[0]) / 2.0,
        (lo[1] + hi[1]) / 2.0,
        (lo[2] + hi[2]) / 2.0,
    ];

    // Weld to a 1 micron lattice. STL has no shared vertices; 3MF wants them,
    // and an unwelded mesh triples the file size for nothing.
    let mut map: BTreeMap<(i64, i64, i64), usize> = BTreeMap::new();
    let mut vertices: Vec<[f64; 3]> = Vec::new();
    let mut index_of = |p: [f64; 3], vertices: &mut Vec<[f64; 3]>| -> usize {
        let key = (
            (p[0] * 1000.0).round() as i64,
            (p[1] * 1000.0).round() as i64,
            (p[2] * 1000.0).round() as i64,
        );
        *map.entry(key).or_insert_with(|| {
            vertices.push([
                key.0 as f64 / 1000.0,
                key.1 as f64 / 1000.0,
                key.2 as f64 / 1000.0,
            ]);
            vertices.len() - 1
        })
    };

    let mut triangles = Vec::with_capacity(stl.faces.len());
    for f in &stl.faces {
        let idx: Vec<usize> = f
            .vertices
            .iter()
            .map(|&vi| {
                let v = stl.vertices[vi];
                index_of(
                    [
                        v[0] as f64 - centre[0],
                        v[1] as f64 - centre[1],
                        v[2] as f64 - centre[2],
                    ],
                    &mut vertices,
                )
            })
            .collect();
        // Drop triangles the weld collapsed to a line or a point.
        if idx[0] != idx[1] && idx[1] != idx[2] && idx[0] != idx[2] {
            triangles.push([idx[0], idx[1], idx[2]]);
        }
    }
    if triangles.is_empty() {
        bail!("{} degenerated to no triangles when welded", path.display());
    }

    let radius = vertices
        .iter()
        .map(|v| v[0].hypot(v[1]))
        .fold(0.0_f64, f64::max);
    let z_centre = (hi[2] - lo[2]) / 2.0;
    Ok((vertices, triangles, radius, z_centre))
}

/// Check bed bounds and pairwise collisions. Both are hard errors: a kit that
/// clashes is not a kit, and the previous script's "print CLASH and write the
/// file anyway" is how a bad plate reaches a printer.
fn check_layout(parts: &[Placed], spec: &KitSpec) -> Result<()> {
    let mut problems: Vec<String> = Vec::new();
    let (w, d, m) = (spec.bed.width, spec.bed.depth, spec.bed.margin);

    for p in parts {
        let r = p.radius + m;
        if p.x - r < 0.0 || p.x + r > w || p.y - r < 0.0 || p.y + r > d {
            problems.push(format!(
                "{} at ({:.1}, {:.1}) r{:.1} runs off the {w:.0}x{d:.0} bed \
                 (margin {m:.1})",
                p.name, p.x, p.y, p.radius
            ));
        }
    }

    for (i, a) in parts.iter().enumerate() {
        for b in &parts[i + 1..] {
            if a.plate != b.plate {
                continue;
            }
            let d = (a.x - b.x).hypot(a.y - b.y);
            let need = a.radius + b.radius + spec.gap;
            if d < need {
                problems.push(format!(
                    "{} and {} on plate {} are {:.2} apart, need {:.2} \
                     (radii {:.2} + {:.2}, gap {:.2})",
                    a.name, b.name, a.plate, d, need, a.radius, b.radius, spec.gap
                ));
            }
        }
    }

    if problems.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(
            "kit layout is not printable:\n  {}",
            problems.join("\n  ")
        ))
    }
}

/// A UUID derived from `n`. 3MF requires the production-extension UUIDs to be
/// well-formed and unique within the file, not to be random — deriving them
/// keeps the output byte-identical between runs.
fn uuid(n: u32) -> String {
    format!("{n:08x}-0000-4000-8000-{n:012x}")
}

const NS: &str = concat!(
    r#"xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" "#,
    r#"xmlns:BambuStudio="http://schemas.bambulab.com/package/2021" "#,
    r#"xmlns:p="http://schemas.microsoft.com/3dmanufacturing/production/2015/06" "#,
    r#"requiredextensions="p""#
);

fn mesh_xml(p: &Placed) -> String {
    let mut s = String::from("<mesh><vertices>");
    for v in &p.vertices {
        s.push_str(&format!(
            "<vertex x=\"{:.3}\" y=\"{:.3}\" z=\"{:.3}\"/>",
            v[0], v[1], v[2]
        ));
    }
    s.push_str("</vertices><triangles>");
    for t in &p.triangles {
        s.push_str(&format!(
            "<triangle v1=\"{}\" v2=\"{}\" v3=\"{}\"/>",
            t[0], t[1], t[2]
        ));
    }
    s.push_str("</triangles></mesh>");
    s
}

/// Build the 3MF bytes for a checked layout.
fn build_3mf(parts: &[Placed], spec: &KitSpec) -> Result<Vec<u8>> {
    let title = spec.name.clone().unwrap_or_else(|| "vcad kit".to_string());

    let mut object_models: Vec<(String, String)> = Vec::new(); // (path, xml)
    let mut components = Vec::new();
    let mut items = Vec::new();
    let mut settings_objects = Vec::new();
    let mut plate_instances: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    let mut assemble = Vec::new();
    let mut uid = 0u32;
    let mut next_uuid = || {
        uid += 1;
        uuid(uid)
    };

    for (i, p) in parts.iter().enumerate() {
        let mesh_id = (i * 2 + 1) as u32;
        let comp_id = (i * 2 + 2) as u32;
        let path = format!("3D/Objects/object_{mesh_id}.model");

        object_models.push((
            path.clone(),
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                 <model unit=\"millimeter\" xml:lang=\"en-US\" {NS}>\n\
                 \x20<metadata name=\"BambuStudio:3mfVersion\">1</metadata>\n\
                 \x20<resources>\n\
                 \x20 <object id=\"{mesh_id}\" p:UUID=\"{}\" type=\"model\">{}</object>\n\
                 \x20</resources>\n <build/>\n</model>",
                next_uuid(),
                mesh_xml(p)
            ),
        ));

        components.push(format!(
            "<object id=\"{comp_id}\" p:UUID=\"{}\" type=\"model\">\
             <components><component p:path=\"/{path}\" objectid=\"{mesh_id}\" \
             p:UUID=\"{}\" transform=\"1 0 0 0 1 0 0 0 1 0 0 0\"/></components></object>",
            next_uuid(),
            next_uuid()
        ));

        // Plates are laid out along world X; z_centre is the drop-to-plate.
        let wx = p.x + spec.plate_pitch * (p.plate.saturating_sub(1)) as f64;
        let xform = format!("1 0 0 0 1 0 0 0 1 {wx} {} {}", p.y, p.z_centre);

        items.push(format!(
            "<item objectid=\"{comp_id}\" p:UUID=\"{}\" transform=\"{xform}\" printable=\"1\"/>",
            next_uuid()
        ));
        settings_objects.push(format!(
            "  <object id=\"{comp_id}\">\n\
             \x20   <metadata key=\"name\" value=\"{}\"/>\n\
             \x20   <metadata key=\"extruder\" value=\"1\"/>\n\
             \x20   <part id=\"1\" subtype=\"normal_part\">\n\
             \x20     <metadata key=\"name\" value=\"{}\"/>\n\
             \x20     <metadata key=\"matrix\" value=\"1 0 0 0 0 1 0 0 0 0 1 0 0 0 0 1\"/>\n\
             \x20   </part>\n  </object>",
            p.name, p.name
        ));
        plate_instances.entry(p.plate).or_default().push(format!(
            "    <model_instance>\n\
                 \x20     <metadata key=\"object_id\" value=\"{comp_id}\"/>\n\
                 \x20     <metadata key=\"instance_id\" value=\"0\"/>\n\
                 \x20     <metadata key=\"identify_id\" value=\"{}\"/>\n    </model_instance>",
            100 + i
        ));
        assemble.push(format!(
            "   <assemble_item object_id=\"{comp_id}\" instance_id=\"0\" \
             transform=\"{xform}\" offset=\"0 0 0\" />"
        ));
    }

    let plate_names: BTreeMap<u32, String> = spec
        .plates
        .iter()
        .filter_map(|p| p.name.clone().map(|n| (p.index, n)))
        .collect();
    let plate_blocks: Vec<String> = plate_instances
        .iter()
        .map(|(idx, insts)| {
            let name = plate_names
                .get(idx)
                .cloned()
                .unwrap_or_else(|| format!("Plate {idx}"));
            format!(
                "  <plate>\n\
                 \x20   <metadata key=\"plater_id\" value=\"{idx}\"/>\n\
                 \x20   <metadata key=\"plater_name\" value=\"{name}\"/>\n\
                 \x20   <metadata key=\"locked\" value=\"false\"/>\n{}\n  </plate>",
                insts.join("\n")
            )
        })
        .collect();

    let producer = format!("vcad {}", env!("CARGO_PKG_VERSION"));
    let main_model = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <model unit=\"millimeter\" xml:lang=\"en-US\" {NS}>\n\
         \x20<metadata name=\"Application\">{producer}</metadata>\n\
         \x20<metadata name=\"vcad:producer\">{producer}</metadata>\n\
         \x20<metadata name=\"BambuStudio:3mfVersion\">1</metadata>\n\
         \x20<metadata name=\"Title\">{title}</metadata>\n\
         \x20<resources>\n  {}\n </resources>\n\
         \x20<build p:UUID=\"{}\">\n  {}\n </build>\n</model>",
        components.join("\n  "),
        next_uuid(),
        items.join("\n  ")
    );

    let model_settings = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<config>\n{}\n{}\n  <assemble>\n{}\n  </assemble>\n</config>",
        settings_objects.join("\n"),
        plate_blocks.join("\n"),
        assemble.join("\n")
    );

    // The producer marker, as a file rather than only as metadata: a Bambu
    // resave rewrites the model XML but this is the thing to look for first
    // when asking "did this file come out of vcad, and from what spec?".
    let marker = serde_json::json!({
        "producer": producer,
        "format": "vcad-kit/1",
        "kit": title,
        "parts": parts.iter().map(|p| serde_json::json!({
            "name": p.name, "plate": p.plate, "x": p.x, "y": p.y,
            "radius": (p.radius * 1000.0).round() / 1000.0,
        })).collect::<Vec<_>>(),
    });

    let rels_3d = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\n{}\n</Relationships>",
        object_models
            .iter()
            .enumerate()
            .map(|(i, (path, _))| format!(
                " <Relationship Target=\"/{path}\" Id=\"rel-{}\" \
                 Type=\"http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel\"/>",
                i + 1
            ))
            .collect::<Vec<_>>()
            .join("\n")
    );

    let mut buffer = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut buffer);
        // Deterministic: a fixed timestamp (the zip epoch) and a pinned
        // compression level, so the same spec always yields the same bytes.
        let options = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .compression_level(Some(6))
            .last_modified_time(
                zip::DateTime::from_date_and_time(1980, 1, 1, 0, 0, 0)
                    .map_err(|e| anyhow!("zip epoch: {e:?}"))?,
            );

        let mut write = |name: &str, body: &str| -> Result<()> {
            zip.start_file(name, options)?;
            zip.write_all(body.as_bytes())?;
            Ok(())
        };

        write("[Content_Types].xml",
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
             <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
             <Default Extension=\"model\" ContentType=\"application/vnd.ms-package.3dmanufacturing-3dmodel+xml\"/>\
             <Default Extension=\"config\" ContentType=\"text/xml\"/>\
             <Default Extension=\"json\" ContentType=\"application/json\"/></Types>")?;
        write("_rels/.rels",
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
             <Relationship Target=\"/3D/3dmodel.model\" Id=\"rel-0\" Type=\"http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel\"/></Relationships>")?;
        write("3D/_rels/3dmodel.model.rels", &rels_3d)?;
        write("3D/3dmodel.model", &main_model)?;
        for (path, xml) in &object_models {
            write(path, xml)?;
        }
        write("Metadata/model_settings.config", &model_settings)?;
        write(
            "Metadata/vcad_kit.json",
            &serde_json::to_string_pretty(&marker)?,
        )?;
        zip.finish()?;
    }
    Ok(buffer.into_inner())
}

/// Build a kit 3MF from `args.spec`. Returns the path written.
pub fn run(args: &KitArgs) -> Result<PathBuf> {
    let text = std::fs::read_to_string(&args.spec)
        .with_context(|| format!("reading kit spec {}", args.spec.display()))?;
    let spec: KitSpec = serde_json::from_str(&text)
        .with_context(|| format!("parsing kit spec {}", args.spec.display()))?;

    if spec.parts.is_empty() {
        bail!("kit spec {} lists no parts", args.spec.display());
    }
    let base = args.spec.parent().unwrap_or(Path::new("."));

    // Meshes are cached by path: a kit that prints three of the same planet
    // reads and welds it once.
    let mut cache: BTreeMap<PathBuf, LoadedPart> = BTreeMap::new();
    let mut parts = Vec::with_capacity(spec.parts.len());
    for p in &spec.parts {
        let path = if p.mesh.is_absolute() {
            p.mesh.clone()
        } else {
            base.join(&p.mesh)
        };
        if !cache.contains_key(&path) {
            cache.insert(path.clone(), load_part(&path)?);
        }
        let (vertices, triangles, radius, z_centre) = cache[&path].clone();
        parts.push(Placed {
            name: p.name.clone().unwrap_or_else(|| {
                path.file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "part".into())
            }),
            plate: p.plate,
            x: p.x,
            y: p.y,
            z_centre,
            radius,
            vertices,
            triangles,
        });
    }

    check_layout(&parts, &spec)?;

    let bytes = build_3mf(&parts, &spec)?;
    let out = args
        .output
        .clone()
        .unwrap_or_else(|| args.spec.with_extension("3mf"));
    if let Some(dir) = out.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir).ok();
        }
    }
    std::fs::write(&out, &bytes).with_context(|| format!("writing kit {}", out.display()))?;
    println!(
        "kit: {} part{} on {} plate{} -> {} ({:.2} MB)",
        parts.len(),
        if parts.len() == 1 { "" } else { "s" },
        parts
            .iter()
            .map(|p| p.plate)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        if parts.len() == 1 { "" } else { "s" },
        out.display(),
        bytes.len() as f64 / 1e6
    );
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A binary STL of an axis-aligned box, `w` x `d` x `h`, sitting with its
    /// underside on z = `z0`. Two triangles per face; normals are zeroed
    /// because nothing downstream reads them.
    fn write_box_stl(path: &Path, w: f64, d: f64, h: f64, z0: f64) {
        let (x0, x1) = (-w / 2.0, w / 2.0);
        let (y0, y1) = (-d / 2.0, d / 2.0);
        let (z0, z1) = (z0, z0 + h);
        let c = [
            [x0, y0, z0],
            [x1, y0, z0],
            [x1, y1, z0],
            [x0, y1, z0],
            [x0, y0, z1],
            [x1, y0, z1],
            [x1, y1, z1],
            [x0, y1, z1],
        ];
        let faces = [
            [0, 2, 1],
            [0, 3, 2], // bottom
            [4, 5, 6],
            [4, 6, 7], // top
            [0, 1, 5],
            [0, 5, 4],
            [1, 2, 6],
            [1, 6, 5],
            [2, 3, 7],
            [2, 7, 6],
            [3, 0, 4],
            [3, 4, 7],
        ];
        let mut b = vec![0u8; 80];
        b.extend_from_slice(&(faces.len() as u32).to_le_bytes());
        for f in faces {
            b.extend_from_slice(&[0u8; 12]); // zero normal
            for vi in f {
                for k in 0..3 {
                    b.extend_from_slice(&(c[vi][k] as f32).to_le_bytes());
                }
            }
            b.extend_from_slice(&0u16.to_le_bytes());
        }
        std::fs::write(path, b).unwrap();
    }

    struct Dir(PathBuf);
    impl Dir {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("vcad_kit_test_{tag}"));
            std::fs::remove_dir_all(&p).ok();
            std::fs::create_dir_all(&p).unwrap();
            Dir(p)
        }
        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    /// Three parts, one plate, no clashes. The heights differ so the z-drop
    /// is checkable: each part must land at its own z.
    fn three_part_kit(dir: &Dir) -> PathBuf {
        write_box_stl(&dir.path("alpha.stl"), 20.0, 20.0, 6.0, -3.0);
        write_box_stl(&dir.path("beta.stl"), 10.0, 10.0, 4.0, 100.0);
        write_box_stl(&dir.path("gamma.stl"), 30.0, 30.0, 10.0, 0.0);
        let spec = r#"{
          "name": "test kit",
          "bed": { "width": 256, "depth": 256, "margin": 3 },
          "gap": 6,
          "plates": [{ "index": 1, "name": "everything" }],
          "parts": [
            { "mesh": "alpha.stl", "plate": 1, "x": 50, "y": 50 },
            { "mesh": "beta.stl", "plate": 1, "x": 120, "y": 50 },
            { "mesh": "gamma.stl", "name": "gamma-part", "plate": 1, "x": 50, "y": 130 }
          ]
        }"#;
        let sp = dir.path("kit.json");
        std::fs::write(&sp, spec).unwrap();
        sp
    }

    fn read_entry(bytes: &[u8], name: &str) -> String {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).unwrap();
        let mut f = zip
            .by_name(name)
            .unwrap_or_else(|_| panic!("missing {name}"));
        let mut s = String::new();
        std::io::Read::read_to_string(&mut f, &mut s).unwrap();
        s
    }

    #[test]
    fn three_part_kit_round_trips() {
        let dir = Dir::new("roundtrip");
        let spec = three_part_kit(&dir);
        let out = run(&KitArgs {
            spec,
            output: Some(dir.path("kit.3mf")),
        })
        .expect("kit export");
        let bytes = std::fs::read(&out).unwrap();

        // It is a zip, and every expected member is present.
        let zip = zip::ZipArchive::new(Cursor::new(bytes.clone())).unwrap();
        let names: Vec<String> = zip.file_names().map(|s| s.to_string()).collect();
        for want in [
            "[Content_Types].xml",
            "_rels/.rels",
            "3D/3dmodel.model",
            "3D/_rels/3dmodel.model.rels",
            "Metadata/model_settings.config",
            "Metadata/vcad_kit.json",
        ] {
            assert!(
                names.iter().any(|n| n == want),
                "missing {want} in {names:?}"
            );
        }

        // One object .model per part.
        let objects: Vec<&String> = names
            .iter()
            .filter(|n| n.starts_with("3D/Objects/"))
            .collect();
        assert_eq!(
            objects.len(),
            3,
            "expected 3 object models, got {objects:?}"
        );

        let main = read_entry(&bytes, "3D/3dmodel.model");
        assert_eq!(main.matches("<item ").count(), 3, "expected 3 build items");
        assert_eq!(
            main.matches("<components>").count(),
            3,
            "expected 3 component objects"
        );

        // Transforms: XY as specified, Z the half-height — parts are stored
        // centred, so half-height puts the underside on the plate. Note beta
        // was authored floating at z=100 and gamma resting at z=0; both land
        // on the plate regardless.
        for (x, y, z) in [(50.0, 50.0, 3.0), (120.0, 50.0, 2.0), (50.0, 130.0, 5.0)] {
            let want = format!("transform=\"1 0 0 0 1 0 0 0 1 {x} {y} {z}\"");
            assert!(main.contains(&want), "no item with {want} in\n{main}");
        }

        // Plate config: one plate, named, with three instances.
        let cfg = read_entry(&bytes, "Metadata/model_settings.config");
        assert_eq!(cfg.matches("<plate>").count(), 1);
        assert!(cfg.contains(r#"<metadata key="plater_id" value="1"/>"#));
        assert!(cfg.contains(r#"<metadata key="plater_name" value="everything"/>"#));
        assert_eq!(cfg.matches("<model_instance>").count(), 3);
        assert_eq!(cfg.matches("<assemble_item ").count(), 3);
        // Names default to the file stem, and an explicit name wins.
        assert!(cfg.contains(r#"value="alpha""#));
        assert!(cfg.contains(r#"value="gamma-part""#));

        // The producer marker identifies the file as ours.
        let marker: serde_json::Value =
            serde_json::from_str(&read_entry(&bytes, "Metadata/vcad_kit.json")).unwrap();
        assert_eq!(marker["format"], "vcad-kit/1");
        assert!(marker["producer"].as_str().unwrap().starts_with("vcad "));
        assert_eq!(marker["parts"].as_array().unwrap().len(), 3);
        assert!(main.contains("vcad:producer"));
    }

    /// The whole point of pinning timestamps: byte-identical output means
    /// `cmp` answers "did this kit actually change?", which matters because a
    /// Bambu resave restructures the file completely.
    #[test]
    fn output_is_deterministic() {
        let dir = Dir::new("deterministic");
        let spec = three_part_kit(&dir);
        let a = run(&KitArgs {
            spec: spec.clone(),
            output: Some(dir.path("a.3mf")),
        })
        .unwrap();
        let b = run(&KitArgs {
            spec,
            output: Some(dir.path("b.3mf")),
        })
        .unwrap();
        assert_eq!(
            std::fs::read(&a).unwrap(),
            std::fs::read(&b).unwrap(),
            "two runs of the same spec differ"
        );
    }

    #[test]
    fn overlapping_parts_are_an_error() {
        let dir = Dir::new("clash");
        write_box_stl(&dir.path("alpha.stl"), 20.0, 20.0, 6.0, 0.0);
        let spec = dir.path("kit.json");
        // Two 20x20 boxes (radius ~14.1) only 10 mm apart: they interpenetrate.
        std::fs::write(
            &spec,
            r#"{"parts": [
                 {"mesh": "alpha.stl", "name": "a", "x": 50, "y": 50},
                 {"mesh": "alpha.stl", "name": "b", "x": 60, "y": 50}]}"#,
        )
        .unwrap();
        let err = run(&KitArgs {
            spec,
            output: Some(dir.path("out.3mf")),
        })
        .expect_err("clash must fail the export");
        let msg = err.to_string();
        assert!(msg.contains("not printable"), "{msg}");
        assert!(!dir.path("out.3mf").exists(), "a clashing kit was written");
    }

    #[test]
    fn off_bed_placement_is_an_error() {
        let dir = Dir::new("offbed");
        write_box_stl(&dir.path("alpha.stl"), 20.0, 20.0, 6.0, 0.0);
        let spec = dir.path("kit.json");
        std::fs::write(
            &spec,
            r#"{"bed": {"width": 100, "depth": 100, "margin": 3},
                "parts": [{"mesh": "alpha.stl", "name": "a", "x": 96, "y": 50}]}"#,
        )
        .unwrap();
        let err = run(&KitArgs {
            spec,
            output: Some(dir.path("out.3mf")),
        })
        .expect_err("off-bed must fail the export");
        assert!(err.to_string().contains("off the"), "{err}");
    }

    /// Parts on different plates cannot clash with each other, and each plate
    /// is offset along world X.
    #[test]
    fn separate_plates_do_not_clash_and_are_offset() {
        let dir = Dir::new("plates");
        write_box_stl(&dir.path("alpha.stl"), 20.0, 20.0, 6.0, 0.0);
        let spec = dir.path("kit.json");
        std::fs::write(
            &spec,
            r#"{"plate_pitch": 320,
                "parts": [
                  {"mesh": "alpha.stl", "name": "a", "plate": 1, "x": 50, "y": 50},
                  {"mesh": "alpha.stl", "name": "b", "plate": 2, "x": 50, "y": 50}]}"#,
        )
        .unwrap();
        let out = run(&KitArgs {
            spec,
            output: Some(dir.path("out.3mf")),
        })
        .expect("two plates must not clash");
        let bytes = std::fs::read(&out).unwrap();
        let main = read_entry(&bytes, "3D/3dmodel.model");
        assert!(main.contains("1 0 0 0 1 0 0 0 1 50 50 3"), "{main}");
        assert!(main.contains("1 0 0 0 1 0 0 0 1 370 50 3"), "{main}");
        let cfg = read_entry(&bytes, "Metadata/model_settings.config");
        assert_eq!(cfg.matches("<plate>").count(), 2);
    }
}
