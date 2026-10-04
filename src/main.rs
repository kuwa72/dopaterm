mod app;
mod colors;
mod config;
mod fx;
mod ime;
mod input;
mod mouse;
mod mouse_inject;
mod render;
mod settings_ui;
mod term_core;

use std::path::PathBuf;

use app::App;
use config::Config;
use term_core::UserEvent;
use winit::event_loop::EventLoop;

fn main() -> anyhow::Result<()> {
    #[cfg(windows)]
    if std::env::args().nth(1).as_deref() == Some("--mouse-helper") {
        let pid = std::env::args()
            .nth(2)
            .ok_or_else(|| anyhow::anyhow!("missing console pid"))?
            .parse()?;
        crate::mouse_inject::run_helper(pid)?;
        return Ok(());
    }
    let mut args = std::env::args().skip(1);
    let mut cfg_path: Option<PathBuf> = None;
    let mut shell: Option<String> = None;
    let mut intensity: Option<String> = None;
    let mut shot: Option<PathBuf> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--config" => cfg_path = args.next().map(PathBuf::from),
            "--shell" | "-e" => shell = args.next(),
            "--calm" => intensity = Some("low".into()),
            "--max" => intensity = Some("max".into()),
            "--no-fx" => intensity = Some("off".into()),
            "--screenshot" => shot = args.next().map(PathBuf::from),
            "--help" | "-h" => {
                println!(
                    "dopaterm - virtual terminal with input/output effects\n\
                     usage: dopaterm [--config FILE] [--shell CMD] [--calm|--max|--no-fx]\n\
                     config: <config dir>/dopaterm/config.toml, lua effects: <config dir>/dopaterm/effects/*.lua"
                );
                return Ok(());
            }
            other => {
                if other.starts_with("--intensity=") {
                    intensity = Some(other.trim_start_matches("--intensity=").into());
                }
            }
        }
    }

    let mut cfg: Config = config::load(cfg_path);
    if let Some(s) = shell {
        cfg.shell = Some(s);
    }
    if let Some(i) = intensity {
        cfg.intensity = match i.as_str() {
            "off" => config::Intensity::Off,
            "low" => config::Intensity::Low,
            "max" => config::Intensity::Max,
            _ => config::Intensity::Normal,
        };
    }

    alacritty_terminal::tty::setup_env();

    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    let mut app = App::new(cfg, proxy, shot);
    event_loop.run_app(&mut app)?;
    Ok(())
}
