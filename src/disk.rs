use crate::inkstitch::{LoadOptions, load_inkstitch_json_file};
use crate::model::{Design, InputYAxis, SignatureMode};
use crate::preview::render_preview_4bpp;
use crate::shv::{OFFICIAL_NOTICE, ShvOptions, ZERO_NOTICE, build_shv, validate_generated_shv};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};

const MAX_MENU_DESIGNS: usize = 6;
const MENU_DIR: &str = "MENU_01";
const MENU_FILE: &str = "MENU_01.MHV";
const ROOT_MENU_FILE: &str = "MENU_SEL.PHV";
const PREVIEW_WIDTH: u8 = 96;
const PREVIEW_HEIGHT: u8 = 24;

const PHV_BITMAP_WIDTH: usize = 173;
const PHV_BITMAP_HEIGHT: usize = 181;
const PHV_BITMAP_OFFSET: usize = 0x04fb;
const MHV_BITMAP_WIDTH: usize = 244;
const MHV_BITMAP_HEIGHT: usize = 238;
const MHV_BITMAP_OFFSET: usize = 0x013f;
const MHV_SCREEN_WIDTH: usize = MHV_BITMAP_HEIGHT;
const MHV_SCREEN_HEIGHT: usize = MHV_BITMAP_WIDTH;
const MHV_GRID_COLS: usize = 3;
const MHV_GRID_ROWS: usize = 2;
const MHV_GRID_CELL_W: usize = MHV_SCREEN_WIDTH / MHV_GRID_COLS;
const MHV_GRID_CELL_H: usize = MHV_SCREEN_HEIGHT / MHV_GRID_ROWS;
const MHV_GRID_THUMB_W: usize = 72;
const MHV_GRID_THUMB_H: usize = 72;
const MHV_GRID_LINE_VALUE: u8 = 0x5;
const MHV_THUMB_VALUE: u8 = 0x0f;

#[derive(Debug, Clone)]
pub struct DiskExportOptions {
    pub signature: SignatureMode,
    pub scale: f64,
    pub center: bool,
    pub input_y_axis: InputYAxis,
    pub disk_title: String,
    pub menu_label: String,
}

impl Default for DiskExportOptions {
    fn default() -> Self {
        Self {
            signature: SignatureMode::Official,
            scale: 1.0,
            center: true,
            input_y_axis: InputYAxis::Down,
            disk_title: "Designer 1 Disk".to_owned(),
            menu_label: "Menu 1".to_owned(),
        }
    }
}

impl DiskExportOptions {
    fn load_options(&self) -> LoadOptions {
        LoadOptions {
            scale: self.scale,
            center: self.center,
            input_y_axis: self.input_y_axis,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DiskDesignInput {
    pub slot: u8,
    pub source: PathBuf,
    pub label: String,
    #[serde(skip)]
    pub design: Design,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiskWrittenDesign {
    pub slot: u8,
    pub source: PathBuf,
    pub output: PathBuf,
    pub label: String,
    pub records: u32,
    pub colors: u8,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiskExportReport {
    pub root: PathBuf,
    pub designs: Vec<DiskWrittenDesign>,
    pub written_files: Vec<PathBuf>,
    pub warnings: Vec<String>,
}

pub fn load_disk_designs(
    root_dir: impl AsRef<Path>,
    options: &DiskExportOptions,
) -> Result<Vec<DiskDesignInput>> {
    let root_dir = root_dir.as_ref();
    let paths = discover_json_files(root_dir)?;
    if paths.is_empty() {
        bail!("no JSON files found in {}", root_dir.display());
    }
    if paths.len() > MAX_MENU_DESIGNS {
        bail!(
            "single-menu disk export supports at most {MAX_MENU_DESIGNS} JSON files; found {}",
            paths.len()
        );
    }

    let mut designs = Vec::with_capacity(paths.len());
    for (idx, path) in paths.into_iter().enumerate() {
        let design = load_inkstitch_json_file(&path, &options.load_options())?;
        designs.push(DiskDesignInput {
            slot: (idx + 1) as u8,
            source: path,
            label: menu_label_for_design(&design),
            design,
        });
    }
    Ok(designs)
}

pub fn export_single_menu_disk(
    root_dir: impl AsRef<Path>,
    options: &DiskExportOptions,
) -> Result<DiskExportReport> {
    let root_dir = root_dir.as_ref();
    let designs = load_disk_designs(root_dir, options)?;
    let menu_dir = root_dir.join(MENU_DIR);
    fs::create_dir_all(&menu_dir)
        .with_context(|| format!("creating menu directory {}", menu_dir.display()))?;

    remove_generated_shvs(&menu_dir)?;
    remove_generated_menu_files(&menu_dir)?;

    let mut written_files = Vec::new();
    let mut written_designs = Vec::new();
    for input in &designs {
        let output = menu_dir.join(format!("DES01_{:02}.SHV", input.slot));
        let shv = build_shv(
            &input.design,
            &ShvOptions {
                name: Some(input.label.clone()),
                signature: options.signature,
                preview_width: PREVIEW_WIDTH,
                preview_height: PREVIEW_HEIGHT,
            },
        )?;
        let report = validate_generated_shv(&shv)?;
        fs::write(&output, shv).with_context(|| format!("writing {}", output.display()))?;
        written_files.push(output.clone());
        written_designs.push(DiskWrittenDesign {
            slot: input.slot,
            source: input.source.clone(),
            output,
            label: input.label.clone(),
            records: report.total_records,
            colors: report.color_count,
        });
    }

    let mhv_path = menu_dir.join(MENU_FILE);
    fs::write(&mhv_path, build_mhv(options, &designs)?)
        .with_context(|| format!("writing {}", mhv_path.display()))?;
    written_files.push(mhv_path);

    let phv_path = root_dir.join(ROOT_MENU_FILE);
    fs::write(&phv_path, build_phv(options)?)
        .with_context(|| format!("writing {}", phv_path.display()))?;
    written_files.push(phv_path);

    Ok(DiskExportReport {
        root: root_dir.to_path_buf(),
        designs: written_designs,
        written_files,
        warnings: Vec::new(),
    })
}

fn discover_json_files(root_dir: &Path) -> Result<Vec<PathBuf>> {
    if !root_dir.is_dir() {
        bail!("{} is not a directory", root_dir.display());
    }
    let mut paths = Vec::new();
    for entry in
        fs::read_dir(root_dir).with_context(|| format!("reading {}", root_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_file()
            && path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        {
            paths.push(path);
        }
    }
    paths.sort_by(compare_paths_case_insensitive);
    Ok(paths)
}

fn compare_paths_case_insensitive(a: &PathBuf, b: &PathBuf) -> Ordering {
    let a_name = a
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let b_name = b
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    a_name
        .cmp(&b_name)
        .then_with(|| a.file_name().cmp(&b.file_name()))
}

fn remove_generated_shvs(menu_dir: &Path) -> Result<()> {
    for slot in 1..=MAX_MENU_DESIGNS {
        let path = menu_dir.join(format!("DES01_{slot:02}.SHV"));
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err).with_context(|| format!("removing {}", path.display())),
        }
    }
    Ok(())
}

fn remove_generated_menu_files(menu_dir: &Path) -> Result<()> {
    for name in [MENU_FILE, "Menu_01.mhv", "menu_01.mhv"] {
        let path = menu_dir.join(name);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err).with_context(|| format!("removing {}", path.display())),
        }
    }
    Ok(())
}

fn menu_label_for_design(design: &Design) -> String {
    let label = design.name.trim();
    if label.is_empty() {
        "DESIGN".to_owned()
    } else {
        label.to_owned()
    }
}

fn build_mhv(options: &DiskExportOptions, designs: &[DiskDesignInput]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(signature_bytes(options.signature));
    let menu_labels = vec![options.menu_label.clone(); 16];
    for label in padded_label_slots(&menu_labels, 12, 16) {
        out.extend_from_slice(&label);
    }
    out.extend_from_slice(&[0x77, 0xfb, 0x06]);
    for idx in 0..36 {
        out.push(if idx < designs.len() {
            (idx + 1) as u8
        } else {
            0
        });
    }
    debug_assert_eq!(out.len(), MHV_BITMAP_OFFSET - 2);
    out.push(MHV_BITMAP_HEIGHT as u8);
    out.push(MHV_BITMAP_WIDTH as u8);
    out.extend_from_slice(&render_mhv_bitmap(designs)?);
    Ok(out)
}

fn build_phv(options: &DiskExportOptions) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(signature_bytes(options.signature));
    out.extend_from_slice(&[0x00, 0x01, 0x00, 0x00]);

    let disk_titles = vec![options.disk_title.clone(); 16];
    for title in padded_label_slots(&disk_titles, 24, 16) {
        out.extend_from_slice(&title);
    }

    out.extend_from_slice(&[0x5a, 0xa0]);
    let menu_meta = [[0x39, 0x82], [0x39, 0x5c], [0x39, 0x36], [0x39, 0x10]];
    for (idx, meta) in menu_meta.iter().enumerate() {
        out.extend_from_slice(meta);
        let labels = if idx == 0 {
            vec![options.menu_label.clone(); 16]
        } else {
            vec![String::new(); 16]
        };
        for label in padded_label_slots(&labels, 12, 16) {
            out.extend_from_slice(&label);
        }
    }
    out.extend_from_slice(&[
        0x00, 0x00, 120, 0x00, 0x00, 0x00, 0x00, 82, 0x00, 0x00, 0x00, 0x00, 44, 0x00, 0x00, 0x00,
        0x00, 6, 0x00, 0x00,
    ]);
    out.push(0x00);
    debug_assert_eq!(out.len(), PHV_BITMAP_OFFSET - 2);
    out.push(PHV_BITMAP_HEIGHT as u8);
    out.push(PHV_BITMAP_WIDTH as u8);
    out.extend_from_slice(&render_text_bitmap(
        PHV_BITMAP_WIDTH,
        PHV_BITMAP_HEIGHT,
        std::slice::from_ref(&options.menu_label),
        12,
        28,
    ));
    Ok(out)
}

fn signature_bytes(signature: SignatureMode) -> &'static [u8; 86] {
    match signature {
        SignatureMode::Official => OFFICIAL_NOTICE,
        SignatureMode::Zero => ZERO_NOTICE,
    }
}

fn padded_label_slots(labels: &[String], width: usize, count: usize) -> Vec<Vec<u8>> {
    (0..count)
        .map(|idx| encode_label(labels.get(idx).map(String::as_str).unwrap_or(""), width))
        .collect()
}

pub fn encode_label(label: &str, width: usize) -> Vec<u8> {
    let mut out = vec![0u8; width];
    for (idx, byte) in label
        .trim()
        .bytes()
        .map(|b| {
            if b.is_ascii_graphic() || b == b' ' {
                b
            } else {
                b'_'
            }
        })
        .take(width)
        .enumerate()
    {
        out[idx] = byte;
    }
    out
}

fn render_text_bitmap(
    width: usize,
    height: usize,
    labels: &[String],
    left: usize,
    top: usize,
) -> Vec<u8> {
    let mut pixels = vec![0u8; width * height];
    for (idx, label) in labels.iter().enumerate() {
        let y = top + idx * 13;
        if y + 7 >= height {
            break;
        }
        draw_text(&mut pixels, width, height, left, y, label, 0x7);
    }
    pack_4bpp(&pixels, width, height)
}

fn render_mhv_bitmap(designs: &[DiskDesignInput]) -> Result<Vec<u8>> {
    let mut pixels = vec![0u8; MHV_SCREEN_WIDTH * MHV_SCREEN_HEIGHT];
    draw_mhv_grid(&mut pixels);

    for (idx, design) in designs.iter().take(MAX_MENU_DESIGNS).enumerate() {
        let col = idx % MHV_GRID_COLS;
        let row = idx / MHV_GRID_COLS;
        let cell_x = col * MHV_GRID_CELL_W;
        let cell_y = row * MHV_GRID_CELL_H;
        let thumb = render_preview_4bpp(
            &design.design,
            MHV_GRID_THUMB_W as u8,
            MHV_GRID_THUMB_H as u8,
            MHV_THUMB_VALUE,
        )?;
        blit_thumb_4bpp(
            &mut pixels,
            cell_x + (MHV_GRID_CELL_W - MHV_GRID_THUMB_W) / 2,
            cell_y + (MHV_GRID_CELL_H - MHV_GRID_THUMB_H) / 2,
            MHV_GRID_THUMB_W,
            MHV_GRID_THUMB_H,
            &thumb,
        );
        let label = design.slot.to_string();
        draw_text(
            &mut pixels,
            MHV_SCREEN_WIDTH,
            MHV_SCREEN_HEIGHT,
            cell_x + 6,
            cell_y + MHV_GRID_CELL_H - 14,
            &label,
            0x7,
        );
    }

    let rotated = rotate_clockwise(&pixels, MHV_SCREEN_WIDTH, MHV_SCREEN_HEIGHT);
    Ok(pack_4bpp(&rotated, MHV_BITMAP_WIDTH, MHV_BITMAP_HEIGHT))
}

fn draw_mhv_grid(pixels: &mut [u8]) {
    for row in 1..MHV_GRID_ROWS {
        let y = row * MHV_GRID_CELL_H;
        for x in 0..MHV_SCREEN_WIDTH {
            pixels[y * MHV_SCREEN_WIDTH + x] = MHV_GRID_LINE_VALUE;
        }
    }
    for col in 1..MHV_GRID_COLS {
        let x = col * MHV_GRID_CELL_W;
        for y in 0..MHV_SCREEN_HEIGHT {
            pixels[y * MHV_SCREEN_WIDTH + x] = MHV_GRID_LINE_VALUE;
        }
    }
}

fn blit_thumb_4bpp(
    dest: &mut [u8],
    dst_x: usize,
    dst_y: usize,
    width: usize,
    height: usize,
    packed: &[u8],
) {
    let stride = width.div_ceil(2);
    for y in 0..height {
        for x in 0..width {
            let byte = packed[y * stride + x / 2];
            let value = if x % 2 == 0 { byte >> 4 } else { byte & 0x0f };
            if value == 0 {
                continue;
            }
            let px = dst_x + x;
            let py = dst_y + y;
            if px < MHV_SCREEN_WIDTH && py < MHV_SCREEN_HEIGHT {
                dest[py * MHV_SCREEN_WIDTH + px] = value;
            }
        }
    }
}

fn rotate_clockwise(pixels: &[u8], width: usize, height: usize) -> Vec<u8> {
    let mut out = vec![0u8; pixels.len()];
    for y in 0..height {
        for x in 0..width {
            let dst_x = height - 1 - y;
            let dst_y = x;
            out[dst_y * height + dst_x] = pixels[y * width + x];
        }
    }
    out
}

fn draw_text(
    pixels: &mut [u8],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    text: &str,
    value: u8,
) {
    let mut cursor = x;
    for ch in text.chars().take(20) {
        draw_char(pixels, width, height, cursor, y, ch, value);
        cursor += 6;
        if cursor + 5 >= width {
            break;
        }
    }
}

fn draw_char(
    pixels: &mut [u8],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    ch: char,
    value: u8,
) {
    for (row, bits) in glyph(ch).iter().enumerate() {
        for col in 0..5 {
            if bits & (1 << (4 - col)) != 0 {
                let px = x + col;
                let py = y + row;
                if px < width && py < height {
                    pixels[py * width + px] = value;
                }
            }
        }
    }
}

fn pack_4bpp(pixels: &[u8], width: usize, height: usize) -> Vec<u8> {
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
    out
}

fn glyph(ch: char) -> [u8; 7] {
    match ch.to_ascii_uppercase() {
        'A' => [0x0e, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11],
        'B' => [0x1e, 0x11, 0x11, 0x1e, 0x11, 0x11, 0x1e],
        'C' => [0x0e, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0e],
        'D' => [0x1e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1e],
        'E' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x1f],
        'F' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x10],
        'G' => [0x0e, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0f],
        'H' => [0x11, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11],
        'I' => [0x0e, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0e],
        'J' => [0x01, 0x01, 0x01, 0x01, 0x11, 0x11, 0x0e],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1f],
        'M' => [0x11, 0x1b, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11],
        'O' => [0x0e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e],
        'P' => [0x1e, 0x11, 0x11, 0x1e, 0x10, 0x10, 0x10],
        'Q' => [0x0e, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0d],
        'R' => [0x1e, 0x11, 0x11, 0x1e, 0x14, 0x12, 0x11],
        'S' => [0x0f, 0x10, 0x10, 0x0e, 0x01, 0x01, 0x1e],
        'T' => [0x1f, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0a, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0a],
        'X' => [0x11, 0x11, 0x0a, 0x04, 0x0a, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x0a, 0x04, 0x04, 0x04, 0x04],
        'Z' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1f],
        '0' => [0x0e, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0e],
        '1' => [0x04, 0x0c, 0x04, 0x04, 0x04, 0x04, 0x0e],
        '2' => [0x0e, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1f],
        '3' => [0x1e, 0x01, 0x01, 0x0e, 0x01, 0x01, 0x1e],
        '4' => [0x02, 0x06, 0x0a, 0x12, 0x1f, 0x02, 0x02],
        '5' => [0x1f, 0x10, 0x10, 0x1e, 0x01, 0x01, 0x1e],
        '6' => [0x0e, 0x10, 0x10, 0x1e, 0x11, 0x11, 0x0e],
        '7' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0e, 0x11, 0x11, 0x0e, 0x11, 0x11, 0x0e],
        '9' => [0x0e, 0x11, 0x11, 0x0f, 0x01, 0x01, 0x0e],
        '-' => [0x00, 0x00, 0x00, 0x1f, 0x00, 0x00, 0x00],
        '_' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1f],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x0c, 0x0c],
        ' ' => [0; 7],
        _ => [0x1f, 0x11, 0x02, 0x04, 0x04, 0x00, 0x04],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn label_encoding_pads_and_truncates() {
        assert_eq!(encode_label("ABC", 6), b"ABC\0\0\0");
        assert_eq!(encode_label("ABCDEFGHI", 4), b"ABCD");
    }

    #[test]
    fn generated_menu_lengths_match_documented_offsets() {
        let options = DiskExportOptions::default();
        let mhv = build_mhv(
            &options,
            &[sample_disk_design(1, "One"), sample_disk_design(2, "Two")],
        )
        .unwrap();
        assert_eq!(
            mhv.len(),
            MHV_BITMAP_OFFSET + MHV_BITMAP_HEIGHT * MHV_BITMAP_WIDTH.div_ceil(2)
        );
        assert_eq!(mhv[MHV_BITMAP_OFFSET - 2], MHV_BITMAP_HEIGHT as u8);
        assert_eq!(mhv[MHV_BITMAP_OFFSET - 1], MHV_BITMAP_WIDTH as u8);
        assert_eq!(&mhv[0x0116..0x0119], &[0x77, 0xfb, 0x06]);
        assert_eq!(&mhv[0x0119..0x011d], &[0x01, 0x02, 0x00, 0x00]);

        let phv = build_phv(&options).unwrap();
        assert_eq!(
            phv.len(),
            PHV_BITMAP_OFFSET + PHV_BITMAP_HEIGHT * PHV_BITMAP_WIDTH.div_ceil(2)
        );
        assert_eq!(phv[PHV_BITMAP_OFFSET - 2], PHV_BITMAP_HEIGHT as u8);
        assert_eq!(phv[PHV_BITMAP_OFFSET - 1], PHV_BITMAP_WIDTH as u8);
    }

    #[test]
    fn export_rejects_empty_folder() {
        let dir = temp_test_dir("empty");
        fs::create_dir_all(&dir).unwrap();
        let err = export_single_menu_disk(&dir, &DiskExportOptions::default()).unwrap_err();
        assert!(err.to_string().contains("no JSON files"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn export_rejects_more_than_sixteen_json_files() {
        let dir = temp_test_dir("too_many");
        fs::create_dir_all(&dir).unwrap();
        for idx in 0..7 {
            fs::write(dir.join(format!("{idx:02}.json")), "{}").unwrap();
        }
        let err = export_single_menu_disk(&dir, &DiskExportOptions::default()).unwrap_err();
        assert!(err.to_string().contains("at most 6"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn export_writes_single_menu_disk() {
        let dir = temp_test_dir("export");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("b.json"), sample_json("Beta")).unwrap();
        fs::write(dir.join("A.json"), sample_json("Alpha")).unwrap();

        let report = export_single_menu_disk(&dir, &DiskExportOptions::default()).unwrap();
        assert_eq!(report.designs.len(), 2);
        assert!(dir.join(ROOT_MENU_FILE).is_file());
        assert!(dir.join(MENU_DIR).join(MENU_FILE).is_file());
        assert!(dir.join(MENU_DIR).join("DES01_01.SHV").is_file());
        assert!(dir.join(MENU_DIR).join("DES01_02.SHV").is_file());
        assert_eq!(report.designs[0].label, "Alpha");
        assert_eq!(report.designs[1].label, "Beta");

        for design in report.designs {
            let bytes = fs::read(design.output).unwrap();
            validate_generated_shv(&bytes).unwrap();
        }
        fs::remove_dir_all(dir).unwrap();
    }

    fn temp_test_dir(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("designer1_disk_{name}_{nonce}"))
    }

    fn sample_json(name: &str) -> String {
        format!(
            r##"{{
  "threadlist": [{{ "color": "#df5bd7" }}],
  "extras": {{ "name": "{name}" }},
  "stitches": [
    [0.0, 0.0, "JUMP"],
    [0.0, 0.0, "STITCH"],
    [20.0, 0.0, "STITCH"],
    [20.0, 20.0, "STITCH"],
    [0.0, 20.0, "STITCH"],
    [0.0, 0.0, "STITCH"],
    [0.0, 0.0, "END"]
  ]
}}"##
        )
    }

    fn sample_disk_design(slot: u8, name: &str) -> DiskDesignInput {
        DiskDesignInput {
            slot,
            source: PathBuf::from(format!("{name}.json")),
            label: name.to_owned(),
            design: Design {
                name: name.to_owned(),
                threads: vec![],
                points: vec![
                    crate::model::StitchPoint {
                        x: 0,
                        y: 0,
                        command: crate::model::StitchCommand::Jump,
                    },
                    crate::model::StitchPoint {
                        x: 0,
                        y: 0,
                        command: crate::model::StitchCommand::Stitch,
                    },
                    crate::model::StitchPoint {
                        x: 30,
                        y: 0,
                        command: crate::model::StitchCommand::Stitch,
                    },
                    crate::model::StitchPoint {
                        x: 30,
                        y: 30,
                        command: crate::model::StitchCommand::Stitch,
                    },
                    crate::model::StitchPoint {
                        x: 0,
                        y: 30,
                        command: crate::model::StitchCommand::Stitch,
                    },
                    crate::model::StitchPoint {
                        x: 0,
                        y: 0,
                        command: crate::model::StitchCommand::End,
                    },
                ],
            },
        }
    }
}
