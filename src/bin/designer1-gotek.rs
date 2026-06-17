use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use designer1_tools::gotek::{
    GotekOptions, check_gotek_device, create_blank_image, init_workspace, inspect_image,
    pack_workspace, read_slot, verify_slot, verify_workspace_slots, write_slot,
    write_workspace_slots,
};
use serde::Serialize;
use std::path::PathBuf;

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Init(args) => {
            init_workspace(&args.root)?;
            print_report(
                cli.json,
                &SimpleStatus::ok(format!(
                    "initialized Gotek workspace at {}",
                    args.root.display()
                )),
            )
        }
        Command::Mkimg(args) => {
            create_blank_image(&args.output, args.label.as_deref())?;
            print_report(
                cli.json,
                &SimpleStatus::ok(format!(
                    "created FAT12 floppy image at {}",
                    args.output.display()
                )),
            )
        }
        Command::Pack(args) => {
            let report = pack_workspace(&GotekOptions { root: args.root })?;
            print_report(cli.json, &report)
        }
        Command::InspectImage(args) => {
            let report = inspect_image(&args.image)?;
            print_report(cli.json, &report)
        }
        Command::CheckDevice(args) => {
            let report = check_gotek_device(&args.device)?;
            print_report(cli.json, &report)
        }
        Command::ReadSlot(args) => {
            let report = read_slot(&args.device, args.slot, &args.output)?;
            print_report(cli.json, &report)
        }
        Command::WriteSlot(args) => {
            args.require_write_confirmation()?;
            let report = write_slot(&args.device, args.slot, &args.image)?;
            print_report(cli.json, &report)
        }
        Command::VerifySlot(args) => {
            let report = verify_slot(&args.device, args.slot, &args.image)?;
            print_report(cli.json, &report)?;
            if report.ok {
                Ok(())
            } else {
                bail!("slot {} verification failed", report.slot)
            }
        }
        Command::Write(args) => {
            args.require_write_confirmation()?;
            let report = write_workspace_slots(
                &args.device,
                &GotekOptions {
                    root: args.common.root,
                },
                &args.slots,
            )?;
            print_report(cli.json, &report)
        }
        Command::Verify(args) => {
            let report = verify_workspace_slots(
                &args.device,
                &GotekOptions {
                    root: args.common.root,
                },
                &args.slots,
            )?;
            print_report(cli.json, &report)?;
            if report.ok {
                Ok(())
            } else {
                bail!("one or more slots failed verification")
            }
        }
        Command::Sync(args) => {
            args.require_write_confirmation()?;
            let options = GotekOptions {
                root: args.common.root.clone(),
            };
            let pack = pack_workspace(&options)?;
            let write = write_workspace_slots(&args.device, &options, &args.slots)?;
            print_report(cli.json, &SyncReport { pack, write })
        }
    }
}

#[derive(Debug, Parser)]
#[command(version, about = "Cross-platform Gotek slot image manager")]
struct Cli {
    #[arg(long, global = true, help = "Print machine-readable JSON")]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Initialize a managed Gotek workspace.
    Init(RootArgs),
    /// Create a blank 1.44MB FAT12 floppy image.
    Mkimg(MkimgArgs),
    /// Pack managed slot folders into .floppy.img files.
    Pack(RootArgs),
    /// Inspect a 1.44MB FAT12 floppy image.
    InspectImage(InspectImageArgs),
    /// Check whether a raw device or bank file looks like initialized Gotek media.
    CheckDevice(DeviceArgs),
    /// Read one fixed Gotek slot from a raw device or bank file.
    ReadSlot(ReadSlotArgs),
    /// Write one 1.44MB image to one fixed Gotek slot.
    WriteSlot(WriteSlotArgs),
    /// Verify one fixed Gotek slot against an image.
    VerifySlot(VerifySlotArgs),
    /// Write changed managed workspace slots.
    Write(WorkspaceDeviceArgs),
    /// Verify managed workspace slots.
    Verify(VerifyWorkspaceArgs),
    /// Pack managed images, then write changed slots.
    Sync(SyncArgs),
}

#[derive(Debug, Args)]
struct RootArgs {
    #[arg(long, default_value = "gotek")]
    root: PathBuf,
}

#[derive(Debug, Args)]
struct MkimgArgs {
    output: PathBuf,
    #[arg(long)]
    label: Option<String>,
}

#[derive(Debug, Args)]
struct InspectImageArgs {
    image: PathBuf,
}

#[derive(Debug, Args)]
struct DeviceArgs {
    device: PathBuf,
}

#[derive(Debug, Args)]
struct ReadSlotArgs {
    device: PathBuf,
    slot: u16,
    output: PathBuf,
}

#[derive(Debug, Args)]
struct WriteSlotArgs {
    device: PathBuf,
    slot: u16,
    image: PathBuf,
    #[arg(long, help = "Required for raw writes")]
    confirm_device: bool,
}

impl WriteSlotArgs {
    fn require_write_confirmation(&self) -> Result<()> {
        require_write_confirmation(self.confirm_device, &self.device)
    }
}

#[derive(Debug, Args)]
struct VerifySlotArgs {
    device: PathBuf,
    slot: u16,
    image: PathBuf,
}

#[derive(Debug, Args)]
struct WorkspaceDeviceArgs {
    #[command(flatten)]
    common: RootArgs,
    device: PathBuf,
    #[arg(help = "Optional slot numbers such as 1 2 003")]
    slots: Vec<u16>,
    #[arg(long, help = "Required for raw writes")]
    confirm_device: bool,
}

impl WorkspaceDeviceArgs {
    fn require_write_confirmation(&self) -> Result<()> {
        require_write_confirmation(self.confirm_device, &self.device)
    }
}

#[derive(Debug, Args)]
struct VerifyWorkspaceArgs {
    #[command(flatten)]
    common: RootArgs,
    device: PathBuf,
    #[arg(help = "Optional slot numbers such as 1 2 003")]
    slots: Vec<u16>,
}

#[derive(Debug, Args)]
struct SyncArgs {
    #[command(flatten)]
    common: RootArgs,
    device: PathBuf,
    #[arg(help = "Optional slot numbers such as 1 2 003")]
    slots: Vec<u16>,
    #[arg(long, help = "Required for raw writes")]
    confirm_device: bool,
}

impl SyncArgs {
    fn require_write_confirmation(&self) -> Result<()> {
        require_write_confirmation(self.confirm_device, &self.device)
    }
}

#[derive(Debug, Serialize)]
struct SimpleStatus {
    status: &'static str,
    message: String,
}

impl SimpleStatus {
    fn ok(message: String) -> Self {
        Self {
            status: "ok",
            message,
        }
    }
}

#[derive(Debug, Serialize)]
struct SyncReport<T, U> {
    pack: T,
    write: U,
}

fn require_write_confirmation(confirm: bool, device: &PathBuf) -> Result<()> {
    if confirm {
        Ok(())
    } else {
        bail!(
            "refusing to write {}; pass --confirm-device after verifying this is the Gotek USB device",
            device.display()
        )
    }
}

fn print_report<T: Serialize + std::fmt::Debug>(json: bool, report: &T) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
    } else {
        print_human(report).context("printing report")?;
    }
    Ok(())
}

fn print_human<T: Serialize + std::fmt::Debug>(report: &T) -> Result<()> {
    let value = serde_json::to_value(report)?;
    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                println!("{}", serde_json::to_string_pretty(&item)?);
            }
        }
        other => println!("{}", serde_json::to_string_pretty(&other)?),
    }
    Ok(())
}
