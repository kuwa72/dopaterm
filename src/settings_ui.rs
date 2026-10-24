//! Simple settings overlay rendered with the same text/quads pipeline as the
//! terminal. Built at the start of each frame while it is open.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::config::{Config, EffectAmounts};
use crate::fx::{Instance, SHAPE_RECT};
use crate::render::{Line, Span};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Page {
    General,
    Effects,
}

#[derive(Clone, Copy)]
pub enum Action {
    /// Switch the settings page.
    SetPage(Page),
    /// Adjust the global amount (delta, e.g. -0.1 / +0.1).
    MasterAmount(f32),
    /// Adjust a per-effect override by `delta`; creates an explicit value.
    Amount(&'static str, f32),
    /// Reset a per-effect override to "inherit the global amount".
    AmountAuto(&'static str),
    ShellNext,
    Theme(&'static str),
    FontSize(f32),
    FontPrev,
    FontNext,
    /// Open the scrollable font picker page.
    FontList,
    /// Pick `fonts[index]`; applies live so the row itself is the preview.
    FontSelect(usize),
    /// Leave the font picker page.
    FontBack,
    Close,
}

pub struct Hit {
    pub rect: (f32, f32, f32, f32), // x, y, w, h
    /// Logical row index; hits sharing a row form one keyboard focus target.
    pub row: u32,
    /// -1 = decrement, +1 = increment, 0 = activate. Drives Left/Right keys.
    pub dir: i8,
    pub action: Action,
}

const PADDING_X: f32 = 24.0;
const PADDING_Y: f32 = 20.0;

/// Rows the panel must fit, including the title/tab row and the close button.
fn page_rows(page: Page) -> f32 {
    match page {
        Page::General => 8.0,
        // title + hint + one row per effect + close
        Page::Effects => 3.0 + EffectAmounts::NAMES.len() as f32,
    }
}

/// Panel width in text columns (64 cells ≈ 520px at the default cell size);
/// scales with the font/DPI so labels keep their relative positions.
const PANEL_COLS: f32 = 64.0;
/// Max font rows visible on the picker page at once.
pub const FONT_LIST_ROWS: usize = 14;

const SHELLS: &[&str] = &[
    "/bin/bash",
    "/bin/zsh",
    "/usr/bin/fish",
    "/usr/bin/pwsh",
    "powershell.exe",
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
            let pos = list
                .iter()
                .position(|s| s.as_ref() == cur)
                .unwrap_or(list.len() - 1);
            let next = (pos + 1) % list.len();
            if next == 0 {
                None
            } else {
                Some(list[next].as_ref().to_string())
            }
        }
    }
}

fn cycle_option_rev<T: AsRef<str>>(current: Option<&str>, list: &[T]) -> Option<String> {
    match current {
        None => Some(list.last()?.as_ref().to_string()),
        Some(cur) => {
            let pos = list.iter().position(|s| s.as_ref() == cur).unwrap_or(0);
            if pos == 0 {
                None
            } else {
                Some(list[pos - 1].as_ref().to_string())
            }
        }
    }
}

/// Truncate `text` to `max_cols` display columns, ending with '…' when cut.
fn truncate_cols(text: &str, max_cols: usize) -> String {
    if text.width() <= max_cols {
        return text.to_string();
    }
    let mut columns = 0;
    let mut label: String = text
        .chars()
        .take_while(|ch| {
            columns += ch.width().unwrap_or(0);
            columns < max_cols
        })
        .collect();
    label.push('…');
    label
}

fn font_label(family: Option<&str>) -> String {
    truncate_cols(&format!("Font: {}", family.unwrap_or("(system)")), 36)
}

/// `page` selects the tab; `font_scroll` `Some(first_visible_index)` shows the
/// font picker page instead. `focus_row` highlights a row for keyboard use.
#[allow(unused_assignments)] // last `end_row!` increment is intentionally unused
pub fn build(
    cfg: &Config,
    ww: f32,
    wh: f32,
    cw: f32,
    ch: f32,
    fonts: &[String],
    page: Page,
    font_scroll: Option<usize>,
    focus_row: Option<u32>,
) -> (Vec<Instance>, Vec<Line>, Vec<Hit>) {
    if let Some(scroll) = font_scroll {
        return build_font_list(cfg, ww, wh, cw, ch, fonts, scroll, focus_row);
    }
    let row_h = ch * 1.8;
    let rows = page_rows(page);
    let panel_w = (cw * PANEL_COLS).min(ww);
    let panel_h = rows * row_h + PADDING_Y * 2.0;
    let px = (ww - panel_w).max(0.0) / 2.0;
    let py = (wh - panel_h).max(0.0) / 2.0;
    let text_x = px + PADDING_X;

    let panel_bg = hex([0x28, 0x2c, 0x34]);
    let title_bg = hex([0x3a, 0x3f, 0x4b]);
    let text = hex([0xc5, 0xc8, 0xc6]);
    let dim = hex([0x70, 0x74, 0x78]);
    let accent = hex([0xf0, 0xc6, 0x74]);
    let focus_bg = hex([0x45, 0x4b, 0x57]);

    let mut bgs = Vec::new();
    let mut lines = Vec::new();
    let mut hits = Vec::new();

    bgs.push(rect(px, py, panel_w, panel_h, panel_bg));
    bgs.push(rect(px, py, panel_w, row_h + PADDING_Y, title_bg));

    let mut y = py + PADDING_Y;
    let mut row = 0u32;
    macro_rules! end_row {
        () => {{
            if focus_row == Some(row) {
                bgs.push(rect(px, y - ch * 0.2, panel_w, row_h, focus_bg));
            }
            row += 1;
            y += row_h;
        }};
    }

    // Title + tab bar (row 0).
    lines.push(Line {
        top: y,
        left: text_x,
        spans: vec![Span {
            text: "dopaterm settings".into(),
            fg: text,
            bold: true,
        }],
        family: None,
    });
    let mut tx = text_x + cw * 40.0;
    for p in [Page::General, Page::Effects] {
        let name = match p {
            Page::General => "general",
            Page::Effects => "effects",
        };
        let is_on = page == p;
        lines.push(Line {
            top: y,
            left: tx,
            spans: vec![Span {
                text: format!("[{}]", name),
                fg: if is_on { accent } else { dim },
                bold: is_on,
            }],
            family: None,
        });
        hits.push(Hit {
            rect: (tx - cw * 0.5, y - ch * 0.2, cw * 9.0, row_h),
            row,
            dir: 0,
            action: Action::SetPage(p),
        });
        tx += cw * 10.0;
    }
    end_row!();

    match page {
        Page::General => {
            // Shell row.
            lines.push(Line {
                top: y,
                left: text_x,
                spans: vec![Span {
                    text: format!(
                        "Shell: {}",
                        truncate_cols(cfg.shell.as_deref().unwrap_or("(default)"), 28)
                    ),
                    fg: text,
                    bold: false,
                }],
                family: None,
            });
            lines.push(Line {
                top: y,
                left: text_x + cw * 42.0,
                spans: vec![Span {
                    text: "[next]".into(),
                    fg: text,
                    bold: false,
                }],
                family: None,
            });
            hits.push(Hit {
                rect: (text_x + cw * 41.0, y - ch * 0.2, cw * 7.0, row_h),
                row,
                dir: 0,
                action: Action::ShellNext,
            });
            end_row!();

            // Theme row.
            let current_theme = THEMES
                .iter()
                .find(|(_, bg, fg)| *bg == cfg.background && *fg == cfg.foreground)
                .map(|(name, _, _)| *name)
                .unwrap_or("custom");
            lines.push(Line {
                top: y,
                left: text_x,
                spans: vec![Span {
                    text: format!("Theme: {}", current_theme),
                    fg: text,
                    bold: false,
                }],
                family: None,
            });
            let mut theme_x = text_x + cw * 14.0;
            for (name, _, _) in THEMES {
                let is_on = current_theme == *name;
                lines.push(Line {
                    top: y,
                    left: theme_x,
                    spans: vec![Span {
                        text: format!("[{}]", name),
                        fg: if is_on { accent } else { dim },
                        bold: is_on,
                    }],
                    family: None,
                });
                hits.push(Hit {
                    rect: (theme_x - cw, y - ch * 0.2, cw * 8.0, row_h),
                    row,
                    dir: 0,
                    action: Action::Theme(name),
                });
                theme_x += cw * 8.0;
            }
            end_row!();

            // Font size row.
            lines.push(Line {
                top: y,
                left: text_x,
                spans: vec![Span {
                    text: format!("Font size: {:.0}", cfg.font_size),
                    fg: text,
                    bold: false,
                }],
                family: None,
            });
            for (i, (label, delta, dir)) in
                [("[-]", -1.0f32, -1i8), ("[+]", 1.0, 1)].iter().enumerate()
            {
                let bx = text_x + cw * (24.0 + i as f32 * 5.0);
                lines.push(Line {
                    top: y,
                    left: bx,
                    spans: vec![Span {
                        text: label.to_string(),
                        fg: text,
                        bold: false,
                    }],
                    family: None,
                });
                hits.push(Hit {
                    rect: (bx - cw * 0.5, y - ch * 0.2, cw * 4.0, row_h),
                    row,
                    dir: *dir,
                    action: Action::FontSize(*delta),
                });
            }
            end_row!();

            // Font family row.
            let font_family = crate::fonts::resolve_family(cfg.font_family.as_deref(), fonts);
            lines.push(Line {
                top: y,
                left: text_x,
                spans: vec![Span {
                    text: font_label(font_family.as_deref()),
                    fg: text,
                    bold: false,
                }],
                family: None,
            });
            if !fonts.is_empty() {
                for (i, (label, action, dir)) in [
                    ("[prev]", Action::FontPrev, -1i8),
                    ("[next]", Action::FontNext, 1),
                    ("[list]", Action::FontList, 0),
                ]
                .into_iter()
                .enumerate()
                {
                    let bx = text_x + cw * (38.0 + i as f32 * 7.0);
                    lines.push(Line {
                        top: y,
                        left: bx,
                        spans: vec![Span {
                            text: label.into(),
                            fg: text,
                            bold: false,
                        }],
                        family: None,
                    });
                    hits.push(Hit {
                        rect: (bx - cw * 0.5, y - ch * 0.2, cw * 6.0, row_h),
                        row,
                        dir,
                        action,
                    });
                }
            }
            end_row!();

            // Master amount row.
            lines.push(Line {
                top: y,
                left: text_x,
                spans: vec![Span {
                    text: format!("Effect amount: {:.2}", cfg.amount),
                    fg: text,
                    bold: false,
                }],
                family: None,
            });
            for (i, (label, delta, dir)) in
                [("[-]", -0.1f32, -1i8), ("[+]", 0.1, 1)].iter().enumerate()
            {
                let bx = text_x + cw * (24.0 + i as f32 * 5.0);
                lines.push(Line {
                    top: y,
                    left: bx,
                    spans: vec![Span {
                        text: label.to_string(),
                        fg: text,
                        bold: false,
                    }],
                    family: None,
                });
                hits.push(Hit {
                    rect: (bx - cw * 0.5, y - ch * 0.2, cw * 4.0, row_h),
                    row,
                    dir: *dir,
                    action: Action::MasterAmount(*delta),
                });
            }
            end_row!();
        }
        Page::Effects => {
            lines.push(Line {
                top: y,
                left: text_x,
                spans: vec![Span {
                    text: "amount: 0 = off, blank = inherit master".into(),
                    fg: dim,
                    bold: false,
                }],
                family: None,
            });
            end_row!();
            for name in EffectAmounts::NAMES {
                let explicit = cfg.effects.amounts.get(name);
                let effective = explicit.unwrap_or(cfg.amount);
                let val = match explicit {
                    Some(v) => format!("{:.2}", v),
                    None => format!("auto {:.2}", effective),
                };
                lines.push(Line {
                    top: y,
                    left: text_x,
                    spans: vec![
                        Span {
                            text: format!("{:<16}", name),
                            fg: text,
                            bold: false,
                        },
                        Span {
                            text: val,
                            fg: if explicit.is_some() { accent } else { dim },
                            bold: explicit.is_some(),
                        },
                    ],
                    family: None,
                });
                for (i, (label, dir, action)) in [
                    ("[-]", -1i8, Action::Amount(name, -0.1)),
                    ("[+]", 1, Action::Amount(name, 0.1)),
                    ("[auto]", 0, Action::AmountAuto(name)),
                ]
                .into_iter()
                .enumerate()
                {
                    let bx = text_x + cw * (30.0 + i as f32 * 7.0);
                    lines.push(Line {
                        top: y,
                        left: bx,
                        spans: vec![Span {
                            text: label.into(),
                            fg: text,
                            bold: false,
                        }],
                        family: None,
                    });
                    hits.push(Hit {
                        rect: (bx - cw * 0.5, y - ch * 0.2, cw * 6.0, row_h),
                        row,
                        dir,
                        action,
                    });
                }
                end_row!();
            }
        }
    }

    // Close button.
    let close_text = "[ close ]";
    let close_w = cw * close_text.len() as f32;
    let close_x = px + (panel_w - close_w) / 2.0;
    lines.push(Line {
        top: y,
        left: close_x,
        spans: vec![Span {
            text: close_text.into(),
            fg: text,
            bold: true,
        }],
        family: None,
    });
    hits.push(Hit {
        rect: (close_x, y - ch * 0.2, close_w, row_h),
        row,
        dir: 0,
        action: Action::Close,
    });
    end_row!();

    (bgs, lines, hits)
}

/// Font picker page: each family name is rendered in its own typeface
/// (Line.family), and clicking a row applies it immediately so the list
/// itself is the preview.
#[allow(unused_assignments)] // last `end_row!` increment is intentionally unused
fn build_font_list(
    cfg: &Config,
    ww: f32,
    wh: f32,
    cw: f32,
    ch: f32,
    fonts: &[String],
    scroll: usize,
    focus_row: Option<u32>,
) -> (Vec<Instance>, Vec<Line>, Vec<Hit>) {
    let row_h = ch * 1.8;
    let visible = fonts.len().min(FONT_LIST_ROWS);
    let scroll = scroll.min(fonts.len().saturating_sub(visible));
    let end = scroll + visible;
    let panel_w = (cw * PANEL_COLS).min(ww);
    let panel_h = (visible + 2) as f32 * row_h + PADDING_Y * 2.0;
    let px = (ww - panel_w).max(0.0) / 2.0;
    let py = (wh - panel_h).max(0.0) / 2.0;
    let text_x = px + PADDING_X;

    let panel_bg = hex([0x28, 0x2c, 0x34]);
    let title_bg = hex([0x3a, 0x3f, 0x4b]);
    let text = hex([0xc5, 0xc8, 0xc6]);
    let dim = hex([0x70, 0x74, 0x78]);
    let accent = hex([0xf0, 0xc6, 0x74]);
    let focus_bg = hex([0x45, 0x4b, 0x57]);

    let mut bgs = vec![
        rect(px, py, panel_w, panel_h, panel_bg),
        rect(px, py, panel_w, row_h + PADDING_Y, title_bg),
    ];
    let mut lines = Vec::new();
    let mut hits = Vec::new();

    let mut y = py + PADDING_Y;
    let mut row = 0u32;
    macro_rules! end_row {
        () => {{
            if focus_row == Some(row) {
                bgs.push(rect(px, y - ch * 0.2, panel_w, row_h, focus_bg));
            }
            row += 1;
            y += row_h;
        }};
    }
    lines.push(Line {
        top: y,
        left: text_x,
        spans: vec![Span {
            text: "Font".into(),
            fg: text,
            bold: true,
        }],
        family: None,
    });
    lines.push(Line {
        top: y,
        left: px + panel_w - PADDING_X - cw * 12.0,
        spans: vec![Span {
            text: format!("{}-{}/{}", scroll + 1, end, fonts.len()),
            fg: dim,
            bold: false,
        }],
        family: None,
    });
    end_row!();

    let current = crate::fonts::resolve_family(cfg.font_family.as_deref(), fonts);
    let name_cols = (((panel_w - PADDING_X * 2.0) / cw) as usize).saturating_sub(2);
    for i in scroll..end {
        let name = &fonts[i];
        let is_on = current.as_deref() == Some(name.as_str());
        let label = format!(
            "{} {}",
            if is_on { ">" } else { " " },
            truncate_cols(name, name_cols)
        );
        let fg = if is_on { accent } else { text };
        lines.push(Line {
            top: y,
            left: text_x,
            spans: vec![Span {
                text: label,
                fg,
                bold: is_on,
            }],
            family: Some(name.clone()),
        });
        hits.push(Hit {
            rect: (px, y - ch * 0.2, panel_w, row_h),
            row,
            dir: 0,
            action: Action::FontSelect(i),
        });
        end_row!();
    }

    lines.push(Line {
        top: y,
        left: text_x,
        spans: vec![Span {
            text: "[back]".into(),
            fg: dim,
            bold: false,
        }],
        family: None,
    });
    hits.push(Hit {
        rect: (text_x - cw * 0.5, y - ch * 0.2, cw * 7.0, row_h),
        row,
        dir: 0,
        action: Action::FontBack,
    });
    end_row!();

    (bgs, lines, hits)
}

pub fn apply_action(cfg: &mut Config, action: &Action, fonts: &[String]) {
    match action {
        Action::SetPage(_) => {}
        Action::MasterAmount(delta) => {
            cfg.amount = (cfg.amount + delta).clamp(0.0, 1.0);
        }
        Action::Amount(field, delta) => {
            let cur = cfg.effects.amounts.get(field).unwrap_or(cfg.amount);
            cfg.effects
                .amounts
                .set(field, Some((cur + delta).clamp(0.0, 1.0)));
        }
        Action::AmountAuto(field) => {
            cfg.effects.amounts.set(field, None);
        }
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
            let family = crate::fonts::resolve_family(cfg.font_family.as_deref(), fonts);
            cfg.font_family = cycle_option(family.as_deref(), fonts);
        }
        Action::FontPrev => {
            let family = crate::fonts::resolve_family(cfg.font_family.as_deref(), fonts);
            cfg.font_family = cycle_option_rev(family.as_deref(), fonts);
        }
        Action::FontSelect(i) => {
            if let Some(family) = fonts.get(*i) {
                cfg.font_family = Some(family.clone());
            }
        }
        Action::FontList | Action::FontBack => {}
        Action::Close => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_selection_cycles_only_the_provided_system_catalog() {
        let fonts = vec!["System Fixed A".to_string(), "System Fixed B".to_string()];
        let mut cfg = Config::default();
        for expected in [
            Some("System Fixed A"),
            Some("System Fixed B"),
            None,
            Some("System Fixed A"),
        ] {
            apply_action(&mut cfg, &Action::FontNext, &fonts);
            assert_eq!(cfg.font_family.as_deref(), expected);
        }
        cfg.font_family = Some("Unavailable Font".into());
        apply_action(&mut cfg, &Action::FontNext, &fonts);
        assert_eq!(cfg.font_family.as_deref(), Some("System Fixed A"));
    }

    #[test]
    fn font_prev_walks_backwards_through_the_catalog() {
        let fonts = vec!["System Fixed A".to_string(), "System Fixed B".to_string()];
        let mut cfg = Config::default();
        for expected in [
            Some("System Fixed B"),
            Some("System Fixed A"),
            None,
            Some("System Fixed B"),
        ] {
            apply_action(&mut cfg, &Action::FontPrev, &fonts);
            assert_eq!(cfg.font_family.as_deref(), expected);
        }
        cfg.font_family = Some("Unavailable Font".into());
        apply_action(&mut cfg, &Action::FontPrev, &fonts);
        assert_eq!(cfg.font_family.as_deref(), Some("System Fixed B"));
    }

    #[test]
    fn font_list_page_emits_per_row_select_hits_in_own_family() {
        let fonts: Vec<String> = (0..20).map(|i| format!("Mono {i}")).collect();
        let mut cfg = Config::default();
        cfg.font_family = Some("Mono 3".into());
        let (_, lines, hits) = build(
            &cfg,
            960.0,
            600.0,
            8.0,
            18.0,
            &fonts,
            Page::General,
            Some(2),
            None,
        );
        let selects: Vec<usize> = hits
            .iter()
            .filter_map(|h| match h.action {
                Action::FontSelect(i) => Some(i),
                _ => None,
            })
            .collect();
        assert_eq!(selects, (2..16).collect::<Vec<_>>());
        assert!(hits.iter().any(|h| matches!(h.action, Action::FontBack)));
        let rows: Vec<&Line> = lines.iter().filter(|l| l.family.is_some()).collect();
        assert_eq!(rows.len(), FONT_LIST_ROWS);
        assert_eq!(rows[1].family.as_deref(), Some("Mono 3"));

        apply_action(&mut cfg, &Action::FontSelect(7), &fonts);
        assert_eq!(cfg.font_family.as_deref(), Some("Mono 7"));
        apply_action(&mut cfg, &Action::FontSelect(99), &fonts);
        assert_eq!(cfg.font_family.as_deref(), Some("Mono 7"));
    }

    #[test]
    fn long_font_labels_do_not_overlap_the_selection_button() {
        let label = font_label(Some(&"日本語等幅フォント".repeat(5)));
        assert!(label.width() <= 36);
        assert!(label.ends_with('…'));
    }

    #[test]
    fn empty_catalog_has_no_font_selection_button() {
        let mut cfg = Config::default();
        cfg.font_family = Some("Unavailable Font".into());
        let (_, lines, hits) = build(
            &cfg,
            960.0,
            600.0,
            8.0,
            18.0,
            &[],
            Page::General,
            None,
            None,
        );
        assert!(
            !hits
                .iter()
                .any(|hit| matches!(hit.action, Action::FontNext))
        );
        assert!(
            lines
                .iter()
                .flat_map(|line| &line.spans)
                .any(|span| span.text == "Font: (system)")
        );
        apply_action(&mut cfg, &Action::FontNext, &[]);
        assert_eq!(cfg.font_family, None);
    }

    #[test]
    fn effects_page_fits_inside_the_panel() {
        let cfg = Config::default();
        let (_, lines, hits) = build(
            &cfg,
            960.0,
            600.0,
            8.0,
            18.0,
            &[],
            Page::Effects,
            None,
            None,
        );
        let row_h = 18.0 * 1.8;
        let panel_h = page_rows(Page::Effects) * row_h + PADDING_Y * 2.0;
        let panel_bottom = (600.0 - panel_h) / 2.0 + panel_h;
        for line in &lines {
            assert!(
                line.top + row_h <= panel_bottom + 0.5,
                "row overflows the panel: {:?} top={} bottom={} panel_bottom={}",
                line.spans.first().map(|s| &s.text),
                line.top,
                line.top + row_h,
                panel_bottom
            );
        }
        for hit in &hits {
            assert!(hit.rect.1 + hit.rect.3 <= panel_bottom + 0.5);
        }
        assert!(hits.iter().any(|h| matches!(h.action, Action::Close)));
        let names = EffectAmounts::NAMES.len();
        assert_eq!(
            hits.iter()
                .filter(|h| matches!(h.action, Action::Amount(..)))
                .count(),
            names * 2
        );
    }
}
