//! RGBA pixel buffer with depth and pick IDs.

/// RGBA pixel buffer with depth and pick IDs.
pub struct RenderBuffer {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    pub depth: Vec<f32>,
    /// Object ID per pixel for click-to-select (0 = background).
    pub pick_ids: Vec<u32>,
}

impl RenderBuffer {
    pub fn new(width: u32, height: u32) -> Self {
        let size = (width * height) as usize;
        Self {
            width,
            height,
            pixels: vec![0; size * 4],
            depth: vec![f32::INFINITY; size],
            pick_ids: vec![0; size],
        }
    }

    pub fn clear(&mut self, r: u8, g: u8, b: u8) {
        let size = (self.width * self.height) as usize;
        for i in 0..size {
            self.pixels[i * 4] = r;
            self.pixels[i * 4 + 1] = g;
            self.pixels[i * 4 + 2] = b;
            self.pixels[i * 4 + 3] = 255;
            self.depth[i] = f32::INFINITY;
            self.pick_ids[i] = 0;
        }
    }

    pub(crate) fn set_pixel_with_id(
        &mut self,
        x: u32,
        y: u32,
        z: f32,
        color: [u8; 3],
        pick_id: u32,
    ) {
        if x >= self.width || y >= self.height {
            return;
        }
        let idx = (y * self.width + x) as usize;
        if z < self.depth[idx] {
            self.depth[idx] = z;
            self.pixels[idx * 4] = color[0];
            self.pixels[idx * 4 + 1] = color[1];
            self.pixels[idx * 4 + 2] = color[2];
            self.pixels[idx * 4 + 3] = 255;
            self.pick_ids[idx] = pick_id;
        }
    }

    /// Look up the pick ID at a terminal cell position (half-block coordinates).
    pub fn pick_at(&self, col: u16, row: u16) -> u32 {
        // Half-block: each terminal row = 2 pixel rows
        let px = col as u32;
        let py = (row as u32) * 2;
        if px < self.width && py < self.height {
            let idx = (py * self.width + px) as usize;
            if idx < self.pick_ids.len() {
                return self.pick_ids[idx];
            }
        }
        0
    }

    /// Look up pick ID mapping terminal coords to pixel coords based on protocol.
    pub fn pick_at_for_protocol(
        &self,
        col: u16,
        row: u16,
        cell_width: u32,
        cell_height: u32,
    ) -> u32 {
        // Map terminal cell center to pixel buffer
        let px = col as u32 * cell_width + cell_width / 2;
        let py = row as u32 * cell_height + cell_height / 2;
        if px < self.width && py < self.height {
            let idx = (py * self.width + px) as usize;
            if idx < self.pick_ids.len() {
                return self.pick_ids[idx];
            }
        }
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_buffer() {
        let buffer = RenderBuffer::new(100, 50);
        assert_eq!(buffer.width, 100);
        assert_eq!(buffer.height, 50);
        assert_eq!(buffer.pixels.len(), 100 * 50 * 4);
        assert_eq!(buffer.depth.len(), 100 * 50);
    }
}
