use serde::Deserialize;
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
    /// Global effect amount 0.0–1.0. Per-effect `amounts` entries override it;
    /// unset entries inherit this value. 0.0 disables effects.
    pub amount: f32,
    /// Background color as #RRGGBB.
    pub background: String,
    /// Foreground color as #RRGGBB.
    pub foreground: String,
    pub effects: EffectsCfg,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct EffectsCfg {
    /// Per-effect amount multipliers. `None` = inherit `Config::amount`,
    /// `Some(0.0)` = disabled.
    pub amounts: EffectAmounts,
    /// Directory containing *.lua effect scripts. Default: <config dir>/effects
    pub lua_dir: Option<PathBuf>,
}

/// Per-effect overrides. `None` inherits the global amount.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct EffectAmounts {
    pub sparks: Option<f32>,
    pub shatter: Option<f32>,
    pub cursor_trail: Option<f32>,
    pub fireworks: Option<f32>,
    pub wait_pulse: Option<f32>,
    pub confetti: Option<f32>,
    pub output_rain: Option<f32>,
    pub process_error: Option<f32>,
    pub bell_flash: Option<f32>,
}

impl EffectAmounts {
    pub const NAMES: [&'static str; 9] = [
        "sparks",
        "shatter",
        "cursor_trail",
        "fireworks",
        "wait_pulse",
        "confetti",
        "output_rain",
        "process_error",
        "bell_flash",
    ];

    pub fn get(&self, name: &str) -> Option<f32> {
        match name {
            "sparks" => self.sparks,
            "shatter" => self.shatter,
            "cursor_trail" => self.cursor_trail,
            "fireworks" => self.fireworks,
            "wait_pulse" => self.wait_pulse,
            "confetti" => self.confetti,
            "output_rain" => self.output_rain,
            "process_error" => self.process_error,
            "bell_flash" => self.bell_flash,
            _ => None,
        }
    }

    pub fn set(&mut self, name: &str, value: Option<f32>) {
        match name {
            "sparks" => self.sparks = value,
            "shatter" => self.shatter = value,
            "cursor_trail" => self.cursor_trail = value,
            "fireworks" => self.fireworks = value,
            "wait_pulse" => self.wait_pulse = value,
            "confetti" => self.confetti = value,
            "output_rain" => self.output_rain = value,
            "process_error" => self.process_error = value,
            "bell_flash" => self.bell_flash = value,
            _ => {}
        }
    }

    /// `Some(v)` applied to every field — used by --no-fx.
    pub fn uniform(value: Option<f32>) -> Self {
        Self {
            sparks: value,
            shatter: value,
            cursor_trail: value,
            fireworks: value,
            wait_pulse: value,
            confetti: value,
            output_rain: value,
            process_error: value,
            bell_flash: value,
        }
    }
}

impl Default for EffectAmounts {
    fn default() -> Self {
        Self::uniform(None)
    }
}

impl Default for EffectsCfg {
    fn default() -> Self {
        Self {
            amounts: EffectAmounts::default(),
            lua_dir: None,
        }
    }
}

/// Pick a system monospace font that is likely to contain box-drawing and
/// common UI symbols. Falls back to the platform's generic monospace.
pub fn preferred_monospace_font(families: &[String]) -> Option<String> {
    if cfg!(windows) {
        for name in ["Cascadia Code", "Cascadia Mono", "Consolas"] {
            if let Some(family) = crate::fonts::resolve_family(Some(name), families) {
                return Some(family);
            }
        }
    }
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
        .find_map(|name| crate::fonts::resolve_family(Some(name), families))
}

impl Default for Config {
    fn default() -> Self {
        Self {
            font_size: 14.0,
            font_family: None,
            shell: None,
            shell_args: Vec::new(),
            amount: 1.0,
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
                eprintln!(
                    "dopaterm: bad config {}: {e}; using defaults",
                    file.display()
                );
                Config::default()
            }
        },
        Err(_) => Config::default(),
    }
}

pub fn parse_rgb(s: &str) -> [u8; 3] {
    let s = s.trim_start_matches('#');
    let v = u32::from_str_radix(s, 16).unwrap_or(0xc5c8c6);
    [
        ((v >> 16) & 0xff) as u8,
        ((v >> 8) & 0xff) as u8,
        (v & 0xff) as u8,
    ]
}
