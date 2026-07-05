use crate::model::{Design, SignatureMode, StitchCommand, Thread};
use crate::preview::render_preview_4bpp_auto;
use anyhow::{Result, bail};
use serde::Serialize;
use std::collections::BTreeMap;

pub const OFFICIAL_NOTICE: &[u8; 86] =
    b"Embroidery disk created using software licensed from Viking Sewing Machines AB, Sweden";
pub const ZERO_NOTICE: &[u8; 86] = &[0u8; 86];
pub const SUMMARY_CONSTANTS: [u8; 6] = [0xc4, 0x28, 0x00, 0x30, 0x00, 0x00];
pub const DEFAULT_BLACK_COLOR_INDEX: u8 = 7;
pub const DEFAULT_OTHER_COLOR_INDEX: u8 = 0;

#[derive(Debug, Clone)]
pub struct ShvOptions {
    pub name: Option<String>,
    pub signature: SignatureMode,
}

impl Default for ShvOptions {
    fn default() -> Self {
        Self {
            name: None,
            signature: SignatureMode::Official,
        }
    }
}

#[derive(Debug, Clone)]
struct Segment {
    color_index: u8,
    start_x_raw: i32,
    start_y_raw: i32,
    records: Vec<u8>,
}

impl Segment {
    fn record_count(&self) -> usize {
        debug_assert_eq!(self.records.len() % 2, 0);
        self.records.len() / 2
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ShvColorRowReport {
    pub records: u32,
    pub color_index: u8,
    pub start_x_raw: i16,
    pub start_y_raw: i16,
    pub raw_hex: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShvReadbackReport {
    pub name: String,
    pub preview_width: u8,
    pub preview_height: u8,
    pub summary_offset: usize,
    pub color_count: u8,
    pub summary_extents: Extents,
    pub total_records: u32,
    pub color_rows: Vec<ShvColorRowReport>,
    pub stitch_offset: usize,
    pub stitch_bytes: usize,
    pub record_count_from_bytes: usize,
    pub parsed_event_count: usize,
    pub command_counts: BTreeMap<String, usize>,
    pub computed_extents: Option<Extents>,
    pub final_position: Point,
    pub preview_offset: usize,
    pub preview_length: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Extents {
    pub right: i16,
    pub top: i16,
    pub left: i16,
    pub bottom: i16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Point {
    pub x: i16,
    pub y: i16,
}

#[derive(Debug, Clone)]
struct Event {
    x_cart: i32,
    y_cart: i32,
}

pub fn build_shv(design: &Design, options: &ShvOptions) -> Result<Vec<u8>> {
    let name = options.name.as_deref().unwrap_or(&design.name);
    let name_bytes = safe_ascii_name(name, 32)?;
    let (segments, event_positions_cart) = build_segments(design)?;
    if segments.len() > u8::MAX as usize {
        bail!("too many color segments: {}", segments.len());
    }

    let stitch_stream: Vec<u8> = segments
        .iter()
        .flat_map(|s| s.records.iter().copied())
        .collect();
    let total_records = stitch_stream.len() / 2;
    if total_records != segments.iter().map(|s| s.record_count()).sum::<usize>() {
        bail!("internal SHV record count mismatch");
    }
    if total_records > u32::MAX as usize {
        bail!("too many SHV records");
    }

    let extents = extents_from_positions(&event_positions_cart)?;

    let prefix: &[u8] = match options.signature {
        SignatureMode::Official => OFFICIAL_NOTICE,
        SignatureMode::Zero => ZERO_NOTICE,
    };

    let (preview_width, preview_height, preview) = render_preview_4bpp_auto(design)?;
    let preview_header = [
        preview_height,
        preview_width,
        preview_height / 2,
        preview_width / 2,
        preview_height / 2,
        preview_width / 2,
    ];

    let mut out = Vec::new();
    out.extend_from_slice(prefix);
    out.push(name_bytes.len() as u8);
    out.extend_from_slice(&name_bytes);
    out.extend_from_slice(&preview_header);
    out.extend_from_slice(&preview);

    out.push(segments.len() as u8);
    out.extend_from_slice(&SUMMARY_CONSTANTS);
    out.extend_from_slice(&i16be(extents.right as i32)?);
    out.extend_from_slice(&i16be(extents.top as i32)?);
    out.extend_from_slice(&i16be(extents.left as i32)?);
    out.extend_from_slice(&i16be(extents.bottom as i32)?);
    out.extend_from_slice(&u32be(total_records as u32));

    for segment in &segments {
        out.extend_from_slice(&u32be(segment.record_count() as u32));
        out.push(segment.color_index);
        out.extend_from_slice(&[0x00, 0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&i16be(segment.start_x_raw)?);
        out.extend_from_slice(&i16be(segment.start_y_raw)?);
    }

    out.extend_from_slice(&stitch_stream);
    Ok(out)
}

fn build_segments(design: &Design) -> Result<(Vec<Segment>, Vec<(i32, i32)>)> {
    let mut thread_index = 0usize;
    let mut x_raw = 0i32;
    let mut y_raw = 0i32;
    let mut segments = vec![new_segment(design, thread_index, x_raw, y_raw)];
    let mut event_positions_cart = vec![(0i32, 0i32)];

    for p in &design.points {
        let target_x_raw = p.x;
        let target_y_raw = -p.y; // SHV raw stream is +Y down; Design is Cartesian +Y up.
        let dx_raw = target_x_raw - x_raw;
        let dy_raw = target_y_raw - y_raw;

        match &p.command {
            StitchCommand::End => break,
            StitchCommand::ColorChange | StitchCommand::Stop => {
                if segments.last().map(|s| s.record_count()).unwrap_or(0) > 0 {
                    thread_index = (thread_index + 1).min(design.threads.len().saturating_sub(1));
                    segments.push(new_segment(design, thread_index, x_raw, y_raw));
                }
            }
            StitchCommand::Jump | StitchCommand::Trim => {
                add_jump16(&mut segments.last_mut().unwrap().records, dx_raw, dy_raw)?;
                x_raw = target_x_raw;
                y_raw = target_y_raw;
                event_positions_cart.push((x_raw, -y_raw));
            }
            StitchCommand::Stitch => {
                for (sx, sy) in split_stitch_delta(dx_raw, dy_raw) {
                    add_stitch_pair(&mut segments.last_mut().unwrap().records, sx, sy)?;
                    x_raw += sx;
                    y_raw += sy;
                    event_positions_cart.push((x_raw, -y_raw));
                }
            }
            StitchCommand::Other(_) => {}
        }
    }

    // Observed SHV samples return to origin with needle lifted.
    if x_raw != 0 || y_raw != 0 {
        add_jump16(&mut segments.last_mut().unwrap().records, -x_raw, -y_raw)?;
        event_positions_cart.push((0, 0));
    }

    segments.retain(|s| s.record_count() > 0);
    if segments.is_empty() {
        bail!("no stitch records were generated");
    }
    Ok((segments, event_positions_cart))
}

fn new_segment(design: &Design, thread_index: usize, x_raw: i32, y_raw: i32) -> Segment {
    let thread = design
        .threads
        .get(thread_index)
        .or_else(|| design.threads.first());
    Segment {
        color_index: thread
            .map(thread_to_color_index)
            .unwrap_or(DEFAULT_BLACK_COLOR_INDEX),
        start_x_raw: x_raw,
        start_y_raw: y_raw,
        records: Vec::new(),
    }
}

fn thread_to_color_index(thread: &Thread) -> u8 {
    if let Some((r, g, b)) = parse_hex_color(thread.color.as_deref()) {
        if r <= 32 && g <= 32 && b <= 32 {
            return DEFAULT_BLACK_COLOR_INDEX;
        }
    }
    DEFAULT_OTHER_COLOR_INDEX
}

fn parse_hex_color(value: Option<&str>) -> Option<(u8, u8, u8)> {
    let mut s = value?.trim();
    if let Some(stripped) = s.strip_prefix('#') {
        s = stripped;
    }
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some((r, g, b))
}

fn safe_ascii_name(name: &str, max_len: usize) -> Result<Vec<u8>> {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_graphic() || ch == ' ' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = if cleaned.is_empty() {
        "design".to_owned()
    } else {
        cleaned
    };
    let mut bytes = cleaned.into_bytes();
    bytes.truncate(max_len.min(255));
    if bytes.is_empty() {
        bail!("empty SHV name");
    }
    Ok(bytes)
}

fn add_jump16(records: &mut Vec<u8>, dx_raw: i32, dy_raw: i32) -> Result<()> {
    records.extend_from_slice(&[0x80, 0x01]);
    records.extend_from_slice(&i16be(dx_raw)?);
    records.extend_from_slice(&i16be(dy_raw)?);
    records.extend_from_slice(&[0x80, 0x02]);
    Ok(())
}

fn add_stitch_pair(records: &mut Vec<u8>, dx_raw: i32, dy_raw: i32) -> Result<()> {
    records.push(s8_byte(dx_raw)?);
    records.push(s8_byte(dy_raw)?);
    Ok(())
}

fn split_stitch_delta(dx: i32, dy: i32) -> Vec<(i32, i32)> {
    let max_abs = dx.abs().max(dy.abs());
    let mut n = ((max_abs as f64) / 127.0).ceil().max(1.0) as i32;
    loop {
        let mut pieces = Vec::new();
        let mut px = 0i32;
        let mut py = 0i32;
        let mut ok = true;
        for i in 1..=n {
            let tx = ((dx as f64) * (i as f64) / (n as f64)).round() as i32;
            let ty = ((dy as f64) * (i as f64) / (n as f64)).round() as i32;
            let ddx = tx - px;
            let ddy = ty - py;
            if !is_legal_stitch_delta(ddx) || !is_legal_stitch_delta(ddy) {
                ok = false;
                break;
            }
            pieces.push((ddx, ddy));
            px = tx;
            py = ty;
        }
        if ok {
            return pieces;
        }
        n += 1;
    }
}

fn is_legal_stitch_delta(v: i32) -> bool {
    (-127..=127).contains(&v)
}

fn s8_byte(v: i32) -> Result<u8> {
    if !is_legal_stitch_delta(v) {
        bail!("SHV 8-bit stitch delta out of range/reserved: {v}");
    }
    Ok((v as i8) as u8)
}

fn i16be(v: i32) -> Result<[u8; 2]> {
    if v < i16::MIN as i32 || v > i16::MAX as i32 {
        bail!("SHV int16 value out of range: {v}");
    }
    Ok((v as i16).to_be_bytes())
}

fn u32be(v: u32) -> [u8; 4] {
    v.to_be_bytes()
}

fn extents_from_positions(positions: &[(i32, i32)]) -> Result<Extents> {
    if positions.is_empty() {
        return Ok(Extents {
            right: 0,
            top: 0,
            left: 0,
            bottom: 0,
        });
    }
    let right = positions.iter().map(|p| p.0).max().unwrap();
    let left = positions.iter().map(|p| p.0).min().unwrap();
    let top = positions.iter().map(|p| p.1).max().unwrap();
    let bottom = positions.iter().map(|p| p.1).min().unwrap();
    Ok(Extents {
        right: checked_i16(right)?,
        top: checked_i16(top)?,
        left: checked_i16(left)?,
        bottom: checked_i16(bottom)?,
    })
}

fn checked_i16(v: i32) -> Result<i16> {
    if v < i16::MIN as i32 || v > i16::MAX as i32 {
        bail!("SHV coordinate out of int16 range: {v}");
    }
    Ok(v as i16)
}

pub fn parse_generated_shv(blob: &[u8]) -> Result<ShvReadbackReport> {
    if blob.len() < 0x57 {
        bail!("SHV file too small");
    }

    let name_len = blob[0x56] as usize;
    let name_start = 0x57;
    let name_end = name_start + name_len;
    if name_end + 6 > blob.len() {
        bail!("SHV name or preview header is truncated");
    }
    let name = String::from_utf8_lossy(&blob[name_start..name_end]).to_string();
    let preview_header_off = name_end;
    let preview_height = blob[preview_header_off];
    let preview_width = blob[preview_header_off + 1];
    let stride = (preview_width as usize).div_ceil(2);
    let preview_len = preview_height as usize * stride;
    let preview_offset = preview_header_off + 6;
    let summary_offset = preview_offset + preview_len;
    if summary_offset + 19 > blob.len() {
        bail!("SHV summary is truncated");
    }

    let color_count = blob[summary_offset];
    let summary_extents = Extents {
        right: read_i16(blob, summary_offset + 7)?,
        top: read_i16(blob, summary_offset + 9)?,
        left: read_i16(blob, summary_offset + 11)?,
        bottom: read_i16(blob, summary_offset + 13)?,
    };
    let total_records = read_u32(blob, summary_offset + 15)?;

    let rows_offset = summary_offset + 19;
    let mut color_rows = Vec::new();
    for i in 0..color_count as usize {
        let off = rows_offset + i * 14;
        if off + 14 > blob.len() {
            bail!("SHV color table is truncated");
        }
        color_rows.push(ShvColorRowReport {
            records: read_u32(blob, off)?,
            color_index: blob[off + 4],
            start_x_raw: read_i16(blob, off + 10)?,
            start_y_raw: read_i16(blob, off + 12)?,
            raw_hex: hex_bytes(&blob[off..off + 14]),
        });
    }

    let stitch_offset = rows_offset + color_count as usize * 14;
    if stitch_offset > blob.len() {
        bail!("SHV stitch stream offset beyond EOF");
    }
    let stitch_stream = &blob[stitch_offset..];
    if stitch_stream.len() % 2 != 0 {
        bail!("SHV stitch stream has odd length");
    }

    let records: Vec<&[u8]> = stitch_stream.chunks_exact(2).collect();
    let mut command_counts = BTreeMap::new();
    let mut events = Vec::new();
    let mut i = 0usize;
    let mut x_raw = 0i32;
    let mut y_raw = 0i32;

    while i < records.len() {
        let a = records[i][0];
        let b = records[i][1];
        if a == 0x80 {
            *command_counts.entry(format!("80 {b:02x}")).or_insert(0) += 1;
            if b == 0x01 && i + 3 < records.len() && records[i + 3] == [0x80, 0x02] {
                let dx = i16::from_be_bytes([records[i + 1][0], records[i + 1][1]]) as i32;
                let dy = i16::from_be_bytes([records[i + 2][0], records[i + 2][1]]) as i32;
                x_raw += dx;
                y_raw += dy;
                events.push(Event {
                    x_cart: x_raw,
                    y_cart: -y_raw,
                });
                i += 4;
                continue;
            }
            events.push(Event {
                x_cart: x_raw,
                y_cart: -y_raw,
            });
            i += 1;
            continue;
        }

        let dx = a as i8 as i32;
        let dy = b as i8 as i32;
        x_raw += dx;
        y_raw += dy;
        events.push(Event {
            x_cart: x_raw,
            y_cart: -y_raw,
        });
        i += 1;
    }

    let computed_extents = if events.is_empty() {
        None
    } else {
        let positions: Vec<(i32, i32)> = events.iter().map(|e| (e.x_cart, e.y_cart)).collect();
        Some(extents_from_positions(&positions)?)
    };

    Ok(ShvReadbackReport {
        name,
        preview_width,
        preview_height,
        summary_offset,
        color_count,
        summary_extents,
        total_records,
        color_rows,
        stitch_offset,
        stitch_bytes: stitch_stream.len(),
        record_count_from_bytes: records.len(),
        parsed_event_count: events.len(),
        command_counts,
        computed_extents,
        final_position: Point {
            x: checked_i16(x_raw)?,
            y: checked_i16(-y_raw)?,
        },
        preview_offset,
        preview_length: preview_len,
    })
}

pub fn validate_generated_shv(blob: &[u8]) -> Result<ShvReadbackReport> {
    let report = parse_generated_shv(blob)?;
    if report.total_records as usize != report.record_count_from_bytes {
        bail!(
            "record count mismatch: summary {} vs bytes {}",
            report.total_records,
            report.record_count_from_bytes
        );
    }
    if Some(report.summary_extents) != report.computed_extents {
        bail!(
            "extent mismatch: summary {:?} vs computed {:?}",
            report.summary_extents,
            report.computed_extents
        );
    }
    if report.final_position != (Point { x: 0, y: 0 }) {
        bail!("final position is not origin: {:?}", report.final_position);
    }
    Ok(report)
}

fn read_i16(blob: &[u8], off: usize) -> Result<i16> {
    if off + 2 > blob.len() {
        bail!("int16 at {off:#x} is truncated");
    }
    Ok(i16::from_be_bytes([blob[off], blob[off + 1]]))
}

fn read_u32(blob: &[u8], off: usize) -> Result<u32> {
    if off + 4 > blob.len() {
        bail!("uint32 at {off:#x} is truncated");
    }
    Ok(u32::from_be_bytes([
        blob[off],
        blob[off + 1],
        blob[off + 2],
        blob[off + 3],
    ]))
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn stitch_points_from_shv(blob: &[u8]) -> Result<Vec<(i32, i32, bool)>> {
    // Helper for external visualizers. bool=true means needle-down stitch, false means jump/command position.
    let report = parse_generated_shv(blob)?;
    let stitch_stream = &blob[report.stitch_offset..];
    let records: Vec<&[u8]> = stitch_stream.chunks_exact(2).collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut x_raw = 0i32;
    let mut y_raw = 0i32;
    while i < records.len() {
        let a = records[i][0];
        let b = records[i][1];
        if a == 0x80 {
            if b == 0x01 && i + 3 < records.len() && records[i + 3] == [0x80, 0x02] {
                let dx = i16::from_be_bytes([records[i + 1][0], records[i + 1][1]]) as i32;
                let dy = i16::from_be_bytes([records[i + 2][0], records[i + 2][1]]) as i32;
                x_raw += dx;
                y_raw += dy;
                out.push((x_raw, -y_raw, false));
                i += 4;
                continue;
            }
            out.push((x_raw, -y_raw, false));
            i += 1;
        } else {
            x_raw += a as i8 as i32;
            y_raw += b as i8 as i32;
            out.push((x_raw, -y_raw, true));
            i += 1;
        }
    }
    Ok(out)
}
