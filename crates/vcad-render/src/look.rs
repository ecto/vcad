//! Render-time material overrides for `--photoreal` (a "look" file).
//!
//! A look re-dresses a document for a shot without touching its source:
//! colourways, a glossier visor, a glowing accent, an oak desk. It is JSON:
//!
//! ```json
//! {
//!   "materials": { "satin_black": { "color": "#1e1813", "roughness": 0.55 } },
//!   "parts": {
//!     "cortex-ear-grilles": { "color": "#ff5f00", "emission": 0.4 },
//!     "desk": { "texture": { "type": "oak", "ring_spacing": 4.0, "axis": [1, 0, 0] } }
//!   }
//! }
//! ```
//!
//! `materials` keys match a document material's name; `parts` keys match a
//! part's labels (root node name, instance id/name, part-def id — the same
//! names `--focus` takes). A part entry wins over a material entry; fields
//! left out keep the document's value.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use serde::Deserialize;
use vcad_kernel::vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_raytrace::pathtrace::{FabricTexture, ImageTexture, Pbr, Texture, WoodTexture};

/// A parsed look file.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Look {
    /// Overrides keyed by document material name.
    #[serde(default)]
    pub materials: BTreeMap<String, MaterialOverride>,
    /// Overrides keyed by part label.
    #[serde(default)]
    pub parts: BTreeMap<String, MaterialOverride>,
}

/// One override. Every field is optional.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaterialOverride {
    /// Base colour, `#rrggbb` (sRGB).
    pub color: Option<String>,
    /// Metallic, 0..1.
    pub metallic: Option<f32>,
    /// Roughness, 0..1.
    pub roughness: Option<f32>,
    /// Clearcoat weight, 0..1.
    pub clearcoat: Option<f32>,
    /// Self-emission as a multiple of the base colour (a soft glow, not a
    /// light source: it is not sampled by next-event estimation).
    pub emission: Option<f32>,
    /// A spatially varying texture.
    pub texture: Option<TextureSpec>,
}

/// A texture in a look file.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum TextureSpec {
    /// Oak-like wood grain.
    Oak {
        /// Distance between growth rings, mm (default 4).
        ring_spacing: Option<f64>,
        /// Grain direction (default +X).
        axis: Option<[f64; 3]>,
        /// A point on the trunk axis (default far below the origin, so rings
        /// read as long flame grain on a flat top).
        origin: Option<[f64; 3]>,
        /// Light / dark wood colours, `#rrggbb`.
        light: Option<String>,
        /// Late-wood ring colour, `#rrggbb`.
        dark: Option<String>,
    },
    /// Plain-weave fabric.
    Fabric {
        /// Thread colour, `#rrggbb` (the weft is a shade darker unless given).
        color: String,
        /// Weft colour, `#rrggbb`.
        weft: Option<String>,
        /// Thread pitch, mm (default 0.8).
        thread_spacing: Option<f64>,
    },
    /// A PNG/JPEG projected triplanar.
    Image {
        /// Image path, relative to the look file.
        path: String,
        /// Millimetres covered by one repeat of the image.
        tile_size: f64,
    },
}

/// A look resolved against the file system, ready to apply.
#[derive(Debug, Clone, Default)]
pub struct ResolvedLook {
    materials: BTreeMap<String, Resolved>,
    parts: BTreeMap<String, Resolved>,
}

#[derive(Debug, Clone)]
struct Resolved {
    ov: MaterialOverride,
    texture: Option<Arc<Texture>>,
}

/// Parse `#rrggbb` to linear RGB.
pub fn hex_linear(s: &str) -> Result<[f32; 3], String> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 {
        return Err(format!("bad colour '{s}' (expected #rrggbb)"));
    }
    let byte =
        |i: usize| u8::from_str_radix(&h[i..i + 2], 16).map_err(|_| format!("bad colour '{s}'"));
    let lin = |b: u8| {
        let c = b as f32 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    Ok([lin(byte(0)?), lin(byte(2)?), lin(byte(4)?)])
}

impl Look {
    /// Read and parse a look file.
    pub fn load(path: &Path) -> Result<ResolvedLook, String> {
        let text =
            std::fs::read_to_string(path).map_err(|e| format!("look {}: {e}", path.display()))?;
        let look: Look =
            serde_json::from_str(&text).map_err(|e| format!("look {}: {e}", path.display()))?;
        look.resolve(path.parent().unwrap_or(Path::new(".")))
    }

    /// Validate colours and build textures, resolving image paths against
    /// `base_dir`.
    pub fn resolve(self, base_dir: &Path) -> Result<ResolvedLook, String> {
        let res =
            |m: BTreeMap<String, MaterialOverride>| -> Result<BTreeMap<String, Resolved>, String> {
                m.into_iter()
                    .map(|(k, ov)| {
                        if let Some(c) = &ov.color {
                            hex_linear(c).map_err(|e| format!("{k}: {e}"))?;
                        }
                        let texture = match &ov.texture {
                            Some(t) => Some(Arc::new(
                                build_texture(t, base_dir).map_err(|e| format!("{k}: {e}"))?,
                            )),
                            None => None,
                        };
                        Ok((k, Resolved { ov, texture }))
                    })
                    .collect()
            };
        Ok(ResolvedLook {
            materials: res(self.materials)?,
            parts: res(self.parts)?,
        })
    }
}

fn build_texture(t: &TextureSpec, base_dir: &Path) -> Result<Texture, String> {
    let v3 = |a: [f64; 3]| Vec3::new(a[0], a[1], a[2]);
    Ok(match t {
        TextureSpec::Oak {
            ring_spacing,
            axis,
            origin,
            light,
            dark,
        } => {
            let o = origin.unwrap_or([0.0, 0.0, -400.0]);
            let mut w = WoodTexture::oak(
                v3(axis.unwrap_or([1.0, 0.0, 0.0])),
                Point3::new(o[0], o[1], o[2]),
                ring_spacing.unwrap_or(4.0),
            );
            if let Some(c) = light {
                w.light = hex_linear(c)?;
            }
            if let Some(c) = dark {
                w.dark = hex_linear(c)?;
            }
            Texture::Wood(w)
        }
        TextureSpec::Fabric {
            color,
            weft,
            thread_spacing,
        } => {
            let mut f = FabricTexture::felt(hex_linear(color)?, thread_spacing.unwrap_or(0.8));
            if let Some(c) = weft {
                f.weft = hex_linear(c)?;
            }
            Texture::Fabric(f)
        }
        TextureSpec::Image { path, tile_size } => {
            let p = base_dir.join(path);
            let img = image::open(&p)
                .map_err(|e| format!("texture image {}: {e}", p.display()))?
                .to_rgb8();
            let (w, h) = img.dimensions();
            Texture::Image(ImageTexture::from_srgb8(
                w as usize,
                h as usize,
                img.as_raw(),
                *tile_size,
            )?)
        }
    })
}

impl ResolvedLook {
    /// Whether any override carries a texture (the GPU tracer has none).
    pub fn has_textures(&self) -> bool {
        self.materials
            .values()
            .chain(self.parts.values())
            .any(|r| r.texture.is_some())
    }

    /// Apply the look to one part: its material name and labels select the
    /// overrides (material first, then part). Returns the new material and
    /// texture.
    pub(crate) fn apply(
        &self,
        mut pbr: Pbr,
        material_name: Option<&str>,
        labels: &[String],
    ) -> (Pbr, Option<Arc<Texture>>) {
        let mut tex = None;
        let mut hits: Vec<&Resolved> = Vec::new();
        if let Some(m) = material_name.and_then(|n| self.materials.get(n)) {
            hits.push(m);
        }
        if let Some(p) = labels.iter().find_map(|l| self.parts.get(l)) {
            hits.push(p);
        }
        for r in hits {
            let ov = &r.ov;
            if let Some(c) = &ov.color {
                pbr.base_color = hex_linear(c).unwrap_or(pbr.base_color);
            }
            if let Some(v) = ov.metallic {
                pbr.metallic = v.clamp(0.0, 1.0);
            }
            if let Some(v) = ov.roughness {
                pbr.roughness = v.clamp(0.03, 1.0);
            }
            if let Some(v) = ov.clearcoat {
                pbr.clearcoat = v.clamp(0.0, 1.0);
            }
            if let Some(e) = ov.emission {
                pbr.emissive = pbr.base_color.map(|c| c * e.max(0.0));
            }
            if r.texture.is_some() {
                tex = r.texture.clone();
            }
        }
        (pbr, tex)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOOK: &str = r##"{
        "materials": { "satin_black": { "color": "#1e1813", "roughness": 0.55 } },
        "parts": {
            "grille": { "color": "#ff5f00", "emission": 0.4 },
            "desk": { "texture": { "type": "oak", "ring_spacing": 4.0 } },
            "mat": { "texture": { "type": "fabric", "color": "#304050" } }
        }
    }"##;

    fn look() -> ResolvedLook {
        serde_json::from_str::<Look>(LOOK)
            .unwrap()
            .resolve(Path::new("."))
            .unwrap()
    }

    #[test]
    fn material_then_part_overrides_apply() {
        let l = look();
        let base = Pbr::default();
        let (p, t) = l.apply(base, Some("satin_black"), &["shell".into()]);
        assert!((p.roughness - 0.55).abs() < 1e-6);
        assert!(p.base_color[0] > p.base_color[2], "warm black");
        assert!(t.is_none());
        // Part entry wins over the material entry.
        let (p, _) = l.apply(base, Some("satin_black"), &["grille".into()]);
        assert!(p.base_color[0] > 0.9 && p.emissive[0] > 0.3);
        assert!(
            (p.roughness - 0.55).abs() < 1e-6,
            "untouched field keeps the material's"
        );
        let (_, t) = l.apply(base, None, &["desk".into()]);
        assert!(matches!(t.as_deref(), Some(Texture::Wood(_))));
        assert!(l.has_textures());
        // No match: unchanged.
        let (p, t) = l.apply(base, Some("other"), &["x".into()]);
        assert_eq!(p, base);
        assert!(t.is_none());
    }

    #[test]
    fn bad_inputs_are_errors() {
        assert!(hex_linear("#12345").is_err());
        assert!(serde_json::from_str::<Look>(r#"{"parts":{"a":{"colour":"#fff"}}}"#).is_err());
        let bad = serde_json::from_str::<Look>(r##"{"parts":{"a":{"color":"#zzzzzz"}}}"##).unwrap();
        assert!(bad.resolve(Path::new(".")).is_err());
        let missing = serde_json::from_str::<Look>(
            r#"{"parts":{"a":{"texture":{"type":"image","path":"nope.png","tile_size":10}}}}"#,
        )
        .unwrap();
        assert!(missing.resolve(Path::new(".")).is_err());
    }
}
