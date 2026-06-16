use designer1_tools::disk::{
    DiskDesignInput, DiskExportOptions, export_single_menu_disk, load_disk_designs,
};
use designer1_tools::inkstitch::{LoadOptions, load_inkstitch_json_file};
use designer1_tools::model::{Design, InputYAxis, SignatureMode, StitchCommand};
use designer1_tools::preview::render_preview_4bpp;
use designer1_tools::shv::{ShvOptions, ShvReadbackReport, build_shv, validate_generated_shv};
use eframe::egui;
use egui::{Color32, Pos2, Rect, Sense, Stroke, Vec2};
use std::path::{Path, PathBuf};

const PREVIEW_WIDTH: u8 = 96;
const PREVIEW_HEIGHT: u8 = 24;

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
    disk_root: Option<PathBuf>,
    disk_designs: Vec<DiskDesignInput>,
    selected_disk_index: Option<usize>,
    design: Option<Design>,
    shv_bytes: Option<Vec<u8>>,
    report: Option<ShvReadbackReport>,
    preview_bytes: Option<Vec<u8>>,

    name_override: String,
    scale: f64,
    center: bool,
    input_y_axis: InputYAxis,
    signature: SignatureMode,
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
            disk_root: None,
            disk_designs: Vec::new(),
            selected_disk_index: None,
            design: None,
            shv_bytes: None,
            report: None,
            preview_bytes: None,
            name_override: String::new(),
            scale: 1.0,
            center: true,
            input_y_axis: InputYAxis::Down,
            signature: SignatureMode::Official,
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
                if ui.button("Open Folder…").clicked() {
                    self.pick_and_load_folder();
                }
                if ui.button("Rebuild").clicked() {
                    self.rebuild_current();
                }
                if ui.button("Export SHV…").clicked() {
                    self.export_shv();
                }
                if ui.button("Generate Disk Files").clicked() {
                    self.export_disk();
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
                    .or_else(|| {
                        self.disk_root
                            .as_ref()
                            .map(|p| format!("Disk root: {}", p.display()))
                    })
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

                if !self.disk_designs.is_empty() {
                    ui.separator();
                    ui.heading("Disk designs");
                    let mut selected = None;
                    for (idx, item) in self.disk_designs.iter().enumerate() {
                        let is_selected = self.selected_disk_index == Some(idx);
                        let label = format!("DES01_{:02} {}", item.slot, item.label);
                        if ui.selectable_label(is_selected, label).clicked() {
                            selected = Some(idx);
                        }
                    }
                    if let Some(idx) = selected {
                        self.select_disk_design(idx);
                    }
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
                if let Some(design) = &self.design {
                    draw_thread_thumbnail(
                        ui,
                        design,
                        PREVIEW_WIDTH as usize,
                        PREVIEW_HEIGHT as usize,
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
            self.disk_root = None;
            self.disk_designs.clear();
            self.selected_disk_index = None;
            self.rebuild_current();
        }
    }

    fn pick_and_load_folder(&mut self) {
        if let Some(path) = rfd::FileDialog::new().pick_folder() {
            self.load_folder(path);
        }
    }

    fn load_options(&self) -> LoadOptions {
        LoadOptions {
            scale: self.scale,
            center: self.center,
            input_y_axis: self.input_y_axis,
        }
    }

    fn disk_options(&self) -> DiskExportOptions {
        DiskExportOptions {
            signature: self.signature,
            scale: self.scale,
            center: self.center,
            input_y_axis: self.input_y_axis,
            disk_title: "Designer 1 Disk".to_owned(),
            menu_label: "Menu 1".to_owned(),
        }
    }

    fn rebuild_current(&mut self) {
        self.error = None;
        if let (Some(root), Some(idx)) = (self.disk_root.clone(), self.selected_disk_index) {
            match load_disk_designs(&root, &self.disk_options()) {
                Ok(designs) => {
                    self.disk_designs = designs;
                    self.select_disk_design(idx.min(self.disk_designs.len().saturating_sub(1)));
                }
                Err(err) => {
                    self.error = Some(err.to_string());
                    self.status = "Folder rebuild failed.".to_owned();
                }
            }
            return;
        }
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

    fn load_folder(&mut self, path: PathBuf) {
        self.error = None;
        match load_disk_designs(&path, &self.disk_options()) {
            Ok(designs) => {
                self.input_path = None;
                self.disk_root = Some(path);
                self.disk_designs = designs;
                self.selected_disk_index = None;
                if !self.disk_designs.is_empty() {
                    self.select_disk_design(0);
                }
                self.status = format!("Loaded {} disk design(s).", self.disk_designs.len());
            }
            Err(err) => {
                self.error = Some(err.to_string());
                self.status = "Folder load failed.".to_owned();
                self.disk_root = None;
                self.disk_designs.clear();
                self.selected_disk_index = None;
            }
        }
    }

    fn select_disk_design(&mut self, idx: usize) {
        self.error = None;
        let Some(item) = self.disk_designs.get(idx) else {
            self.error = Some("Selected disk design is out of range.".to_owned());
            return;
        };
        let design = item.design.clone();
        let slot = item.slot;
        let label = item.label.clone();
        match self.rebuild_from_design(design) {
            Ok(()) => {
                self.selected_disk_index = Some(idx);
                self.name_override.clear();
                self.status = format!("Selected DES01_{slot:02}: {label}");
            }
            Err(err) => {
                self.error = Some(err.to_string());
                self.status = "Preview build failed.".to_owned();
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

        self.rebuild_from_design(design)
    }

    fn rebuild_from_design(&mut self, design: Design) -> anyhow::Result<()> {
        let options = ShvOptions {
            name: None,
            signature: self.signature,
            preview_width: PREVIEW_WIDTH,
            preview_height: PREVIEW_HEIGHT,
        };
        let shv = build_shv(&design, &options)?;
        let report = validate_generated_shv(&shv)?;
        let preview = render_preview_4bpp(&design, PREVIEW_WIDTH, PREVIEW_HEIGHT, 4)?;

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

    fn export_disk(&mut self) {
        self.error = None;
        let Some(root) = self.disk_root.clone() else {
            self.error = Some("Open a disk root folder first.".to_owned());
            return;
        };
        match export_single_menu_disk(&root, &self.disk_options()) {
            Ok(report) => {
                self.status = format!(
                    "Generated {} design(s) and {} disk file(s).",
                    report.designs.len(),
                    report.written_files.len()
                );
            }
            Err(err) => {
                self.error = Some(err.to_string());
                self.status = "Disk export failed.".to_owned();
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

fn draw_thread_thumbnail(ui: &mut egui::Ui, design: &Design, width: usize, height: usize) {
    let pixels = render_thread_thumbnail(design, width, height);
    let cell = 5.0f32;
    let bounds = thumbnail_pixel_bounds(&pixels, width, height).unwrap_or((0, width, 0, height));
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
    for y in bounds.2..bounds.3 {
        for x in bounds.0..bounds.1 {
            let Some(color) = pixels[y * width + x] else {
                continue;
            };
            let r = Rect::from_min_size(
                Pos2::new(
                    rect.left() + (x - bounds.0) as f32 * sx,
                    rect.top() + (y - bounds.2) as f32 * sy,
                ),
                Vec2::new(sx.max(1.0), sy.max(1.0)),
            );
            painter.rect_filled(r, 0.0, color);
        }
    }
}

fn render_thread_thumbnail(design: &Design, width: usize, height: usize) -> Vec<Option<Color32>> {
    let drawable: Vec<_> = design
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
    let to_pixel = |x: i32, y: i32| -> (i32, i32) {
        let px = ((x - min_x) as f64 * scale + pad).round() as i32;
        let py = ((max_y - y) as f64 * scale + pad).round() as i32;
        (
            px.clamp(0, width as i32 - 1),
            py.clamp(0, height as i32 - 1),
        )
    };

    let mut pixels = vec![None; width * height];
    let mut prev: Option<(i32, i32)> = None;
    let mut thread_index = 0usize;
    let mut current_color = thread_color(design, thread_index);
    for p in &design.points {
        match &p.command {
            StitchCommand::End => break,
            StitchCommand::Jump | StitchCommand::Trim => {
                let pt = to_pixel(p.x, p.y);
                set_thumbnail_pixel(&mut pixels, width, height, pt.0, pt.1, current_color);
                prev = Some(pt);
            }
            StitchCommand::Stitch => {
                let pt = to_pixel(p.x, p.y);
                if let Some(prev_pt) = prev {
                    draw_thumbnail_line(
                        &mut pixels,
                        width,
                        height,
                        prev_pt.0,
                        prev_pt.1,
                        pt.0,
                        pt.1,
                        current_color,
                    );
                } else {
                    set_thumbnail_pixel(&mut pixels, width, height, pt.0, pt.1, current_color);
                }
                prev = Some(pt);
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

    pixels
}

fn set_thumbnail_pixel(
    pixels: &mut [Option<Color32>],
    width: usize,
    height: usize,
    x: i32,
    y: i32,
    color: Color32,
) {
    if x >= 0 && y >= 0 && (x as usize) < width && (y as usize) < height {
        pixels[y as usize * width + x as usize] = Some(color);
    }
}

fn draw_thumbnail_line(
    pixels: &mut [Option<Color32>],
    width: usize,
    height: usize,
    mut x0: i32,
    mut y0: i32,
    x1: i32,
    y1: i32,
    color: Color32,
) {
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;

    loop {
        set_thumbnail_pixel(pixels, width, height, x0, y0, color);
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

fn thumbnail_pixel_bounds(
    pixels: &[Option<Color32>],
    width: usize,
    height: usize,
) -> Option<(usize, usize, usize, usize)> {
    let mut left = width;
    let mut right = 0usize;
    let mut top = height;
    let mut bottom = 0usize;

    for y in 0..height {
        for x in 0..width {
            if pixels[y * width + x].is_none() {
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
