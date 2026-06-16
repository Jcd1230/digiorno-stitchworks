use designer1_tools::inkstitch::{LoadOptions, load_inkstitch_json_file};
use designer1_tools::model::{Design, InputYAxis, SignatureMode, StitchCommand};
use designer1_tools::preview::render_preview_4bpp;
use designer1_tools::shv::{ShvOptions, ShvReadbackReport, build_shv, validate_generated_shv};
use eframe::egui;
use egui::{Color32, Pos2, Rect, Sense, Stroke, Vec2};
use std::path::{Path, PathBuf};

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1120.0, 760.0])
            .with_min_inner_size([900.0, 600.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Designer 1 SHV Converter",
        options,
        Box::new(|_cc| Ok(Box::new(Designer1App::default()))),
    )
}

struct Designer1App {
    input_path: Option<PathBuf>,
    design: Option<Design>,
    shv_bytes: Option<Vec<u8>>,
    report: Option<ShvReadbackReport>,
    preview_bytes: Option<Vec<u8>>,

    name_override: String,
    scale: f64,
    center: bool,
    input_y_axis: InputYAxis,
    signature: SignatureMode,
    preview_width: u8,
    preview_height: u8,
    show_cm_grid: bool,
    show_jumps: bool,
    path_zoom: f32,
    path_pan: Vec2,

    status: String,
    error: Option<String>,
}

impl Default for Designer1App {
    fn default() -> Self {
        Self {
            input_path: None,
            design: None,
            shv_bytes: None,
            report: None,
            preview_bytes: None,
            name_override: String::new(),
            scale: 1.0,
            center: true,
            input_y_axis: InputYAxis::Down,
            signature: SignatureMode::Official,
            preview_width: 96,
            preview_height: 24,
            show_cm_grid: true,
            show_jumps: true,
            path_zoom: 1.0,
            path_pan: Vec2::ZERO,
            status: "Load an Ink/Stitch JSON file to begin.".to_owned(),
            error: None,
        }
    }
}

impl eframe::App for Designer1App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx();

        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Designer 1 SHV Converter");
                ui.separator();
                if ui.button("Open JSON…").clicked() {
                    self.pick_and_load_json();
                }
                if ui.button("Rebuild").clicked() {
                    self.rebuild_current();
                }
                if ui.button("Export SHV…").clicked() {
                    self.export_shv();
                }
            });
        });

        egui::SidePanel::left("options")
            .resizable(true)
            .default_width(320.0)
            .show(ctx, |ui| {
                ui.heading("Options");
                ui.add_space(8.0);

                ui.label("Input");
                let input_text = self
                    .input_path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "No file loaded".to_owned());
                ui.monospace(input_text);

                ui.separator();
                ui.label("Internal SHV name");
                ui.text_edit_singleline(&mut self.name_override);

                ui.horizontal(|ui| {
                    ui.label("Scale");
                    ui.add(
                        egui::DragValue::new(&mut self.scale)
                            .speed(0.05)
                            .range(0.01..=100.0),
                    );
                });
                ui.checkbox(&mut self.center, "Center design at SHV origin");

                egui::ComboBox::from_label("Input Y axis")
                    .selected_text(self.input_y_axis.to_string())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.input_y_axis,
                            InputYAxis::Down,
                            "down / Ink-Stitch SVG",
                        );
                        ui.selectable_value(
                            &mut self.input_y_axis,
                            InputYAxis::Up,
                            "up / Cartesian",
                        );
                    });

                egui::ComboBox::from_label("SHV signature")
                    .selected_text(self.signature.to_string())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.signature,
                            SignatureMode::Official,
                            "official Viking notice",
                        );
                        ui.selectable_value(
                            &mut self.signature,
                            SignatureMode::Zero,
                            "zero-filled Embird-style",
                        );
                    });

                ui.horizontal(|ui| {
                    ui.label("Preview");
                    ui.add(
                        egui::DragValue::new(&mut self.preview_width)
                            .speed(1)
                            .range(1..=255),
                    );
                    ui.label("×");
                    ui.add(
                        egui::DragValue::new(&mut self.preview_height)
                            .speed(1)
                            .range(1..=255),
                    );
                });

                ui.add_space(8.0);
                if ui.button("Rebuild preview + SHV bytes").clicked() {
                    self.rebuild_current();
                }

                ui.separator();
                ui.heading("Path view");
                ui.checkbox(&mut self.show_cm_grid, "Show 1 cm grid");
                ui.checkbox(&mut self.show_jumps, "Show jump stitches");
                ui.horizontal(|ui| {
                    if ui.button("Reset view").clicked() {
                        self.path_zoom = 1.0;
                        self.path_pan = Vec2::ZERO;
                    }
                    ui.label(format!("Zoom {:.0}%", self.path_zoom * 100.0));
                });

                ui.separator();
                ui.heading("Status");
                ui.label(&self.status);
                if let Some(error) = &self.error {
                    ui.colored_label(Color32::from_rgb(190, 40, 40), error);
                }

                if let Some(design) = &self.design {
                    ui.separator();
                    ui.heading("Design stats");
                    let stats = design.stats();
                    egui::Grid::new("stats_grid")
                        .num_columns(2)
                        .striped(true)
                        .show(ui, |ui| {
                            ui.label("Name");
                            ui.monospace(&design.name);
                            ui.end_row();
                            ui.label("Threads");
                            ui.label(stats.thread_count.to_string());
                            ui.end_row();
                            ui.label("Points");
                            ui.label(stats.point_count.to_string());
                            ui.end_row();
                            ui.label("Stitches");
                            ui.label(stats.stitches.to_string());
                            ui.end_row();
                            ui.label("Jumps");
                            ui.label(stats.jumps.to_string());
                            ui.end_row();
                            ui.label("Bounds");
                            ui.monospace(format!(
                                "L{} R{} B{} T{} mm",
                                mm(stats.left),
                                mm(stats.right),
                                mm(stats.bottom),
                                mm(stats.top)
                            ));
                            ui.end_row();
                            ui.label("Size");
                            ui.monospace(format!("{} × {} mm", mm(stats.width), mm(stats.height)));
                            ui.end_row();
                        });
                }

                if let Some(report) = &self.report {
                    ui.separator();
                    ui.heading("SHV readback");
                    egui::Grid::new("readback_grid")
                        .num_columns(2)
                        .striped(true)
                        .show(ui, |ui| {
                            ui.label("SHV name");
                            ui.monospace(&report.name);
                            ui.end_row();
                            ui.label("Colors");
                            ui.label(report.color_count.to_string());
                            ui.end_row();
                            ui.label("Records");
                            ui.label(report.total_records.to_string());
                            ui.end_row();
                            ui.label("Stitch bytes");
                            ui.label(report.stitch_bytes.to_string());
                            ui.end_row();
                            ui.label("Final position");
                            ui.monospace(format!(
                                "{}, {}",
                                report.final_position.x, report.final_position.y
                            ));
                            ui.end_row();
                        });
                }

                ui.separator();
                ui.heading("Embedded SHV thumbnail preview");
                if let Some(bytes) = &self.preview_bytes {
                    draw_4bpp_preview(
                        ui,
                        bytes,
                        self.preview_width as usize,
                        self.preview_height as usize,
                    );
                } else {
                    ui.label("No preview built.");
                }
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Normalized stitch path");
            ui.add_space(6.0);
            if let Some(design) = &self.design {
                draw_design_path(
                    ui,
                    design,
                    self.show_cm_grid,
                    self.show_jumps,
                    &mut self.path_zoom,
                    &mut self.path_pan,
                );
            } else {
                ui.label("No design loaded.");
            }
        });
    }
}

impl Designer1App {
    fn pick_and_load_json(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Ink/Stitch JSON", &["json"])
            .pick_file()
        {
            self.input_path = Some(path);
            self.rebuild_current();
        }
    }

    fn load_options(&self) -> LoadOptions {
        LoadOptions {
            scale: self.scale,
            center: self.center,
            input_y_axis: self.input_y_axis,
        }
    }

    fn rebuild_current(&mut self) {
        self.error = None;
        let Some(path) = self.input_path.clone() else {
            self.error = Some("No JSON file selected.".to_owned());
            return;
        };

        match self.rebuild_from_path(&path) {
            Ok(()) => {
                self.status = "Loaded and validated generated SHV bytes.".to_owned();
            }
            Err(err) => {
                self.error = Some(err.to_string());
                self.status = "Rebuild failed.".to_owned();
                self.design = None;
                self.shv_bytes = None;
                self.report = None;
                self.preview_bytes = None;
            }
        }
    }

    fn rebuild_from_path(&mut self, path: &Path) -> anyhow::Result<()> {
        let mut design = load_inkstitch_json_file(path, &self.load_options())?;
        if self.name_override.trim().is_empty() {
            self.name_override = design.name.clone();
        } else {
            design.name = self.name_override.trim().to_owned();
        }

        let options = ShvOptions {
            name: None,
            signature: self.signature,
            preview_width: self.preview_width,
            preview_height: self.preview_height,
        };
        let shv = build_shv(&design, &options)?;
        let report = validate_generated_shv(&shv)?;
        let preview = render_preview_4bpp(&design, self.preview_width, self.preview_height, 4)?;

        self.design = Some(design);
        self.shv_bytes = Some(shv);
        self.report = Some(report);
        self.preview_bytes = Some(preview);
        Ok(())
    }

    fn export_shv(&mut self) {
        self.rebuild_current();
        if self.shv_bytes.is_none() {
            return;
        }
        let default_name = if self.name_override.trim().is_empty() {
            "DESIGN.SHV".to_owned()
        } else {
            format!("{}.SHV", self.name_override.trim().to_ascii_uppercase())
        };
        if let Some(mut path) = rfd::FileDialog::new()
            .add_filter("Husqvarna SHV", &["shv", "SHV"])
            .set_file_name(&default_name)
            .save_file()
        {
            if path.extension().is_none() {
                path.set_extension("SHV");
            }
            let bytes = self.shv_bytes.as_ref().unwrap();
            match std::fs::write(&path, bytes) {
                Ok(()) => {
                    self.status = format!("Exported {}", path.display());
                    self.error = None;
                }
                Err(err) => {
                    self.error = Some(format!("Failed to write {}: {err}", path.display()));
                }
            }
        }
    }
}

fn draw_design_path(
    ui: &mut egui::Ui,
    design: &Design,
    show_cm_grid: bool,
    show_jumps: bool,
    zoom: &mut f32,
    pan: &mut Vec2,
) {
    let desired = ui.available_size().max(Vec2::new(240.0, 240.0));
    let (rect, response) = ui.allocate_exact_size(desired, Sense::drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6.0, Color32::from_gray(248));

    let stats = design.stats();
    let span_x = stats.width.max(1) as f32;
    let span_y = stats.height.max(1) as f32;
    let pad = 28.0f32;
    let fit_scale = ((rect.width() - 2.0 * pad) / span_x).min((rect.height() - 2.0 * pad) / span_y);
    let scale = fit_scale * *zoom;
    let center = rect.center() + *pan;
    let design_center_x = (stats.left + stats.right) as f32 / 2.0;
    let design_center_y = (stats.bottom + stats.top) as f32 / 2.0;

    let map = |x: i32, y: i32| -> Pos2 {
        Pos2::new(
            center.x + (x as f32 - design_center_x) * scale,
            center.y - (y as f32 - design_center_y) * scale,
        )
    };

    if response.dragged() {
        *pan += ui.input(|i| i.pointer.delta());
    }
    if response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            let old_zoom = *zoom;
            *zoom = (*zoom * (1.0 + scroll * 0.0015)).clamp(0.1, 20.0);
            if old_zoom != 0.0 {
                *pan *= *zoom / old_zoom;
            }
        }
    }

    if show_cm_grid {
        draw_cm_grid(
            &painter,
            rect,
            stats.left,
            stats.right,
            stats.bottom,
            stats.top,
            &map,
        );
    }

    let mut prev: Option<Pos2> = None;
    let mut thread_index = 0usize;
    let mut current_color = thread_color(design, thread_index);
    for p in &design.points {
        match &p.command {
            StitchCommand::End => break,
            StitchCommand::Jump | StitchCommand::Trim => {
                let pos = map(p.x, p.y);
                if show_jumps {
                    if let Some(prev_pos) = prev {
                        draw_dotted_line(
                            &painter,
                            prev_pos,
                            pos,
                            Stroke::new(1.0, Color32::from_rgb(210, 42, 42)),
                        );
                    }
                    painter.circle_filled(pos, 1.5, Color32::from_rgb(210, 42, 42));
                }
                prev = Some(pos);
            }
            StitchCommand::Stitch => {
                let pos = map(p.x, p.y);
                if let Some(prev_pos) = prev {
                    painter.line_segment([prev_pos, pos], Stroke::new(1.0, current_color));
                } else {
                    painter.circle_filled(pos, 1.5, current_color);
                }
                prev = Some(pos);
            }
            StitchCommand::ColorChange | StitchCommand::Stop => {
                thread_index = (thread_index + 1).min(design.threads.len().saturating_sub(1));
                current_color = thread_color(design, thread_index);
                prev = None;
            }
            StitchCommand::Other(_) => {
                prev = None;
            }
        }
    }
}

fn draw_cm_grid(
    painter: &egui::Painter,
    rect: Rect,
    left: i32,
    right: i32,
    bottom: i32,
    top: i32,
    map: &impl Fn(i32, i32) -> Pos2,
) {
    const CM_IN_SHV_UNITS: i32 = 100;
    let stroke = Stroke::new(1.0, Color32::from_gray(220));
    let axis_stroke = Stroke::new(1.0, Color32::from_gray(185));
    let grid_left = (left / CM_IN_SHV_UNITS - 1) * CM_IN_SHV_UNITS;
    let grid_right = (right / CM_IN_SHV_UNITS + 1) * CM_IN_SHV_UNITS;
    let grid_bottom = (bottom / CM_IN_SHV_UNITS - 1) * CM_IN_SHV_UNITS;
    let grid_top = (top / CM_IN_SHV_UNITS + 1) * CM_IN_SHV_UNITS;

    let mut x = grid_left;
    while x <= grid_right {
        let a = map(x, grid_bottom);
        let b = map(x, grid_top);
        let line = clipped_segment(rect, a, b);
        if let Some([a, b]) = line {
            painter.line_segment([a, b], if x == 0 { axis_stroke } else { stroke });
        }
        x += CM_IN_SHV_UNITS;
    }

    let mut y = grid_bottom;
    while y <= grid_top {
        let a = map(grid_left, y);
        let b = map(grid_right, y);
        let line = clipped_segment(rect, a, b);
        if let Some([a, b]) = line {
            painter.line_segment([a, b], if y == 0 { axis_stroke } else { stroke });
        }
        y += CM_IN_SHV_UNITS;
    }
}

fn clipped_segment(rect: Rect, a: Pos2, b: Pos2) -> Option<[Pos2; 2]> {
    let min_x = a.x.min(b.x).max(rect.left());
    let max_x = a.x.max(b.x).min(rect.right());
    let min_y = a.y.min(b.y).max(rect.top());
    let max_y = a.y.max(b.y).min(rect.bottom());
    if (a.x - b.x).abs() < f32::EPSILON && min_y <= max_y {
        Some([Pos2::new(a.x, min_y), Pos2::new(a.x, max_y)])
    } else if (a.y - b.y).abs() < f32::EPSILON && min_x <= max_x {
        Some([Pos2::new(min_x, a.y), Pos2::new(max_x, a.y)])
    } else {
        None
    }
}

fn mm(units: i32) -> String {
    format!("{:.1}", units as f32 * 0.1)
}

fn draw_dotted_line(painter: &egui::Painter, start: Pos2, end: Pos2, stroke: Stroke) {
    let delta = end - start;
    let length = delta.length();
    if length <= f32::EPSILON {
        return;
    }

    let direction = delta / length;
    let dash = 5.0;
    let gap = 4.0;
    let mut offset = 0.0;
    while offset < length {
        let dash_end = (offset + dash).min(length);
        painter.line_segment(
            [start + direction * offset, start + direction * dash_end],
            stroke,
        );
        offset += dash + gap;
    }
}

fn thread_color(design: &Design, index: usize) -> Color32 {
    let Some(thread) = design.threads.get(index).or_else(|| design.threads.first()) else {
        return Color32::BLACK;
    };
    parse_hex_color(thread.color.as_deref()).unwrap_or(Color32::BLACK)
}

fn parse_hex_color(value: Option<&str>) -> Option<Color32> {
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
    Some(Color32::from_rgb(r, g, b))
}

fn draw_4bpp_preview(ui: &mut egui::Ui, bytes: &[u8], width: usize, height: usize) {
    let cell = 5.0f32;
    let bounds = preview_pixel_bounds(bytes, width, height).unwrap_or((0, width, 0, height));
    let preview_width = bounds.1.saturating_sub(bounds.0).max(1);
    let preview_height = bounds.3.saturating_sub(bounds.2).max(1);
    let native = Vec2::new(preview_width as f32 * cell, preview_height as f32 * cell);
    let max_size = Vec2::new(ui.available_width(), 260.0);
    let scale = (max_size.x / native.x)
        .min(max_size.y / native.y)
        .min(1.0)
        .max(0.0);
    let desired = native * scale;
    let (rect, _response) = ui.allocate_exact_size(desired, Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, Color32::from_gray(250));

    let sx = rect.width() / preview_width as f32;
    let sy = rect.height() / preview_height as f32;
    let stride = width.div_ceil(2);
    for y in bounds.2..bounds.3 {
        for x in bounds.0..bounds.1 {
            let byte = bytes.get(y * stride + x / 2).copied().unwrap_or(0);
            let value = if x % 2 == 0 { byte >> 4 } else { byte & 0x0f };
            if value == 0 {
                continue;
            }
            let shade = 240u8.saturating_sub(value.saturating_mul(28));
            let r = Rect::from_min_size(
                Pos2::new(
                    rect.left() + (x - bounds.0) as f32 * sx,
                    rect.top() + (y - bounds.2) as f32 * sy,
                ),
                Vec2::new(sx.max(1.0), sy.max(1.0)),
            );
            painter.rect_filled(r, 0.0, Color32::from_gray(shade));
        }
    }
}

fn preview_pixel_bounds(
    bytes: &[u8],
    width: usize,
    height: usize,
) -> Option<(usize, usize, usize, usize)> {
    let stride = width.div_ceil(2);
    let mut left = width;
    let mut right = 0usize;
    let mut top = height;
    let mut bottom = 0usize;

    for y in 0..height {
        for x in 0..width {
            let byte = bytes.get(y * stride + x / 2).copied().unwrap_or(0);
            let value = if x % 2 == 0 { byte >> 4 } else { byte & 0x0f };
            if value == 0 {
                continue;
            }
            left = left.min(x);
            right = right.max(x + 1);
            top = top.min(y);
            bottom = bottom.max(y + 1);
        }
    }

    if left < right && top < bottom {
        Some((
            left.saturating_sub(1),
            (right + 1).min(width),
            top.saturating_sub(1),
            (bottom + 1).min(height),
        ))
    } else {
        None
    }
}
