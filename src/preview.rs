use crate::model::{Design, StitchCommand, StitchPoint};
use anyhow::{Result, bail};

pub const DESIGNER1_PALETTE: [[u8; 3]; 16] = [
    [230, 190, 100],
    [40, 40, 40],
    [240, 240, 240],
    [0, 100, 200],
    [0, 60, 160],
    [20, 100, 40],
    [40, 160, 40],
    [200, 40, 40],
    [150, 60, 180],
    [220, 170, 40],
    [120, 120, 120],
    [200, 100, 80],
    [220, 120, 40],
    [180, 100, 140],
    [220, 140, 160],
    [160, 40, 40],
];

const SHV_PREVIEW_UNITS_PER_PIXEL: i32 = 10;

/// Render a quantized 4bpp SHV preview bitmap using the observed Designer 1 palette.
///
/// Pixels are packed high-nibble first, low-nibble second.
pub fn render_preview_4bpp(design: &Design, width: u8, height: u8) -> Result<Vec<u8>> {
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
    let drawn_w = span_x * scale;
    let drawn_h = span_y * scale;
    let offset_x = pad + (((width as f64 - 1.0) - 2.0 * pad) - drawn_w) / 2.0;
    let offset_y = pad + (((height as f64 - 1.0) - 2.0 * pad) - drawn_h) / 2.0;

    let to_pixel = |p: &StitchPoint| -> (i32, i32) {
        let px = ((p.x - min_x) as f64 * scale + offset_x).round() as i32;
        // Bitmap row 0 is top; design coordinates are Cartesian +Y up.
        let py = ((max_y - p.y) as f64 * scale + offset_y).round() as i32;
        (
            px.clamp(0, width as i32 - 1),
            py.clamp(0, height as i32 - 1),
        )
    };

    let mut pixels = vec![0u8; width * height];
    let mut prev: Option<(i32, i32)> = None;
    let mut thread_index = 0usize;
    let mut color_index = quantized_thread_index(design, thread_index);
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
                    draw_line(
                        &mut pixels,
                        width,
                        height,
                        prev_pt.0,
                        prev_pt.1,
                        pt.0,
                        pt.1,
                        color_index,
                    );
                } else {
                    set_pixel(&mut pixels, width, height, pt.0, pt.1, color_index);
                }
                prev = Some(pt);
            }
            StitchCommand::ColorChange | StitchCommand::Stop => {
                thread_index = (thread_index + 1).min(design.threads.len().saturating_sub(1));
                color_index = quantized_thread_index(design, thread_index);
                prev = None;
            }
            StitchCommand::Other(_) => {
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

pub fn render_preview_4bpp_auto(design: &Design) -> Result<(u8, u8, Vec<u8>)> {
    let drawable: Vec<&StitchPoint> = design
        .points
        .iter()
        .filter(|p| p.command.is_positioning_move())
        .collect();

    let (min_x, max_x, min_y, max_y) = if drawable.is_empty() {
        (0, 0, 0, 0)
    } else {
        (
            drawable.iter().map(|p| p.x).min().unwrap(),
            drawable.iter().map(|p| p.x).max().unwrap(),
            drawable.iter().map(|p| p.y).min().unwrap(),
            drawable.iter().map(|p| p.y).max().unwrap(),
        )
    };

    let span_x = (max_x - min_x).max(0);
    let span_y = (max_y - min_y).max(0);
    let width_px = (span_x + SHV_PREVIEW_UNITS_PER_PIXEL - 1) / SHV_PREVIEW_UNITS_PER_PIXEL + 1;
    let height_px = (span_y + SHV_PREVIEW_UNITS_PER_PIXEL - 1) / SHV_PREVIEW_UNITS_PER_PIXEL + 1;
    let width = width_px.clamp(1, u8::MAX as i32) as usize;
    let height = height_px.clamp(1, u8::MAX as i32) as usize;

    let to_pixel = |p: &StitchPoint| -> (i32, i32) {
        let px = (p.x - min_x) / SHV_PREVIEW_UNITS_PER_PIXEL;
        let py = (max_y - p.y) / SHV_PREVIEW_UNITS_PER_PIXEL;
        (
            px.clamp(0, width as i32 - 1),
            py.clamp(0, height as i32 - 1),
        )
    };

    let mut pixels = vec![0u8; width * height];
    let mut prev: Option<(i32, i32)> = None;
    let mut thread_index = 0usize;
    let mut color_index = quantized_thread_index(design, thread_index);
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
                    draw_line(
                        &mut pixels,
                        width,
                        height,
                        prev_pt.0,
                        prev_pt.1,
                        pt.0,
                        pt.1,
                        color_index,
                    );
                } else {
                    set_pixel(&mut pixels, width, height, pt.0, pt.1, color_index);
                }
                prev = Some(pt);
            }
            StitchCommand::ColorChange | StitchCommand::Stop => {
                thread_index = (thread_index + 1).min(design.threads.len().saturating_sub(1));
                color_index = quantized_thread_index(design, thread_index);
                prev = None;
            }
            StitchCommand::Other(_) => {
                prev = None;
            }
        }
    }

    let rotated = rotate_pixels_clockwise(&pixels, width, height);
    let rotated_width = height;
    let rotated_height = width;
    let stride = rotated_width.div_ceil(2);
    let mut out = Vec::with_capacity(rotated_height * stride);
    for y in 0..rotated_height {
        for x in (0..rotated_width).step_by(2) {
            let hi = rotated[y * rotated_width + x] & 0x0f;
            let lo = if x + 1 < rotated_width {
                rotated[y * rotated_width + x + 1] & 0x0f
            } else {
                0
            };
            out.push((hi << 4) | lo);
        }
    }

    Ok((rotated_width as u8, rotated_height as u8, out))
}

pub fn quantized_thread_index(design: &Design, index: usize) -> u8 {
    let Some(thread) = design.threads.get(index).or_else(|| design.threads.first()) else {
        return 1;
    };
    let Some(rgb) = parse_hex_rgb(thread.color.as_deref()) else {
        return 1;
    };
    nearest_palette_value(rgb)
}

fn parse_hex_rgb(value: Option<&str>) -> Option<[u8; 3]> {
    let mut s = value?.trim();
    if let Some(stripped) = s.strip_prefix('#') {
        s = stripped;
    }
    if s.len() != 6 {
        return None;
    }
    Some([
        u8::from_str_radix(&s[0..2], 16).ok()?,
        u8::from_str_radix(&s[2..4], 16).ok()?,
        u8::from_str_radix(&s[4..6], 16).ok()?,
    ])
}

fn nearest_palette_value(rgb: [u8; 3]) -> u8 {
    DESIGNER1_PALETTE
        .iter()
        .enumerate()
        .min_by_key(|(_, color)| {
            let dr = rgb[0] as i32 - color[0] as i32;
            let dg = rgb[1] as i32 - color[1] as i32;
            let db = rgb[2] as i32 - color[2] as i32;
            dr * dr + dg * dg + db * db
        })
        .map(|(idx, _)| idx as u8)
        .unwrap_or(1)
}

fn set_pixel(pixels: &mut [u8], width: usize, height: usize, x: i32, y: i32, value: u8) {
    if x >= 0 && y >= 0 && (x as usize) < width && (y as usize) < height {
        pixels[y as usize * width + x as usize] = value & 0x0f;
    }
}

fn draw_line(
    pixels: &mut [u8],
    width: usize,
    height: usize,
    mut x0: i32,
    mut y0: i32,
    x1: i32,
    y1: i32,
    value: u8,
) {
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

fn rotate_pixels_clockwise(src: &[u8], src_width: usize, src_height: usize) -> Vec<u8> {
    let mut out = vec![0u8; src.len()];
    let dest_width = src_height;
    for y in 0..src_height {
        for x in 0..src_width {
            let dest_x = src_height - 1 - y;
            let dest_y = x;
            out[dest_y * dest_width + dest_x] = src[y * src_width + x];
        }
    }
    out
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
