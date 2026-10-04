use serde::Deserialize;
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Font size in points.
    pub font_size: f32,
    /// Monospace family override. None = system monospace.
    pub font_family: Option<String>,
    /// Shell program. None = platform default (SHELL / powershell).
    pub shell: Option<String>,
    /// Shell arguments.
    pub shell_args: Vec<String>,
    /// Global effect intensity: off | low | normal | max.
    pub intensity: Intensity,
    /// Background color as #RRGGBB.
    pub background: String,
    /// Foreground color as #RRGGBB.
    pub foreground: String,
    pub effects: EffectsCfg,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct EffectsCfg {
    pub sparks: bool,
    pub shatter: bool,
    pub cursor_trail: bool,
    /// Fireworks when a command appears to finish (heuristic).
    pub fireworks: bool,
    /// Pulsing indicator while the terminal waits for input.
    pub wait_pulse: bool,
    /// Confetti launched from the bottom edge on Enter.
    pub confetti: bool,
    /// Shooting stars triggered by PTY output.
    pub output_rain: bool,
    /// Massive detonation when a child process exits with a non-zero status.
    pub process_error: bool,
    /// Directory containing *.lua effect scripts. Default: <config dir>/effects
    pub lua_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Intensity {
    Off,
    Low,
    #[default]
    Normal,
    Max,
}

impl Intensity {
    /// Particle-count / lifetime multiplier.
    pub fn scale(self) -> f32 {
        match self {
            Intensity::Off => 0.0,
            Intensity::Low => 0.4,
            Intensity::Normal => 1.0,
            Intensity::Max => 2.0,
        }
    }
}

impl Default for EffectsCfg {
    fn default() -> Self {
        Self {
            sparks: true,
            shatter: true,
            cursor_trail: true,
            fireworks: true,
            wait_pulse: true,
            confetti: true,
            output_rain: true,
            process_error: true,
            lua_dir: None,
        }
    }
}

/// Pick a system monospace font that is likely to contain box-drawing and
/// common UI symbols. Falls back to the platform's generic monospace.
fn preferred_monospace_font() -> Option<String> {
    #[cfg(windows)]
    {
        return Some("Cascadia Code".to_string());
    }
    let output = std::process::Command::new("fc-list")
        .args([":", "family"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let families: HashSet<String> = text
        .lines()
        .flat_map(|line| line.split(','))
        .map(|s| s.trim().to_string())
        .collect();
    const CANDIDATES: &[&str] = &[
        "SauceCodePro Nerd Font",
        "JetBrainsMono Nerd Font",
        "FiraCode Nerd Font",
        "Hack Nerd Font",
        "DejaVu Sans Mono",
        "Ubuntu Sans Mono",
        "Ubuntu Mono",
        "Noto Sans Mono",
        "Cascadia Code",
        "Cascadia Mono",
        "Consolas",
    ];
    CANDIDATES
        .iter()
        .find(|&&name| families.contains(name))
        .map(|s| s.to_string())
}

impl Default for Config {
    fn default() -> Self {
        Self {
            font_size: 14.0,
            font_family: preferred_monospace_font(),
            shell: None,
            shell_args: Vec::new(),
            intensity: Intensity::Normal,
            background: "#1d1f21".into(),
            foreground: "#c5c8c6".into(),
            effects: EffectsCfg::default(),
        }
    }
}

/// Directory used for config.toml and effects/*.lua.
pub fn config_dir() -> PathBuf {
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("dopaterm")
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(|| PathBuf::from("."))
            .join("dopaterm")
    }
}

pub fn load(path: Option<PathBuf>) -> Config {
    let file = path.unwrap_or_else(|| config_dir().join("config.toml"));
    match std::fs::read_to_string(&file) {
        Ok(text) => match toml::from_str::<Config>(&text) {
            Ok(cfg) => cfg,
            Err(e) => {
                eprintln!("dopaterm: bad config {}: {e}; using defaults", file.display());
                Config::default()
            }
        },
        Err(_) => Config::default(),
    }
}

pub fn parse_rgb(s: &str) -> [u8; 3] {
    let s = s.trim_start_matches('#');
    let v = u32::from_str_radix(s, 16).unwrap_or(0xc5c8c6);
    [((v >> 16) & 0xff) as u8, ((v >> 8) & 0xff) as u8, (v & 0xff) as u8]
}
