mod bar;
mod config;
mod control;
mod icons;
mod logger;
mod niri;
mod render;
mod tray;

fn main() -> anyhow::Result<()> {
    logger::init();

    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if let Some(command) = args
        .first()
        .and_then(|arg| control::VisibilityCommand::from_arg(arg))
    {
        anyhow::ensure!(
            args.len() == 1,
            "--hide, --show, and --toggle must be used separately"
        );
        return command.send();
    }

    let config = config::Config::load_from_args(args.into_iter())?;
    bar::run(config)
}
