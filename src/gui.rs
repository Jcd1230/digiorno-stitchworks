use crate::disk::{
    DiskDesignInput, DiskExportOptions, MHV_PREVIEW_HEIGHT, MHV_PREVIEW_WIDTH, build_menu_assets,
    export_single_menu_disk, load_disk_designs, render_mhv_preview_pixels,
};
use crate::gotek::{
    GotekDeviceCandidate, SLOT_COUNT, SlotInspection, create_designer_disk_image,
    create_updated_designer_menu_image, discover_gotek_devices, gotek_device_paths_match,
    gotek_slot_count_for_size, inspect_slot, verify_slot, write_slot,
};
use crate::inkstitch::{LoadOptions, load_inkstitch_json_file};
use crate::model::{Design, InputYAxis, SignatureMode, StitchCommand};
use crate::preview::DESIGNER1_PALETTE;
use crate::shv::{ShvOptions, ShvReadbackReport, build_shv, validate_generated_shv};
use eframe::egui;
use egui::{Color32, Pos2, Rect, Sense, Stroke, Vec2};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

const SETTINGS_DIR: &str = "designer1-rs";
const SETTINGS_FILE: &str = "gui.toml";
const GOTEK_INSPECTION_DEBOUNCE: Duration = Duration::from_millis(250);
const GOTEK_INSPECTION_POLL: Duration = Duration::from_millis(40);

struct GotekInspectionMessage {
    generation: u64,
    device: PathBuf,
    slot: u16,
    result: Result<SlotInspection, String>,
}

#[derive(Debug, Clone)]
struct GotekEmptyScan {
    device: PathBuf,
    next_slot: u16,
    slot_limit: u16,
    remaining: u16,
    checked: u16,
    generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GotekExportMode {
    SingleMenu,
    Menu1,
    Menu2,
    Menu3,
    Menu4,
}

impl GotekExportMode {
    const ALL: [Self; 5] = [
        Self::SingleMenu,
        Self::Menu1,
        Self::Menu2,
        Self::Menu3,
        Self::Menu4,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::SingleMenu => "Single menu (replace whole slot)",
            Self::Menu1 => "Menu 1",
            Self::Menu2 => "Menu 2",
            Self::Menu3 => "Menu 3",
            Self::Menu4 => "Menu 4",
        }
    }

    fn menu_index(self) -> Option<u8> {
        match self {
            Self::SingleMenu => None,
            Self::Menu1 => Some(1),
            Self::Menu2 => Some(2),
            Self::Menu3 => Some(3),
            Self::Menu4 => Some(4),
        }
    }
}
fn window_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(include_bytes!("../assets/icons/256x256/designer1-gui.png"))
        .expect("the bundled Designer 1 window icon must be a valid PNG")
}

pub fn run() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id("designer1-gui")
            .with_icon(window_icon())
            .with_inner_size([1280.0, 920.0])
            .with_min_inner_size([900.0, 600.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Designer 1 SHV Converter",
        options,
        Box::new(|cc| Ok(Box::new(Designer1App::new(&cc.egui_ctx)))),
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
    show_mhv_preview: bool,
    show_color_debug: bool,
    dark_mode: bool,
    last_open_dir: PathBuf,
    gotek_device_path: String,
    gotek_devices: Vec<GotekDeviceCandidate>,
    gotek_slot: u16,
    gotek_export_mode: GotekExportMode,
    gotek_slot_inspection: Option<SlotInspection>,
    gotek_slot_inspection_error: Option<String>,
    gotek_slot_cache: BTreeMap<(PathBuf, u16), SlotInspection>,
    gotek_empty_scan: Option<GotekEmptyScan>,
    gotek_inspection_tx: Sender<GotekInspectionMessage>,
    gotek_inspection_rx: Receiver<GotekInspectionMessage>,
    gotek_inspection_generation: u64,
    gotek_inspection_target: Option<(PathBuf, u16)>,
    gotek_inspection_due: Option<Instant>,
    gotek_inspection_loading: bool,

    status: String,
    error: Option<String>,
}

impl Default for Designer1App {
    fn default() -> Self {
        let (gotek_inspection_tx, gotek_inspection_rx) = mpsc::channel();
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
            show_mhv_preview: false,
            show_color_debug: false,
            dark_mode: true,
            last_open_dir: initial_open_dir(),
            gotek_device_path: String::new(),
            gotek_devices: Vec::new(),
            gotek_slot: 1,
            gotek_export_mode: GotekExportMode::SingleMenu,
            gotek_slot_inspection: None,
            gotek_slot_inspection_error: None,
            gotek_slot_cache: BTreeMap::new(),
            gotek_empty_scan: None,
            gotek_inspection_tx,
            gotek_inspection_rx,
            gotek_inspection_generation: 0,
            gotek_inspection_target: None,
            gotek_inspection_due: None,
            gotek_inspection_loading: false,
            status: "Load an Ink/Stitch JSON file to begin.".to_owned(),
            error: None,
        }
    }
}

impl eframe::App for Designer1App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.update_gotek_slot_inspection(&ctx);
        egui::Panel::top("top_bar").show_inside(ui, |ui| {
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
                if ui
                    .add_enabled(
                        self.disk_root.is_some(),
                        egui::Button::new("Export To Gotek Slot"),
                    )
                    .clicked()
                {
                    self.export_to_gotek_slot();
                }
                if ui
                    .add_enabled(
                        !self.disk_designs.is_empty(),
                        egui::Button::new("MHV Preview"),
                    )
                    .clicked()
                {
                    self.show_mhv_preview = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if self.dark_mode {
                        "☀ Light"
                    } else {
                        "🌙 Dark"
                    };
                    if ui.button(label).clicked() {
                        self.dark_mode = !self.dark_mode;
                        self.apply_visuals(ui.ctx());
                    }
                });
            });
        });

        self.show_mhv_preview_window(&ctx);

        egui::Panel::left("options")
            .resizable(true)
            .default_size(320.0)
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        self.show_sidebar(ui);
                    });
            });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            if let Some(design) = &self.design {
                draw_design_path(
                    ui,
                    design,
                    &mut self.show_cm_grid,
                    &mut self.show_jumps,
                    &mut self.path_zoom,
                    &mut self.path_pan,
                );
            } else {
                ui.label("No design loaded.");
            }
        });
        self.update_gotek_slot_inspection(&ctx);
    }
}

impl Designer1App {
    fn new(ctx: &egui::Context) -> Self {
        let mut app = Self::default();
        app.apply_visuals(ctx);
        app.refresh_gotek_devices();
        if let Some(path) = load_last_open_dir().filter(|path| path.is_dir()) {
            app.last_open_dir = path.clone();
            app.load_folder(path);
        }
        app
    }

    fn apply_visuals(&self, ctx: &egui::Context) {
        if self.dark_mode {
            ctx.set_visuals(egui::Visuals::dark());
        } else {
            ctx.set_visuals(egui::Visuals::light());
        }
    }

    fn pick_and_load_json(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .set_directory(&self.last_open_dir)
            .add_filter("Ink/Stitch JSON", &["json"])
            .pick_file()
        {
            self.remember_open_dir(path.parent().unwrap_or(&path));
            self.input_path = Some(path);
            self.disk_root = None;
            self.disk_designs.clear();
            self.selected_disk_index = None;
            self.rebuild_current();
        }
    }

    fn pick_and_load_folder(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .set_directory(&self.last_open_dir)
            .pick_folder()
        {
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
            show_color_debug: self.show_color_debug,
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
                self.remember_open_dir(&path);
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
                let menu_index = self.gotek_export_mode.menu_index().unwrap_or(1);
                self.status = format!("Selected DES{menu_index:02}_{slot:02}: {label}");
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
        };
        let shv = build_shv(&design, &options)?;
        let report = validate_generated_shv(&shv)?;
        let preview =
            shv[report.preview_offset..report.preview_offset + report.preview_length].to_vec();

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
            .set_directory(&self.last_open_dir)
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
                    if let Some(parent) = path.parent() {
                        self.remember_open_dir(parent);
                    }
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

    fn pick_gotek_device_file(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .set_directory(&self.last_open_dir)
            .pick_file()
        {
            if let Some(parent) = path.parent() {
                self.remember_open_dir(parent);
            }
            self.gotek_device_path = path.display().to_string();
            self.schedule_gotek_slot_inspection(GOTEK_INSPECTION_DEBOUNCE);
        }
    }

    fn refresh_gotek_devices(&mut self) {
        match discover_gotek_devices() {
            Ok((devices, warning)) => {
                self.gotek_devices = devices;
                if let Some(warning) = warning {
                    self.status = warning;
                }
            }
            Err(err) => {
                self.gotek_devices.clear();
                self.status = format!("Gotek device discovery failed: {err:#}");
            }
        }
        if self.gotek_device_path.trim().is_empty() {
            if let Some(candidate) = self.gotek_devices.first() {
                self.gotek_device_path = candidate.path.display().to_string();
            }
        }
        self.schedule_gotek_slot_inspection(GOTEK_INSPECTION_DEBOUNCE);
    }

    fn current_gotek_device(&self) -> Option<PathBuf> {
        let path = self.gotek_device_path.trim();
        (!path.is_empty()).then(|| PathBuf::from(path))
    }

    fn gotek_slot_limit(&self) -> u16 {
        let selected = Path::new(self.gotek_device_path.trim());
        let detected = self
            .gotek_devices
            .iter()
            .find(|candidate| gotek_device_paths_match(&candidate.path, selected))
            .and_then(|candidate| candidate.check.slot_count)
            .or_else(|| {
                fs::metadata(selected)
                    .ok()
                    .filter(|metadata| metadata.is_file())
                    .map(|metadata| gotek_slot_count_for_size(metadata.len()))
            })
            .unwrap_or(SLOT_COUNT as u64);
        detected.clamp(1, SLOT_COUNT as u64) as u16
    }
    fn select_gotek_slot(&mut self, slot: u16, delay: Duration) {
        let limit = self.gotek_slot_limit();
        self.gotek_slot = slot.min(limit.saturating_sub(1));
        self.schedule_gotek_slot_inspection(delay);
    }

    fn schedule_gotek_slot_inspection(&mut self, delay: Duration) {
        self.schedule_selected_gotek_slot(delay, false);
    }

    fn force_gotek_slot_inspection(&mut self) {
        self.schedule_selected_gotek_slot(Duration::ZERO, true);
    }

    fn schedule_selected_gotek_slot(&mut self, delay: Duration, force: bool) {
        self.gotek_empty_scan = None;
        self.gotek_inspection_generation = self.gotek_inspection_generation.wrapping_add(1);
        self.gotek_inspection_target = None;
        self.gotek_inspection_due = None;
        self.gotek_slot_inspection_error = None;

        let Some(device) = self.current_gotek_device() else {
            self.gotek_slot_inspection = None;
            return;
        };
        let limit = self.gotek_slot_limit();
        self.gotek_slot = self.gotek_slot.min(limit.saturating_sub(1));
        let key = (device.clone(), self.gotek_slot);
        if force {
            self.gotek_slot_cache.remove(&key);
        }
        if let Some(cached) = self.gotek_slot_cache.get(&key).cloned() {
            self.gotek_slot_inspection = Some(cached);
            return;
        }

        self.gotek_slot_inspection = None;
        self.gotek_inspection_target = Some(key);
        self.gotek_inspection_due = Some(Instant::now() + delay);
    }

    fn start_gotek_empty_scan(&mut self) {
        self.error = None;
        let Some(device) = self.current_gotek_device() else {
            self.error =
                Some("Select a Gotek device before scanning for an empty slot.".to_owned());
            return;
        };
        let slot_limit = self.gotek_slot_limit();
        if slot_limit <= 1 {
            self.status = "There are no other slots to scan.".to_owned();
            return;
        }

        self.gotek_inspection_generation = self.gotek_inspection_generation.wrapping_add(1);
        let generation = self.gotek_inspection_generation;
        self.gotek_inspection_target = None;
        self.gotek_inspection_due = None;
        self.gotek_slot_inspection_error = None;
        self.gotek_empty_scan = Some(GotekEmptyScan {
            device,
            next_slot: advance_slot_number(self.gotek_slot, slot_limit),
            slot_limit,
            remaining: slot_limit - 1,
            checked: 0,
            generation,
        });
        self.status = "Scanning for the next empty Gotek slot…".to_owned();
        self.advance_gotek_empty_scan();
    }

    fn cancel_gotek_empty_scan(&mut self) {
        if self.gotek_empty_scan.take().is_some() {
            self.gotek_inspection_generation = self.gotek_inspection_generation.wrapping_add(1);
            self.gotek_inspection_target = None;
            self.gotek_inspection_due = None;
            self.status = "Empty-slot scan canceled.".to_owned();
        }
    }

    fn advance_gotek_empty_scan(&mut self) {
        loop {
            let next = {
                let Some(scan) = self.gotek_empty_scan.as_mut() else {
                    return;
                };
                if scan.remaining == 0 {
                    None
                } else {
                    let slot = scan.next_slot;
                    scan.next_slot = advance_slot_number(slot, scan.slot_limit);
                    scan.remaining -= 1;
                    scan.checked += 1;
                    Some((scan.device.clone(), slot, scan.generation, scan.checked))
                }
            };

            let Some((device, slot, generation, _checked)) = next else {
                let checked = self
                    .gotek_empty_scan
                    .as_ref()
                    .map_or(0, |scan| scan.checked);
                self.gotek_empty_scan = None;
                self.gotek_inspection_target = None;
                self.gotek_inspection_due = None;
                self.status = format!("No empty slot found after checking {checked} slot(s).");
                return;
            };

            let key = (device.clone(), slot);
            if let Some(cached) = self.gotek_slot_cache.get(&key).cloned() {
                if slot_is_available(&cached) {
                    self.finish_gotek_empty_scan(cached);
                    return;
                }
                continue;
            }

            self.gotek_inspection_target = Some((device, slot));
            self.gotek_inspection_due = Some(Instant::now());
            self.gotek_inspection_generation = generation;
            return;
        }
    }

    fn finish_gotek_empty_scan(&mut self, inspection: SlotInspection) {
        let checked = self
            .gotek_empty_scan
            .as_ref()
            .map_or(0, |scan| scan.checked);
        self.gotek_slot = inspection.slot;
        self.gotek_slot_inspection = Some(inspection);
        self.gotek_slot_inspection_error = None;
        self.gotek_empty_scan = None;
        self.gotek_inspection_target = None;
        self.gotek_inspection_due = None;
        self.status = format!(
            "Selected empty Gotek slot {} after checking {checked} slot(s).",
            self.gotek_slot
        );
    }

    fn update_gotek_slot_inspection(&mut self, ctx: &egui::Context) {
        while let Ok(message) = self.gotek_inspection_rx.try_recv() {
            self.gotek_inspection_loading = false;
            if message.generation != self.gotek_inspection_generation {
                continue;
            }
            if self.gotek_inspection_target.as_ref()
                == Some(&(message.device.clone(), message.slot))
            {
                self.gotek_inspection_target = None;
            }

            match message.result {
                Ok(inspection) => {
                    self.gotek_slot_cache.insert(
                        (inspection.device.clone(), inspection.slot),
                        inspection.clone(),
                    );
                    let scanning = self.gotek_empty_scan.as_ref().is_some_and(|scan| {
                        scan.generation == message.generation && scan.device == message.device
                    });
                    if scanning {
                        if slot_is_available(&inspection) {
                            self.finish_gotek_empty_scan(inspection);
                        } else {
                            self.advance_gotek_empty_scan();
                        }
                    } else if self.current_gotek_device().as_deref()
                        == Some(message.device.as_path())
                        && self.gotek_slot == message.slot
                    {
                        self.gotek_slot_inspection = Some(inspection);
                    }
                }
                Err(err) => {
                    if self.gotek_empty_scan.as_ref().is_some_and(|scan| {
                        scan.generation == message.generation && scan.device == message.device
                    }) {
                        self.gotek_empty_scan = None;
                        self.error = Some(format!(
                            "Empty-slot scan failed while reading slot {}: {err}",
                            message.slot
                        ));
                        self.status = "Empty-slot scan failed.".to_owned();
                    } else {
                        self.gotek_slot_inspection_error = Some(err);
                    }
                }
            }
        }

        if self.gotek_inspection_loading {
            ctx.request_repaint_after(GOTEK_INSPECTION_POLL);
            return;
        }
        let Some(due) = self.gotek_inspection_due else {
            return;
        };
        let now = Instant::now();
        if due > now {
            ctx.request_repaint_after(due - now);
            return;
        }

        self.gotek_inspection_due = None;
        let Some((device, slot)) = self.gotek_inspection_target.clone() else {
            return;
        };
        let generation = self.gotek_inspection_generation;
        let tx = self.gotek_inspection_tx.clone();
        self.gotek_inspection_loading = true;
        std::thread::spawn(move || {
            let result = inspect_slot(&device, slot).map_err(|err| err.to_string());
            let _ = tx.send(GotekInspectionMessage {
                generation,
                device,
                slot,
                result,
            });
        });
        ctx.request_repaint_after(GOTEK_INSPECTION_POLL);
    }

    fn export_to_gotek_slot(&mut self) {
        self.error = None;
        let Some(root) = self.disk_root.clone() else {
            self.error = Some("Open a disk root folder first.".to_owned());
            return;
        };
        if self.gotek_device_path.trim().is_empty() {
            self.error = Some("Enter a Gotek device path or bank file.".to_owned());
            return;
        }
        let device = PathBuf::from(self.gotek_device_path.trim());
        let slot = self.gotek_slot;
        let image = temp_gotek_image_path(slot);
        let mode = self.gotek_export_mode;
        let options = self.disk_options();
        let inspection = self.gotek_slot_inspection.clone();

        let result = (|| -> anyhow::Result<String> {
            match mode.menu_index() {
                None => {
                    let export = export_single_menu_disk(&root, &options)?;
                    create_designer_disk_image(&root, &export.written_files, &image, None)?;
                    let write = write_slot(&device, slot, &image)?;
                    let verify = verify_slot(&device, slot, &image)?;
                    if !verify.ok {
                        anyhow::bail!(
                            "Gotek slot verification failed: local {} device {}",
                            verify.local_sha256,
                            verify.device_sha256
                        );
                    }
                    Ok(format!(
                        "Exported {} design(s) as a single-menu disk, replaced slot {slot}, wrote {} bytes, and verified.",
                        export.designs.len(),
                        write.bytes
                    ))
                }
                Some(menu_index) => {
                    let current = inspection
                        .as_ref()
                        .filter(|inspection| {
                            inspection.device == device
                                && inspection.slot == slot
                                && inspection.designer1_disk
                        })
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "Inspect the selected complete Designer 1 slot before using targeted menu mode"
                            )
                        })?;
                    let menu_label = current
                        .menu(menu_index)
                        .and_then(|menu| menu.label.clone())
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "Could not read the existing Menu {menu_index} label; refusing to update it without changing MENU_SEL.PHV"
                            )
                        })?;
                    let designs = load_disk_designs(&root, &options)?;
                    let assets = build_menu_assets(menu_index, &designs, &options, &menu_label)?;
                    let replacements = assets
                        .into_iter()
                        .map(|asset| {
                            (
                                asset.relative_path.to_string_lossy().replace('\\', "/"),
                                asset.bytes,
                            )
                        })
                        .collect::<Vec<_>>();
                    let update = create_updated_designer_menu_image(
                        &device,
                        slot,
                        menu_index,
                        &replacements,
                        &image,
                    )?;
                    let write = write_slot(&device, slot, &image)?;
                    let verify = verify_slot(&device, slot, &image)?;
                    if !verify.ok {
                        anyhow::bail!(
                            "Gotek slot verification failed: local {} device {}",
                            verify.local_sha256,
                            verify.device_sha256
                        );
                    }
                    Ok(format!(
                        "Updated Menu {menu_index} ({menu_label}) with {} design(s), preserved {} other file(s), wrote {} bytes to slot {slot}, and verified.",
                        designs.len(),
                        update.preserved_files,
                        write.bytes
                    ))
                }
            }
        })();

        let _ = fs::remove_file(&image);
        match result {
            Ok(status) => {
                self.status = status;
                self.force_gotek_slot_inspection();
            }
            Err(err) => {
                self.error = Some(format!("{err:#}"));
                self.status = "Gotek export failed.".to_owned();
            }
        }
    }

    fn show_sidebar(&mut self, ui: &mut egui::Ui) {
        ui.heading("Options");
        ui.add_space(8.0);
        self.show_input_section(ui);
        ui.separator();
        self.show_conversion_section(ui);

        if self.design.is_some() {
            ui.separator();
            self.show_design_stats_section(ui);
        }
        if !self.disk_designs.is_empty() {
            ui.separator();
            self.show_disk_designs_section(ui);
        }
        if self.disk_root.is_some() {
            ui.separator();
            self.show_gotek_section(ui);
        }

        ui.separator();
        self.show_thumbnail_section(ui);
        ui.separator();
        self.show_status_section(ui);
    }

    fn show_input_section(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Input")
            .default_open(true)
            .show(ui, |ui| {
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
            });
    }

    fn show_conversion_section(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Conversion")
            .default_open(true)
            .show(ui, |ui| {
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

                ui.checkbox(
                    &mut self.show_color_debug,
                    "Show color debug swatch in MHV slot 6",
                );
            });
    }

    fn show_design_stats_section(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Design stats")
            .default_open(true)
            .show(ui, |ui| {
                let Some(design) = &self.design else {
                    return;
                };
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
            });
    }

    fn show_disk_designs_section(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Disk designs")
            .default_open(true)
            .show(ui, |ui| {
                if ui.button("MHV Preview").clicked() {
                    self.show_mhv_preview = true;
                }
                let menu_index = self.gotek_export_mode.menu_index().unwrap_or(1);
                let mut selected = None;
                for (idx, item) in self.disk_designs.iter().enumerate() {
                    let is_selected = self.selected_disk_index == Some(idx);
                    let label = format!("DES{menu_index:02}_{:02} {}", item.slot, item.label);
                    if ui.selectable_label(is_selected, label).clicked() {
                        selected = Some(idx);
                    }
                }
                if let Some(idx) = selected {
                    self.select_disk_design(idx);
                }
            });
    }

    fn show_gotek_section(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Gotek")
            .default_open(true)
            .show(ui, |ui| {
                let mut inspect_delay = None;
                let mut selected_slot = None;
                let mut force_inspect = false;
                let mut start_empty_scan = false;
                let mut cancel_empty_scan = false;

                ui.horizontal(|ui| {
                    ui.label("Device");
                    if ui.button("Refresh").clicked() {
                        self.refresh_gotek_devices();
                    }
                });
                if self.gotek_devices.is_empty() {
                    ui.label("No initialized Gotek devices detected.");
                } else {
                    let selected = self
                        .gotek_devices
                        .iter()
                        .find(|candidate| {
                            gotek_device_paths_match(&candidate.path, Path::new(self.gotek_device_path.trim()))
                        })
                        .map(|candidate| candidate.label.clone())
                        .unwrap_or_else(|| "Select device…".to_owned());
                    egui::ComboBox::from_id_salt("gotek_device")
                        .selected_text(selected)
                        .show_ui(ui, |ui| {
                            for candidate in self.gotek_devices.clone() {
                                if ui
                                    .selectable_label(
                                        gotek_device_paths_match(&candidate.path, Path::new(self.gotek_device_path.trim())),
                                        &candidate.label,
                                    )
                                    .clicked()
                                {
                                    self.gotek_device_path = candidate.path.display().to_string();
                                    inspect_delay = Some(GOTEK_INSPECTION_DEBOUNCE);
                                }
                            }
                        });
                }
                ui.horizontal(|ui| {
                    let response = ui.text_edit_singleline(&mut self.gotek_device_path);
                    if response.changed() {
                        inspect_delay = Some(GOTEK_INSPECTION_DEBOUNCE);
                    } else if response.lost_focus() {
                        inspect_delay = Some(Duration::ZERO);
                    }
                    if ui.button("Browse…").clicked() {
                        self.pick_gotek_device_file();
                    }
                });

                let slot_limit = self.gotek_slot_limit();
                ui.horizontal(|ui| {
                    ui.label("Slot");
                    if ui
                        .add_enabled(self.gotek_slot > 0, egui::Button::new("◀"))
                        .on_hover_text("Previous slot")
                        .clicked()
                    {
                        selected_slot = Some(self.gotek_slot - 1);
                    }
                    if ui
                        .add(
                            egui::DragValue::new(&mut self.gotek_slot)
                                .range(0..=slot_limit.saturating_sub(1)),
                        )
                        .changed()
                    {
                        inspect_delay = Some(GOTEK_INSPECTION_DEBOUNCE);
                    }
                    if ui
                        .add_enabled(
                            self.gotek_slot + 1 < slot_limit,
                            egui::Button::new("▶"),
                        )
                        .on_hover_text("Next slot")
                        .clicked()
                    {
                        selected_slot = Some(self.gotek_slot + 1);
                    }
                    if ui
                        .small_button("↻")
                        .on_hover_text("Reload this slot, ignoring the cache")
                        .clicked()
                    {
                        force_inspect = true;
                    }
                });

                ui.horizontal(|ui| {
                    if let Some(scan) = &self.gotek_empty_scan {
                        ui.label(format!(
                            "Scanning… {} checked",
                            scan.checked
                        ));
                        if ui.small_button("Stop").clicked() {
                            cancel_empty_scan = true;
                        }
                    } else if ui
                        .button("Find next empty")
                        .on_hover_text(
                            "Find the next unformatted, non-Designer, or design-free slot. Cached slots are reused.",
                        )
                        .clicked()
                    {
                        start_empty_scan = true;
                    }
                    if let Some(device) = self.current_gotek_device() {
                        let cached = self
                            .gotek_slot_cache
                            .keys()
                            .filter(|(cached_device, _)| cached_device == &device)
                            .count();
                        if cached > 0 {
                            ui.label(egui::RichText::new(format!("{cached} cached")).weak());
                        }
                    }
                });

                if let Some(slot) = selected_slot {
                    self.select_gotek_slot(slot, GOTEK_INSPECTION_DEBOUNCE);
                } else if force_inspect {
                    self.force_gotek_slot_inspection();
                } else if let Some(delay) = inspect_delay {
                    self.schedule_gotek_slot_inspection(delay);
                }
                if cancel_empty_scan {
                    self.cancel_gotek_empty_scan();
                } else if start_empty_scan {
                    self.start_gotek_empty_scan();
                }

                ui.separator();
                egui::ComboBox::from_label("Upload mode")
                    .selected_text(self.gotek_export_mode.label())
                    .show_ui(ui, |ui| {
                        for mode in GotekExportMode::ALL {
                            ui.selectable_value(&mut self.gotek_export_mode, mode, mode.label());
                        }
                    });
                if let Some(menu_index) = self.gotek_export_mode.menu_index() {
                    ui.label(
                        egui::RichText::new(format!(
                            "Only MENU_{menu_index:02} will be replaced; all other slot files are preserved."
                        ))
                        .weak(),
                    );
                } else {
                    ui.label(
                        egui::RichText::new(
                            "Build a new single-menu disk and replace the entire slot."
                        )
                        .weak(),
                    );
                }

                self.show_gotek_slot_info(ui);

                let targeted_ready = self.gotek_export_mode.menu_index().is_none()
                    || self.gotek_slot_inspection.as_ref().is_some_and(|inspection| {
                        inspection.designer1_disk
                            && inspection.slot == self.gotek_slot
                            && self.current_gotek_device().as_deref()
                                == Some(inspection.device.as_path())
                    });
                let export_button = ui
                    .add_enabled(targeted_ready, egui::Button::new("Export To Gotek Slot"))
                    .on_hover_text(if targeted_ready {
                        "Write and verify the selected Gotek slot"
                    } else {
                        "Wait for a complete Designer 1 slot to be inspected, or choose Single menu mode"
                    });
                if export_button.clicked() {
                    self.export_to_gotek_slot();
                }
            });
    }

    fn show_gotek_slot_info(&self, ui: &mut egui::Ui) {
        let expected = self
            .disk_designs
            .iter()
            .map(|design| design.label.clone())
            .collect::<Vec<_>>();
        let target_menu = self.gotek_export_mode.menu_index();
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                let scope = target_menu
                    .map(|index| format!("Menu {index}"))
                    .unwrap_or_else(|| "whole slot".to_owned());
                ui.strong(format!("Slot {} — {scope}", self.gotek_slot));

                if let Some(scan) = &self.gotek_empty_scan {
                    let target = self
                        .gotek_inspection_target
                        .as_ref()
                        .map(|(_, slot)| *slot)
                        .unwrap_or(scan.next_slot);
                    ui.label(
                        egui::RichText::new(format!(
                            "Scanning slot {target} for availability ({} checked)…",
                            scan.checked
                        ))
                        .weak(),
                    );
                }
                if let Some(err) = &self.gotek_slot_inspection_error {
                    ui.colored_label(ui.visuals().warn_fg_color, format!("Could not inspect: {err}"));
                    return;
                }
                let Some(inspection) = &self.gotek_slot_inspection else {
                    if self.gotek_inspection_loading {
                        ui.label(egui::RichText::new("Loading slot information…").weak());
                    } else if self.gotek_inspection_due.is_some() {
                        ui.label(
                            egui::RichText::new("Waiting for slot selection to settle…").weak(),
                        );
                    } else {
                        ui.label(
                            egui::RichText::new("Select or inspect a device and slot.").weak(),
                        );
                    }
                    return;
                };

                if !inspection.valid_fat12_1440 {
                    ui.label(
                        egui::RichText::new(
                            "No valid 1.44 MB FAT12 disk was detected; Single menu mode can replace it."
                        )
                        .weak(),
                    );
                } else if !inspection.designer1_disk {
                    ui.label(
                        egui::RichText::new(
                            "This is not a complete Designer 1 menu disk; Single menu mode can replace it."
                        )
                        .weak(),
                    );
                } else {
                    let label = inspection.label.as_deref().unwrap_or("unlabeled");
                    ui.label(format!("Designer 1 disk: {label}"));
                }

                let existing_names = if let Some(menu_index) = target_menu {
                    let menu = inspection.menu(menu_index);
                    if let Some(label) = menu.and_then(|menu| menu.label.as_deref()) {
                        ui.label(format!("Menu {menu_index} label: {label}"));
                    }
                    let names = menu
                        .map(|menu| menu.design_names.as_slice())
                        .unwrap_or(&[]);
                    ui.label(format!("{} current design(s) in Menu {menu_index}", names.len()));
                    if inspection.designer1_disk {
                        ui.label(
                            egui::RichText::new(format!(
                                "Menus other than Menu {menu_index}, MENU_SEL.PHV, and other root files will be preserved."
                            ))
                            .weak(),
                        );
                    } else {
                        ui.colored_label(
                            ui.visuals().warn_fg_color,
                            "Targeted menu upload requires an existing complete Designer 1 disk.",
                        );
                    }
                    names
                } else {
                    ui.label(format!("{} current design(s) on the slot", inspection.design_names.len()));
                    inspection.design_names.as_slice()
                };

                let comparison = compare_design_names(existing_names, &expected);
                show_slot_design_group(ui, "Will be replaced", &comparison.replaced, None);
                show_slot_design_group(ui, "Will be added", &comparison.added, None);
                let removed_heading = target_menu
                    .map(|index| format!("Only in Menu {index} — will be removed"))
                    .unwrap_or_else(|| "Only on current slot — will be removed".to_owned());
                show_slot_design_group(
                    ui,
                    &removed_heading,
                    &comparison.removed,
                    Some(ui.visuals().warn_fg_color),
                );

                if inspection.designer1_disk && comparison.removed.is_empty() {
                    ui.label(
                        egui::RichText::new("No unexpected designs will be removed from this scope.")
                            .weak(),
                    );
                }
            });
    }

    fn show_thumbnail_section(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Embedded SHV thumbnail preview")
            .default_open(true)
            .show(ui, |ui| {
                if let (Some(preview), Some(report)) = (&self.preview_bytes, &self.report) {
                    ui.label(format!(
                        "{} x {} px",
                        report.preview_width, report.preview_height
                    ));
                    draw_shv_preview(
                        ui,
                        preview,
                        report.preview_width as usize,
                        report.preview_height as usize,
                    );
                } else {
                    ui.label("No preview built.");
                }
            });
    }

    fn show_status_section(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Status")
            .default_open(true)
            .show(ui, |ui| {
                ui.label(&self.status);
                if let Some(error) = &self.error {
                    ui.colored_label(Color32::from_rgb(190, 40, 40), error);
                }
            });
    }

    fn show_mhv_preview_window(&mut self, ctx: &egui::Context) {
        if !self.show_mhv_preview {
            return;
        }

        let menu_index = self.gotek_export_mode.menu_index().unwrap_or(1);
        let preview_title = format!("MENU_{menu_index:02}.MHV Preview");
        egui::Window::new(preview_title)
            .open(&mut self.show_mhv_preview)
            .default_size(Vec2::new(560.0, 620.0))
            .resizable(true)
            .show(ctx, |ui| {
                if self.disk_designs.is_empty() {
                    ui.label(format!(
                        "Open a folder to preview MENU_{menu_index:02}.MHV."
                    ));
                    return;
                }

                match render_mhv_preview_pixels(&self.disk_designs, self.show_color_debug) {
                    Ok(pixels) => draw_mhv_preview(ui, &pixels),
                    Err(err) => {
                        ui.colored_label(
                            Color32::from_rgb(190, 40, 40),
                            format!("Failed to render MHV preview: {err}"),
                        );
                    }
                }
            });
    }

    fn remember_open_dir(&mut self, path: &Path) {
        if path.is_dir() {
            self.last_open_dir = path.to_path_buf();
            let _ = save_last_open_dir(path);
        }
    }
}

fn advance_slot_number(slot: u16, slot_limit: u16) -> u16 {
    if slot + 1 < slot_limit { slot + 1 } else { 0 }
}

fn slot_is_available(inspection: &SlotInspection) -> bool {
    !inspection.valid_fat12_1440 || !inspection.designer1_disk || inspection.design_names.is_empty()
}

#[derive(Debug, Default, PartialEq, Eq)]
struct GotekSlotComparison {
    replaced: Vec<String>,
    added: Vec<String>,
    removed: Vec<String>,
}

fn compare_design_names(existing: &[String], expected: &[String]) -> GotekSlotComparison {
    let existing = design_name_map(existing);
    let expected = design_name_map(expected);
    let mut comparison = GotekSlotComparison::default();

    for (key, expected_names) in &expected {
        let existing_count = existing.get(key).map_or(0, Vec::len);
        let replaced_count = existing_count.min(expected_names.len());
        comparison
            .replaced
            .extend(expected_names[..replaced_count].iter().cloned());
        comparison
            .added
            .extend(expected_names[replaced_count..].iter().cloned());
    }
    for (key, existing_names) in &existing {
        let expected_count = expected.get(key).map_or(0, Vec::len);
        let replaced_count = expected_count.min(existing_names.len());
        comparison
            .removed
            .extend(existing_names[replaced_count..].iter().cloned());
    }

    comparison
        .replaced
        .sort_by_key(|name| name.to_ascii_uppercase());
    comparison
        .added
        .sort_by_key(|name| name.to_ascii_uppercase());
    comparison
        .removed
        .sort_by_key(|name| name.to_ascii_uppercase());
    comparison
}

fn design_name_map(names: &[String]) -> BTreeMap<String, Vec<String>> {
    let mut grouped = BTreeMap::<String, Vec<String>>::new();
    for name in names {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        grouped
            .entry(name.to_ascii_uppercase())
            .or_default()
            .push(name.to_owned());
    }
    grouped
}

fn show_slot_design_group(
    ui: &mut egui::Ui,
    heading: &str,
    designs: &[String],
    color: Option<Color32>,
) {
    if designs.is_empty() {
        return;
    }
    ui.add_space(4.0);
    let heading = egui::RichText::new(format!("{heading} ({})", designs.len())).strong();
    if let Some(color) = color {
        ui.label(heading.color(color));
        for design in designs {
            ui.label(egui::RichText::new(format!("• {design}")).color(color));
        }
    } else {
        ui.label(heading);
        for design in designs {
            ui.label(format!("• {design}"));
        }
    }
}
fn initial_open_dir() -> PathBuf {
    load_last_open_dir()
        .filter(|path| path.is_dir())
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(target_os = "windows")]
fn settings_path() -> Option<PathBuf> {
    windows_settings_path(std::env::var_os("LOCALAPPDATA"))
}

#[cfg(target_os = "windows")]
fn windows_settings_path(local_app_data: Option<std::ffi::OsString>) -> Option<PathBuf> {
    local_app_data
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|path| path.join(SETTINGS_DIR).join(SETTINGS_FILE))
}

#[cfg(not(target_os = "windows"))]
fn settings_path() -> Option<PathBuf> {
    let cache_home = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".local").join("cache"))
        })?;
    Some(cache_home.join(SETTINGS_DIR).join(SETTINGS_FILE))
}

fn load_last_open_dir() -> Option<PathBuf> {
    load_last_open_dir_from(&settings_path()?)
}

fn load_last_open_dir_from(settings_path: &Path) -> Option<PathBuf> {
    let contents = fs::read_to_string(settings_path).ok()?;
    for line in contents.lines() {
        let line = line.trim();
        let Some(value) = line.strip_prefix("last_open_dir") else {
            continue;
        };
        let Some(value) = value.trim_start().strip_prefix('=') else {
            continue;
        };
        let value = parse_toml_string(value.trim())?;
        return Some(PathBuf::from(value));
    }
    None
}

fn save_last_open_dir(path: &Path) -> std::io::Result<()> {
    let Some(settings_path) = settings_path() else {
        return Ok(());
    };
    save_last_open_dir_to(&settings_path, path)
}

fn save_last_open_dir_to(settings_path: &Path, path: &Path) -> std::io::Result<()> {
    if let Some(parent) = settings_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        settings_path,
        format!(
            "last_open_dir = \"{}\"\n",
            escape_toml_string(&path.display().to_string())
        ),
    )
}

fn parse_toml_string(value: &str) -> Option<String> {
    let mut chars = value.chars();
    if chars.next()? != '"' {
        return None;
    }
    let mut out = String::new();
    let mut escaped = false;
    for ch in chars {
        if escaped {
            out.push(match ch {
                '"' => '"',
                '\\' => '\\',
                'n' => '\n',
                't' => '\t',
                other => other,
            });
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return Some(out);
        } else {
            out.push(ch);
        }
    }
    None
}

fn escape_toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn temp_gotek_image_path(slot: u16) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!(
        "designer1-gotek-slot-{slot:03}-{}-{nonce}.img",
        std::process::id()
    ))
}

fn draw_design_path(
    ui: &mut egui::Ui,
    design: &Design,
    show_cm_grid: &mut bool,
    show_jumps: &mut bool,
    zoom: &mut f32,
    pan: &mut Vec2,
) {
    let desired = ui.available_size().max(Vec2::new(240.0, 240.0));
    let (rect, response) = ui.allocate_exact_size(desired, Sense::drag());
    let painter = ui.painter_at(rect);
    let visuals = ui.visuals();
    painter.rect_filled(rect, 6.0, visuals.extreme_bg_color);
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

    egui::Area::new(ui.id().with("path_view_toolbar"))
        .order(egui::Order::Foreground)
        .fixed_pos(rect.right_top() + Vec2::new(-8.0, 8.0))
        .pivot(egui::Align2::RIGHT_TOP)
        .show(ui.ctx(), |ui| {
            egui::Frame::NONE
                .fill(visuals.window_fill().linear_multiply(0.92))
                .stroke(visuals.window_stroke())
                .corner_radius(6.0)
                .inner_margin(egui::Margin::symmetric(6, 4))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui.selectable_label(*show_jumps, "Show Jumps").clicked() {
                            *show_jumps = !*show_jumps;
                        }
                        if ui.selectable_label(*show_cm_grid, "Show Grid").clicked() {
                            *show_cm_grid = !*show_cm_grid;
                        }
                        if ui.button("−").clicked() {
                            *zoom = (*zoom / 1.25).clamp(0.1, 20.0);
                        }
                        if ui.button("+").clicked() {
                            *zoom = (*zoom * 1.25).clamp(0.1, 20.0);
                        }
                        if ui.button("Fit").clicked() {
                            *zoom = 1.0;
                            *pan = Vec2::ZERO;
                        }
                    });
                });
        });

    if *show_cm_grid {
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
                if *show_jumps {
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

fn draw_mhv_preview(ui: &mut egui::Ui, pixels: &[u8]) {
    let native = Vec2::new(MHV_PREVIEW_HEIGHT as f32, MHV_PREVIEW_WIDTH as f32);
    let available = ui.available_size().max(Vec2::new(160.0, 160.0));
    let scale = (available.x / native.x)
        .min(available.y / native.y)
        .max(0.25);
    let desired = native * scale;
    let (rect, _response) = ui.allocate_exact_size(desired, Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, mhv_preview_color(0));

    let sx = rect.width() / MHV_PREVIEW_HEIGHT as f32;
    let sy = rect.height() / MHV_PREVIEW_WIDTH as f32;
    for y in 0..MHV_PREVIEW_HEIGHT {
        for x in 0..MHV_PREVIEW_WIDTH {
            let value = pixels[y * MHV_PREVIEW_WIDTH + x];
            if value == 0 {
                continue;
            }
            let preview_x = y;
            let preview_y = MHV_PREVIEW_WIDTH - 1 - x;
            let color = mhv_preview_color(value);
            let cell = Rect::from_min_size(
                Pos2::new(
                    rect.left() + preview_x as f32 * sx,
                    rect.top() + preview_y as f32 * sy,
                ),
                Vec2::new(sx.max(1.0), sy.max(1.0)),
            );
            painter.rect_filled(cell, 0.0, color);
        }
    }
}

fn mhv_preview_color(value: u8) -> Color32 {
    let [r, g, b] = DESIGNER1_PALETTE
        .get(value as usize)
        .copied()
        .unwrap_or([30, 30, 30]);
    Color32::from_rgb(r, g, b)
}

fn draw_shv_preview(ui: &mut egui::Ui, preview_bytes: &[u8], width: usize, height: usize) {
    let pixels = decode_4bpp_preview(preview_bytes, width, height);
    let cell = 5.0f32;
    let native = Vec2::new(height as f32 * cell, width as f32 * cell);
    let max_size = Vec2::new(ui.available_width(), 260.0);
    let scale = (max_size.x / native.x)
        .min(max_size.y / native.y)
        .min(1.0)
        .max(0.0);
    let desired = native * scale;
    let (rect, _response) = ui.allocate_exact_size(desired, Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, mhv_preview_color(0));

    let sx = rect.width() / height as f32;
    let sy = rect.height() / width as f32;
    for y in 0..height {
        for x in 0..width {
            let value = pixels[y * width + x];
            if value == 0 {
                continue;
            }
            let color = mhv_preview_color(value);
            let preview_x = y;
            let preview_y = width - 1 - x;
            let r = Rect::from_min_size(
                Pos2::new(
                    rect.left() + preview_x as f32 * sx,
                    rect.top() + preview_y as f32 * sy,
                ),
                Vec2::new(sx.max(1.0), sy.max(1.0)),
            );
            painter.rect_filled(r, 0.0, color);
        }
    }
}

fn decode_4bpp_preview(preview_bytes: &[u8], width: usize, height: usize) -> Vec<u8> {
    let stride = width.div_ceil(2);
    let mut pixels = vec![0u8; width * height];
    for y in 0..height {
        for x in 0..stride {
            let byte = preview_bytes.get(y * stride + x).copied().unwrap_or(0);
            let px = x * 2;
            if px < width {
                pixels[y * width + px] = (byte >> 4) & 0x0f;
            }
            if px + 1 < width {
                pixels[y * width + px + 1] = byte & 0x0f;
            }
        }
    }
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_path_string_round_trips_quotes_and_backslashes() {
        let path = r#"/tmp/designer "one"\folder"#;
        let encoded = format!("\"{}\"", escape_toml_string(path));
        assert_eq!(parse_toml_string(&encoded).as_deref(), Some(path));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_settings_use_local_app_data() {
        let path = windows_settings_path(Some(r"C:\Users\Test User\AppData\Local".into())).unwrap();
        assert_eq!(
            path,
            PathBuf::from(r"C:\Users\Test User\AppData\Local\designer1-rs\gui.toml")
        );
        assert_eq!(windows_settings_path(None), None);
        assert_eq!(windows_settings_path(Some("".into())), None);
    }

    #[test]
    fn persisted_directory_round_trips_windows_paths() {
        let dir = std::env::temp_dir().join(format!(
            "designer1-settings-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let settings = dir.join("nested/gui.toml");
        let path = PathBuf::from(r#"D:\刺繍 designs\folder "one""#);
        assert_eq!(load_last_open_dir_from(&settings), None);
        save_last_open_dir_to(&settings, &path).unwrap();
        assert_eq!(load_last_open_dir_from(&settings), Some(path));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn design_comparison_classifies_replacements_additions_and_removals() {
        let existing = vec![
            "Flowers".to_owned(),
            "Bird".to_owned(),
            "Bird".to_owned(),
            "Old Design".to_owned(),
        ];
        let expected = vec![
            "flowers".to_owned(),
            "Bird".to_owned(),
            "New Design".to_owned(),
        ];

        let comparison = compare_design_names(&existing, &expected);
        assert_eq!(
            comparison.replaced,
            vec!["Bird".to_owned(), "flowers".to_owned()]
        );
        assert_eq!(comparison.added, vec!["New Design".to_owned()]);
        assert_eq!(
            comparison.removed,
            vec!["Bird".to_owned(), "Old Design".to_owned()]
        );
    }

    #[test]
    fn slot_navigation_wraps_for_scans() {
        assert_eq!(advance_slot_number(0, 3), 1);
        assert_eq!(advance_slot_number(2, 3), 0);
    }

    #[test]
    fn availability_includes_unformatted_non_designer_and_design_free_slots() {
        let inspection =
            |valid_fat12_1440, designer1_disk, design_names: Vec<&str>| SlotInspection {
                device: PathBuf::from("bank.img"),
                slot: 1,
                valid_fat12_1440,
                designer1_disk,
                label: None,
                files: Vec::new(),
                design_names: design_names.into_iter().map(str::to_owned).collect(),
                menus: Vec::new(),
            };

        assert!(slot_is_available(&inspection(false, false, vec![])));
        assert!(slot_is_available(&inspection(true, false, vec![])));
        assert!(slot_is_available(&inspection(true, true, vec![])));
        assert!(!slot_is_available(&inspection(true, true, vec!["Flowers"])));
    }

    #[test]
    fn cached_slot_is_reused_until_forced_refresh() {
        let mut app = Designer1App {
            gotek_device_path: "bank.img".to_owned(),
            gotek_slot: 7,
            ..Default::default()
        };
        let cached = SlotInspection {
            device: PathBuf::from("bank.img"),
            slot: 7,
            valid_fat12_1440: true,
            designer1_disk: true,
            label: Some("DESIGNER1".to_owned()),
            files: Vec::new(),
            design_names: vec!["Flowers".to_owned()],
            menus: Vec::new(),
        };
        app.gotek_slot_cache
            .insert((cached.device.clone(), cached.slot), cached.clone());

        app.schedule_gotek_slot_inspection(GOTEK_INSPECTION_DEBOUNCE);
        assert_eq!(
            app.gotek_slot_inspection
                .as_ref()
                .map(|inspection| &inspection.design_names),
            Some(&cached.design_names)
        );
        assert!(app.gotek_inspection_due.is_none());

        app.force_gotek_slot_inspection();
        assert!(app.gotek_slot_inspection.is_none());
        assert!(app.gotek_inspection_due.is_some());
        assert!(
            !app.gotek_slot_cache
                .contains_key(&(PathBuf::from("bank.img"), 7))
        );
    }
}
