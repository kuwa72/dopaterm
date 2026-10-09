#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

mod app;
mod colors;
mod config;
mod fonts;
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
            "--debug-console" => {
                #[cfg(windows)]
                win_console::enable();
            }
            "--help" | "-h" => {
                #[cfg(windows)]
                win_console::attach_parent();
                println!(
                    "dopaterm - virtual terminal with input/output effects\n\
                     usage: dopaterm [--config FILE] [--shell CMD] [--calm|--max|--no-fx] [--debug-console]\n\
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

#[cfg(windows)]
mod win_console {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE,
        INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AllocConsole, AttachConsole, GetStdHandle, STD_ERROR_HANDLE,
        STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle,
    };

    fn open_device(name: &str, access: u32) -> HANDLE {
        let wide: Vec<u16> = std::ffi::OsStr::new(name)
            .encode_wide()
            .chain(Some(0))
            .collect();
        unsafe {
            CreateFileW(
                wide.as_ptr(),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        }
    }

    /// AttachConsole does not initialize the standard handles of a
    /// GUI-subsystem process, so point them at CONIN$/CONOUT$ explicitly.
    /// Inherited handles that are already valid (pipes, parent console)
    /// are kept as-is.
    fn bind_std_handles() {
        unsafe {
            if GetStdHandle(STD_INPUT_HANDLE).is_null() {
                let input = open_device("CONIN$", GENERIC_READ | GENERIC_WRITE);
                if !input.is_null() && input != INVALID_HANDLE_VALUE {
                    SetStdHandle(STD_INPUT_HANDLE, input);
                }
            }
            for id in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
                if GetStdHandle(id).is_null() {
                    let out = open_device("CONOUT$", GENERIC_READ | GENERIC_WRITE);
                    if !out.is_null() && out != INVALID_HANDLE_VALUE {
                        SetStdHandle(id, out);
                    }
                }
            }
        }
    }

    /// Attach to the parent's console only; used by --help so no window is
    /// allocated when there is no terminal to print to.
    pub fn attach_parent() {
        unsafe {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
        bind_std_handles();
    }

    /// Show log output: reuse the parent console when launched from a
    /// terminal, otherwise allocate a fresh console window.
    pub fn enable() {
        unsafe {
            if AttachConsole(ATTACH_PARENT_PROCESS) == 0 && GetLastError() != ERROR_ACCESS_DENIED {
                AllocConsole();
            }
        }
        bind_std_handles();
    }
}
