use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use designer1_tools::disk::{DiskExportOptions, export_single_menu_disk};
use designer1_tools::inkstitch::{LoadOptions, load_inkstitch_json_file};
use designer1_tools::model::{InputYAxis, SignatureMode};
use designer1_tools::preview::design_path_svg;
use designer1_tools::shv::{ShvOptions, build_shv, validate_generated_shv};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "designer1-cli")]
#[command(about = "Experimental Ink/Stitch JSON to Husqvarna/Viking Designer 1 SHV converter")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Convert Ink/Stitch JSON into an SHV design file.
    Convert(ConvertArgs),
    /// Read JSON, normalize it, and print design statistics.
    Inspect(InspectArgs),
    /// Write a quick SVG preview of the normalized stitch path.
    PreviewSvg(PreviewSvgArgs),
    /// Validate/read back a generated SHV file.
    ValidateShv(ValidateShvArgs),
    /// Export a folder of JSON files as a single-menu Designer 1 disk layout.
    ExportDisk(ExportDiskArgs),
}

#[derive(Debug, Args)]
struct CommonInputArgs {
    /// Ink/Stitch JSON file.
    input: PathBuf,

    /// Coordinate scale into SHV units. Default assumes Ink/Stitch JSON units are already 0.1 mm-ish.
    #[arg(long, default_value_t = 1.0)]
    scale: f64,

    /// Do not center the design around SHV origin.
    #[arg(long)]
    no_center: bool,

    /// Y-axis convention of input JSON. Ink/Stitch/SVG normally uses +Y down.
    #[arg(long, value_enum, default_value_t = YAxisArg::Down)]
    input_y_axis: YAxisArg,
}

impl CommonInputArgs {
    fn load_options(&self) -> LoadOptions {
        LoadOptions {
            scale: self.scale,
            center: !self.no_center,
            input_y_axis: self.input_y_axis.into(),
        }
    }
}

#[derive(Debug, Args)]
struct ConvertArgs {
    #[command(flatten)]
    common: CommonInputArgs,

    /// Output SHV file.
    #[arg(short, long)]
    output: PathBuf,

    /// Override internal SHV design name.
    #[arg(long)]
    name: Option<String>,

    /// Signature/notice region to write.
    #[arg(long, value_enum, default_value_t = SignatureArg::Official)]
    signature: SignatureArg,

    /// Embedded preview width in pixels.
    #[arg(long, default_value_t = 96)]
    preview_width: u8,

    /// Embedded preview height in pixels.
    #[arg(long, default_value_t = 24)]
    preview_height: u8,

    /// Optional path to write a JSON readback validation report.
    #[arg(long)]
    validation_report: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct InspectArgs {
    #[command(flatten)]
    common: CommonInputArgs,
}

#[derive(Debug, Args)]
struct PreviewSvgArgs {
    #[command(flatten)]
    common: CommonInputArgs,

    /// Output SVG file.
    #[arg(short, long)]
    output: PathBuf,

    #[arg(long, default_value_t = 1000.0)]
    width: f64,

    #[arg(long, default_value_t = 300.0)]
    height: f64,
}

#[derive(Debug, Args)]
struct ValidateShvArgs {
    /// Generated SHV file to read back.
    input: PathBuf,
}

#[derive(Debug, Args)]
struct ExportDiskArgs {
    /// Disk root folder containing Ink/Stitch JSON files.
    root: PathBuf,

    /// Coordinate scale into SHV units. Default assumes Ink/Stitch JSON units are already 0.1 mm-ish.
    #[arg(long, default_value_t = 1.0)]
    scale: f64,

    /// Do not center each design around SHV origin.
    #[arg(long)]
    no_center: bool,

    /// Y-axis convention of input JSON. Ink/Stitch/SVG normally uses +Y down.
    #[arg(long, value_enum, default_value_t = YAxisArg::Down)]
    input_y_axis: YAxisArg,

    /// Signature/notice region to write.
    #[arg(long, value_enum, default_value_t = SignatureArg::Official)]
    signature: SignatureArg,

    /// Root disk title used in MENU_SEL.PHV.
    #[arg(long, default_value = "Designer 1 Disk")]
    disk_title: String,

    /// Root menu label used in MENU_SEL.PHV.
    #[arg(long, default_value = "Menu 1")]
    menu_label: String,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum YAxisArg {
    Down,
    Up,
}

impl From<YAxisArg> for InputYAxis {
    fn from(value: YAxisArg) -> Self {
        match value {
            YAxisArg::Down => Self::Down,
            YAxisArg::Up => Self::Up,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SignatureArg {
    Official,
    Zero,
}

impl From<SignatureArg> for SignatureMode {
    fn from(value: SignatureArg) -> Self {
        match value {
            SignatureArg::Official => Self::Official,
            SignatureArg::Zero => Self::Zero,
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Convert(args) => convert(args),
        Command::Inspect(args) => inspect(args),
        Command::PreviewSvg(args) => preview_svg(args),
        Command::ValidateShv(args) => validate_shv(args),
        Command::ExportDisk(args) => export_disk(args),
    }
}

fn convert(args: ConvertArgs) -> Result<()> {
    if args.preview_width == 0 || args.preview_height == 0 {
        bail!("preview dimensions must be non-zero");
    }

    let mut design = load_inkstitch_json_file(&args.common.input, &args.common.load_options())?;
    if let Some(name) = &args.name {
        design.name = name.clone();
    }

    let shv = build_shv(
        &design,
        &ShvOptions {
            name: None,
            signature: args.signature.into(),
            preview_width: args.preview_width,
            preview_height: args.preview_height,
        },
    )?;
    let report = validate_generated_shv(&shv)?;

    if let Some(parent) = args.output.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating output directory {}", parent.display()))?;
        }
    }
    std::fs::write(&args.output, &shv)
        .with_context(|| format!("writing SHV {}", args.output.display()))?;

    if let Some(path) = args.validation_report {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating report directory {}", parent.display()))?;
            }
        }
        let json = serde_json::to_vec_pretty(&report)?;
        std::fs::write(&path, json).with_context(|| format!("writing {}", path.display()))?;
    }

    println!("Wrote {}", args.output.display());
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn inspect(args: InspectArgs) -> Result<()> {
    let design = load_inkstitch_json_file(&args.common.input, &args.common.load_options())?;
    let stats = design.stats();
    println!("{}", serde_json::to_string_pretty(&stats)?);
    Ok(())
}

fn preview_svg(args: PreviewSvgArgs) -> Result<()> {
    let design = load_inkstitch_json_file(&args.common.input, &args.common.load_options())?;
    let svg = design_path_svg(&design, args.width, args.height);
    std::fs::write(&args.output, svg)
        .with_context(|| format!("writing SVG {}", args.output.display()))?;
    println!("Wrote {}", args.output.display());
    Ok(())
}

fn validate_shv(args: ValidateShvArgs) -> Result<()> {
    let blob =
        std::fs::read(&args.input).with_context(|| format!("reading {}", args.input.display()))?;
    let report = validate_generated_shv(&blob)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn export_disk(args: ExportDiskArgs) -> Result<()> {
    let report = export_single_menu_disk(
        &args.root,
        &DiskExportOptions {
            signature: args.signature.into(),
            scale: args.scale,
            center: !args.no_center,
            input_y_axis: args.input_y_axis.into(),
            disk_title: args.disk_title,
            menu_label: args.menu_label,
        },
    )?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
