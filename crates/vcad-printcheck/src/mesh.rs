//! Triangle soup + the ray machinery every check is built on.
//!
//! The checks here deliberately run on the *exported* triangle soup rather
//! than on a BRep or on the generator's own parameters. The rana `60c` shell
//! passed its author's analytic profile verification while the shipped STL
//! had 0.05 mm cracks around 85% of its circumference: only rays cast at the
//! file caught it. See `crates/vcad-printcheck/README.md`.

use std::path::Path;

/// A loaded triangle soup, in millimetres, already rotated so that the print
/// direction is `+Z`.
#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub tris: Vec<[[f64; 3]; 3]>,
}

/// Which model axis points up on the build plate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orientation {
    ZUp,
    ZDown,
    XUp,
    XDown,
    YUp,
    YDown,
}

impl Orientation {
    /// Parse the CLI spelling (`z`, `-z`, `x`, `-x`, `y`, `-y`).
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "z" | "+z" | "z+" | "zup" => Some(Orientation::ZUp),
            "-z" | "z-" | "zdown" => Some(Orientation::ZDown),
            "x" | "+x" | "x+" | "xup" => Some(Orientation::XUp),
            "-x" | "x-" | "xdown" => Some(Orientation::XDown),
            "y" | "+y" | "y+" | "yup" => Some(Orientation::YUp),
            "-y" | "y-" | "ydown" => Some(Orientation::YDown),
            _ => None,
        }
    }

    /// Rotate a point so the chosen axis becomes `+Z`. These are proper
    /// rotations (determinant +1), so triangle winding — and therefore the
    /// facet normals the overhang census reads — survives the change.
    fn apply(self, p: [f64; 3]) -> [f64; 3] {
        let [x, y, z] = p;
        match self {
            Orientation::ZUp => [x, y, z],
            Orientation::ZDown => [x, -y, -z],
            Orientation::XUp => [y, z, x],
            Orientation::XDown => [-y, z, -x],
            Orientation::YUp => [z, x, y],
            Orientation::YDown => [z, -x, -y],
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MeshError {
    #[error("read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("mesh has no triangles: {0}")]
    Empty(String),
}

impl Mesh {
    /// Load a binary or ASCII STL, rotated into the print frame.
    pub fn load_stl(path: &Path, orientation: Orientation) -> Result<Mesh, MeshError> {
        let file = std::fs::File::open(path).map_err(|e| MeshError::Io {
            path: path.display().to_string(),
            source: e,
        })?;
        let mut reader = std::io::BufReader::new(file);
        let stl = stl_io::read_stl(&mut reader).map_err(|e| MeshError::Io {
            path: path.display().to_string(),
            source: e,
        })?;

        let mut tris = Vec::with_capacity(stl.faces.len());
        for face in &stl.faces {
            let mut t = [[0.0; 3]; 3];
            for (i, vi) in face.vertices.iter().enumerate() {
                let v = stl.vertices[*vi];
                t[i] = orientation.apply([v[0] as f64, v[1] as f64, v[2] as f64]);
            }
            tris.push(t);
        }
        if tris.is_empty() {
            return Err(MeshError::Empty(path.display().to_string()));
        }
        Ok(Mesh { tris })
    }

    /// Axis-aligned bounds as `([min], [max])`.
    pub fn bounds(&self) -> ([f64; 3], [f64; 3]) {
        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        for t in &self.tris {
            for v in t {
                for k in 0..3 {
                    lo[k] = lo[k].min(v[k]);
                    hi[k] = hi[k].max(v[k]);
                }
            }
        }
        (lo, hi)
    }

    /// A copy with coordinates cycled so that `axis` (0=X, 1=Y, 2=Z) plays the
    /// role of the ray direction. Used by the min-wall check, which needs to
    /// measure material thickness along all three axes — a 0.2 mm vertical
    /// comb wall is invisible to a vertical ray and only shows up sideways.
    pub fn permuted(&self, axis: usize) -> Mesh {
        if axis == 2 {
            return self.clone();
        }
        let tris = self
            .tris
            .iter()
            .map(|t| {
                let mut out = [[0.0; 3]; 3];
                for (i, v) in t.iter().enumerate() {
                    out[i] = match axis {
                        // cyclic permutations keep the winding, so parity stays
                        // meaningful along the permuted ray
                        0 => [v[1], v[2], v[0]],
                        _ => [v[2], v[0], v[1]],
                    };
                }
                out
            })
            .collect();
        Mesh { tris }
    }
}

/// Uniform XY bucket grid over triangle bounding boxes, so a vertical ray only
/// tests the triangles that can possibly contain it.
pub struct RayIndex<'a> {
    tris: &'a [[[f64; 3]; 3]],
    min: [f64; 2],
    cell: f64,
    nx: usize,
    ny: usize,
    buckets: Vec<Vec<u32>>,
}

impl<'a> RayIndex<'a> {
    pub fn build(mesh: &'a Mesh) -> RayIndex<'a> {
        let (lo, hi) = mesh.bounds();
        let span_x = (hi[0] - lo[0]).max(1e-6);
        let span_y = (hi[1] - lo[1]).max(1e-6);
        // ~128 cells across the longer axis, clamped so a huge mesh does not
        // explode the bucket count.
        let target = (span_x.max(span_y)) / 128.0;
        let cell = target.max(1e-3);
        let nx = ((span_x / cell).ceil() as usize + 1).min(4096);
        let ny = ((span_y / cell).ceil() as usize + 1).min(4096);
        let mut buckets = vec![Vec::new(); nx * ny];
        for (i, t) in mesh.tris.iter().enumerate() {
            let (mut x0, mut x1) = (f64::INFINITY, f64::NEG_INFINITY);
            let (mut y0, mut y1) = (f64::INFINITY, f64::NEG_INFINITY);
            for v in t {
                x0 = x0.min(v[0]);
                x1 = x1.max(v[0]);
                y0 = y0.min(v[1]);
                y1 = y1.max(v[1]);
            }
            let cx0 = (((x0 - lo[0]) / cell).floor().max(0.0) as usize).min(nx - 1);
            let cx1 = (((x1 - lo[0]) / cell).floor().max(0.0) as usize).min(nx - 1);
            let cy0 = (((y0 - lo[1]) / cell).floor().max(0.0) as usize).min(ny - 1);
            let cy1 = (((y1 - lo[1]) / cell).floor().max(0.0) as usize).min(ny - 1);
            for cy in cy0..=cy1 {
                for cx in cx0..=cx1 {
                    buckets[cy * nx + cx].push(i as u32);
                }
            }
        }
        RayIndex {
            tris: &mesh.tris,
            min: [lo[0], lo[1]],
            cell,
            nx,
            ny,
            buckets,
        }
    }

    /// Sorted crossings of a vertical ray at `(x, y)` with the surface.
    ///
    /// Ported from `rana tools/support-check.py::crossings` — barycentric
    /// solve in the XY plane, `u`/`v` inclusive to a 1e-9 tolerance.
    pub fn crossings(&self, x: f64, y: f64) -> Vec<Crossing> {
        let cx = (((x - self.min[0]) / self.cell).floor()).max(0.0) as usize;
        let cy = (((y - self.min[1]) / self.cell).floor()).max(0.0) as usize;
        if cx >= self.nx || cy >= self.ny {
            return Vec::new();
        }
        let mut out = Vec::new();
        for &ti in &self.buckets[cy * self.nx + cx] {
            let t = &self.tris[ti as usize];
            let (a, b, c) = (t[0], t[1], t[2]);
            let d1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let d2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let den = d1[0] * d2[1] - d1[1] * d2[0];
            if den.abs() < 1e-12 {
                continue;
            }
            let u = ((x - a[0]) * d2[1] - (y - a[1]) * d2[0]) / den;
            let v = (d1[0] * (y - a[1]) - d1[1] * (x - a[0])) / den;
            if u < -1e-9 || v < -1e-9 || u + v > 1.0 + 1e-9 {
                continue;
            }
            // Unit facet normal. The min-wall check reads two things off it:
            // how squarely the ray hits the surface (|n·ray|, near 0 means the
            // ray skims and its chord means nothing), and whether the faces at
            // the two ends of a span oppose each other — thickness is only
            // defined between opposing surfaces, never across a taper.
            let mut n = [
                d1[1] * d2[2] - d1[2] * d2[1],
                d1[2] * d2[0] - d1[0] * d2[2],
                den,
            ];
            let nlen = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if nlen > 0.0 {
                n = [n[0] / nlen, n[1] / nlen, n[2] / nlen];
            }
            out.push(Crossing {
                z: a[2] + u * d1[2] + v * d2[2],
                normal: n,
            });
        }
        out.sort_by(|p, q| p.z.partial_cmp(&q.z).unwrap_or(std::cmp::Ordering::Equal));
        out
    }
}

/// Where a ray meets the surface, and how squarely it hits it.
#[derive(Clone, Copy, Debug)]
pub struct Crossing {
    pub z: f64,
    /// Unit normal of the facet that was hit, in the ray's frame (the ray runs
    /// along `+Z`, so `normal[2]` is the cosine of the incidence angle).
    pub normal: [f64; 3],
}

impl Crossing {
    /// |cos| between ray and surface: 1 = head on, 0 = grazing.
    pub fn align(&self) -> f64 {
        self.normal[2].abs()
    }
}

/// Material intervals along a ray, by STRICT PARITY (odd crossing count =
/// inside) — exactly what a slicer's mesh analysis sees.
///
/// Parity is chosen over a winding-number sum on purpose. Parity reads a
/// z-overlap between two *separate* bodies as an interior void strip, and that
/// is a defect we want flagged: rana finding #11 was a rim chamfer ring
/// overlapping the sector columns by 0.05 mm, which parity showed as a 0.05 mm
/// crack ring at both rims. The first version of that checker used a winding
/// sum with an inverted sign, saw zero material anywhere, and passed
/// vacuously — hence the fixtures in `tests/`.
pub fn intervals(crossings: &[Crossing]) -> Vec<(f64, f64)> {
    solid_spans(crossings)
        .into_iter()
        .map(|s| (s.lo, s.hi))
        .collect()
}

/// One run of material along a ray.
#[derive(Clone, Copy, Debug)]
pub struct Span {
    pub lo: f64,
    pub hi: f64,
    /// The weaker of the two end faces' alignments with the ray. A span whose
    /// ends are both grazed is a silhouette chord, not a wall thickness.
    pub align: f64,
    /// Do the faces at the two ends point away from each other? Only then is
    /// the span a thickness between two surfaces of one wall. A chamfer or
    /// fillet feathering to an edge produces short spans between faces that
    /// meet rather than oppose — real geometry, but not a thin wall, and
    /// enough of them to bury every genuine finding.
    pub opposing: bool,
    /// Distance to the previous and next material along the same ray;
    /// infinite where the span faces open air.
    ///
    /// A wall is only thin if what sits either side of it is a real void. The
    /// rana shell is meshed as 0.5° sector prisms whose seams parity reads as
    /// 0.06 mm gaps, which turns a solid 2 mm tube into a stack of 0.24 mm
    /// "walls" for any ray that crosses the seams. Those slabs have a hairline
    /// either side, so this pair is what tells them apart from a comb wall
    /// standing in open air.
    pub void_before: f64,
    pub void_after: f64,
}

pub fn solid_spans(crossings: &[Crossing]) -> Vec<Span> {
    let mut iv: Vec<Span> = Vec::new();
    let mut inside = false;
    let mut start = Crossing {
        z: 0.0,
        normal: [0.0; 3],
    };
    for &c in crossings {
        if !inside {
            start = c;
        } else {
            let dot = start.normal[0] * c.normal[0]
                + start.normal[1] * c.normal[1]
                + start.normal[2] * c.normal[2];
            iv.push(Span {
                lo: start.z,
                hi: c.z,
                align: start.align().min(c.align()),
                // within 30° of exactly opposed
                opposing: dot <= -0.866,
                void_before: f64::INFINITY,
                void_after: f64::INFINITY,
            });
        }
        inside = !inside;
    }
    iv.retain(|s| s.hi - s.lo > 1e-4);
    for k in 0..iv.len() {
        if k > 0 {
            iv[k].void_before = iv[k].lo - iv[k - 1].hi;
        }
        if k + 1 < iv.len() {
            iv[k].void_after = iv[k + 1].lo - iv[k].hi;
        }
    }
    iv
}
