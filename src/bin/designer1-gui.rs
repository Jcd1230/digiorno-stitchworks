use designer1_tools::inkstitch::{load_inkstitch_json_file, LoadOptions};
use designer1_tools::model::{Design, InputYAxis, SignatureMode, StitchCommand};
use designer1_tools::preview::render_preview_4bpp;
use designer1_tools::shv::{build_shv, validate_generated_shv, ShvOptions, ShvReadbackReport};
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
            status: "Load an Ink/Stitch JSON file to begin.".to_owned(),
            error: None,
        }
    }
}

impl eframe::App for Designer1App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
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
                    ui.add(egui::DragValue::new(&mut self.scale).speed(0.05).range(0.01..=100.0));
                });
                ui.checkbox(&mut self.center, "Center design at SHV origin");

                egui::ComboBox::from_label("Input Y axis")
                    .selected_text(self.input_y_axis.to_string())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.input_y_axis, InputYAxis::Down, "down / Ink-Stitch SVG");
                        ui.selectable_value(&mut self.input_y_axis, InputYAxis::Up, "up / Cartesian");
                    });

                egui::ComboBox::from_label("SHV signature")
                    .selected_text(self.signature.to_string())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.signature, SignatureMode::Official, "official Viking notice");
                        ui.selectable_value(&mut self.signature, SignatureMode::Zero, "zero-filled Embird-style");
                    });

                ui.horizontal(|ui| {
                    ui.label("Preview");
                    ui.add(egui::DragValue::new(&mut self.preview_width).speed(1).range(1..=255));
                    ui.label("×");
                    ui.add(egui::DragValue::new(&mut self.preview_height).speed(1).range(1..=255));
                });

                ui.add_space(8.0);
                if ui.button("Rebuild preview + SHV bytes").clicked() {
                    self.rebuild_current();
                }

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
                    egui::Grid::new("stats_grid").num_columns(2).striped(true).show(ui, |ui| {
                        ui.label("Name"); ui.monospace(&design.name); ui.end_row();
                        ui.label("Threads"); ui.label(stats.thread_count.to_string()); ui.end_row();
                        ui.label("Points"); ui.label(stats.point_count.to_string()); ui.end_row();
                        ui.label("Stitches"); ui.label(stats.stitches.to_string()); ui.end_row();
                        ui.label("Jumps"); ui.label(stats.jumps.to_string()); ui.end_row();
                        ui.label("Bounds"); ui.monospace(format!("L{} R{} B{} T{}", stats.left, stats.right, stats.bottom, stats.top)); ui.end_row();
                        ui.label("Size"); ui.monospace(format!("{} × {} units", stats.width, stats.height)); ui.end_row();
                    });
                }

                if let Some(report) = &self.report {
                    ui.separator();
                    ui.heading("SHV readback");
                    egui::Grid::new("readback_grid").num_columns(2).striped(true).show(ui, |ui| {
                        ui.label("SHV name"); ui.monospace(&report.name); ui.end_row();
                        ui.label("Colors"); ui.label(report.color_count.to_string()); ui.end_row();
                        ui.label("Records"); ui.label(report.total_records.to_string()); ui.end_row();
                        ui.label("Stitch bytes"); ui.label(report.stitch_bytes.to_string()); ui.end_row();
                        ui.label("Final position"); ui.monospace(format!("{}, {}", report.final_position.x, report.final_position.y)); ui.end_row();
                    });
                }
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading("Normalized stitch path");
                if let Some(design) = &self.design {
                    draw_design_path(ui, design, Vec2::new(ui.available_width(), 430.0));
                } else {
                    ui.label("No design loaded.");
                }

                ui.add_space(12.0);
                ui.heading("Embedded SHV thumbnail preview");
                if let Some(bytes) = &self.preview_bytes {
                    draw_4bpp_preview(ui, bytes, self.preview_width as usize, self.preview_height as usize);
                } else {
                    ui.label("No preview built.");
                }
            });
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

fn draw_design_path(ui: &mut egui::Ui, design: &Design, desired: Vec2) {
    let (rect, _response) = ui.allocate_exact_size(desired, Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6.0, Color32::from_gray(248));

    let stats = design.stats();
    let span_x = stats.width.max(1) as f32;
    let span_y = stats.height.max(1) as f32;
    let pad = 18.0f32;
    let scale = ((rect.width() - 2.0 * pad) / span_x).min((rect.height() - 2.0 * pad) / span_y);

    let map = |x: i32, y: i32| -> Pos2 {
        Pos2::new(
            rect.left() + (x - stats.left) as f32 * scale + pad,
            rect.top() + (stats.top - y) as f32 * scale + pad,
        )
    };

    let mut prev: Option<Pos2> = None;
    for p in &design.points {
        match &p.command {
            StitchCommand::End => break,
            StitchCommand::Jump | StitchCommand::Trim => {
                let pos = map(p.x, p.y);
                painter.circle_filled(pos, 1.5, Color32::from_gray(120));
                prev = Some(pos);
            }
            StitchCommand::Stitch => {
                let pos = map(p.x, p.y);
                if let Some(prev_pos) = prev {
                    painter.line_segment([prev_pos, pos], Stroke::new(1.0, Color32::BLACK));
                } else {
                    painter.circle_filled(pos, 1.5, Color32::BLACK);
                }
                prev = Some(pos);
            }
            StitchCommand::ColorChange | StitchCommand::Stop | StitchCommand::Other(_) => {
                prev = None;
            }
        }
    }
}

fn draw_4bpp_preview(ui: &mut egui::Ui, bytes: &[u8], width: usize, height: usize) {
    let cell = 5.0f32;
    let desired = Vec2::new(width as f32 * cell, height as f32 * cell).min(Vec2::new(ui.available_width(), 260.0));
    let (rect, _response) = ui.allocate_exact_size(desired, Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, Color32::from_gray(250));

    let sx = rect.width() / width.max(1) as f32;
    let sy = rect.height() / height.max(1) as f32;
    let stride = width.div_ceil(2);
    for y in 0..height {
        for x in 0..width {
            let byte = bytes.get(y * stride + x / 2).copied().unwrap_or(0);
            let value = if x % 2 == 0 { byte >> 4 } else { byte & 0x0f };
            if value == 0 {
                continue;
            }
            let shade = 240u8.saturating_sub(value.saturating_mul(28));
            let r = Rect::from_min_size(
                Pos2::new(rect.left() + x as f32 * sx, rect.top() + y as f32 * sy),
                Vec2::new(sx.max(1.0), sy.max(1.0)),
            );
            painter.rect_filled(r, 0.0, Color32::from_gray(shade));
        }
    }
}
