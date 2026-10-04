//! Simple settings overlay rendered with the same text/quads pipeline as the
//! terminal. Built at the start of each frame while it is open.

use crate::config::{Config, Intensity};
use crate::fx::{Instance, SHAPE_RECT};
use crate::render::{Line, Span};

pub enum Action {
    Intensity(Intensity),
    Toggle(&'static str),
    ShellNext,
    Theme(&'static str),
    FontSize(f32),
    FontNext,
    Close,
}

pub struct Hit {
    pub rect: (f32, f32, f32, f32), // x, y, w, h
    pub action: Action,
}

const PANEL_W: f32 = 520.0;
const PADDING_X: f32 = 24.0;
const PADDING_Y: f32 = 20.0;

const SHELLS: &[&str] = &["/bin/bash", "/bin/zsh", "/usr/bin/fish", "/usr/bin/pwsh", "powershell.exe"];
const FONTS: &[&str] = &[
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
const THEMES: &[(&str, &str, &str)] = &[
    ("dark", "#1d1f21", "#c5c8c6"),
    ("light", "#ffffff", "#000000"),
];

fn hex(c: [u8; 3]) -> [u8; 4] {
    [c[0], c[1], c[2], 255]
}

fn rect(x: f32, y: f32, w: f32, h: f32, color: [u8; 4]) -> Instance {
    Instance {
        pos: [x + w / 2.0, y + h / 2.0],
        size: [w, h],
        rot: 0.0,
        kind: SHAPE_RECT,
        color: [
            color[0] as f32 / 255.0,
            color[1] as f32 / 255.0,
            color[2] as f32 / 255.0,
            color[3] as f32 / 255.0,
        ],
    }
}

fn cycle_option<T: AsRef<str>>(current: Option<&str>, list: &[T]) -> Option<String> {
    match current {
        None => Some(list.first()?.as_ref().to_string()),
        Some(cur) => {
            let pos = list.iter().position(|s| s.as_ref() == cur).unwrap_or(list.len() - 1);
            let next = (pos + 1) % list.len();
            if next == 0 {
                None
            } else {
                Some(list[next].as_ref().to_string())
            }
        }
    }
}

pub fn build(cfg: &Config, ww: f32, wh: f32, cw: f32, ch: f32) -> (Vec<Instance>, Vec<Line>, Vec<Hit>) {
    let row_h = ch * 1.8;
    let rows = 16f32;
    let panel_h = rows * row_h + PADDING_Y * 2.0;
    let px = (ww - PANEL_W).max(0.0) / 2.0;
    let py = (wh - panel_h).max(0.0) / 2.0;
    let text_x = px + PADDING_X;

    let panel_bg = hex([0x28, 0x2c, 0x34]);
    let title_bg = hex([0x3a, 0x3f, 0x4b]);
    let text = hex([0xc5, 0xc8, 0xc6]);
    let dim = hex([0x70, 0x74, 0x78]);
    let accent = hex([0xf0, 0xc6, 0x74]);

    let mut bgs = Vec::new();
    let mut lines = Vec::new();
    let mut hits = Vec::new();

    bgs.push(rect(px, py, PANEL_W, panel_h, panel_bg));
    bgs.push(rect(px, py, PANEL_W, row_h + PADDING_Y, title_bg));

    let mut y = py + PADDING_Y;

    // Title.
    lines.push(Line {
        top: y,
        left: text_x,
        spans: vec![Span { text: "dopaterm settings".into(), fg: text, bold: true }],
    });
    y += row_h;

    // Shell row.
    let shell_label = format!("Shell: {}", cfg.shell.as_deref().unwrap_or("(default)"));
    lines.push(Line {
        top: y,
        left: text_x,
        spans: vec![Span { text: shell_label, fg: text, bold: false }],
    });
    lines.push(Line {
        top: y,
        left: text_x + cw * 35.0,
        spans: vec![Span { text: "[next]".into(), fg: text, bold: false }],
    });
    hits.push(Hit {
        rect: (text_x + cw * 34.0, y - ch * 0.2, cw * 7.0, row_h),
        action: Action::ShellNext,
    });
    y += row_h;

    // Theme row.
    let current_theme = THEMES
        .iter()
        .find(|(_, bg, fg)| *bg == cfg.background && *fg == cfg.foreground)
        .map(|(name, _, _)| *name)
        .unwrap_or("custom");
    lines.push(Line {
        top: y,
        left: text_x,
        spans: vec![Span { text: format!("Theme: {}", current_theme), fg: text, bold: false }],
    });
    let mut theme_x = text_x + cw * 14.0;
    for (name, _, _) in THEMES {
        let is_on = current_theme == *name;
        let color = if is_on { accent } else { dim };
        lines.push(Line {
            top: y,
            left: theme_x,
            spans: vec![
                Span { text: "[".into(), fg: dim, bold: false },
                Span { text: (*name).into(), fg: color, bold: is_on },
                Span { text: "] ".into(), fg: dim, bold: false },
            ],
        });
        hits.push(Hit { rect: (theme_x - cw, y - ch * 0.2, cw * 8.0, row_h), action: Action::Theme(name) });
        theme_x += cw * 8.0;
    }
    y += row_h;

    // Font size row.
    lines.push(Line {
        top: y,
        left: text_x,
        spans: vec![Span { text: format!("Font size: {:.0}", cfg.font_size), fg: text, bold: false }],
    });
    lines.push(Line {
        top: y,
        left: text_x + cw * 25.0,
        spans: vec![Span { text: "[-]".into(), fg: text, bold: false }],
    });
    hits.push(Hit {
        rect: (text_x + cw * 24.0, y - ch * 0.2, cw * 4.0, row_h),
        action: Action::FontSize(-1.0),
    });
    lines.push(Line {
        top: y,
        left: text_x + cw * 29.0,
        spans: vec![Span { text: "[+]".into(), fg: text, bold: false }],
    });
    hits.push(Hit {
        rect: (text_x + cw * 28.0, y - ch * 0.2, cw * 4.0, row_h),
        action: Action::FontSize(1.0),
    });
    y += row_h;

    // Font family row.
    lines.push(Line {
        top: y,
        left: text_x,
        spans: vec![Span { text: format!("Font: {}", cfg.font_family.as_deref().unwrap_or("(system)")), fg: text, bold: false }],
    });
    lines.push(Line {
        top: y,
        left: text_x + cw * 38.0,
        spans: vec![Span { text: "[next]".into(), fg: text, bold: false }],
    });
    hits.push(Hit {
        rect: (text_x + cw * 37.0, y - ch * 0.2, cw * 7.0, row_h),
        action: Action::FontNext,
    });
    y += row_h;

    // Separator.
    lines.push(Line {
        top: y,
        left: text_x,
        spans: vec![Span { text: "Effects".into(), fg: dim, bold: false }],
    });
    y += row_h;

    // Intensity row.
    let intensity_opts = [("Off", Intensity::Off), ("Low", Intensity::Low),
                          ("Normal", Intensity::Normal), ("Max", Intensity::Max)];
    let mut spans: Vec<Span> = vec![Span { text: "Intensity: ".into(), fg: text, bold: false }];
    let opt_w = cw * 6.0;
    let first_opt_x = text_x + cw * 12.0;
    let mut ix = first_opt_x;
    for (label, val) in &intensity_opts {
        let is_on = cfg.intensity == *val;
        let color = if is_on { accent } else { dim };
        spans.push(Span { text: "[".into(), fg: dim, bold: false });
        spans.push(Span { text: label.to_string(), fg: color, bold: is_on });
        spans.push(Span { text: "] ".into(), fg: dim, bold: false });
        hits.push(Hit { rect: (ix - cw, y - ch * 0.2, opt_w, row_h), action: Action::Intensity(*val) });
        ix += opt_w;
    }
    lines.push(Line { top: y, left: text_x, spans });
    y += row_h;

    // Effects toggles.
    let toggles: [(&str, bool); 8] = [
        ("sparks", cfg.effects.sparks),
        ("shatter", cfg.effects.shatter),
        ("cursor_trail", cfg.effects.cursor_trail),
        ("fireworks", cfg.effects.fireworks),
        ("wait_pulse", cfg.effects.wait_pulse),
        ("confetti", cfg.effects.confetti),
        ("output_rain", cfg.effects.output_rain),
        ("process_error", cfg.effects.process_error),
    ];
    let check_x = px + PANEL_W - PADDING_X - cw * 4.0;
    for (name, on) in toggles {
        let line_text = format!("{:<18} {}", name, if on { "[x]" } else { "[ ]" });
        lines.push(Line {
            top: y,
            left: text_x,
            spans: vec![Span { text: line_text, fg: text, bold: false }],
        });
        hits.push(Hit { rect: (check_x - cw, y - ch * 0.2, cw * 4.0, row_h), action: Action::Toggle(name) });
        y += row_h;
    }

    // Close button.
    let close_text = "[ close ]";
    let close_w = cw * close_text.len() as f32;
    let close_x = px + (PANEL_W - close_w) / 2.0;
    lines.push(Line {
        top: y,
        left: close_x,
        spans: vec![Span { text: close_text.into(), fg: text, bold: true }],
    });
    hits.push(Hit { rect: (close_x, y - ch * 0.2, close_w, row_h), action: Action::Close });

    (bgs, lines, hits)
}

pub fn apply_action(cfg: &mut Config, action: &Action) {
    match action {
        Action::Intensity(i) => cfg.intensity = *i,
        Action::Toggle(field) => match *field {
            "sparks" => cfg.effects.sparks = !cfg.effects.sparks,
            "shatter" => cfg.effects.shatter = !cfg.effects.shatter,
            "cursor_trail" => cfg.effects.cursor_trail = !cfg.effects.cursor_trail,
            "fireworks" => cfg.effects.fireworks = !cfg.effects.fireworks,
            "wait_pulse" => cfg.effects.wait_pulse = !cfg.effects.wait_pulse,
            "confetti" => cfg.effects.confetti = !cfg.effects.confetti,
            "output_rain" => cfg.effects.output_rain = !cfg.effects.output_rain,
            "process_error" => cfg.effects.process_error = !cfg.effects.process_error,
            _ => {}
        },
        Action::ShellNext => {
            cfg.shell = cycle_option(cfg.shell.as_deref(), SHELLS);
        }
        Action::Theme(name) => {
            if let Some((_, bg, fg)) = THEMES.iter().find(|(n, _, _)| n == name) {
                cfg.background = (*bg).to_string();
                cfg.foreground = (*fg).to_string();
            }
        }
        Action::FontSize(delta) => {
            cfg.font_size = (cfg.font_size + delta).clamp(4.0, 128.0);
        }
        Action::FontNext => {
            cfg.font_family = cycle_option(cfg.font_family.as_deref(), FONTS);
        }
        Action::Close => {}
    }
}
