use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use winit::event::Ime;

use crate::colors::to_f32;
use crate::fx::Instance;
use crate::render::{Line, Span};

#[derive(Default)]
pub struct Preedit {
    text: String,
    cursor: Option<(usize, usize)>,
}

impl Preedit {
    pub fn active(&self) -> bool {
        !self.text.is_empty()
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = None;
    }

    pub fn update(&mut self, event: Ime) -> Option<String> {
        match event {
            Ime::Preedit(text, cursor) => {
                self.cursor = cursor.filter(|&(start, end)| {
                    start <= end && text.is_char_boundary(start) && text.is_char_boundary(end)
                });
                self.text = text;
            }
            Ime::Commit(text) => {
                self.clear();
                return (!text.is_empty()).then_some(text);
            }
            Ime::Disabled => self.clear(),
            Ime::Enabled => {}
        }
        None
    }

    fn visible(&self, columns: usize) -> (&str, usize) {
        let caret = self.cursor.map_or(self.text.len(), |(_, end)| end);
        let mut start = 0;
        for (index, ch) in self.text[..caret].char_indices() {
            if self.text[start..caret].width() < columns {
                break;
            }
            start = index + ch.len_utf8();
        }
        let mut width = 0;
        let mut end = start;
        for (index, ch) in self.text[start..].char_indices() {
            width += ch.width().unwrap_or(0);
            if width > columns {
                break;
            }
            end = start + index + ch.len_utf8();
        }
        (&self.text[start..end], start)
    }

    pub fn overlay(
        &self,
        anchor: (f32, f32),
        cell: (f32, f32),
        viewport: (f32, f32),
        foreground: [u8; 3],
        background: [u8; 3],
    ) -> (Vec<Instance>, Vec<Line>) {
        if !self.active() || viewport.0 < cell.0 || viewport.1 < cell.1 {
            return (Vec::new(), Vec::new());
        }
        let columns = (viewport.0 / cell.0) as usize;
        let (text, offset) = self.visible(columns);
        let width = ((text.width() + 1) as f32 * cell.0).min(viewport.0);
        let left = anchor.0.clamp(0.0, (viewport.0 - width).max(0.0));
        let top = anchor.1.clamp(0.0, (viewport.1 - cell.1).max(0.0));
        let fg = [foreground[0], foreground[1], foreground[2], 255];
        let bg = [background[0], background[1], background[2], 255];
        let rect = |x: f32, y: f32, w: f32, h: f32, color| Instance {
            pos: [x + w / 2.0, y + h / 2.0],
            size: [w, h],
            rot: 0.0,
            kind: 0,
            color: to_f32(color),
        };
        let mut quads = vec![
            rect(left, top, width, cell.1, bg),
            rect(left, top + cell.1 - 1.0, width, 1.0, fg),
        ];
        if let Some((start, end)) = self.cursor {
            let begin = start.saturating_sub(offset).min(text.len());
            let end = end.saturating_sub(offset).min(text.len());
            let x = text[..begin].width() as f32 * cell.0;
            let end_x = text[..end].width() as f32 * cell.0;
            if end_x > x {
                quads.push(rect(left + x, top + cell.1 - 3.0, end_x - x, 2.0, fg));
            }
            quads.push(rect(left + end_x.min(width - 1.0), top, 1.0, cell.1, fg));
        }
        let lines = vec![Line {
            top,
            left,
            spans: vec![Span {
                text: text.to_string(),
                fg,
                bold: false,
            }],
            family: None,
        }];
        (quads, lines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preedit_is_not_sent_until_commit() {
        let mut state = Preedit::default();
        assert_eq!(state.update(Ime::Enabled), None);
        assert!(!state.active());
        assert_eq!(
            state.update(Ime::Preedit("にほんご".into(), Some((12, 12)))),
            None
        );
        assert!(state.active());
        assert_eq!(state.update(Ime::Preedit(String::new(), None)), None);
        let text = state.update(Ime::Commit("日本語".into())).unwrap();
        assert_eq!(text.as_bytes(), "日本語".as_bytes());
        assert!(!state.active());
        assert!(state.cursor.is_none());
    }

    #[test]
    fn cancellation_and_empty_commit_clear_preedit() {
        let mut state = Preedit::default();
        state.update(Ime::Preedit("未確定".into(), None));
        assert_eq!(state.update(Ime::Disabled), None);
        assert!(!state.active());
        state.update(Ime::Preedit("未確定".into(), Some((0, 9))));
        assert_eq!(state.update(Ime::Commit(String::new())), None);
        assert!(!state.active());
    }

    #[test]
    fn cursor_offsets_are_utf8_bytes() {
        let mut state = Preedit::default();
        state.update(Ime::Preedit("a日😀本".into(), Some((4, 8))));
        let (quads, lines) =
            state.overlay((0.0, 0.0), (8.0, 18.0), (320.0, 180.0), [255; 3], [0; 3]);
        assert_eq!(lines[0].spans[0].text, "a日😀本");
        assert_eq!(quads[2].size[0], 16.0);
        assert_eq!(quads[3].pos[0], 40.5);
        state.update(Ime::Preedit("日本".into(), Some((1, 2))));
        assert!(state.cursor.is_none());
    }

    #[test]
    fn long_preedit_keeps_cursor_visible_at_window_edge() {
        let mut state = Preedit::default();
        state.update(Ime::Preedit("にほんご入力".into(), Some((18, 18))));
        let (quads, lines) =
            state.overlay((72.0, 90.0), (8.0, 18.0), (80.0, 90.0), [255; 3], [0; 3]);
        assert_eq!(lines[0].top, 72.0);
        assert!(lines[0].left + quads[0].size[0] <= 80.0);
        assert!(quads.last().unwrap().pos[0] < 80.0);
        assert_eq!(lines[0].spans[0].text, "んご入力");
    }

    #[test]
    fn no_cursor_range_hides_caret() {
        let mut state = Preedit::default();
        state.update(Ime::Preedit("日本語".into(), None));
        let (quads, _) = state.overlay((0.0, 0.0), (8.0, 18.0), (320.0, 180.0), [255; 3], [0; 3]);
        assert_eq!(quads.len(), 2);
    }
}
