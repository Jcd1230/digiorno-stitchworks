fn main() -> anyhow::Result<()> {
    let mut args: Vec<_> = std::env::args_os().collect();
    let launch_gui = matches!(args.get(1).and_then(|arg| arg.to_str()), Some("--gui"));

    if launch_gui {
        args.remove(1);
        designer1_tools::gui::run().map_err(anyhow::Error::from)
    } else {
        designer1_tools::cli::run_from(args)
    }
}
