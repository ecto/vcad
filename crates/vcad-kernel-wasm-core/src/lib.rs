//! WASM bindings for vcad B-rep kernel - core module.
//!
//! This is the main module that provides the Solid type and core operations.
//! Always loaded on initialization.

use serde::{Deserialize, Serialize};
use vcad_kernel::vcad_kernel_math::{Point2, Point3, Vec3};
use vcad_kernel::vcad_kernel_sketch::{SketchProfile, SketchSegment};
use wasm_bindgen::prelude::*;

/// Version string for verifying correct WASM build is loaded in browser.
const KERNEL_VERSION: &str = "0.8.0-core";

/// Get the kernel version string.
#[wasm_bindgen]
pub fn get_kernel_version() -> String {
    KERNEL_VERSION.to_string()
}

/// Initialize the WASM module (sets up panic hook for better error messages).
#[wasm_bindgen(start)]
pub fn init() {
    #[cfg(feature = "console_error_panic_hook")]
    console_error_panic_hook::set_once();
    web_sys::console::log_1(&format!("[WASM] vcad-kernel-wasm-core {} loaded", KERNEL_VERSION).into());
}

/// Triangle mesh output for rendering.
#[derive(Serialize, Deserialize)]
pub struct WasmMesh {
    /// Flat array of vertex positions: [x0, y0, z0, x1, y1, z1, ...]
    pub positions: Vec<f32>,
    /// Flat array of triangle indices: [i0, i1, i2, ...]
    pub indices: Vec<u32>,
}

/// A 2D sketch segment (line or arc) for WASM input.
#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WasmSketchSegment {
    Line { start: [f64; 2], end: [f64; 2] },
    Arc {
        start: [f64; 2],
        end: [f64; 2],
        center: [f64; 2],
        ccw: bool,
    },
}

/// Input for creating a sketch profile from JS.
#[derive(Serialize, Deserialize)]
pub struct WasmSketchProfile {
    pub origin: [f64; 3],
    pub x_dir: [f64; 3],
    pub y_dir: [f64; 3],
    pub segments: Vec<WasmSketchSegment>,
}

impl WasmSketchProfile {
    fn to_kernel_profile(&self) -> Result<SketchProfile, String> {
        let segments: Vec<SketchSegment> = self
            .segments
            .iter()
            .map(|s| match s {
                WasmSketchSegment::Line { start, end } => SketchSegment::Line {
                    start: Point2::new(start[0], start[1]),
                    end: Point2::new(end[0], end[1]),
                },
                WasmSketchSegment::Arc {
                    start,
                    end,
                    center,
                    ccw,
                } => SketchSegment::Arc {
                    start: Point2::new(start[0], start[1]),
                    end: Point2::new(end[0], end[1]),
                    center: Point2::new(center[0], center[1]),
                    ccw: *ccw,
                },
            })
            .collect();

        SketchProfile::new(
            Point3::new(self.origin[0], self.origin[1], self.origin[2]),
            Vec3::new(self.x_dir[0], self.x_dir[1], self.x_dir[2]),
            Vec3::new(self.y_dir[0], self.y_dir[1], self.y_dir[2]),
            segments,
        )
        .map_err(|e| e.to_string())
    }
}

/// A 3D solid geometry object.
#[wasm_bindgen]
pub struct Solid {
    inner: vcad_kernel::Solid,
}

#[wasm_bindgen]
impl Solid {
    /// Create an empty solid.
    #[wasm_bindgen(js_name = empty)]
    pub fn empty() -> Solid {
        Solid {
            inner: vcad_kernel::Solid::empty(),
        }
    }

    /// Create a box with corner at origin and dimensions (sx, sy, sz).
    #[wasm_bindgen(js_name = cube)]
    pub fn cube(sx: f64, sy: f64, sz: f64) -> Solid {
        Solid {
            inner: vcad_kernel::Solid::cube(sx, sy, sz),
        }
    }

    /// Create a cylinder along Z axis with given radius and height.
    #[wasm_bindgen(js_name = cylinder)]
    pub fn cylinder(radius: f64, height: f64, segments: Option<u32>) -> Solid {
        Solid {
            inner: vcad_kernel::Solid::cylinder(radius, height, segments.unwrap_or(32)),
        }
    }

    /// Create a sphere centered at origin with given radius.
    #[wasm_bindgen(js_name = sphere)]
    pub fn sphere(radius: f64, segments: Option<u32>) -> Solid {
        Solid {
            inner: vcad_kernel::Solid::sphere(radius, segments.unwrap_or(32)),
        }
    }

    /// Create a cone/frustum along Z axis.
    #[wasm_bindgen(js_name = cone)]
    pub fn cone(radius_bottom: f64, radius_top: f64, height: f64, segments: Option<u32>) -> Solid {
        Solid {
            inner: vcad_kernel::Solid::cone(radius_bottom, radius_top, height, segments.unwrap_or(32)),
        }
    }

    /// Boolean union (self + other).
    #[wasm_bindgen(js_name = union)]
    pub fn union(&self, other: &Solid) -> Solid {
        Solid {
            inner: self.inner.union(&other.inner),
        }
    }

    /// Boolean difference (self - other).
    #[wasm_bindgen(js_name = difference)]
    pub fn difference(&self, other: &Solid) -> Solid {
        Solid {
            inner: self.inner.difference(&other.inner),
        }
    }

    /// Boolean intersection (self & other).
    #[wasm_bindgen(js_name = intersection)]
    pub fn intersection(&self, other: &Solid) -> Solid {
        Solid {
            inner: self.inner.intersection(&other.inner),
        }
    }

    /// Translate the solid by (x, y, z).
    #[wasm_bindgen(js_name = translate)]
    pub fn translate(&self, x: f64, y: f64, z: f64) -> Solid {
        Solid {
            inner: self.inner.translate(x, y, z),
        }
    }

    /// Rotate the solid by angles in degrees around X, Y, Z axes.
    #[wasm_bindgen(js_name = rotate)]
    pub fn rotate(&self, x_deg: f64, y_deg: f64, z_deg: f64) -> Solid {
        Solid {
            inner: self.inner.rotate(x_deg, y_deg, z_deg),
        }
    }

    /// Scale the solid by (x, y, z).
    #[wasm_bindgen(js_name = scale)]
    pub fn scale(&self, x: f64, y: f64, z: f64) -> Solid {
        Solid {
            inner: self.inner.scale(x, y, z),
        }
    }

    /// Chamfer all edges by the given distance.
    #[wasm_bindgen(js_name = chamfer)]
    pub fn chamfer(&self, distance: f64) -> Solid {
        Solid {
            inner: self.inner.chamfer(distance),
        }
    }

    /// Fillet all edges with the given radius.
    #[wasm_bindgen(js_name = fillet)]
    pub fn fillet(&self, radius: f64) -> Solid {
        Solid {
            inner: self.inner.fillet(radius),
        }
    }

    /// Shell (hollow) the solid by offsetting all faces inward.
    #[wasm_bindgen(js_name = shell)]
    pub fn shell(&self, thickness: f64) -> Solid {
        Solid {
            inner: self.inner.shell(thickness),
        }
    }

    /// Create a linear pattern of the solid.
    #[wasm_bindgen(js_name = linearPattern)]
    pub fn linear_pattern(&self, dir_x: f64, dir_y: f64, dir_z: f64, count: u32, spacing: f64) -> Solid {
        Solid {
            inner: self.inner.linear_pattern(Vec3::new(dir_x, dir_y, dir_z), count, spacing),
        }
    }

    /// Create a circular pattern of the solid.
    #[wasm_bindgen(js_name = circularPattern)]
    #[allow(clippy::too_many_arguments)]
    pub fn circular_pattern(
        &self,
        axis_origin_x: f64,
        axis_origin_y: f64,
        axis_origin_z: f64,
        axis_dir_x: f64,
        axis_dir_y: f64,
        axis_dir_z: f64,
        count: u32,
        angle_deg: f64,
    ) -> Solid {
        Solid {
            inner: self.inner.circular_pattern(
                Point3::new(axis_origin_x, axis_origin_y, axis_origin_z),
                Vec3::new(axis_dir_x, axis_dir_y, axis_dir_z),
                count,
                angle_deg,
            ),
        }
    }

    /// Check if the solid is empty.
    #[wasm_bindgen(js_name = isEmpty)]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Get the triangle mesh representation.
    #[wasm_bindgen(js_name = getMesh)]
    pub fn get_mesh(&self, segments: Option<u32>) -> JsValue {
        let mesh = self.inner.to_mesh(segments.unwrap_or(32));
        let wasm_mesh = WasmMesh {
            positions: mesh.vertices,
            indices: mesh.indices,
        };
        serde_wasm_bindgen::to_value(&wasm_mesh).unwrap_or(JsValue::NULL)
    }

    /// Compute the volume of the solid.
    #[wasm_bindgen(js_name = volume)]
    pub fn volume(&self) -> f64 {
        self.inner.volume()
    }

    /// Compute the surface area of the solid.
    #[wasm_bindgen(js_name = surfaceArea)]
    pub fn surface_area(&self) -> f64 {
        self.inner.surface_area()
    }

    /// Get the bounding box as [minX, minY, minZ, maxX, maxY, maxZ].
    #[wasm_bindgen(js_name = boundingBox)]
    pub fn bounding_box(&self) -> Vec<f64> {
        let (min, max) = self.inner.bounding_box();
        vec![min[0], min[1], min[2], max[0], max[1], max[2]]
    }

    /// Get the center of mass as [x, y, z].
    #[wasm_bindgen(js_name = centerOfMass)]
    pub fn center_of_mass(&self) -> Vec<f64> {
        let com = self.inner.center_of_mass();
        vec![com[0], com[1], com[2]]
    }

    /// Extrude a 2D sketch profile.
    #[wasm_bindgen(js_name = extrude)]
    pub fn extrude(profile_js: JsValue, direction: Vec<f64>) -> Result<Solid, JsError> {
        let profile: WasmSketchProfile = serde_wasm_bindgen::from_value(profile_js)
            .map_err(|e| JsError::new(&format!("Invalid profile: {}", e)))?;

        if direction.len() != 3 {
            return Err(JsError::new("Direction must have 3 components"));
        }

        let kernel_profile = profile.to_kernel_profile().map_err(|e| JsError::new(&e))?;
        let dir = Vec3::new(direction[0], direction[1], direction[2]);

        vcad_kernel::Solid::extrude(kernel_profile, dir)
            .map(|inner| Solid { inner })
            .map_err(|e| JsError::new(&e.to_string()))
    }

    /// Revolve a 2D sketch profile around an axis.
    #[wasm_bindgen(js_name = revolve)]
    pub fn revolve(
        profile_js: JsValue,
        axis_origin: Vec<f64>,
        axis_dir: Vec<f64>,
        angle_deg: f64,
    ) -> Result<Solid, JsError> {
        let profile: WasmSketchProfile = serde_wasm_bindgen::from_value(profile_js)
            .map_err(|e| JsError::new(&format!("Invalid profile: {}", e)))?;

        if axis_origin.len() != 3 || axis_dir.len() != 3 {
            return Err(JsError::new("Axis vectors must have 3 components"));
        }

        let kernel_profile = profile.to_kernel_profile().map_err(|e| JsError::new(&e))?;
        let origin = Point3::new(axis_origin[0], axis_origin[1], axis_origin[2]);
        let dir = Vec3::new(axis_dir[0], axis_dir[1], axis_dir[2]);

        vcad_kernel::Solid::revolve(kernel_profile, origin, dir, angle_deg)
            .map(|inner| Solid { inner })
            .map_err(|e| JsError::new(&e.to_string()))
    }
}
