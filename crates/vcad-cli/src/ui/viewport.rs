//! 3D viewport rendering — HalfBlock (true-color) and Braille (fallback).

use super::buffer::{Color, CellBuffer, Rect};
use super::theme;
use crate::render::RenderBuffer;

/// Sample a pixel from the render buffer, returning Color.
fn sample(render_buffer: &RenderBuffer, px: u32, py: u32) -> Color {
    if px >= render_buffer.width || py >= render_buffer.height {
        return theme::BG();
    }
    let idx = (py * render_buffer.width + px) as usize * 4;
    if idx + 2 < render_buffer.pixels.len() {
        Color::rgb(
            render_buffer.pixels[idx],
            render_buffer.pixels[idx + 1],
            render_buffer.pixels[idx + 2],
        )
    } else {
        theme::BG()
    }
}

/// Render viewport using half-block characters for 2x vertical resolution.
/// Each terminal cell renders two pixels: top pixel as fg, bottom pixel as bg.
pub fn render_viewport(buf: &mut CellBuffer, render_buffer: &RenderBuffer, area: Rect) {
    for row in 0..area.height {
        for col in 0..area.width {
            let top = sample(render_buffer, col as u32, (row as u32) * 2);
            let bot = sample(render_buffer, col as u32, (row as u32) * 2 + 1);
            if let Some(cell) = buf.cell_mut(area.x + col, area.y + row) {
                cell.ch = '\u{2580}'; // ▀ upper half block
                cell.fg = top;
                cell.bg = bot;
            }
        }
    }
}

/// Render viewport using braille characters for non-true-color terminals.
/// Each braille character represents a 2x4 pixel grid.
#[allow(dead_code)]
pub fn render_viewport_braille(buf: &mut CellBuffer, render_buffer: &RenderBuffer, area: Rect) {
    for row in 0..area.height {
        for col in 0..area.width {
            let px = (col as u32) * 2;
            let py = (row as u32) * 4;
            let mut dots = 0u8;
            let mut total_r = 0u32;
            let mut total_g = 0u32;
            let mut total_b = 0u32;
            let mut count = 0u32;

            for dy in 0..4u32 {
                for dx in 0..2u32 {
                    let sx = px + dx;
                    let sy = py + dy;
                    if sx < render_buffer.width && sy < render_buffer.height {
                        let idx = (sy * render_buffer.width + sx) as usize * 4;
                        if idx + 2 < render_buffer.pixels.len() {
                            let r = render_buffer.pixels[idx] as u32;
                            let g = render_buffer.pixels[idx + 1] as u32;
                            let b = render_buffer.pixels[idx + 2] as u32;
                            total_r += r;
                            total_g += g;
                            total_b += b;
                            count += 1;
                            if (r + g + b) / 3 > 50 {
                                let bit = match (dx, dy) {
                                    (0, 0) => 0,
                                    (0, 1) => 1,
                                    (0, 2) => 2,
                                    (1, 0) => 3,
                                    (1, 1) => 4,
                                    (1, 2) => 5,
                                    (0, 3) => 6,
                                    (1, 3) => 7,
                                    _ => 0,
                                };
                                dots |= 1 << bit;
                            }
                        }
                    }
                }
            }

            let ch = char::from_u32(0x2800 + dots as u32).unwrap_or(' ');
            let color = if count > 0 {
                Color::rgb(
                    (total_r / count) as u8,
                    (total_g / count) as u8,
                    (total_b / count) as u8,
                )
            } else {
                theme::BG()
            };

            if let Some(cell) = buf.cell_mut(area.x + col, area.y + row) {
                cell.ch = ch;
                cell.fg = color;
                cell.bg = theme::BG();
            }
        }
    }
}
