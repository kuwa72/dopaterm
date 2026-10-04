//! Terminal color resolution: Cell fg/bg + flags + OSC-set overrides -> RGBA8.

use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor};

const NORMAL: [[u8; 3]; 8] = [
    [0x1d, 0x1f, 0x21], // black
    [0xcc, 0x66, 0x66], // red
    [0xb5, 0xbd, 0x68], // green
    [0xf0, 0xc6, 0x74], // yellow
    [0x81, 0xa2, 0xbe], // blue
    [0xb2, 0x94, 0xbb], // magenta
    [0x8a, 0xbe, 0xb7], // cyan
    [0xc5, 0xc8, 0xc6], // white
];

const BRIGHT: [[u8; 3]; 8] = [
    [0x66, 0x66, 0x66],
    [0xd5, 0x4e, 0x53],
    [0xb9, 0xca, 0x4a],
    [0xe7, 0xc5, 0x47],
    [0x7a, 0xa6, 0xda],
    [0xc3, 0x97, 0xd8],
    [0x70, 0xc0, 0xb1],
    [0xea, 0xea, 0xea],
];

fn dim(c: [u8; 3]) -> [u8; 3] {
    [
        (c[0] as f32 * 0.66) as u8,
        (c[1] as f32 * 0.66) as u8,
        (c[2] as f32 * 0.66) as u8,
    ]
}

fn ansi256(i: u8) -> [u8; 3] {
    match i {
        0..=7 => NORMAL[i as usize],
        8..=15 => BRIGHT[i as usize - 8],
        16..=231 => {
            let i = i - 16;
            let conv = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
            [conv(i / 36), conv((i / 6) % 6), conv(i % 6)]
        }
        _ => {
            let v = 8 + (i - 232) * 10;
            [v, v, v]
        }
    }
}

fn named_rgb(n: NamedColor, default_fg: [u8; 3], default_bg: [u8; 3]) -> [u8; 3] {
    let i = n as usize;
    match n {
        NamedColor::Foreground => default_fg,
        NamedColor::Background => default_bg,
        NamedColor::BrightForeground => BRIGHT[7],
        NamedColor::DimForeground => dim(default_fg),
        NamedColor::Cursor => default_fg,
        _ if i < 8 => NORMAL[i],
        _ if i < 16 => BRIGHT[i - 8],
        _ if (259..267).contains(&i) => dim(NORMAL[i - 259]),
        _ => default_fg,
    }
}

/// Resolve a cell's display (fg, bg) honoring INVERSE/BOLD/DIM/HIDDEN flags
/// and runtime color overrides.
pub fn cell_colors(
    cell: &Cell,
    colors: &Colors,
    default_fg: [u8; 3],
    default_bg: [u8; 3],
) -> ([u8; 4], [u8; 4]) {
    let resolve = |c: Color| -> [u8; 3] {
        match c {
            Color::Spec(rgb) => [rgb.r, rgb.g, rgb.b],
            Color::Named(n) => {
                colors[n].map(|r| [r.r, r.g, r.b]).unwrap_or_else(|| named_rgb(n, default_fg, default_bg))
            }
            Color::Indexed(i) => {
                colors[i as usize].map(|r| [r.r, r.g, r.b]).unwrap_or_else(|| ansi256(i))
            }
        }
    };

    let mut fg = resolve(cell.fg);
    let mut bg = resolve(cell.bg);

    let flags = cell.flags;
    if flags.contains(Flags::INVERSE) {
        std::mem::swap(&mut fg, &mut bg);
    }
    if flags.contains(Flags::BOLD) {
        if let Color::Named(n) = cell.fg {
            if (n as usize) < 8 {
                fg = BRIGHT[n as usize];
            }
        } else if let Color::Indexed(i) = cell.fg {
            if i < 8 {
                fg = BRIGHT[i as usize];
            }
        }
    }
    if flags.contains(Flags::DIM) {
        fg = dim(fg);
    }
    if flags.contains(Flags::HIDDEN) {
        fg = bg;
    }

    ([fg[0], fg[1], fg[2], 255], [bg[0], bg[1], bg[2], 255])
}

/// u8 RGBA -> sRGB floats in 0..1.
pub fn to_f32(c: [u8; 4]) -> [f32; 4] {
    [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, c[3] as f32 / 255.0]
}

pub fn to_f32_3(c: [u8; 3]) -> [f32; 4] {
    [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, 1.0]
}

/// sRGB float -> linear (for *_SRGB render targets).
pub fn srgb_to_linear(c: [f32; 4]) -> [f32; 4] {
    let f = |v: f32| {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    [f(c[0]), f(c[1]), f(c[2]), c[3]]
}
