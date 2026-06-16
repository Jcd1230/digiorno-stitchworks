use crate::model::{Design, StitchCommand, StitchPoint};
use anyhow::{bail, Result};

/// Render a simple monochrome-ish 4bpp SHV preview bitmap.
///
/// Pixels are packed high-nibble first, low-nibble second.
pub fn render_preview_4bpp(design: &Design, width: u8, height: u8, color_index: u8) -> Result<Vec<u8>> {
    if width == 0 || height == 0 {
        bail!("preview width and height must be non-zero");
    }

    let width = width as usize;
    let height = height as usize;
    let drawable: Vec<&StitchPoint> = design
        .points
        .iter()
        .filter(|p| p.command.is_positioning_move())
        .collect();

    let (min_x, max_x, min_y, max_y) = if drawable.is_empty() {
        (0, 1, 0, 1)
    } else {
        (
            drawable.iter().map(|p| p.x).min().unwrap(),
            drawable.iter().map(|p| p.x).max().unwrap(),
            drawable.iter().map(|p| p.y).min().unwrap(),
            drawable.iter().map(|p| p.y).max().unwrap(),
        )
    };

    let span_x = (max_x - min_x).max(1) as f64;
    let span_y = (max_y - min_y).max(1) as f64;
    let pad = 2.0;
    let scale_x = ((width as f64 - 1.0) - 2.0 * pad) / span_x;
    let scale_y = ((height as f64 - 1.0) - 2.0 * pad) / span_y;
    let scale = scale_x.min(scale_y).max(0.001);

    let to_pixel = |p: &StitchPoint| -> (i32, i32) {
        let px = ((p.x - min_x) as f64 * scale + pad).round() as i32;
        // Bitmap row 0 is top; design coordinates are Cartesian +Y up.
        let py = ((max_y - p.y) as f64 * scale + pad).round() as i32;
        (
            px.clamp(0, width as i32 - 1),
            py.clamp(0, height as i32 - 1),
        )
    };

    let mut pixels = vec![0u8; width * height];
    let mut prev: Option<(i32, i32)> = None;
    for p in &design.points {
        match &p.command {
            StitchCommand::End => break,
            StitchCommand::Jump | StitchCommand::Trim => {
                let pt = to_pixel(p);
                set_pixel(&mut pixels, width, height, pt.0, pt.1, color_index);
                prev = Some(pt);
            }
            StitchCommand::Stitch => {
                let pt = to_pixel(p);
                if let Some(prev_pt) = prev {
                    draw_line(&mut pixels, width, height, prev_pt.0, prev_pt.1, pt.0, pt.1, color_index);
                } else {
                    set_pixel(&mut pixels, width, height, pt.0, pt.1, color_index);
                }
                prev = Some(pt);
            }
            StitchCommand::ColorChange | StitchCommand::Stop | StitchCommand::Other(_) => {
                prev = None;
            }
        }
    }

    let stride = width.div_ceil(2);
    let mut out = Vec::with_capacity(height * stride);
    for y in 0..height {
        for x in (0..width).step_by(2) {
            let hi = pixels[y * width + x] & 0x0f;
            let lo = if x + 1 < width {
                pixels[y * width + x + 1] & 0x0f
            } else {
                0
            };
            out.push((hi << 4) | lo);
        }
    }
    Ok(out)
}

fn set_pixel(pixels: &mut [u8], width: usize, height: usize, x: i32, y: i32, value: u8) {
    if x >= 0 && y >= 0 && (x as usize) < width && (y as usize) < height {
        pixels[y as usize * width + x as usize] = value & 0x0f;
    }
}

fn draw_line(pixels: &mut [u8], width: usize, height: usize, mut x0: i32, mut y0: i32, x1: i32, y1: i32, value: u8) {
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;

    loop {
        set_pixel(pixels, width, height, x0, y0, value);
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
}

pub fn design_path_svg(design: &Design, svg_width: f64, svg_height: f64) -> String {
    let stats = design.stats();
    let span_x = stats.width.max(1) as f64;
    let span_y = stats.height.max(1) as f64;
    let pad = 12.0;
    let scale = ((svg_width - 2.0 * pad) / span_x).min((svg_height - 2.0 * pad) / span_y);

    let map = |x: i32, y: i32| -> (f64, f64) {
        let sx = (x - stats.left) as f64 * scale + pad;
        let sy = (stats.top - y) as f64 * scale + pad;
        (sx, sy)
    };

    let mut d = String::new();
    let mut pen_down = false;
    for p in &design.points {
        match &p.command {
            StitchCommand::End => break,
            StitchCommand::Jump | StitchCommand::Trim => {
                let (x, y) = map(p.x, p.y);
                d.push_str(&format!("M {:.2} {:.2} ", x, y));
                pen_down = false;
            }
            StitchCommand::Stitch => {
                let (x, y) = map(p.x, p.y);
                if pen_down {
                    d.push_str(&format!("L {:.2} {:.2} ", x, y));
                } else {
                    d.push_str(&format!("M {:.2} {:.2} ", x, y));
                    pen_down = true;
                }
            }
            StitchCommand::ColorChange | StitchCommand::Stop | StitchCommand::Other(_) => {
                pen_down = false;
            }
        }
    }

    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{svg_width}" height="{svg_height}" viewBox="0 0 {svg_width} {svg_height}">
  <rect width="100%" height="100%" fill="white"/>
  <path d="{d}" fill="none" stroke="black" stroke-width="1" stroke-linecap="round" stroke-linejoin="round"/>
</svg>
"#
    )
}
