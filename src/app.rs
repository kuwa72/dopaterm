//! winit application: owns the window, renderer, terminal and effect manager.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Point};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::tty::Shell;
use alacritty_terminal::vte::ansi::CursorShape;
use winit::application::ApplicationHandler;
use winit::event::{
    ElementState, KeyEvent, MouseButton as WinitMouseButton, MouseScrollDelta, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

use crate::colors::{cell_colors, to_f32};
use crate::config::{self, Config, parse_rgb};
use crate::fx::{self, FxEvent, Instance, Manager};
use crate::input::{KeyKind, decode_key};
use crate::render::{FrameOutcome, Line, Renderer, Span};
use crate::settings_ui;
use crate::term_core::{EventProxy, TermCore, UserEvent};

pub struct App {
    cfg: Config,
    proxy: EventLoopProxy<UserEvent>,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    term: Option<TermCore>,
    fx: Manager,
    mods: ModifiersState,
    /// Previous frame's cells, indexed line*cols+col.
    snapshot: Vec<SnapCell>,
    /// Cached styled lines for the renderer; rebuilt per dirty row.
    lines: Vec<Option<Line>>,
    /// Rows whose text needs re-shaping this frame.
    dirty_lines: Vec<bool>,
    /// Cached cell background/cursor quads.
    bgs: Vec<Instance>,
    /// Terminal-side mouse selection in buffer coordinates
    /// (line = viewport row - display_offset; negative = scrollback).
    selection: Option<Selection>,
    /// Highlight quads for the active selection; rebuilt every frame.
    sel_quads: Vec<Instance>,
    /// `bgs` + `sel_quads` merged for the draw call; reused every frame.
    draw_quads: Vec<Instance>,
    /// Active character hop ("ぴょん") overlay state.
    hop: Option<Hop>,
    /// When the next hop is allowed to start.
    next_hop: Instant,
    hop_seed: u64,
    /// Consecutive render failures; a GPU adapter/surface change is
    /// unrecoverable for the old device, so a full rebuild is scheduled.
    render_failures: u32,
    /// Rebuild attempts since the last good frame; the first ones retry on
    /// the same window, repeated failure escalates to a window recreate.
    gpu_resets: u32,
    /// Consecutive frames the surface declined to present (timeout /
    /// occluded / validation). A long streak right after a rebuild means
    /// the window's compositor binding is dead — escalate.
    skipped_frames: u32,
    /// Log the first presented frame after a GPU recovery, so frozen-
    // screen reports can distinguish "presents lost" from "no redraw".
    presented_after_reset: bool,
    last_renderer_reset: Option<Instant>,
    frame_bg: [u8; 3],
    prev_offset: usize,
    prev_cursor: Option<Point>,
    /// Cell indices shattered proactively on Backspace/Delete; used to dedupe
    /// against the diff-based erase detection for ~200ms.
    predicted_erase: Vec<(usize, Instant)>,
    last_frame: Instant,
    /// PTY wakeups seen since the last frame. Bursts are coalesced into a
    /// single `PtyOutput` event so effects scale with frames, not reads.
    pending_output: usize,
    /// Scrollback lines moved since the last frame, coalesced into one
    /// `Scrolled` event (positive = into history).
    pending_scroll: i32,
    dirty: bool,
    last_cursor_px: (f32, f32),
    /// Enter was pressed; becomes `command_running` when output arrives.
    command_pending: bool,
    /// A command produced output after Enter; quiet -> CommandDone.
    command_running: bool,
    last_output: Instant,
    /// Last input or output activity; drives the Waiting indicator.
    last_activity: Instant,
    /// Last keystroke; CommandDone requires input idle too.
    last_input: Instant,
    last_wait_ping: Instant,
    /// --screenshot target path; take shot after the terminal has rendered a few frames.
    shot_path: Option<PathBuf>,
    frames: u32,
    demo_injected: bool,
    /// Delayed exit after a child process error so the explosion can play.
    pending_exit: Option<Instant>,
    /// Current mouse position in pixels and cell coordinates.
    mouse_px: (f32, f32),
    mouse_cell: (usize, usize),
    /// Currently held mouse button for drag reporting.
    mouse_button: Option<u8>,
    /// True while an IME composition is active (prevents duplicate key events).
    ime_composing: bool,
    ime_preedit: crate::ime::Preedit,
    /// Settings overlay state.
    show_settings: bool,
    /// Active settings tab.
    settings_page: crate::settings_ui::Page,
    /// Keyboard-focused row id within the settings overlay.
    settings_focus: u32,
    /// Font picker page state: `Some` = first visible index into the
    /// renderer's monospace family catalog.
    settings_fonts: Option<usize>,
    mouse_demo_done: bool,
    /// Active hitboxes in the settings overlay for the current frame.
    settings_hits: Vec<crate::settings_ui::Hit>,
}

impl App {
    pub fn new(cfg: Config, proxy: EventLoopProxy<UserEvent>, shot_path: Option<PathBuf>) -> Self {
        let frame_bg = parse_rgb(&cfg.background);
        Self {
            cfg,
            proxy,
            window: None,
            renderer: None,
            term: None,
            fx: Manager::new(1.0),
            mods: ModifiersState::empty(),
            snapshot: Vec::new(),
            lines: Vec::new(),
            dirty_lines: Vec::new(),
            bgs: Vec::new(),
            selection: None,
            sel_quads: Vec::new(),
            draw_quads: Vec::new(),
            hop: None,
            next_hop: Instant::now() + Duration::from_secs(2),
            hop_seed: 0xC0FFEE_u64,
            render_failures: 0,
            gpu_resets: 0,
            skipped_frames: 0,
            presented_after_reset: true,
            last_renderer_reset: None,
            frame_bg,
            prev_offset: usize::MAX,
            prev_cursor: None,
            predicted_erase: Vec::new(),
            last_frame: Instant::now(),
            pending_output: 0,
            pending_scroll: 0,
            dirty: true,
            last_cursor_px: (0.0, 0.0),
            command_pending: false,
            command_running: false,
            last_output: Instant::now(),
            last_activity: Instant::now(),
            last_input: Instant::now(),
            last_wait_ping: Instant::now(),
            shot_path,
            frames: 0,
            demo_injected: false,
            pending_exit: None,
            mouse_px: (0.0, 0.0),
            mouse_cell: (0, 0),
            mouse_button: None,
            ime_composing: false,
            ime_preedit: crate::ime::Preedit::default(),
            show_settings: false,
            settings_page: crate::settings_ui::Page::General,
            settings_focus: 0,
            settings_fonts: None,
            mouse_demo_done: false,
            settings_hits: Vec::new(),
        }
    }

    fn cell_rect(&self, point: Point) -> (f32, f32, f32, f32) {
        let r = self.renderer.as_ref().unwrap();
        let x = point.column.0 as f32 * r.cell_w;
        let y = point.line.0.max(0) as f32 * r.cell_h;
        (x, y, r.cell_w, r.cell_h)
    }

    /// Window size in physical pixels, with a sane default before the first
    /// frame (effects need it to span the screen).
    fn window_px(&self) -> (f32, f32) {
        self.window
            .as_ref()
            .map(|w| {
                let s = w.inner_size();
                (s.width as f32, s.height as f32)
            })
            .unwrap_or((960.0, 600.0))
    }

    fn build_settings_overlay(&self) -> (Vec<Instance>, Vec<Line>, Vec<settings_ui::Hit>) {
        let r = self.renderer.as_ref().unwrap();
        let w = self.window.as_ref().unwrap().inner_size();
        settings_ui::build(
            &self.cfg,
            w.width as f32,
            w.height as f32,
            r.cell_w,
            r.cell_h,
            r.font_families(),
            self.settings_page,
            self.settings_fonts,
            Some(self.settings_focus),
        )
    }

    fn build_overlay(&self) -> (Vec<Instance>, Vec<Line>, Vec<settings_ui::Hit>) {
        let (mut quads, mut lines, hits) = if self.show_settings {
            self.build_settings_overlay()
        } else {
            let r = self.renderer.as_ref().unwrap();
            let size = self.window.as_ref().unwrap().inner_size();
            let anchor = (
                self.last_cursor_px.0 - r.cell_w / 2.0,
                self.last_cursor_px.1 - r.cell_h / 2.0,
            );
            let (q, l) = self.ime_preedit.overlay(
                anchor,
                (r.cell_w, r.cell_h),
                (size.width as f32, size.height as f32),
                parse_rgb(&self.cfg.foreground),
                parse_rgb(&self.cfg.background),
            );
            (q, l, Vec::new())
        };
        let (hq, hl) = self.hop_overlay();
        quads.extend(hq);
        lines.extend(hl);
        (quads, lines, hits)
    }

    fn sync_ime(&mut self) {
        if self.show_settings {
            self.ime_preedit.clear();
            self.ime_composing = false;
        }
        if let Some(window) = &self.window {
            window.set_ime_allowed(!self.show_settings);
        }
    }

    /// Count a failed or panicked frame; persistent failure rebuilds the
    /// window + renderer. Terminal output is cheap to regenerate, so a
    /// frame that can't be presented is simply dropped and redrawn.
    fn note_render_failure(&mut self, why: &str, el: &ActiveEventLoop) {
        self.render_failures += 1;
        eprintln!("dopaterm: render error: {why}");
        if let Some(w) = self.window.as_ref() {
            w.request_redraw();
        }
        if self.render_failures >= 3
            && self
                .last_renderer_reset
                .is_none_or(|t| t.elapsed() > Duration::from_secs(2))
        {
            // A surface rebuilt on the same HWND can present to nothing:
            // the window's compositor binding was tied to the dead
            // adapter. Recreate the window directly — the PTY lives on.
            self.recreate_window_and_renderer(el);
            self.render_failures = 0;
            self.last_renderer_reset = Some(Instant::now());
        }
    }

    /// After a device loss the window's compositor binding can be tied to
    /// the dead adapter — a fresh surface on the same HWND may present to
    /// nothing. Recreate the whole window; the PTY/shell keeps running
    /// because TermCore is independent of the renderer.
    fn recreate_window_and_renderer(&mut self, el: &ActiveEventLoop) {
        let Some(old_win) = self.window.take() else {
            return;
        };
        let size = old_win.inner_size();
        let pos = old_win.outer_position().ok();
        // Hide the old window first: its leaked surface holds an
        // Arc<Window> and keeps the HWND alive — left visible it would
        // sit on screen frozen forever.
        old_win.set_visible(false);
        // Dropping a lost-device renderer panics inside wgpu-hal's
        // surface teardown, but the unwind still destroys the swapchain —
        // a partial drop leaks whatever is left, which is fine.
        if let Some(old) = self.renderer.take() {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(old)));
        }
        let mut attrs = Window::default_attributes()
            .with_title("dopaterm")
            .with_window_icon(window_icon())
            .with_inner_size(size);
        if let Some(p) = pos {
            attrs = attrs.with_position(winit::dpi::PhysicalPosition::new(p.x, p.y));
        }
        match el.create_window(attrs) {
            Ok(w) => {
                let window = Arc::new(w);
                window.set_ime_allowed(!self.show_settings);
                self.window = Some(window);
                self.recreate_renderer();
                eprintln!("dopaterm: window recreated after GPU change");
            }
            Err(e) => {
                eprintln!("dopaterm: window recreate failed: {e}");
                old_win.set_visible(true);
                self.window = Some(old_win);
            }
        }
    }

    /// Rebuild the renderer (instance/surface/adapter/device/pipelines)
    /// after the GPU configuration changed underneath us, keeping the
    /// same window.
    fn recreate_renderer(&mut self) {
        let Some(window) = self.window.clone() else {
            return;
        };
        let size = window.inner_size();
        // Drop the old renderer BEFORE creating the new surface. Its drop
        // panics inside wgpu-hal on a lost device (unreleased acquire
        // semaphores), but the unwind still runs NativeSwapchain::drop ->
        // vkDestroySwapchainKHR, which releases the HWND. Without this the
        // old swapchain stays bound and a second surface on the same
        // window presents to nothing.
        if let Some(old) = self.renderer.take() {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(old)));
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            pollster::block_on(Renderer::new(
                window,
                self.cfg.font_size,
                self.cfg.font_family.clone(),
            ))
        }));
        match result {
            Ok(Ok(mut r)) => {
                if self.shot_path.is_some() {
                    r.set_scale_factor(1.0);
                }
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    r.resize(size.width, size.height)
                }));
                self.renderer = Some(r);
                self.snapshot.clear();
                self.dirty_lines.iter_mut().for_each(|d| *d = true);
                self.resize_terminal_to_window();
                self.dirty = true;
                self.window.as_ref().unwrap().request_redraw();
                self.presented_after_reset = false;
                eprintln!("dopaterm: renderer re-initialized after GPU change");
            }
            Ok(Err(e)) => eprintln!("dopaterm: renderer re-init failed: {e}"),
            Err(_) => eprintln!("dopaterm: renderer re-init panicked"),
        }
    }

    fn resize_terminal_to_window(&mut self) {
        let Some(r) = self.renderer.as_ref() else {
            return;
        };
        let Some(term) = &self.term else { return };
        let size = self.window.as_ref().unwrap().inner_size();
        let ws = alacritty_terminal::event::WindowSize {
            num_cols: ((size.width as f32 / r.cell_w) as u16).max(1),
            num_lines: ((size.height as f32 / r.cell_h) as u16).max(1),
            cell_width: r.cell_w as u16,
            cell_height: r.cell_h as u16,
        };
        term.resize(ws);
        self.snapshot.clear();
        self.dirty_lines = vec![true; self.lines.len().max(1)];
        self.dirty = true;
    }

    fn update_effects(&mut self) {
        self.fx.clear();
        self.fx.set_global(self.cfg.amount);
        self.fx.push(Box::new(fx::builtin::Sparks::new()));
        self.fx.push(Box::new(fx::builtin::Shatter::new()));
        self.fx.push(Box::new(fx::builtin::CursorTrail::new()));
        self.fx.push(Box::new(fx::builtin::ScrollTrail::new()));
        self.fx.push(Box::new(fx::builtin::Fireworks::new()));
        self.fx.push(Box::new(fx::builtin::WaitPulse::new()));
        self.fx.push(Box::new(fx::builtin::Confetti::new()));
        self.fx.push(Box::new(fx::builtin::OutputRain::new()));
        self.fx.push(Box::new(fx::builtin::BellFlash::new()));
        self.fx.push(Box::new(fx::builtin::ProcessError::new()));
        for name in config::EffectAmounts::NAMES {
            if let Some(v) = self.cfg.effects.amounts.get(name) {
                self.fx.set_amount(name, v);
            }
        }
        let lua_dir = self
            .cfg
            .effects
            .lua_dir
            .clone()
            .unwrap_or_else(|| config::config_dir().join("effects"));
        if let Ok(dir) = std::fs::read_dir(&lua_dir) {
            for entry in dir.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "lua") {
                    match fx::lua_fx::LuaEffect::load(&path) {
                        Ok(eff) => self.fx.push(Box::new(eff)),
                        Err(e) => eprintln!("dopaterm: lua effect {} failed: {e}", path.display()),
                    }
                }
            }
        }
    }

    fn mouse_mode(&self, term: &TermCore) -> TermMode {
        let mode = *term.term.lock().mode();
        #[cfg(windows)]
        if !crate::mouse::tracking_enabled(mode)
            && term
                .child_pid
                .and_then(crate::mouse_inject::input_mode)
                .is_some_and(crate::mouse_inject::native_tracking_enabled)
        {
            return mode | TermMode::MOUSE_MOTION;
        }
        mode
    }

    fn send_mouse_event(
        &self,
        term: &TermCore,
        button: u8,
        col: usize,
        line: usize,
        release: bool,
    ) {
        let mode = self.mouse_mode(term);
        let Some(seq) = crate::mouse::encode(
            &mode,
            button,
            col,
            line,
            release,
            self.mods.shift_key(),
            self.mods.alt_key(),
            self.mods.control_key(),
        ) else {
            return;
        };
        if std::env::var_os("DOPA_MOUSE_LOG").is_some() {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/dopaterm_mouse.log")
            {
                let _ = writeln!(
                    f,
                    "btn={button} col={col} line={line} release={release} seq={seq:?}"
                );
            }
        }

        #[cfg(windows)]
        {
            let Some(pid) = term.child_pid else {
                eprintln!("dopaterm: no child pid, cannot inject mouse event");
                return;
            };
            if self.is_vt_bridge_child() || crate::mouse_inject::uses_vt_input(pid) {
                if !crate::mouse_inject::send_vt_input(pid, &seq) {
                    eprintln!("dopaterm: VT mouse injection failed pid={pid}");
                }
            } else {
                let is_motion = button & 32 != 0;
                let is_wheel = !release && (button == 64 || button == 65);
                let btn = button & 0x3;
                let (button_state, event_flags) = if release {
                    (0, 0)
                } else if is_wheel {
                    let delta: i32 = if button == 64 { 120 } else { -120 };
                    ((delta as u32) << 16, crate::mouse_inject::MOUSE_WHEELED)
                } else {
                    use crate::mouse_inject::{
                        FROM_LEFT_1ST_BUTTON_PRESSED, FROM_LEFT_2ND_BUTTON_PRESSED,
                        RIGHTMOST_BUTTON_PRESSED,
                    };
                    let state = match btn {
                        0 => FROM_LEFT_1ST_BUTTON_PRESSED,
                        1 => FROM_LEFT_2ND_BUTTON_PRESSED,
                        2 => RIGHTMOST_BUTTON_PRESSED,
                        _ => 0,
                    };
                    let flags = if is_motion {
                        crate::mouse_inject::MOUSE_MOVED
                    } else {
                        0
                    };
                    (state, flags)
                };
                let mut ctrl = 0u32;
                if self.mods.shift_key() {
                    ctrl |= crate::mouse_inject::SHIFT_PRESSED;
                }
                if self.mods.control_key() {
                    ctrl |= crate::mouse_inject::LEFT_CTRL_PRESSED;
                }
                if self.mods.alt_key() {
                    ctrl |= crate::mouse_inject::LEFT_ALT_PRESSED;
                }
                if !crate::mouse_inject::send_mouse_event(
                    pid,
                    col as i16,
                    line as i16,
                    button_state,
                    ctrl,
                    event_flags,
                ) {
                    eprintln!("dopaterm: native mouse injection failed pid={pid}");
                }
            }
            return;
        }

        #[cfg(not(windows))]
        term.write(&seq);
    }

    /// True if the current shell is a VT bridge (wsl.exe / ssh.exe) where mouse
    /// sequences must be delivered as raw bytes rather than Win32 records.
    fn is_vt_bridge_child(&self) -> bool {
        let prog = self
            .cfg
            .shell
            .as_ref()
            .map(|s| s.as_str())
            .unwrap_or("powershell");
        crate::mouse::is_vt_bridge(prog)
    }

    /// Copy the active terminal selection to the clipboard.
    fn copy_selection(&self) {
        let Some(sel) = self.selection else { return };
        let rows = self.lines.len();
        if rows == 0 || self.snapshot.is_empty() {
            return;
        }
        let cols = self.snapshot.len() / rows;
        let Some(term) = self.term.as_ref() else {
            return;
        };
        let offset = term.term.lock().grid().display_offset();
        let text = selected_text(&self.snapshot, cols, rows, offset, &sel);
        if !text.is_empty() {
            if let Ok(mut cb) = arboard::Clipboard::new() {
                let _ = cb.set_text(text);
            }
        }
    }

    /// Paste clipboard content: an image becomes a PNG temp file whose path
    /// is pasted (for CLI agents), otherwise text is pasted.
    fn paste_clipboard(&self, term: &TermCore) {
        let Ok(mut cb) = arboard::Clipboard::new() else {
            return;
        };
        if let Ok(img) = cb.get_image() {
            if let Some(path) = save_clipboard_image(&img) {
                #[cfg(windows)]
                let path = if self.is_vt_bridge_child() {
                    windows_path_to_wsl(&path).unwrap_or(path)
                } else {
                    path
                };
                self.paste_text(term, &path);
                return;
            }
        }
        if let Ok(text) = cb.get_text() {
            self.paste_text(term, &text);
        }
    }

    /// Write paste content to the PTY, wrapped in bracketed-paste markers
    /// when the application enabled them.
    fn paste_text(&self, term: &TermCore, text: &str) {
        let bracketed = term.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        if bracketed {
            term.write(b"\x1b[200~");
        }
        term.write(text.as_bytes());
        if bracketed {
            term.write(b"\x1b[201~");
        }
    }

    fn simulate_mouse_demo(&mut self) {
        if self.mouse_demo_done {
            return;
        }
        self.mouse_demo_done = true;
        let Some(term) = self.term.as_ref() else {
            return;
        };
        // Move to a known cell and generate press / release / wheel events.
        self.mouse_cell = (4, 2);
        self.send_mouse_event(term, 0, self.mouse_cell.0, self.mouse_cell.1, false);
        self.send_mouse_event(term, 0, self.mouse_cell.0, self.mouse_cell.1, true);
        self.mouse_cell = (6, 3);
        self.send_mouse_event(term, 64, self.mouse_cell.0, self.mouse_cell.1, false);
        eprintln!("dopaterm: synthetic mouse events injected; exiting in 500ms");
        self.pending_exit = Some(Instant::now() + Duration::from_millis(500));
    }

    /// Open the font picker page, scrolled so the active family is visible.
    fn open_font_list(&mut self) {
        let fonts = self.renderer.as_ref().unwrap().font_families();
        let current = crate::fonts::resolve_family(self.cfg.font_family.as_deref(), fonts)
            .and_then(|f| fonts.iter().position(|x| x == &f))
            .unwrap_or(0);
        let max = fonts.len().saturating_sub(settings_ui::FONT_LIST_ROWS);
        self.settings_fonts = Some(
            current
                .saturating_sub(settings_ui::FONT_LIST_ROWS / 2)
                .min(max),
        );
    }

    /// Apply a settings action (mouse click or keyboard activation).
    fn apply_settings_action(&mut self, action: &settings_ui::Action) {
        let mut needs_font_reload = false;
        let mut needs_effects = false;
        match action {
            settings_ui::Action::Close => {
                self.show_settings = false;
                self.sync_ime();
            }
            settings_ui::Action::SetPage(p) => {
                self.settings_page = *p;
                self.settings_focus = 0;
            }
            settings_ui::Action::MasterAmount(_)
            | settings_ui::Action::Amount(_, _)
            | settings_ui::Action::AmountAuto(_) => {
                settings_ui::apply_action(
                    &mut self.cfg,
                    action,
                    self.renderer.as_ref().unwrap().font_families(),
                );
                needs_effects = true;
            }
            settings_ui::Action::FontSize(_)
            | settings_ui::Action::FontNext
            | settings_ui::Action::FontPrev
            | settings_ui::Action::FontSelect(_) => {
                settings_ui::apply_action(
                    &mut self.cfg,
                    action,
                    self.renderer.as_ref().unwrap().font_families(),
                );
                needs_font_reload = true;
            }
            settings_ui::Action::FontList => self.open_font_list(),
            settings_ui::Action::FontBack => {
                self.settings_fonts = None;
            }
            settings_ui::Action::ShellNext | settings_ui::Action::Theme(_) => {
                settings_ui::apply_action(
                    &mut self.cfg,
                    action,
                    self.renderer.as_ref().unwrap().font_families(),
                );
            }
        }
        if needs_font_reload {
            let size = self.cfg.font_size;
            let family = self.cfg.font_family.clone();
            if let Some(r) = &mut self.renderer {
                r.set_font(size, family.as_deref());
            }
            self.resize_terminal_to_window();
        }
        if needs_effects {
            self.update_effects();
        }
        self.dirty = true;
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn handle_settings_click(&mut self) {
        let (mx, my) = self.mouse_px;
        for hit in &self.settings_hits {
            let (x, y, w, h) = hit.rect;
            if mx >= x && mx < x + w && my >= y && my < y + h {
                self.settings_focus = hit.row;
                let action = hit.action;
                self.apply_settings_action(&action);
                break;
            }
        }
        self.sync_ime();
    }

    /// Keyboard navigation inside the settings overlay. Returns true when the
    /// key was consumed.
    fn handle_settings_key(&mut self, event: &KeyEvent) -> bool {
        let max_row = self.settings_hits.iter().map(|h| h.row).max().unwrap_or(0);
        let mut moved = false;
        match &event.logical_key {
            Key::Named(NamedKey::ArrowUp) => {
                self.settings_focus = self.settings_focus.saturating_sub(1);
                moved = true;
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.settings_focus = (self.settings_focus + 1).min(max_row);
                moved = true;
            }
            Key::Named(NamedKey::Tab) => {
                self.settings_page = match self.settings_page {
                    settings_ui::Page::General => settings_ui::Page::Effects,
                    settings_ui::Page::Effects => settings_ui::Page::General,
                };
                self.settings_focus = 0;
                moved = true;
            }
            _ => {}
        }
        if moved {
            self.dirty = true;
            if let Some(w) = &self.window {
                w.request_redraw();
            }
            return true;
        }
        let dir: i8 = match &event.logical_key {
            Key::Named(NamedKey::ArrowLeft) => -1,
            Key::Named(NamedKey::ArrowRight) => 1,
            Key::Named(NamedKey::Enter | NamedKey::Space) => 0,
            Key::Character(c) if c == " " => 0,
            _ => return false,
        };
        // Pick a hit in the focused row: Left/Right prefer dir-matched hits,
        // Enter/Space prefer dir==0; fall back to the row's first hit.
        let mut row_hits: Vec<&settings_ui::Hit> = self
            .settings_hits
            .iter()
            .filter(|h| h.row == self.settings_focus)
            .collect();
        row_hits.sort_by(|a, b| a.rect.0.partial_cmp(&b.rect.0).unwrap());
        if row_hits.is_empty() {
            return true;
        }
        let pick = if dir < 0 {
            row_hits
                .iter()
                .find(|h| h.dir < 0)
                .or_else(|| row_hits.first())
        } else if dir > 0 {
            row_hits
                .iter()
                .find(|h| h.dir > 0)
                .or_else(|| row_hits.last())
        } else {
            row_hits
                .iter()
                .find(|h| h.dir == 0)
                .or_else(|| row_hits.first())
        };
        if let Some(hit) = pick {
            let action = hit.action;
            self.apply_settings_action(&action);
        }
        true
    }

    fn handle_term_event(&mut self, ev: Event, el: &ActiveEventLoop) {
        if let Some(t) = self.pending_exit {
            if Instant::now() >= t {
                el.exit();
                return;
            }
        }
        let Some(term) = &self.term else { return };
        match ev {
            Event::Wakeup => {
                // Coalesced per frame in `build_frame`; firing per read turns
                // fast output into a wall of particles.
                self.pending_output += 1;
                let now = Instant::now();
                self.last_output = now;
                self.last_activity = now;
                if self.command_pending {
                    self.command_pending = false;
                    self.command_running = true;
                }
                self.dirty = true;
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            Event::PtyWrite(text) => term.write(text.as_bytes()),
            Event::Title(t) => {
                if let Some(w) = &self.window {
                    w.set_title(&t);
                }
            }
            Event::ResetTitle => {
                if let Some(w) = &self.window {
                    w.set_title("dopaterm");
                }
            }
            Event::ClipboardStore(_, text) => {
                if let Ok(mut cb) = arboard::Clipboard::new() {
                    let _ = cb.set_text(text);
                }
            }
            Event::ClipboardLoad(_, fmt) => {
                if let Ok(mut cb) = arboard::Clipboard::new() {
                    if let Ok(text) = cb.get_text() {
                        term.write(fmt(&text).as_bytes());
                    }
                }
            }
            Event::ColorRequest(index, fmt) => {
                let color = {
                    let guard = term.term.lock();
                    crate::colors::color_at_index(
                        guard.renderable_content().colors,
                        index,
                        parse_rgb(&self.cfg.foreground),
                        parse_rgb(&self.cfg.background),
                    )
                };
                if let Some(color) = color {
                    term.write(fmt(color).as_bytes());
                } else {
                    // Respond with default black; full color reporting is a TODO.
                    term.write(
                        fmt(alacritty_terminal::vte::ansi::Rgb { r: 0, g: 0, b: 0 }).as_bytes(),
                    );
                }
            }
            Event::TextAreaSizeRequest(fmt) => {
                if let Some(r) = &self.renderer {
                    let w = self.window.as_ref().unwrap().inner_size();
                    term.write(
                        fmt(WindowSize {
                            num_cols: (w.width as f32 / r.cell_w) as u16,
                            num_lines: (w.height as f32 / r.cell_h) as u16,
                            cell_width: r.cell_w as u16,
                            cell_height: r.cell_h as u16,
                        })
                        .as_bytes(),
                    );
                }
            }
            Event::Bell => {
                let (x, y) = self.last_cursor_px;
                let r = self.renderer.as_ref().unwrap();
                self.fx.event(&FxEvent::Bell {
                    x,
                    y,
                    w: r.cell_w,
                    h: r.cell_h,
                });
                self.dirty = true;
            }
            Event::ChildExit(status) => {
                if !status.success() {
                    let code = status.code().unwrap_or(1);
                    let (cx, cy) = self.last_cursor_px;
                    let (ww, wh) = self
                        .window
                        .as_ref()
                        .map(|w| {
                            let s = w.inner_size();
                            (s.width as f32, s.height as f32)
                        })
                        .unwrap_or((960.0, 600.0));
                    self.fx.event(&FxEvent::ChildError {
                        x: cx,
                        y: cy,
                        w: ww,
                        h: wh,
                        status: code,
                    });
                    self.pending_exit = Some(Instant::now() + Duration::from_millis(1800));
                    self.dirty = true;
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                } else {
                    el.exit();
                }
            }
            Event::Exit => {
                el.exit();
            }
            Event::MouseCursorDirty | Event::CursorBlinkingChange => {}
        }
    }

    /// Diff the term grid against the cached snapshot, emit effect events, and
    /// rebuild only the rows that changed (`self.lines`, `self.bgs`).
    fn build_frame(&mut self) {
        if self.pending_output > 0 {
            let (ww, wh) = self.window_px();
            let n = std::mem::take(&mut self.pending_output);
            self.fx.event(&FxEvent::PtyOutput { n, w: ww, h: wh });
        }
        if self.pending_scroll != 0 {
            let (ww, wh) = self.window_px();
            let lines = std::mem::take(&mut self.pending_scroll);
            self.fx.event(&FxEvent::Scrolled {
                lines,
                w: ww,
                h: wh,
            });
        }
        let term = self.term.as_ref().unwrap();
        let renderer = self.renderer.as_ref().unwrap();
        let guard = term.term.lock();
        let content = guard.renderable_content();
        let colors = content.colors;
        let mut cursor = content.cursor;
        let offset = content.display_offset;
        cursor.point.line += offset as i32;
        let cursor_point = cursor.point;
        let (cw, ch) = (renderer.cell_w, renderer.cell_h);
        let (default_fg, default_bg) = crate::colors::default_colors(
            colors,
            parse_rgb(&self.cfg.foreground),
            parse_rgb(&self.cfg.background),
        );
        let background_changed = self.frame_bg != default_bg;
        self.frame_bg = default_bg;
        let def_bg4 = [default_bg[0], default_bg[1], default_bg[2], 255];
        let cols = guard.columns();
        let rows = guard.screen_lines();

        if self.lines.len() != rows || self.snapshot.len() != cols * rows {
            self.lines = vec![None; rows];
            self.dirty_lines = vec![true; rows];
            self.snapshot.clear();
        }

        // Pass 1: snapshot cells, detect erase events.
        let snap = snapshot_grid(&guard, default_fg, default_bg);
        let scrolled = offset != self.prev_offset;
        // Pagers and TUIs (less, vim) scroll by repainting rows instead of
        // moving the scrollback, so display_offset never changes. Detect a
        // row shift and drive ScrollTrail from it too. In the alt screen even
        // a one-line wheel step counts; on the primary screen one-line shifts
        // are ordinary output and stay with OutputRain.
        let alt_screen = guard.mode().contains(TermMode::ALT_SCREEN);
        let internal_scroll = if scrolled {
            None
        } else {
            let min_shift = if alt_screen { 1 } else { 2 };
            detect_scroll_shift(&self.snapshot, &snap, cols, rows, min_shift)
        };
        if let Some(lines) = internal_scroll {
            let (ww, wh) = self.window_px();
            self.fx.event(&FxEvent::Scrolled {
                lines,
                w: ww,
                h: wh,
            });
        }
        let view_scrolled = scrolled || internal_scroll.is_some();

        if std::env::var_os("DOPA_DEBUG").is_some() {
            let nonspace = snap.iter().filter(|c| !matches!(c.0, ' ' | '\0')).count();
            eprintln!(
                "dopaterm: nonspace={} scrolled={} internal={:?} rows={}",
                nonspace, scrolled, internal_scroll, rows
            );
        }

        // Mass changes = scroll/redraw; suppress shatter storms.
        if !view_scrolled {
            // The bound follows the 10-step effect amount (and any shatter
            // override). At stage 5 the diff also includes in-place text
            // replacements, so every screen update gets debris.
            let shatter = self
                .cfg
                .effects
                .amounts
                .get("shatter")
                .unwrap_or(self.cfg.amount);
            let erased = find_erased(&self.snapshot, &snap, shatter >= 0.9);
            if std::env::var_os("DOPA_DEBUG").is_some() && !erased.is_empty() {
                eprintln!(
                    "dopaterm: erased {:?}",
                    erased.iter().map(|e| e.1).collect::<String>()
                );
            }
            self.predicted_erase
                .retain(|(_, at)| at.elapsed() < Duration::from_secs(1));
            // A burst of erases is a redraw, not per-character editing: keep
            // the count bounded instead of shattering every cell.
            for (i, ch_, fg_) in sample_erases(erased, erase_budget(shatter)) {
                // Skip cells already shattered by key-press prediction.
                if self
                    .predicted_erase
                    .iter()
                    .any(|(pi, at)| *pi == i && at.elapsed() < Duration::from_millis(200))
                {
                    continue;
                }
                let line = i / cols;
                let col = i % cols;
                self.fx.event(&FxEvent::Erased {
                    ch: ch_,
                    x: col as f32 * cw + cw / 2.0,
                    y: line as f32 * ch + ch / 2.0,
                    w: cw,
                    h: ch,
                    color: to_f32(fg_),
                });
            }
        }

        // Pass 2: mark dirty rows (cell diffs, scroll, cursor in/out).
        let mut changed_cells = 0usize;
        if self.snapshot.is_empty() {
            self.dirty_lines.iter_mut().for_each(|d| *d = true);
        } else {
            for (i, (old, new)) in self.snapshot.iter().zip(&snap).enumerate() {
                if *old != *new {
                    changed_cells += 1;
                    let line = i / cols;
                    if line < rows {
                        self.dirty_lines[line] = true;
                    }
                }
            }
        }
        // Big repaints (output bursts, clear, alt-screen apps) should not
        // spray cursor-trail comets across the screen.
        let heavy_update = is_heavy_update(view_scrolled, changed_cells, snap.len());
        if view_scrolled {
            self.dirty_lines.iter_mut().for_each(|d| *d = true);
        }
        if self.prev_cursor != Some(cursor_point) {
            for p in [self.prev_cursor, Some(cursor_point)].into_iter().flatten() {
                if p.line.0 >= 0 && (p.line.0 as usize) < rows {
                    self.dirty_lines[p.line.0 as usize] = true;
                }
            }
        }
        self.prev_offset = offset;

        // Rebuild dirty rows' spans from the snapshot. Flags stay set until
        // the renderer consumes them after draw.
        let mut any_dirty = false;
        for i in 0..rows {
            if !self.dirty_lines[i] {
                continue;
            }
            any_dirty = true;
            let mut spans: Vec<Span> = Vec::new();
            let mut text = String::new();
            let mut span_key: Option<([u8; 4], bool)> = None;
            for col in 0..cols {
                let s = &snap[i * cols + col];
                let (c, mut fg, bold) = (s.0, s.1, s.3);
                if c == '\0' {
                    continue;
                }
                if p_eq(cursor_point, i, col) && cursor.shape != CursorShape::Hidden {
                    fg = s.2;
                }
                if span_key != Some((fg, bold)) {
                    if let Some((f, b)) = span_key.take() {
                        if !text.is_empty() {
                            spans.push(Span {
                                text: std::mem::take(&mut text),
                                fg: f,
                                bold: b,
                            });
                        }
                    }
                    span_key = Some((fg, bold));
                }
                text.push(c);
                text.extend(s.4.iter().copied());
            }
            if let Some((f, b)) = span_key {
                if !text.is_empty() {
                    spans.push(Span {
                        text,
                        fg: f,
                        bold: b,
                    });
                }
            }
            self.lines[i] = Some(Line {
                top: i as f32 * ch,
                left: 0.0,
                spans,
                family: None,
            });
        }

        // Rebuild cell background + cursor quads when content changed.
        if any_dirty || background_changed {
            self.bgs.clear();
            for i in 0..rows {
                for col in 0..cols {
                    let (_, fg, bg, ..) = snap[i * cols + col];
                    let is_cursor = cursor_covers(&snap, cols, cursor_point, i, col)
                        && cursor.shape != CursorShape::Hidden;
                    let qcol = if is_cursor { fg } else { bg };
                    if is_cursor || bg != def_bg4 {
                        self.bgs.push(Instance {
                            pos: [col as f32 * cw + cw / 2.0, i as f32 * ch + ch / 2.0],
                            size: [cw, ch],
                            rot: 0.0,
                            kind: 0,
                            color: to_f32(qcol),
                        });
                    }
                }
            }
        }

        // Selection highlight quads; rebuilt every frame so they follow
        // scrolling and drag updates.
        self.sel_quads.clear();
        if let Some(sel) = self.selection {
            let ((sl, sc), (el, ec)) = sel.ordered();
            for i in 0..rows {
                let buf = i as i32 - offset as i32;
                if buf < sl || buf > el {
                    continue;
                }
                let c0 = if buf == sl { sc } else { 0 }.min(cols - 1);
                let c1 = if buf == el { ec } else { cols - 1 }.min(cols - 1);
                for col in c0..=c1 {
                    self.sel_quads.push(Instance {
                        pos: [col as f32 * cw + cw / 2.0, i as f32 * ch + ch / 2.0],
                        size: [cw, ch],
                        rot: 0.0,
                        kind: 0,
                        color: [0.30, 0.50, 0.85, 0.35],
                    });
                }
            }
        }

        self.snapshot = snap;

        // Cursor move events. Skipped on heavy repaints: the cursor jumping
        // because the screen scrolled is not a movement the user made.
        if let Some(prev) = self.prev_cursor {
            if !heavy_update && prev != cursor_point {
                let (px, py, w, h) = self.cell_rect(prev);
                let dx = (cursor_point.column.0 as i32 - prev.column.0 as i32) as f32 * cw;
                let dy = (cursor_point.line.0 - prev.line.0) as f32 * ch;
                self.fx.event(&FxEvent::CursorMoved {
                    x: px + w / 2.0,
                    y: py + h / 2.0,
                    w,
                    h,
                    dx,
                    dy,
                });
            }
        }
        self.prev_cursor = Some(cursor_point);
        self.last_cursor_px = (
            cursor_point.column.0 as f32 * cw + cw / 2.0,
            cursor_point.line.0.max(0) as f32 * ch + ch / 2.0,
        );

        drop(guard);
        if let Some(window) = &self.window {
            window.set_ime_cursor_area(
                winit::dpi::PhysicalPosition::new(
                    self.last_cursor_px.0 - cw / 2.0,
                    self.last_cursor_px.1 - ch / 2.0,
                ),
                winit::dpi::PhysicalSize::new(cw as u32, ch as u32),
            );
        }
    }

    /// Periodic (200ms) housekeeping driven by the Wake thread: emit
    /// CommandDone when a command's output goes quiet, and Waiting pulses
    /// while the terminal is idle.
    fn check_timers(&mut self) {
        let mut fired = false;
        // Fire only when output AND input are quiet — popping fireworks while
        // the user is mid-typing reads as a random screen flash.
        if self.command_running
            && self.last_output.elapsed() > Duration::from_millis(600)
            && self.last_input.elapsed() > Duration::from_millis(600)
        {
            self.command_running = false;
            let (x, y) = self.last_cursor_px;
            self.fx.event(&FxEvent::CommandDone { x, y });
            fired = true;
        }
        if self.last_activity.elapsed() > Duration::from_secs(3)
            && self.last_wait_ping.elapsed() > Duration::from_millis(1500)
        {
            self.last_wait_ping = Instant::now();
            let (x, y) = self.last_cursor_px;
            self.fx.event(&FxEvent::Waiting { x, y });
            fired = true;
        }
        // Occasional character hop ("ぴょん"). The amount gates the
        // probability of a hop each scheduling window.
        let jump_amt = self
            .cfg
            .effects
            .amounts
            .get("char_jump")
            .unwrap_or(self.cfg.amount);
        if self.hop.is_none() && Instant::now() >= self.next_hop && jump_amt > 0.0 {
            self.next_hop =
                Instant::now() + Duration::from_secs_f32(1.2 + frand(&mut self.hop_seed) * 2.5);
            if frand(&mut self.hop_seed) < jump_amt && self.start_hop() {
                fired = true;
            }
        }
        if fired {
            self.dirty = true;
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        }
    }

    /// Pick a random visible printable cell and start a hop on it.
    fn start_hop(&mut self) -> bool {
        let rows = self.lines.len();
        if rows == 0 || self.snapshot.is_empty() {
            return false;
        }
        let cols = self.snapshot.len() / rows;
        if cols == 0 {
            return false;
        }
        let count = self
            .snapshot
            .iter()
            .filter(|c| c.0 != ' ' && c.0 != '\0')
            .count();
        if count == 0 {
            return false;
        }
        let pick = (frand(&mut self.hop_seed) * count as f32) as usize;
        let idx = self
            .snapshot
            .iter()
            .enumerate()
            .filter(|(_, c)| c.0 != ' ' && c.0 != '\0')
            .nth(pick)
            .map(|(i, _)| i)
            .unwrap_or(0);
        let (ch, fg, bg, bold, zero) = &self.snapshot[idx];
        let mut text = String::from(*ch);
        text.extend(zero.iter());
        let wide = idx + 1 < self.snapshot.len() && self.snapshot[idx + 1].0 == '\0';
        self.hop = Some(Hop {
            idx,
            text,
            fg: *fg,
            bg: *bg,
            bold: *bold,
            wide,
            started: Instant::now(),
            hops_left: if frand(&mut self.hop_seed) < 0.35 {
                2
            } else {
                1
            },
        });
        true
    }

    /// Advance/expire the hop; a second hop may chain while `hops_left` lasts.
    fn update_hop(&mut self) {
        let expired = self
            .hop
            .as_ref()
            .map(|h| h.started.elapsed().as_secs_f32() >= HOP_DUR)
            .unwrap_or(false);
        if !expired {
            return;
        }
        let h = self.hop.as_mut().unwrap();
        if h.hops_left > 1 {
            h.hops_left -= 1;
            h.started = Instant::now();
        } else {
            self.hop = None;
        }
    }

    /// Overlay quads + text for the active hop: a bg-colored cover over the
    /// original cell, then the glyph drawn at its airborne offset.
    fn hop_overlay(&self) -> (Vec<Instance>, Vec<Line>) {
        let Some(h) = &self.hop else {
            return (Vec::new(), Vec::new());
        };
        let Some(r) = self.renderer.as_ref() else {
            return (Vec::new(), Vec::new());
        };
        let rows = self.lines.len();
        if rows == 0 || self.snapshot.is_empty() {
            return (Vec::new(), Vec::new());
        }
        let cols = self.snapshot.len() / rows;
        if cols == 0 {
            return (Vec::new(), Vec::new());
        }
        let row = h.idx / cols;
        let col = h.idx % cols;
        // DOPA_HOP_DEMO freezes the arc at mid-air for screenshot verification.
        let t = if std::env::var_os("DOPA_HOP_DEMO").is_some() {
            0.5
        } else {
            (h.started.elapsed().as_secs_f32() / HOP_DUR).clamp(0.0, 1.0)
        };
        // Parabola up ~1 cell, with a decaying horizontal wobble.
        let off_y = -r.cell_h * (std::f32::consts::PI * t).sin();
        let off_x = 1.5 * (4.0 * std::f32::consts::PI * t).sin() * (1.0 - t);
        let cover_w = r.cell_w * if h.wide { 2.0 } else { 1.0 };
        let cover = Instance {
            pos: [
                col as f32 * r.cell_w + cover_w / 2.0,
                row as f32 * r.cell_h + r.cell_h / 2.0,
            ],
            size: [cover_w, r.cell_h],
            rot: 0.0,
            kind: fx::SHAPE_RECT,
            color: [
                h.bg[0] as f32 / 255.0,
                h.bg[1] as f32 / 255.0,
                h.bg[2] as f32 / 255.0,
                1.0,
            ],
        };
        let line = Line {
            top: row as f32 * r.cell_h + off_y,
            left: col as f32 * r.cell_w + off_x,
            spans: vec![Span {
                text: h.text.clone(),
                fg: h.fg,
                bold: h.bold,
            }],
            family: None,
        };
        (vec![cover], vec![line])
    }

    /// Shatter the character that Backspace/Delete is about to remove, read
    /// straight from the grid at key-press time. The diff-based path may miss
    /// it when a shell erases and repaints within a single PTY burst.
    fn predict_shatter(&mut self, col_offset: isize) {
        let term = self.term.as_ref().unwrap();
        let mut t = term.term.lock();
        let pt = t.grid().cursor.point;
        if pt.line.0 < 0 {
            return;
        }
        let base_col = pt.column.0 as isize + col_offset;
        if base_col < 0 {
            return;
        }
        let default_fg = parse_rgb(&self.cfg.foreground);
        let default_bg = parse_rgb(&self.cfg.background);
        // Backspace scans left (nearest non-space cell), Delete scans right.
        let cols_to_try: Vec<isize> = if col_offset < 0 {
            (0..=8)
                .map(|d| base_col - d)
                .take_while(|c| *c >= 0)
                .collect()
        } else {
            (0..=8).map(|d| base_col + d).collect()
        };
        for col in cols_to_try {
            let target = Point::new(pt.line, Column(col as usize));
            let Some((c, fg)) = doomed_cell(&mut t, target, default_fg, default_bg) else {
                continue;
            };
            let (cw, ch) = {
                let r = self.renderer.as_ref().unwrap();
                (r.cell_w, r.cell_h)
            };
            self.fx.event(&FxEvent::Erased {
                ch: c,
                x: col as f32 * cw + cw / 2.0,
                y: pt.line.0 as f32 * ch + ch / 2.0,
                w: cw,
                h: ch,
                color: to_f32(fg),
            });
            let cols = t.columns();
            self.predicted_erase
                .push((pt.line.0 as usize * cols + col as usize, Instant::now()));
            if self.predicted_erase.len() > 64 {
                self.predicted_erase.drain(..32);
            }
            break;
        }
    }
}

/// Char + resolved fg of the cell at `point`, if it holds a printable glyph.
fn doomed_cell<E: alacritty_terminal::event::EventListener>(
    t: &mut alacritty_terminal::Term<E>,
    point: Point,
    def_fg: [u8; 3],
    def_bg: [u8; 3],
) -> Option<(char, [u8; 4])> {
    let content = t.renderable_content();
    for indexed in content.display_iter {
        if indexed.point.line == point.line && indexed.point.column == point.column {
            let cell = indexed.cell;
            if cell.c != ' ' && !cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                let (fg, _) = cell_colors(cell, content.colors, def_fg, def_bg);
                return Some((cell.c, fg));
            }
            return None;
        }
    }
    None
}

fn p_eq(p: Point, line: usize, col: usize) -> bool {
    p.line.0 == line as i32 && p.column.0 == col
}

/// Seconds per character hop ("ぴょん").
const HOP_DUR: f32 = 0.42;

/// One hopping character, captured from the cell snapshot when it starts.
struct Hop {
    /// Index into `snapshot` (row * cols + col).
    idx: usize,
    /// Glyph text incl. zero-width joiners.
    text: String,
    fg: [u8; 4],
    bg: [u8; 4],
    bold: bool,
    /// Occupies two cells (wide char).
    wide: bool,
    started: Instant,
    hops_left: u8,
}

/// xorshift64* — same shape as the one in fx::builtin, kept local.
fn frand(seed: &mut u64) -> f32 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    (((*seed >> 8) as u32) & 0x00FF_FFFF) as f32 / ((1u32 << 24) as f32)
}

/// A mouse selection over the terminal grid. Lines are buffer coordinates:
/// viewport row minus display_offset (negative reaches into scrollback).
#[derive(Clone, Copy)]
struct Selection {
    anchor: (i32, usize),
    head: (i32, usize),
    dragging: bool,
}

impl Selection {
    fn ordered(&self) -> ((i32, usize), (i32, usize)) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }
}

/// Extract the selected text from a cell snapshot. Rows are joined with
/// newlines; trailing whitespace per row and wide-char spacers are dropped.
fn selected_text(
    snap: &[SnapCell],
    cols: usize,
    rows: usize,
    offset: usize,
    sel: &Selection,
) -> String {
    let ((sl, sc), (el, ec)) = sel.ordered();
    let mut lines_out: Vec<String> = Vec::new();
    for buf in sl..=el {
        let vis = buf + offset as i32;
        if vis < 0 || vis as usize >= rows {
            continue;
        }
        let vis = vis as usize;
        let c0 = if buf == sl { sc } else { 0 }.min(cols - 1);
        let c1 = if buf == el { ec } else { cols - 1 }.min(cols - 1);
        let mut line = String::new();
        for col in c0..=c1 {
            let s = &snap[vis * cols + col];
            if s.0 != '\0' {
                line.push(s.0);
                line.extend(s.4.iter().copied());
            }
        }
        lines_out.push(line.trim_end().to_string());
    }
    lines_out.join("\n")
}

/// Save a clipboard image as PNG and return its path (for CLI agents that
/// read image files from a pasted path).
fn save_clipboard_image(img: &arboard::ImageData) -> Option<String> {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis();
    let path = std::env::temp_dir().join(format!("dopaterm-paste-{ms}.png"));
    let rgba = image::RgbaImage::from_raw(img.width as u32, img.height as u32, img.bytes.to_vec())?;
    rgba.save(&path).ok()?;
    Some(path.to_string_lossy().into_owned())
}

/// `C:\foo\bar` -> `/mnt/c/foo/bar` for shells running inside WSL.
#[cfg(windows)]
fn windows_path_to_wsl(p: &str) -> Option<String> {
    let b = p.as_bytes();
    if b.len() < 4 || !b[0].is_ascii_alphabetic() || b[1] != b':' {
        return None;
    }
    let drive = (b[0] as char).to_ascii_lowercase();
    Some(format!("/mnt/{drive}/{}", p[3..].replace('\\', "/")))
}

/// Decode the bundled app icon for `Window::with_window_icon`.
fn window_icon() -> Option<winit::window::Icon> {
    let png = include_bytes!("../assets/icon.png");
    let img = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    winit::window::Icon::from_rgba(rgba.into_raw(), w, h).ok()
}

/// Text to insert for a dropped file: WSL children get `/mnt/<drive>` paths
/// and paths containing whitespace are double-quoted.
fn drop_path_text(path: &std::path::Path, wsl: bool) -> String {
    let mut s = path.to_string_lossy().into_owned();
    #[cfg(windows)]
    if wsl {
        s = windows_path_to_wsl(&s).unwrap_or(s);
    }
    #[cfg(not(windows))]
    let _ = wsl;
    if s.chars().any(char::is_whitespace) {
        s = format!("\"{s}\"");
    }
    s
}

/// The block cursor spans the whole glyph beneath it: a wide char's spacer
/// cell ('\0') is covered when the cursor sits on its lead cell.
fn cursor_covers(snap: &[SnapCell], cols: usize, cursor: Point, i: usize, col: usize) -> bool {
    p_eq(cursor, i, col) || (col > 0 && snap[i * cols + col].0 == '\0' && p_eq(cursor, i, col - 1))
}

/// (char, fg, bg, bold, zerowidth chars). The last field carries the
/// combining marks / VS16 / ZWJ / emoji modifiers alacritty stores on the
/// cell so shaped text keeps the full grapheme cluster.
type SnapCell = (char, [u8; 4], [u8; 4], bool, Vec<char>);

fn snapshot_grid<E: alacritty_terminal::event::EventListener>(
    term: &alacritty_terminal::Term<E>,
    default_fg: [u8; 3],
    default_bg: [u8; 3],
) -> Vec<SnapCell> {
    let cols = term.columns();
    let rows = term.screen_lines();
    let content = term.renderable_content();
    let fg = [default_fg[0], default_fg[1], default_fg[2], 255];
    let bg = [default_bg[0], default_bg[1], default_bg[2], 255];
    let mut snap = vec![(' ', fg, bg, false, Vec::new()); cols * rows];
    for indexed in content.display_iter {
        let line = indexed.point.line.0 + content.display_offset as i32;
        if line < 0 || line as usize >= rows {
            continue;
        }
        let cell = indexed.cell;
        let (fg, bg) = cell_colors(cell, content.colors, default_fg, default_bg);
        let (ch, zero) = if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            ('\0', Vec::new())
        } else if cell.c == '\t' {
            (' ', Vec::new())
        } else {
            (
                cell.c,
                cell.zerowidth().map(<[char]>::to_vec).unwrap_or_default(),
            )
        };
        snap[line as usize * cols + indexed.point.column.0] =
            (ch, fg, bg, cell.flags.contains(Flags::BOLD), zero);
    }
    snap
}

/// Upper bound on shatter events emitted for one frame's cell changes.
const MAX_ERASE_EVENTS: usize = 12;

/// Cell impacts allowed in one frame for a given effect amount. The settings
/// slider moves in 0.1 steps, so the budget follows it directly: 1 impact at
/// 0.1, all 12 at 1.0, none at 0.
fn erase_budget(amount: f32) -> usize {
    (MAX_ERASE_EVENTS as f32 * amount.clamp(0.0, 1.0)).round() as usize
}

/// Keep every erase for normal editing; for bulk repaints, take an even
/// spread across the changed cells. Dropping the whole frame made updates
/// feel dead, while emitting all of them buried the text.
fn sample_erases<T>(erased: Vec<T>, cap: usize) -> Vec<T> {
    if cap == 0 {
        return Vec::new();
    }
    if erased.len() <= cap {
        return erased;
    }
    let step = erased.len().div_ceil(cap);
    erased.into_iter().step_by(step).collect()
}

/// True when a frame repaints a large share of the grid (scroll, clear,
/// alt-screen redraw). Effects that decorate user actions should stay quiet
/// there: the motion is the screen's, not the user's.
fn is_heavy_update(scrolled: bool, changed_cells: usize, cells: usize) -> bool {
    scrolled || changed_cells * 8 >= cells
}

fn row_has_content(row: &[SnapCell]) -> bool {
    row.iter().any(|c| !matches!(c.0, ' ' | '\0'))
}

/// Detect an internal scroll in the visible grid: pagers and TUIs (less, vim)
/// move content by repainting rows, so `display_offset` never changes. Returns
/// the shift with `Scrolled` sign (positive = text moved down). `min_shift`
/// lets the alt screen accept one-line wheel steps while the primary screen
/// leaves one-line output scrolling to `OutputRain`.
fn detect_scroll_shift(
    old: &[SnapCell],
    new: &[SnapCell],
    cols: usize,
    rows: usize,
    min_shift: usize,
) -> Option<i32> {
    if cols == 0 || rows < 4 || old.len() != cols * rows || new.len() != cols * rows || old == new {
        return None;
    }
    let old_rows: Vec<&[SnapCell]> = old.chunks_exact(cols).collect();
    let new_rows: Vec<&[SnapCell]> = new.chunks_exact(cols).collect();
    let mut best: Option<(usize, i32)> = None;
    for k in min_shift.max(1)..=rows / 2 {
        let span = rows - k;
        if span < 3 {
            break;
        }
        let mut up = 0usize;
        let mut up_content = 0usize;
        let mut down = 0usize;
        let mut down_content = 0usize;
        for i in 0..span {
            if new_rows[i] == old_rows[i + k] {
                up += 1;
                if row_has_content(new_rows[i]) {
                    up_content += 1;
                }
            }
            if new_rows[i + k] == old_rows[i] {
                down += 1;
                if row_has_content(new_rows[i + k]) {
                    down_content += 1;
                }
            }
        }
        let enough = span * 8;
        if up * 10 >= enough && up_content >= 2 && best.is_none_or(|(s, _)| up > s) {
            best = Some((up, -(k as i32)));
        }
        if down * 10 >= enough && down_content >= 2 && best.is_none_or(|(s, _)| down > s) {
            best = Some((down, k as i32));
        }
    }
    best.map(|(_, lines)| lines)
}

/// Cells that disappeared or changed between snapshots (candidate impacts).
/// `include_overwrites` also returns in-place character replacements, which
/// only unlocks at the top of the dial so ordinary updates get debris too.
/// Returns (cell_index, old_char, old_fg). Suppressed when more than a third
/// of the grid changed (scroll/redraw storms aren't deletions).
fn find_erased(
    old: &[SnapCell],
    new: &[SnapCell],
    include_overwrites: bool,
) -> Vec<(usize, char, [u8; 4])> {
    if old.len() != new.len() || new.is_empty() {
        return Vec::new();
    }
    let mut changed = 0usize;
    let mut out = Vec::new();
    for (i, (o, n)) in old.iter().zip(new.iter()).enumerate() {
        if o != n {
            changed += 1;
            let gone = !matches!(o.0, ' ' | '\0') && matches!(n.0, ' ' | '\0');
            let replaced = include_overwrites && !matches!(o.0, ' ' | '\0') && o.0 != n.0;
            if gone || replaced {
                out.push((i, o.0, o.1));
            }
        }
    }
    if changed * 3 >= new.len() {
        out.clear();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::term_core::Dims;
    use alacritty_terminal::Term;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::vte::ansi::Processor;

    fn cell(c: char) -> SnapCell {
        (c, [255; 4], [0, 0, 0, 255], false, Vec::new())
    }

    fn feed(t: &mut Term<VoidListener>, bytes: &[u8]) {
        let mut p = Processor::<alacritty_terminal::vte::ansi::StdSyncHandler>::new();
        p.advance(t, bytes);
    }

    #[test]
    fn fullwidth_spacer_keeps_the_character_background() {
        let mut term = Term::new(
            alacritty_terminal::term::Config::default(),
            &Dims { cols: 8, lines: 3 },
            VoidListener,
        );
        feed(&mut term, "\x1b[48;2;38;42;49m日本".as_bytes());
        let snap = snapshot_grid(&term, [255; 3], [29, 31, 33]);
        assert_eq!(snap[0].2, [38, 42, 49, 255]);
        assert_eq!(snap[1].2, snap[0].2);
        assert_eq!(snap[3].2, snap[2].2);
        assert_eq!(snap[1].0, '\0');
        assert_eq!(
            snap[..4]
                .iter()
                .map(|cell| cell.0)
                .filter(|ch| *ch != '\0')
                .collect::<String>(),
            "日本"
        );
    }

    #[test]
    fn zerowidth_chars_travel_with_their_cell() {
        // VS16 and emoji modifiers are zero-width: alacritty stores them on
        // the preceding cell, and the snapshot must carry them so shaping
        // can form the full emoji cluster.
        let mut term = Term::new(
            alacritty_terminal::term::Config::default(),
            &Dims { cols: 8, lines: 2 },
            VoidListener,
        );
        feed(&mut term, "A\u{2764}\u{FE0F}e\u{0301}\u{1F44D}".as_bytes());
        let snap = snapshot_grid(&term, [255; 3], [0; 3]);
        assert_eq!(snap[0].0, 'A');
        assert_eq!(
            (snap[1].0, snap[1].4.as_slice()),
            ('\u{2764}', &['\u{FE0F}'][..])
        );
        assert_eq!((snap[2].0, snap[2].4.as_slice()), ('e', &['\u{0301}'][..]));
        assert_eq!(snap[3].0, '\u{1F44D}');
        assert_eq!(snap[4].0, '\0');
        // Zerowidth-only changes still count as a diff for dirty-row marking.
        let mut other = snap.clone();
        other[1].4.clear();
        assert_ne!(snap[1], other[1]);
    }

    #[test]
    fn scrollback_cells_are_mapped_to_viewport_rows() {
        let mut term = Term::new(
            alacritty_terminal::term::Config::default(),
            &Dims { cols: 8, lines: 2 },
            VoidListener,
        );
        feed(&mut term, b"\x1b[48;2;38;42;49mA\r\nB\r\nC");
        term.scroll_display(Scroll::Delta(1));
        assert_eq!(term.renderable_content().display_offset, 1);
        let snap = snapshot_grid(&term, [255; 3], [29, 31, 33]);
        assert_eq!(snap[0].0, 'A');
        assert_eq!(snap[8].0, 'B');
        assert_eq!(snap[0].2, [38, 42, 49, 255]);
        term.scroll_display(Scroll::Bottom);
        let snap = snapshot_grid(&term, [255; 3], [29, 31, 33]);
        assert_eq!(snap[0].0, 'B');
        assert_eq!(snap[8].0, 'C');
    }

    #[test]
    fn block_cursor_covers_both_cells_of_a_wide_char() {
        let mut term = Term::new(
            alacritty_terminal::term::Config::default(),
            &Dims { cols: 8, lines: 3 },
            VoidListener,
        );
        feed(&mut term, "日本ab".as_bytes());
        let snap = snapshot_grid(&term, [255; 3], [0; 3]);
        let line = alacritty_terminal::index::Line(0);

        // Cursor on '本' (lead col 2, spacer col 3): both cells are covered.
        let on_hon = Point::new(line, Column(2));
        assert!(cursor_covers(&snap, 8, on_hon, 0, 2));
        assert!(cursor_covers(&snap, 8, on_hon, 0, 3));
        assert!(!cursor_covers(&snap, 8, on_hon, 0, 0));
        assert!(!cursor_covers(&snap, 8, on_hon, 0, 4));

        // Cursor on 'a' covers only its own cell.
        let on_a = Point::new(line, Column(4));
        assert!(cursor_covers(&snap, 8, on_a, 0, 4));
        assert!(!cursor_covers(&snap, 8, on_a, 0, 5));

        // A spacer cell whose lead is not the cursor is never covered.
        assert!(!cursor_covers(&snap, 8, on_a, 0, 1));
    }

    #[test]
    fn selection_extracts_visible_text() {
        // 6 cols x 3 rows; row 1 has a wide-char spacer at col 1.
        let mut snap = Vec::new();
        for c in "hello ".chars() {
            snap.push(cell(c));
        }
        for (i, c) in "aXXc  ".chars().enumerate() {
            snap.push(cell(if i == 1 { '\0' } else { c }));
        }
        for c in "tail  ".chars() {
            snap.push(cell(c));
        }
        let sel = Selection {
            anchor: (1, 1),
            head: (2, 3),
            dragging: false,
        };
        assert_eq!(selected_text(&snap, 6, 3, 0, &sel), "Xc\ntail");

        // Reversed drag order selects the same region.
        let rev = Selection {
            anchor: (2, 3),
            head: (1, 1),
            dragging: false,
        };
        assert_eq!(selected_text(&snap, 6, 3, 0, &rev), "Xc\ntail");

        // Buffer-line coordinates account for the scroll offset.
        let top = Selection {
            anchor: (0, 0),
            head: (0, 3),
            dragging: false,
        };
        assert_eq!(selected_text(&snap, 6, 3, 1, &top), "aXc");
    }

    #[test]
    fn dropped_file_path_is_quoted_when_it_has_whitespace() {
        assert_eq!(
            drop_path_text(std::path::Path::new("/tmp/a b.png"), false),
            "\"/tmp/a b.png\""
        );
        assert_eq!(
            drop_path_text(std::path::Path::new("/tmp/ab.png"), false),
            "/tmp/ab.png"
        );
    }

    #[test]
    #[cfg(windows)]
    fn dropped_file_path_translates_for_wsl() {
        assert_eq!(
            drop_path_text(std::path::Path::new("C:\\tmp\\a.png"), true),
            "/mnt/c/tmp/a.png"
        );
        assert_eq!(
            drop_path_text(std::path::Path::new("C:\\tmp\\a b.png"), true),
            "\"/mnt/c/tmp/a b.png\""
        );
    }

    #[test]
    #[cfg(windows)]
    fn windows_path_converts_to_wsl_mount() {
        assert_eq!(
            windows_path_to_wsl("C:\\Users\\y\\AppData\\Local\\Temp\\x.png").unwrap(),
            "/mnt/c/Users/y/AppData/Local/Temp/x.png"
        );
        assert!(windows_path_to_wsl("/tmp/x.png").is_none());
        assert!(windows_path_to_wsl("\\\\wsl$\\Ubuntu\\x").is_none());
    }

    #[test]
    fn tabs_are_represented_by_the_grid_spaces_not_a_second_tab_stop() {
        let mut term = Term::new(
            alacritty_terminal::term::Config::default(),
            &Dims { cols: 16, lines: 2 },
            VoidListener,
        );
        feed(&mut term, b"A\tB");
        let snap = snapshot_grid(&term, [255; 3], [0; 3]);
        assert_eq!(
            snap[..9].iter().map(|cell| cell.0).collect::<String>(),
            "A       B"
        );
    }

    #[test]
    fn spacer_removal_does_not_spawn_a_glyph_particle() {
        let mut old = vec![cell(' '); 12];
        old[2] = cell('\0');
        assert!(find_erased(&old, &vec![cell(' '); 12], false).is_empty());
    }

    #[test]
    fn doomed_cell_reads_grid() {
        let mut t = Term::new(
            alacritty_terminal::term::Config::default(),
            &Dims {
                cols: 80,
                lines: 24,
            },
            VoidListener,
        );
        feed(&mut t, b"$ ab");
        // Cursor sits at col 4; the cell a Backspace would delete is col 3 ('b').
        let pt = t.grid().cursor.point;
        assert_eq!(pt.column.0, 4);
        let got = doomed_cell(&mut t, Point::new(pt.line, Column(3)), [255; 3], [0; 3]);
        assert_eq!(got.map(|g| g.0), Some('b'));
        // Blank cell -> None.
        let blank = doomed_cell(&mut t, Point::new(pt.line, Column(4)), [255; 3], [0; 3]);
        assert!(blank.is_none());
    }

    #[test]
    fn backspace_erase_detected() {
        // "$ x" then x erased -> single erase at index 2.
        let old: Vec<SnapCell> = vec![cell('$'), cell(' '), cell('x'), cell(' ')];
        let new: Vec<SnapCell> = vec![cell('$'), cell(' '), cell(' '), cell(' ')];
        let er = find_erased(&old, &new, false);
        assert_eq!(er.len(), 1);
        assert_eq!(er[0].0, 2);
        assert_eq!(er[0].1, 'x');
    }

    #[test]
    fn overwrite_is_not_erase() {
        let old: Vec<SnapCell> = vec![cell('a'), cell(' ')];
        let new: Vec<SnapCell> = vec![cell('b'), cell(' ')];
        assert!(find_erased(&old, &new, false).is_empty());
    }

    #[test]
    fn top_amount_shatters_replaced_text_too() {
        // Two replaced cells out of ten stay below the 1/3 redraw gate.
        let mut old = vec![cell(' '); 10];
        old[0] = cell('a');
        old[1] = cell('b');
        let mut new = vec![cell(' '); 10];
        new[0] = cell('c');
        new[1] = cell('d');
        assert!(find_erased(&old, &new, false).is_empty());
        assert_eq!(find_erased(&old, &new, true).len(), 2);
    }

    #[test]
    fn internal_scrolls_are_detected() {
        let cols = 4;
        let rows = 8;
        let make = |chars: [char; 8]| -> Vec<SnapCell> {
            chars
                .iter()
                .flat_map(|&c| (0..cols).map(move |_| cell(c)))
                .collect()
        };
        let old = make(['A', 'B', 'C', 'D', 'E', 'F', 'G', 'H']);
        assert_eq!(
            detect_scroll_shift(
                &old,
                &make(['C', 'D', 'E', 'F', 'G', 'H', 'X', 'Y']),
                cols,
                rows,
                1
            ),
            Some(-2)
        );
        assert_eq!(
            detect_scroll_shift(
                &old,
                &make(['X', 'Y', 'A', 'B', 'C', 'D', 'E', 'F']),
                cols,
                rows,
                1
            ),
            Some(2)
        );
        // One-line wheel steps count in the alt screen...
        let one = make(['B', 'C', 'D', 'E', 'F', 'G', 'H', 'X']);
        assert_eq!(detect_scroll_shift(&old, &one, cols, rows, 1), Some(-1));
        // ...but not on the primary screen (OutputRain covers that).
        assert_eq!(detect_scroll_shift(&old, &one, cols, rows, 2), None);
        assert_eq!(
            detect_scroll_shift(
                &old,
                &make(['1', '2', '3', '4', '5', '6', '7', '8']),
                cols,
                rows,
                1
            ),
            None
        );
    }

    #[test]
    fn full_repaint_suppressed() {
        // 2/4 cells changed incl. erases -> exceeds 1/3 threshold.
        let old: Vec<SnapCell> = vec![cell('a'), cell('b'), cell('c'), cell('d')];
        let new: Vec<SnapCell> = vec![cell(' '), cell('x'), cell(' '), cell('y')];
        assert!(find_erased(&old, &new, false).is_empty());
    }

    #[test]
    fn size_mismatch_ignored() {
        let old: Vec<SnapCell> = vec![cell('a')];
        let new: Vec<SnapCell> = vec![cell(' '), cell(' ')];
        assert!(find_erased(&old, &new, false).is_empty());
    }

    #[test]
    fn bulk_repaints_are_heavy_but_typing_is_not() {
        assert!(is_heavy_update(true, 0, 1920));
        assert!(is_heavy_update(false, 240, 1920));
        assert!(!is_heavy_update(false, 2, 1920));
    }

    #[test]
    fn bulk_erases_are_sampled_not_dropped() {
        let all: Vec<usize> = (0..100).collect();
        let kept = sample_erases(all.clone(), MAX_ERASE_EVENTS);
        assert_eq!(kept.len(), MAX_ERASE_EVENTS);
        assert_eq!(kept[0], 0);
        assert_eq!(*kept.last().unwrap(), 99);

        let few = sample_erases(all[..5].to_vec(), MAX_ERASE_EVENTS);
        assert_eq!(few, all[..5]);
        assert!(sample_erases(all, 0).is_empty());
    }

    #[test]
    fn erase_budget_tracks_the_master_amount() {
        assert_eq!(erase_budget(0.0), 0);
        assert_eq!(erase_budget(0.1), 1);
        assert_eq!(erase_budget(0.5), 6);
        assert_eq!(erase_budget(1.0), MAX_ERASE_EVENTS);
        // Per-effect overrides are clamped like the master slider.
        assert_eq!(erase_budget(2.0), MAX_ERASE_EVENTS);
    }

    #[test]
    fn a_moderate_redraw_still_shatters_a_few_cells() {
        // 100 of 1000 cells go blank: enough to be an update, not a scroll.
        let mut old = vec![cell(' '); 1000];
        old[..100].fill(cell('x'));
        let new = vec![cell(' '); 1000];
        let erased = find_erased(&old, &new, false);
        assert_eq!(erased.len(), 100);
        assert_eq!(
            sample_erases(erased, erase_budget(1.0)).len(),
            MAX_ERASE_EVENTS
        );
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("dopaterm")
            .with_window_icon(window_icon())
            .with_inner_size(winit::dpi::LogicalSize::new(960.0, 600.0));
        let window = Arc::new(el.create_window(attrs).expect("create window"));
        window.set_ime_allowed(true);
        let mut renderer = pollster::block_on(Renderer::new(
            Arc::clone(&window),
            self.cfg.font_size,
            self.cfg.font_family.clone(),
        ))
        .expect("init renderer");
        // Screenshots are pinned to scale 1 so verification scripts can
        // assume fixed cell geometry regardless of the monitor's DPI.
        if self.shot_path.is_some() {
            renderer.set_scale_factor(1.0);
        }
        self.cfg.font_family = renderer.font_family().map(str::to_string);

        let size = window.inner_size();
        let window_size = WindowSize {
            num_cols: (size.width as f32 / renderer.cell_w) as u16,
            num_lines: (size.height as f32 / renderer.cell_h) as u16,
            cell_width: renderer.cell_w as u16,
            cell_height: renderer.cell_h as u16,
        };

        let shell = match &self.cfg.shell {
            Some(prog) => Some(Shell::new(prog.clone(), self.cfg.shell_args.clone())),
            None => default_shell(),
        };
        let proxy = EventProxy::new(self.proxy.clone());
        let term = TermCore::spawn(window_size, shell, proxy).expect("spawn pty");

        // Built-in + Lua effects.
        self.update_effects();

        self.window = Some(window);
        self.renderer = Some(renderer);
        self.term = Some(term);

        // 200ms heartbeat for idle/command-finish detection.
        let wake_proxy = self.proxy.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(200));
                if wake_proxy.send_event(UserEvent::Wake).is_err() {
                    break;
                }
            }
        });
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, ev: WindowEvent) {
        if let Some(t) = self.pending_exit {
            if Instant::now() >= t {
                el.exit();
                return;
            }
        }
        let Some(term) = &self.term else { return };
        match ev {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                // Font size is configured in points; re-measure cells so
                // text keeps its size when the window moves to a display
                // with a different DPI.
                if self.shot_path.is_none() {
                    if let Some(r) = &mut self.renderer {
                        r.set_scale_factor(scale_factor);
                    }
                    self.resize_terminal_to_window();
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(r) = &mut self.renderer {
                    // surface.configure can panic on a dead device.
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        r.resize(size.width, size.height)
                    }));
                    let ws = WindowSize {
                        num_cols: ((size.width as f32 / r.cell_w) as u16).max(1),
                        num_lines: ((size.height as f32 / r.cell_h) as u16).max(1),
                        cell_width: r.cell_w as u16,
                        cell_height: r.cell_h as u16,
                    };
                    term.resize(ws);
                    self.snapshot.clear();
                    self.dirty = true;
                }
            }
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }
                if !event.repeat
                    && (event.logical_key == Key::Named(NamedKey::F1)
                        || (self.mods.control_key()
                            && self.mods.shift_key()
                            && matches!(&event.logical_key, Key::Character(c) if c == "," || c == "<")))
                {
                    self.show_settings = !self.show_settings;
                    self.settings_fonts = None;
                    self.sync_ime();
                    self.dirty = true;
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                    return;
                }
                if self.show_settings {
                    if event.logical_key == Key::Named(NamedKey::Escape) {
                        if self.settings_fonts.take().is_none() {
                            self.show_settings = false;
                        }
                        self.sync_ime();
                        self.dirty = true;
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                    } else {
                        self.handle_settings_key(&event);
                    }
                    return;
                }
                if self.ime_composing {
                    return;
                }
                let (app_cursor, alt_screen) = {
                    let t = term.term.lock();
                    (
                        t.mode().contains(TermMode::APP_CURSOR),
                        t.mode().contains(TermMode::ALT_SCREEN),
                    )
                };
                // Paste / copy shortcuts fire once per press, not per repeat.
                if !event.repeat && self.mods.control_key() && self.mods.shift_key() {
                    if let winit::keyboard::Key::Character(c) = &event.logical_key {
                        if c.eq_ignore_ascii_case("v") {
                            self.paste_clipboard(term);
                            return;
                        }
                        if c.eq_ignore_ascii_case("c") {
                            self.copy_selection();
                            return;
                        }
                    }
                }
                if let Some(d) = decode_key(&event, self.mods, app_cursor) {
                    term.write(&d.bytes);
                    term.term.lock().scroll_display(Scroll::Bottom);
                    let (x, y) = self.last_cursor_px;
                    self.fx.event(&FxEvent::Key {
                        kind: d.kind,
                        ch: d.ch,
                        x,
                        y,
                    });
                    let shatter_col = match &event.logical_key {
                        Key::Named(NamedKey::Backspace) => Some(-1),
                        Key::Named(NamedKey::Delete) => Some(0),
                        _ => None,
                    };
                    if let Some(off) = shatter_col {
                        self.predict_shatter(off);
                    }
                    self.last_activity = Instant::now();
                    self.last_input = self.last_activity;
                    // Enter on the normal screen marks a possible command.
                    if d.kind == KeyKind::Enter && !alt_screen {
                        self.command_pending = true;
                    }
                    if d.kind == KeyKind::Enter && !event.repeat {
                        let w = self.window.as_ref().unwrap().inner_size();
                        self.fx.event(&FxEvent::Confetti {
                            x,
                            y,
                            w: w.width as f32,
                            h: w.height as f32,
                        });
                    }
                    self.dirty = true;
                    self.window.as_ref().unwrap().request_redraw();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let Some(r) = self.renderer.as_ref() else {
                    return;
                };
                let x = position.x as f32;
                let y = position.y as f32;
                self.mouse_px = (x, y);
                self.mouse_cell = ((x / r.cell_w) as usize, (y / r.cell_h) as usize);
                if self.show_settings {
                    return;
                }
                if self.selection.as_ref().is_some_and(|s| s.dragging) {
                    let off = term.term.lock().grid().display_offset() as i32;
                    let sel = self.selection.as_mut().unwrap();
                    sel.head = (self.mouse_cell.1 as i32 - off, self.mouse_cell.0);
                    self.dirty = true;
                    self.window.as_ref().unwrap().request_redraw();
                    return;
                }
                let mode = self.mouse_mode(term);
                if !self.mods.shift_key()
                    && (mode.contains(TermMode::MOUSE_MOTION)
                        || (mode.contains(TermMode::MOUSE_DRAG) && self.mouse_button.is_some()))
                {
                    let b = self.mouse_button.unwrap_or(3) | 32;
                    let (col, line) = self.mouse_cell;
                    self.send_mouse_event(term, b, col, line, false);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if self.show_settings {
                    if state == ElementState::Pressed && button == WinitMouseButton::Left {
                        self.handle_settings_click();
                    }
                    return;
                }
                let mode = self.mouse_mode(term);
                let tracking = crate::mouse::tracking_enabled(mode);
                // Terminal-side selection: used when the app is not tracking
                // the mouse, or forced with Shift while it is.
                if button == WinitMouseButton::Left {
                    if state == ElementState::Pressed && (!tracking || self.mods.shift_key()) {
                        let off = term.term.lock().grid().display_offset() as i32;
                        let buf = self.mouse_cell.1 as i32 - off;
                        self.selection = Some(Selection {
                            anchor: (buf, self.mouse_cell.0),
                            head: (buf, self.mouse_cell.0),
                            dragging: true,
                        });
                        self.dirty = true;
                        self.window.as_ref().unwrap().request_redraw();
                        return;
                    }
                    if state == ElementState::Released
                        && self.selection.as_ref().is_some_and(|s| s.dragging)
                    {
                        let sel = self.selection.as_mut().unwrap();
                        sel.dragging = false;
                        if sel.anchor == sel.head {
                            self.selection = None;
                        }
                        self.dirty = true;
                        self.window.as_ref().unwrap().request_redraw();
                        return;
                    }
                }
                if !tracking {
                    return;
                }
                let btn = match button {
                    WinitMouseButton::Left => 0,
                    WinitMouseButton::Middle => 1,
                    WinitMouseButton::Right => 2,
                    _ => return,
                };
                let (col, line) = self.mouse_cell;
                let release = state == ElementState::Released;
                if release {
                    if let Some(b) = self.mouse_button.take() {
                        self.send_mouse_event(term, b, col, line, true);
                    }
                } else {
                    self.send_mouse_event(term, btn, col, line, false);
                    self.mouse_button = Some(btn);
                }
            }
            WindowEvent::Ime(ime) => {
                if self.show_settings {
                    self.ime_preedit.clear();
                    self.ime_composing = false;
                    return;
                }
                let starting_composition =
                    cfg!(windows) && matches!(&ime, winit::event::Ime::Enabled);
                if let Some(text) = self.ime_preedit.update(ime) {
                    term.write(text.as_bytes());
                    term.term.lock().scroll_display(Scroll::Bottom);
                    let (x, y) = self.last_cursor_px;
                    self.fx.event(&FxEvent::Key {
                        kind: KeyKind::Char,
                        ch: text.chars().next(),
                        x,
                        y,
                    });
                }
                self.ime_composing = starting_composition || self.ime_preedit.active();
                self.last_activity = Instant::now();
                self.last_input = self.last_activity;
                self.dirty = true;
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::Focused(focused) => {
                if focused {
                    // Re-assert ime_allowed: the IMM32 context can go stale
                    // across focus loss and refuse to open the IME.
                    self.sync_ime();
                } else {
                    self.ime_preedit.clear();
                    self.ime_composing = false;
                    self.mods = ModifiersState::empty();
                }
                self.dirty = true;
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::DroppedFile(path) => {
                let text = drop_path_text(&path, self.is_vt_bridge_child());
                self.paste_text(term, &text);
                self.dirty = true;
                self.window.as_ref().unwrap().request_redraw();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if self.show_settings {
                    // The wheel scrolls the font picker page; the main
                    // settings page fits without scrolling.
                    if self.settings_fonts.is_some() {
                        let lines = match delta {
                            MouseScrollDelta::LineDelta(_, y) => y as i32,
                            MouseScrollDelta::PixelDelta(p) => (p.y as f32 / 20.0).round() as i32,
                        };
                        if lines != 0 {
                            let total = self
                                .renderer
                                .as_ref()
                                .map(|r| r.font_families().len())
                                .unwrap_or(0);
                            let max = total.saturating_sub(settings_ui::FONT_LIST_ROWS) as i32;
                            let cur = self.settings_fonts.unwrap_or(0) as i32;
                            self.settings_fonts = Some((cur - lines).clamp(0, max) as usize);
                            self.dirty = true;
                            self.window.as_ref().unwrap().request_redraw();
                        }
                    }
                    return;
                }
                let mode = self.mouse_mode(term);
                if crate::mouse::tracking_enabled(mode) && !self.mods.shift_key() {
                    let button = match delta {
                        MouseScrollDelta::LineDelta(_, y) if y > 0.0 => 64,
                        MouseScrollDelta::LineDelta(_, y) if y < 0.0 => 65,
                        MouseScrollDelta::PixelDelta(p) if p.y > 0.0 => 64,
                        MouseScrollDelta::PixelDelta(p) if p.y < 0.0 => 65,
                        _ => return,
                    };
                    let (col, line) = self.mouse_cell;
                    self.send_mouse_event(term, button, col, line, false);
                    return;
                }
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => -y as i32,
                    MouseScrollDelta::PixelDelta(p) => -(p.y as f32 / 20.0).round() as i32,
                };
                if lines != 0 {
                    let moved = {
                        let mut t = term.term.lock();
                        let before = t.grid().display_offset() as i32;
                        t.scroll_display(Scroll::Delta(lines));
                        t.grid().display_offset() as i32 - before
                    };
                    // Only report actual movement: scrolling at either end
                    // must not fire streaks.
                    if moved != 0 {
                        self.pending_scroll += moved;
                        self.dirty = true;
                        self.window.as_ref().unwrap().request_redraw();
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                if let Some(t) = self.pending_exit {
                    if Instant::now() >= t {
                        el.exit();
                        return;
                    }
                }
                self.frames += 1;
                let now = Instant::now();
                let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.1);
                self.last_frame = now;

                self.build_frame();
                self.update_hop();
                let animating = self.fx.tick(dt);
                let fx_instances: Vec<Instance> = self.fx.instances().to_vec();
                let clear = crate::colors::srgb_to_linear(crate::colors::to_f32_3(self.frame_bg));
                let (overlay_bg, overlay_lines, hits) = self.build_overlay();
                self.settings_hits = hits;
                self.draw_quads.clear();
                self.draw_quads.extend_from_slice(&self.bgs);
                self.draw_quads.extend_from_slice(&self.sel_quads);
                let mut rendered = false;
                if let Some(r) = &mut self.renderer {
                    // A wgpu panic here would unwind through the winit
                    // callback and kill the event loop — the exact "frozen
                    // window" symptom — so treat it as a render failure too.
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        r.render(
                            clear,
                            &self.draw_quads,
                            &self.lines,
                            &self.dirty_lines,
                            &fx_instances,
                            &overlay_bg,
                            &overlay_lines,
                        )
                    }));
                    match result {
                        Ok(Ok(FrameOutcome::Presented)) => {
                            self.render_failures = 0;
                            self.gpu_resets = 0;
                            self.skipped_frames = 0;
                            if !self.presented_after_reset {
                                self.presented_after_reset = true;
                                eprintln!("dopaterm: frame presented after GPU recovery");
                            }
                            rendered = true;
                        }
                        Ok(Ok(FrameOutcome::Skipped(why))) => {
                            self.skipped_frames += 1;
                            if self.skipped_frames <= 10 {
                                eprintln!("dopaterm: frame skipped: {why}");
                            }
                            // A rebuilt renderer whose frames never reach
                            // the screen is presenting to a dead window —
                            // escalate to a window recreate once per reset.
                            let recent_reset = self
                                .last_renderer_reset
                                .is_some_and(|t| t.elapsed() < Duration::from_secs(10));
                            if self.skipped_frames >= 60 && recent_reset {
                                eprintln!(
                                    "dopaterm: frames not reaching window; recreating window"
                                );
                                self.skipped_frames = 0;
                                self.recreate_window_and_renderer(el);
                            } else if recent_reset {
                                self.window.as_ref().unwrap().request_redraw();
                            }
                        }
                        Ok(Err(e)) => self.note_render_failure(&e.to_string(), el),
                        Err(_) => self.note_render_failure("render panicked", el),
                    }
                }
                // Only clear dirty rows when the frame actually presented —
                // a dropped frame must re-upload its changed rows next time.
                if rendered {
                    self.dirty_lines.iter_mut().for_each(|d| *d = false);
                }
                if animating || self.dirty || self.shot_path.is_some() || self.hop.is_some() {
                    self.window.as_ref().unwrap().request_redraw();
                    self.dirty = false;
                }

                // Optional synthetic mouse test for WSL/Linux verification.
                if std::env::var_os("DOPA_MOUSE_DEMO").is_some()
                    && self.frames == 30
                    && !self.show_settings
                    && !self.mouse_demo_done
                {
                    self.simulate_mouse_demo();
                    return;
                }

                // Screenshot mode: let the shell settle, fire demo effects,
                // then capture a frame and exit.
                if let Some(path) = self.shot_path.clone() {
                    if self.frames == 6 && !self.demo_injected {
                        self.demo_injected = true;
                        let (x, y) = self.last_cursor_px;
                        let r = self.renderer.as_ref().unwrap();
                        let (cw, ch) = (r.cell_w, r.cell_h);
                        let ws = self.window.as_ref().unwrap().inner_size();
                        for _ in 0..4 {
                            self.fx.event(&FxEvent::Key {
                                kind: crate::input::KeyKind::Char,
                                ch: Some('x'),
                                x,
                                y,
                            });
                        }
                        self.fx.event(&FxEvent::Erased {
                            ch: 'x',
                            x: x + cw * 3.0,
                            y,
                            w: cw,
                            h: ch,
                            color: [1.0, 0.5, 0.3, 1.0],
                        });
                        if std::env::var_os("DOPA_ERASE_DEMO").is_some() {
                            // Four impacts total: the 4th is a nova at max.
                            for i in 1..=3 {
                                self.fx.event(&FxEvent::Erased {
                                    ch: 'x',
                                    x: x + cw * 3.0 + cw * 9.0 * i as f32,
                                    y: y + ch * 3.0 * i as f32,
                                    w: cw,
                                    h: ch,
                                    color: [1.0, 0.5 + 0.1 * i as f32, 0.3, 1.0],
                                });
                            }
                        }
                        self.fx.event(&FxEvent::CursorMoved {
                            x,
                            y,
                            w: cw,
                            h: ch,
                            dx: cw * 8.0,
                            dy: ch * 2.0,
                        });
                        self.fx.event(&FxEvent::Bell {
                            x: x + cw * 8.0,
                            y: y + ch * 2.0,
                            w: cw,
                            h: ch,
                        });
                        self.fx.event(&FxEvent::CommandDone {
                            x: x + 380.0,
                            y: y + 340.0,
                        });
                        self.fx.event(&FxEvent::Waiting { x, y });
                        self.fx.event(&FxEvent::Confetti {
                            x: x + 260.0,
                            y: y + 120.0,
                            w: ws.width as f32,
                            h: ws.height as f32,
                        });
                        if std::env::var_os("DOPA_HOP_DEMO").is_some() {
                            self.start_hop();
                        }
                        self.fx.event(&FxEvent::ChildError {
                            x: x + 220.0,
                            y: y + 180.0,
                            w: ws.width as f32,
                            h: ws.height as f32,
                            status: 1,
                        });
                        self.window.as_ref().unwrap().request_redraw();
                    } else if self.frames >= if self.demo_injected { 12 } else { 30 }
                        && (self.frames >= 150
                            || self.snapshot.iter().any(|c| c.0 != ' ')
                            || self.demo_injected)
                    {
                        if std::env::var_os("DOPA_BACKGROUND_SCROLL_DEMO").is_some() {
                            if let Some(term) = &self.term {
                                term.term.lock().scroll_display(Scroll::Delta(3));
                            }
                        }
                        if std::env::var_os("DOPA_SETTINGS_DEMO").is_some() {
                            self.show_settings = true;
                            self.sync_ime();
                        }
                        if std::env::var_os("DOPA_FONTLIST_DEMO").is_some() {
                            self.show_settings = true;
                            self.sync_ime();
                            self.open_font_list();
                        }
                        if std::env::var_os("DOPA_IME_DEMO").is_some() {
                            let text = "日本語入力中".to_string();
                            let end = text.len();
                            self.ime_preedit
                                .update(winit::event::Ime::Preedit(text, Some((end, end))));
                        }
                        // Feed literal bytes (incl. escapes) into the grid —
                        // verifies rendering independent of the shell.
                        if let Ok(feed) = std::env::var("DOPA_FEED") {
                            let mut p = alacritty_terminal::vte::ansi::Processor::<
                                alacritty_terminal::vte::ansi::StdSyncHandler,
                            >::new();
                            let arc = self.term.as_ref().unwrap().term.clone();
                            p.advance(&mut *arc.lock(), feed.as_bytes());
                        }
                        self.build_frame();
                        // Advance particles so the still captures the burst
                        // spread instead of every effect sitting at its origin.
                        for _ in 0..27 {
                            self.fx.tick(1.0 / 60.0);
                        }
                        // Scroll streaks are short-lived, so fire them after
                        // the settle ticks; the still must catch them alive.
                        if let Ok(v) = std::env::var("DOPA_SCROLL_DEMO") {
                            let ws = self.window.as_ref().unwrap().inner_size();
                            self.fx.event(&FxEvent::Scrolled {
                                lines: v.parse().unwrap_or(6),
                                w: ws.width as f32,
                                h: ws.height as f32,
                            });
                        }
                        // Same for the Enter burst's beams, which fade first.
                        if std::env::var_os("DOPA_ENTER_DEMO").is_some() {
                            let (x, y) = self.last_cursor_px;
                            let ws = self.window.as_ref().unwrap().inner_size();
                            self.fx.event(&FxEvent::Confetti {
                                x,
                                y,
                                w: ws.width as f32,
                                h: ws.height as f32,
                            });
                        }
                        let fx_instances: Vec<Instance> = self.fx.instances().to_vec();
                        let w = self.window.as_ref().unwrap().inner_size();
                        let clear =
                            crate::colors::srgb_to_linear(crate::colors::to_f32_3(self.frame_bg));
                        let (overlay_bg, overlay_lines, hits) = self.build_overlay();
                        self.settings_hits = hits;
                        self.draw_quads.clear();
                        self.draw_quads.extend_from_slice(&self.bgs);
                        self.draw_quads.extend_from_slice(&self.sel_quads);
                        let r = self.renderer.as_mut().unwrap();
                        match r.screenshot(
                            w.width,
                            w.height,
                            clear,
                            &self.draw_quads,
                            &self.lines,
                            &fx_instances,
                            &overlay_bg,
                            &overlay_lines,
                        ) {
                            Ok(px) => {
                                if let Err(e) = image::save_buffer(
                                    &path,
                                    &px,
                                    w.width,
                                    w.height,
                                    image::ExtendedColorType::Rgba8,
                                ) {
                                    eprintln!("dopaterm: save screenshot: {e}");
                                } else {
                                    eprintln!("dopaterm: wrote {}", path.display());
                                }
                            }
                            Err(e) => eprintln!("dopaterm: screenshot: {e}"),
                        }
                        el.exit();
                    }
                }
            }
            _ => {}
        }
    }

    fn user_event(&mut self, el: &ActiveEventLoop, ev: UserEvent) {
        match ev {
            UserEvent::Term(e) => self.handle_term_event(e, el),
            UserEvent::Wake => {
                self.check_timers();
            }
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if let Some(t) = self.pending_exit {
            if Instant::now() >= t {
                el.exit();
                return;
            }
            el.set_control_flow(ControlFlow::WaitUntil(t));
            return;
        }
        // Keep animating while particles are alive; idle otherwise.
        el.set_control_flow(ControlFlow::Wait);
    }
}

#[cfg(not(windows))]
fn default_shell() -> Option<Shell> {
    let prog = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
    Some(Shell::new(prog, vec!["-i".into()]))
}

#[cfg(windows)]
fn default_shell() -> Option<Shell> {
    Some(Shell::new("powershell.exe".into(), Vec::new()))
}
