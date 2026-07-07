//! SDF octree representation of stock material.

use serde::{Deserialize, Serialize};

/// An octree node for SDF representation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OctreeNode {
    /// Leaf node with single SDF value.
    Leaf {
        /// Signed distance value (negative = inside, positive = outside).
        sdf: f64,
    },
    /// Branch node with 8 children.
    Branch {
        /// Children in Morton order: (-x-y-z), (+x-y-z), (-x+y-z), (+x+y-z),
        /// (-x-y+z), (+x-y+z), (-x+y+z), (+x+y+z)
        children: Box<[OctreeNode; 8]>,
    },
    /// Solid node (entirely inside or outside).
    Solid {
        /// True if entirely inside (material), false if entirely outside (air).
        inside: bool,
    },
}

impl OctreeNode {
    /// Create a solid node (inside = material).
    pub fn solid(inside: bool) -> Self {
        OctreeNode::Solid { inside }
    }

    /// Create a leaf node with given SDF value.
    pub fn leaf(sdf: f64) -> Self {
        OctreeNode::Leaf { sdf }
    }

    /// Check if this node represents inside (material).
    pub fn is_inside(&self) -> bool {
        match self {
            OctreeNode::Leaf { sdf } => *sdf < 0.0,
            OctreeNode::Solid { inside } => *inside,
            OctreeNode::Branch { children } => {
                // True if any child is inside
                children.iter().any(|c| c.is_inside())
            }
        }
    }

    /// Get the SDF value at this node (approximate for branches).
    pub fn sdf(&self) -> f64 {
        match self {
            OctreeNode::Leaf { sdf } => *sdf,
            OctreeNode::Solid { inside } => {
                if *inside {
                    -1.0
                } else {
                    1.0
                }
            }
            OctreeNode::Branch { children } => {
                // Return minimum (most inside) of children
                children
                    .iter()
                    .map(|c| c.sdf())
                    .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                    .unwrap_or(0.0)
            }
        }
    }
}

/// Stock material represented as an SDF octree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stock {
    root: OctreeNode,
    /// Bounding box [min_x, min_y, min_z, max_x, max_y, max_z].
    bounds: [f64; 6],
    /// Maximum octree depth.
    max_depth: u8,
    /// Minimum cell size.
    min_cell_size: f64,
}

impl Stock {
    /// Create stock from a bounding box with given resolution.
    ///
    /// # Arguments
    ///
    /// * `bounds` - [min_x, min_y, min_z, max_x, max_y, max_z]
    /// * `resolution` - Minimum cell size in mm
    pub fn from_box(bounds: [f64; 6], resolution: f64) -> Self {
        let dx = bounds[3] - bounds[0];
        let dy = bounds[4] - bounds[1];
        let dz = bounds[5] - bounds[2];

        // Calculate max depth needed for resolution
        let max_dim = dx.max(dy).max(dz);
        let max_depth = ((max_dim / resolution).log2().ceil() as u8).min(10);

        // Initial root is solid (all material)
        let root = OctreeNode::Solid { inside: true };

        Self {
            root,
            bounds,
            max_depth,
            min_cell_size: resolution,
        }
    }

    /// Get the stock bounds.
    pub fn bounds(&self) -> [f64; 6] {
        self.bounds
    }

    /// Get the minimum cell size.
    pub fn resolution(&self) -> f64 {
        self.min_cell_size
    }

    /// Get the maximum octree depth.
    pub fn max_depth(&self) -> u8 {
        self.max_depth
    }

    /// Sample the SDF at a point.
    pub fn sdf_at(&self, x: f64, y: f64, z: f64) -> f64 {
        // Check if point is outside bounds
        if x < self.bounds[0]
            || x > self.bounds[3]
            || y < self.bounds[1]
            || y > self.bounds[4]
            || z < self.bounds[2]
            || z > self.bounds[5]
        {
            // Distance to nearest boundary
            let dx = (x - self.bounds[0])
                .min(0.0)
                .abs()
                .max((x - self.bounds[3]).max(0.0));
            let dy = (y - self.bounds[1])
                .min(0.0)
                .abs()
                .max((y - self.bounds[4]).max(0.0));
            let dz = (z - self.bounds[2])
                .min(0.0)
                .abs()
                .max((z - self.bounds[5]).max(0.0));
            return (dx * dx + dy * dy + dz * dz).sqrt();
        }

        self.sample_node(&self.root, x, y, z, &self.bounds, 0)
    }

    fn sample_node(
        &self,
        node: &OctreeNode,
        x: f64,
        y: f64,
        z: f64,
        bounds: &[f64; 6],
        _depth: u8,
    ) -> f64 {
        match node {
            OctreeNode::Leaf { sdf } => *sdf,
            OctreeNode::Solid { inside } => {
                // SDF for a box centered at bounds
                let cx = (bounds[0] + bounds[3]) / 2.0;
                let cy = (bounds[1] + bounds[4]) / 2.0;
                let cz = (bounds[2] + bounds[5]) / 2.0;
                let hx = (bounds[3] - bounds[0]) / 2.0;
                let hy = (bounds[4] - bounds[1]) / 2.0;
                let hz = (bounds[5] - bounds[2]) / 2.0;

                // Box SDF
                let dx = (x - cx).abs() - hx;
                let dy = (y - cy).abs() - hy;
                let dz = (z - cz).abs() - hz;

                let outside =
                    (dx.max(0.0).powi(2) + dy.max(0.0).powi(2) + dz.max(0.0).powi(2)).sqrt();
                let inside_dist = dx.max(dy).max(dz).min(0.0);

                let sdf = outside + inside_dist;
                if *inside {
                    -sdf.abs().max(0.001) // Inside: negative
                } else {
                    sdf.abs().max(0.001) // Outside: positive
                }
            }
            OctreeNode::Branch { children } => {
                // Find which child contains this point
                let cx = (bounds[0] + bounds[3]) / 2.0;
                let cy = (bounds[1] + bounds[4]) / 2.0;
                let cz = (bounds[2] + bounds[5]) / 2.0;

                let idx = (if x >= cx { 1 } else { 0 })
                    | (if y >= cy { 2 } else { 0 })
                    | (if z >= cz { 4 } else { 0 });

                let child_bounds = Self::child_bounds(bounds, idx);
                self.sample_node(&children[idx], x, y, z, &child_bounds, 0)
            }
        }
    }

    fn child_bounds(bounds: &[f64; 6], index: usize) -> [f64; 6] {
        let cx = (bounds[0] + bounds[3]) / 2.0;
        let cy = (bounds[1] + bounds[4]) / 2.0;
        let cz = (bounds[2] + bounds[5]) / 2.0;

        [
            if index & 1 == 0 { bounds[0] } else { cx },
            if index & 2 == 0 { bounds[1] } else { cy },
            if index & 4 == 0 { bounds[2] } else { cz },
            if index & 1 == 0 { cx } else { bounds[3] },
            if index & 2 == 0 { cy } else { bounds[4] },
            if index & 4 == 0 { cz } else { bounds[5] },
        ]
    }

    /// Subtract an arbitrary signed-distance region from the stock.
    ///
    /// The remaining material is `stock ∖ region`. `sdf` must be negative
    /// inside the removal region, positive outside, and 1-Lipschitz (a true
    /// signed distance or a conservative lower bound of one) — the octree
    /// descent relies on the Lipschitz property to prune cells that provably
    /// don't touch the region boundary.
    pub fn subtract_sdf<F: Fn([f64; 3]) -> f64>(&mut self, sdf: F) {
        let bounds = self.bounds;
        let max_depth = self.max_depth;
        let old_root = std::mem::replace(&mut self.root, OctreeNode::Solid { inside: false });
        self.root = Self::subtract_sdf_node_impl(old_root, &sdf, &bounds, 0, max_depth);
    }

    fn subtract_sdf_node_impl<F: Fn([f64; 3]) -> f64>(
        node: OctreeNode,
        sdf: &F,
        bounds: &[f64; 6],
        depth: u8,
        max_depth: u8,
    ) -> OctreeNode {
        // Already air: subtracting more changes nothing.
        if matches!(node, OctreeNode::Solid { inside: false }) {
            return node;
        }

        let cell_center = [
            (bounds[0] + bounds[3]) / 2.0,
            (bounds[1] + bounds[4]) / 2.0,
            (bounds[2] + bounds[5]) / 2.0,
        ];
        let half_size = [
            (bounds[3] - bounds[0]) / 2.0,
            (bounds[4] - bounds[1]) / 2.0,
            (bounds[5] - bounds[2]) / 2.0,
        ];
        let cell_diag = (half_size[0].powi(2) + half_size[1].powi(2) + half_size[2].powi(2)).sqrt();

        let d = sdf(cell_center);

        // Region provably misses the cell: nothing to remove.
        if d > cell_diag {
            return node;
        }
        // Cell provably swallowed by the region: all material removed.
        if d < -cell_diag {
            return OctreeNode::Solid { inside: false };
        }

        // At max depth, store the boolean result as a leaf SDF.
        if depth >= max_depth {
            let sdf_before = match &node {
                OctreeNode::Leaf { sdf } => *sdf,
                OctreeNode::Solid { inside } => {
                    if *inside {
                        -cell_diag
                    } else {
                        cell_diag
                    }
                }
                OctreeNode::Branch { .. } => -cell_diag, // Assume some material
            };

            // Subtraction is intersection with the region's complement, so
            // the region SDF enters NEGATED: material = max(before, -d).
            return OctreeNode::Leaf {
                sdf: sdf_before.max(-d),
            };
        }

        // Subdivide and recurse
        let children = match node {
            OctreeNode::Branch { children } => *children,
            _ => {
                let child = node.clone();
                [
                    child.clone(),
                    child.clone(),
                    child.clone(),
                    child.clone(),
                    child.clone(),
                    child.clone(),
                    child.clone(),
                    child,
                ]
            }
        };

        let new_children: Vec<OctreeNode> = children
            .into_iter()
            .enumerate()
            .map(|(i, child)| {
                let child_bounds = Self::child_bounds(bounds, i);
                Self::subtract_sdf_node_impl(child, sdf, &child_bounds, depth + 1, max_depth)
            })
            .collect();
        let new_children: [OctreeNode; 8] = new_children
            .try_into()
            .expect("octree branch always has 8 children");

        // Try to collapse if all children are same solid
        if let Some(inside) = Self::can_collapse(&new_children) {
            OctreeNode::Solid { inside }
        } else {
            OctreeNode::Branch {
                children: Box::new(new_children),
            }
        }
    }

    /// Subtract a sphere from the stock.
    pub fn subtract_sphere(&mut self, center: [f64; 3], radius: f64) {
        self.subtract_sdf(|p| {
            ((p[0] - center[0]).powi(2) + (p[1] - center[1]).powi(2) + (p[2] - center[2]).powi(2))
                .sqrt()
                - radius
        });
    }

    fn can_collapse(children: &[OctreeNode; 8]) -> Option<bool> {
        let first = match &children[0] {
            OctreeNode::Solid { inside } => *inside,
            _ => return None,
        };

        for child in &children[1..] {
            match child {
                OctreeNode::Solid { inside } if *inside == first => continue,
                _ => return None,
            }
        }

        Some(first)
    }

    /// Subtract a capsule (swept sphere) from the stock.
    pub fn subtract_capsule(&mut self, from: [f64; 3], to: [f64; 3], radius: f64) {
        self.subtract_sdf(|p| capsule_sdf(p, from, to, radius));
    }

    /// Convert stock to a triangle mesh using marching cubes.
    pub fn to_mesh(&self) -> (Vec<[f64; 3]>, Vec<u32>) {
        crate::marching_cubes::MarchingCubes::extract(self)
    }

    /// Get reference to the root node.
    pub fn root(&self) -> &OctreeNode {
        &self.root
    }

    /// Count total nodes in the octree.
    pub fn node_count(&self) -> usize {
        Self::count_nodes(&self.root)
    }

    fn count_nodes(node: &OctreeNode) -> usize {
        match node {
            OctreeNode::Leaf { .. } | OctreeNode::Solid { .. } => 1,
            OctreeNode::Branch { children } => {
                1 + children.iter().map(Self::count_nodes).sum::<usize>()
            }
        }
    }
}

/// Compute SDF for a capsule (line segment with radius).
fn capsule_sdf(p: [f64; 3], a: [f64; 3], b: [f64; 3], r: f64) -> f64 {
    let pa = [p[0] - a[0], p[1] - a[1], p[2] - a[2]];
    let ba = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];

    let ba_len_sq = ba[0] * ba[0] + ba[1] * ba[1] + ba[2] * ba[2];
    let h = if ba_len_sq < 1e-10 {
        0.0
    } else {
        ((pa[0] * ba[0] + pa[1] * ba[1] + pa[2] * ba[2]) / ba_len_sq).clamp(0.0, 1.0)
    };

    let closest = [a[0] + h * ba[0], a[1] + h * ba[1], a[2] + h * ba[2]];
    let dist =
        ((p[0] - closest[0]).powi(2) + (p[1] - closest[1]).powi(2) + (p[2] - closest[2]).powi(2))
            .sqrt();

    dist - r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stock_from_box() {
        let stock = Stock::from_box([0.0, 0.0, 0.0, 100.0, 100.0, 50.0], 2.0);
        assert_eq!(stock.bounds(), [0.0, 0.0, 0.0, 100.0, 100.0, 50.0]);
        assert!(stock.resolution() <= 2.0);
    }

    #[test]
    fn test_sdf_inside() {
        let stock = Stock::from_box([0.0, 0.0, 0.0, 100.0, 100.0, 50.0], 2.0);
        let sdf = stock.sdf_at(50.0, 50.0, 25.0);
        assert!(sdf < 0.0, "Center should be inside (negative SDF)");
    }

    #[test]
    fn test_sdf_outside() {
        let stock = Stock::from_box([0.0, 0.0, 0.0, 100.0, 100.0, 50.0], 2.0);
        let sdf = stock.sdf_at(150.0, 50.0, 25.0);
        assert!(sdf > 0.0, "Outside should have positive SDF");
    }

    #[test]
    fn test_subtract_sphere() {
        let mut stock = Stock::from_box([0.0, 0.0, 0.0, 100.0, 100.0, 50.0], 5.0);
        stock.subtract_sphere([50.0, 50.0, 25.0], 20.0);

        // Center of sphere should now be outside (positive SDF)
        let sdf = stock.sdf_at(50.0, 50.0, 25.0);
        assert!(
            sdf > 0.0,
            "After subtracting sphere, center should be outside"
        );
    }

    #[test]
    fn test_subtract_sphere_boundary_cells() {
        // Points inside the sphere but in partially-covered boundary cells
        // must read as removed too — this exercises the leaf-level SDF
        // (subtraction must negate the region SDF), not just the
        // whole-cell quick-accept path.
        let mut stock = Stock::from_box([0.0, 0.0, 0.0, 100.0, 100.0, 50.0], 5.0);
        stock.subtract_sphere([50.0, 50.0, 25.0], 20.0);

        let just_inside = stock.sdf_at(50.0, 50.0, 25.0 + 18.0);
        assert!(
            just_inside > 0.0,
            "point 2mm inside the sphere surface should be removed, got {just_inside}"
        );
        let just_outside = stock.sdf_at(50.0, 50.0, 25.0 + 22.0);
        assert!(
            just_outside < 0.0,
            "point 2mm outside the sphere surface should remain, got {just_outside}"
        );
    }

    #[test]
    fn test_capsule_sdf() {
        let sdf = capsule_sdf([0.0, 0.0, 0.0], [0.0, 0.0, -10.0], [0.0, 0.0, 10.0], 5.0);
        assert!(
            (sdf - (-5.0)).abs() < 0.01,
            "Point on axis should be at -radius"
        );

        let sdf = capsule_sdf([5.0, 0.0, 0.0], [0.0, 0.0, -10.0], [0.0, 0.0, 10.0], 5.0);
        assert!(sdf.abs() < 0.01, "Point on surface should be near 0");
    }
}
