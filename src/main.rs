mod bar;
mod config;
mod icons;
mod logger;
mod niri;
mod render;
mod tray;

fn main() -> anyhow::Result<()> {
    logger::init();

    let config = config::Config::load_from_args(std::env::args_os().skip(1))?;
    bar::run(config)
}
