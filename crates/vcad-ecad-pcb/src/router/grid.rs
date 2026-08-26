//! Lee/wave grid-based router.
//!
//! Implements a classic BFS wavefront expansion on a discretized grid.
//! Each cell can be empty, occupied by a specific net, or blocked (obstacle).
//! The router expands from the start cell until it reaches the end cell,
//! then backtraces to produce a route.

use std::collections::VecDeque;

use vcad_ir::Vec2;

use super::{NetId, RouteResult};

/// Cell state in the routing grid.
#[derive(Debug, Clone, Copy, PartialEq)]
enum CellState {
    /// Empty cell, available for routing.
    Empty,
    /// Occupied by a net (cannot be crossed by other nets).
    Occupied(NetId),
    /// Permanently blocked (board edge, keepout, etc.).
    Blocked,
}

/// Grid-based Lee/wave router.
///
/// Discretizes the board area into a regular grid and uses BFS wavefront
/// expansion to find shortest paths between pads. Obstacles and existing
/// routes are respected as blocked or occupied cells.
pub struct GridRouter {
    grid: Vec<Vec<CellState>>,
    width: usize,
    height: usize,
    resolution: f64,
}

impl GridRouter {
    /// Create a new grid router for a board of the given dimensions.
    ///
    /// # Arguments
    ///
    /// * `board_width` -- Board width in mm.
    /// * `board_height` -- Board height in mm.
    /// * `resolution` -- Grid cell size in mm (smaller = finer but slower).
    pub fn new(board_width: f64, board_height: f64, resolution: f64) -> Self {
        let width = (board_width / resolution).ceil() as usize;
        let height = (board_height / resolution).ceil() as usize;
        let grid = vec![vec![CellState::Empty; height]; width];
        Self {
            grid,
            width,
            height,
            resolution,
        }
    }

    /// Mark a rectangular region as blocked (obstacle).
    ///
    /// Coordinates are in board space (mm). The region `[min, max]` will be
    /// rasterized onto the grid and all overlapping cells marked as blocked.
    pub fn add_obstacle(&mut self, min: Vec2, max: Vec2) {
        let x0 = ((min.x / self.resolution).floor() as isize).max(0) as usize;
        let y0 = ((min.y / self.resolution).floor() as isize).max(0) as usize;
        let x1 = ((max.x / self.resolution).ceil() as usize).min(self.width);
        let y1 = ((max.y / self.resolution).ceil() as usize).min(self.height);

        for col in &mut self.grid[x0..x1] {
            col[y0..y1].fill(CellState::Blocked);
        }
    }

    /// Mark a rectangular region as occupied by a specific net.
    ///
    /// Cells occupied by a net can be traversed by routes of the same net
    /// but not by routes of other nets.
    pub fn add_net_obstacle(&mut self, min: Vec2, max: Vec2, net_id: NetId) {
        let x0 = ((min.x / self.resolution).floor() as isize).max(0) as usize;
        let y0 = ((min.y / self.resolution).floor() as isize).max(0) as usize;
        let x1 = ((max.x / self.resolution).ceil() as usize).min(self.width);
        let y1 = ((max.y / self.resolution).ceil() as usize).min(self.height);

        for col in &mut self.grid[x0..x1] {
            for cell in &mut col[y0..y1] {
                if *cell == CellState::Empty {
                    *cell = CellState::Occupied(net_id);
                }
            }
        }
    }

    /// Convert board coordinates (mm) to grid cell indices.
    fn to_grid(&self, pos: Vec2) -> Option<(usize, usize)> {
        let gx = (pos.x / self.resolution).round() as isize;
        let gy = (pos.y / self.resolution).round() as isize;
        if gx >= 0 && gy >= 0 && (gx as usize) < self.width && (gy as usize) < self.height {
            Some((gx as usize, gy as usize))
        } else {
            None
        }
    }

    /// Convert grid cell indices back to board coordinates (mm), returning
    /// the center of the cell.
    fn to_board(&self, gx: usize, gy: usize) -> Vec2 {
        Vec2::new((gx as f64) * self.resolution, (gy as f64) * self.resolution)
    }

    /// Check if a cell is passable for the given net.
    fn is_passable(&self, x: usize, y: usize, net_id: NetId) -> bool {
        match self.grid[x][y] {
            CellState::Empty => true,
            CellState::Occupied(id) => id == net_id,
            CellState::Blocked => false,
        }
    }

    /// Route a single net from start to end using Lee/BFS wavefront expansion.
    ///
    /// # Arguments
    ///
    /// * `net` -- Human-readable net name for the result.
    /// * `start` -- Start position in board coordinates (mm).
    /// * `end` -- End position in board coordinates (mm).
    ///
    /// # Returns
    ///
    /// A [`RouteResult`] indicating success or failure, with the routed
    /// segments if successful. On success, the grid is updated to mark
    /// routed cells as occupied.
    pub fn route_net(&mut self, net: &str, start: Vec2, end: Vec2) -> RouteResult {
        let net_id = self.net_name_to_id(net);

        let (sx, sy) = match self.to_grid(start) {
            Some(pos) => pos,
            None => {
                return RouteResult {
                    net: net.to_string(),
                    segments: vec![],
                    vias: vec![],
                    success: false,
                }
            }
        };

        let (ex, ey) = match self.to_grid(end) {
            Some(pos) => pos,
            None => {
                return RouteResult {
                    net: net.to_string(),
                    segments: vec![],
                    vias: vec![],
                    success: false,
                }
            }
        };

        // BFS wavefront expansion
        // dist[x][y] = distance from start, or usize::MAX if unvisited
        let mut dist = vec![vec![usize::MAX; self.height]; self.width];
        let mut prev: Vec<Vec<Option<(usize, usize)>>> = vec![vec![None; self.height]; self.width];
        let mut queue = VecDeque::new();

        dist[sx][sy] = 0;
        queue.push_back((sx, sy));

        // 4-connected neighbors (up, down, left, right)
        let neighbors: [(isize, isize); 4] = [(-1, 0), (1, 0), (0, -1), (0, 1)];

        let mut found = false;

        while let Some((cx, cy)) = queue.pop_front() {
            if cx == ex && cy == ey {
                found = true;
                break;
            }

            let next_dist = dist[cx][cy] + 1;

            for &(dx, dy) in &neighbors {
                let nx = cx as isize + dx;
                let ny = cy as isize + dy;

                if nx < 0 || ny < 0 {
                    continue;
                }
                let nx = nx as usize;
                let ny = ny as usize;

                if nx >= self.width || ny >= self.height {
                    continue;
                }

                if dist[nx][ny] != usize::MAX {
                    continue;
                }

                if !self.is_passable(nx, ny, net_id) {
                    // Allow the exact target cell even if occupied by another net,
                    // since we need to reach the pad.
                    if !(nx == ex && ny == ey) {
                        continue;
                    }
                }

                dist[nx][ny] = next_dist;
                prev[nx][ny] = Some((cx, cy));
                queue.push_back((nx, ny));
            }
        }

        if !found {
            return RouteResult {
                net: net.to_string(),
                segments: vec![],
                vias: vec![],
                success: false,
            };
        }

        // Backtrace to build the path
        let mut path = Vec::new();
        let mut cx = ex;
        let mut cy = ey;
        path.push((cx, cy));
        while let Some((px, py)) = prev[cx][cy] {
            path.push((px, py));
            cx = px;
            cy = py;
        }
        path.reverse();

        // Mark route cells as occupied
        for &(px, py) in &path {
            if self.grid[px][py] == CellState::Empty {
                self.grid[px][py] = CellState::Occupied(net_id);
            }
        }

        // Simplify path into straight segments (merge colinear runs)
        let segments = self.simplify_path(&path);

        RouteResult {
            net: net.to_string(),
            segments,
            vias: vec![],
            success: true,
        }
    }

    /// Simplify a grid path into minimal straight-line segments by merging
    /// colinear consecutive cells.
    fn simplify_path(&self, path: &[(usize, usize)]) -> Vec<(Vec2, Vec2)> {
        if path.len() < 2 {
            return vec![];
        }

        let mut segments = Vec::new();
        let mut seg_start = 0;
        let mut dir = (
            path[1].0 as isize - path[0].0 as isize,
            path[1].1 as isize - path[0].1 as isize,
        );

        for i in 2..path.len() {
            let new_dir = (
                path[i].0 as isize - path[i - 1].0 as isize,
                path[i].1 as isize - path[i - 1].1 as isize,
            );
            if new_dir != dir {
                // Direction changed -- emit the segment up to previous point
                let a = self.to_board(path[seg_start].0, path[seg_start].1);
                let b = self.to_board(path[i - 1].0, path[i - 1].1);
                segments.push((a, b));
                seg_start = i - 1;
                dir = new_dir;
            }
        }

        // Emit final segment
        let a = self.to_board(path[seg_start].0, path[seg_start].1);
        let b = self.to_board(path[path.len() - 1].0, path[path.len() - 1].1);
        segments.push((a, b));

        segments
    }

    /// Simple hash of net name to a numeric ID.
    fn net_name_to_id(&self, name: &str) -> NetId {
        let mut hash: u32 = 5381;
        for byte in name.bytes() {
            hash = hash.wrapping_mul(33).wrapping_add(u32::from(byte));
        }
        // Ensure non-zero
        if hash == 0 {
            1
        } else {
            hash
        }
    }

    /// Returns the grid width in cells.
    pub fn grid_width(&self) -> usize {
        self.width
    }

    /// Returns the grid height in cells.
    pub fn grid_height(&self) -> usize {
        self.height
    }

    /// Returns the resolution (mm per grid cell).
    pub fn resolution(&self) -> f64 {
        self.resolution
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_route() {
        let mut router = GridRouter::new(10.0, 10.0, 0.5);
        let result = router.route_net("VCC", Vec2::new(1.0, 1.0), Vec2::new(8.0, 1.0));
        assert!(result.success);
        assert!(!result.segments.is_empty());
        assert_eq!(result.net, "VCC");

        // Start and end of the route should match our coordinates
        let first_seg = &result.segments[0];
        assert!((first_seg.0.x - 1.0).abs() < 0.6);
        assert!((first_seg.0.y - 1.0).abs() < 0.6);

        let last_seg = &result.segments[result.segments.len() - 1];
        assert!((last_seg.1.x - 8.0).abs() < 0.6);
        assert!((last_seg.1.y - 1.0).abs() < 0.6);
    }

    #[test]
    fn route_around_obstacle() {
        let mut router = GridRouter::new(10.0, 10.0, 0.5);
        // Block a vertical wall in the middle
        router.add_obstacle(Vec2::new(4.5, 0.0), Vec2::new(5.5, 8.0));

        let result = router.route_net("SIG", Vec2::new(2.0, 5.0), Vec2::new(8.0, 5.0));
        assert!(result.success);
        // Route must go around the obstacle, so it needs more than one segment
        assert!(
            result.segments.len() > 1,
            "route should detour around obstacle"
        );
    }

    #[test]
    fn route_blocked() {
        let mut router = GridRouter::new(10.0, 10.0, 0.5);
        // Block the entire board
        router.add_obstacle(Vec2::new(0.0, 0.0), Vec2::new(10.0, 10.0));

        let result = router.route_net("NET", Vec2::new(1.0, 1.0), Vec2::new(9.0, 9.0));
        assert!(!result.success);
        assert!(result.segments.is_empty());
    }

    #[test]
    fn route_out_of_bounds() {
        let mut router = GridRouter::new(10.0, 10.0, 0.5);
        let result = router.route_net("NET", Vec2::new(-5.0, -5.0), Vec2::new(5.0, 5.0));
        assert!(!result.success);
    }

    #[test]
    fn straight_line_simplified() {
        let mut router = GridRouter::new(20.0, 20.0, 1.0);
        let result = router.route_net("NET", Vec2::new(2.0, 5.0), Vec2::new(18.0, 5.0));
        assert!(result.success);
        // A straight horizontal route should be a single segment
        assert_eq!(result.segments.len(), 1);
    }

    #[test]
    fn grid_dimensions() {
        let router = GridRouter::new(50.0, 30.0, 0.25);
        assert_eq!(router.grid_width(), 200);
        assert_eq!(router.grid_height(), 120);
        assert!((router.resolution() - 0.25).abs() < f64::EPSILON);
    }

    #[test]
    fn same_net_passable() {
        let mut router = GridRouter::new(10.0, 10.0, 0.5);
        // Place a net obstacle for net "VCC" across the horizontal path
        // Leave a gap at the top (y > 8) so OTHER net can route around
        router.add_net_obstacle(Vec2::new(4.0, 0.0), Vec2::new(6.0, 8.0), 1);

        // A route with a different net_id must route around the obstacle
        let result = router.route_net("OTHER", Vec2::new(2.0, 5.0), Vec2::new(8.0, 5.0));
        assert!(result.success);
        assert!(
            result.segments.len() > 1,
            "OTHER net should detour around occupied cells"
        );
    }

    #[test]
    fn multiple_routes() {
        let mut router = GridRouter::new(20.0, 10.0, 0.5);

        let r1 = router.route_net("NET1", Vec2::new(1.0, 5.0), Vec2::new(19.0, 5.0));
        assert!(r1.success);

        // Second route on a parallel line should also succeed
        let r2 = router.route_net("NET2", Vec2::new(1.0, 3.0), Vec2::new(19.0, 3.0));
        assert!(r2.success);
    }
}
