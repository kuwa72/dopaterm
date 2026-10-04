//! winit application: owns the window, renderer, terminal and effect manager.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Point};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::tty::Shell;
use alacritty_terminal::vte::ansi::CursorShape;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton as WinitMouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

use crate::colors::{cell_colors, to_f32};
use crate::config::{self, parse_rgb, Config};
use crate::fx::{self, FxEvent, Instance, Manager};
use crate::input::{decode_key, KeyKind};
use crate::render::{Line, Renderer, Span};
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
    /// Previous frame's cells: (char, fg, bg, bold), indexed line*cols+col.
    snapshot: Vec<(char, [u8; 4], [u8; 4], bool)>,
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
    /// Consecutive render failures; a GPU adapter/surface change is
    /// unrecoverable for the old device, so a full rebuild is scheduled.
    render_failures: u32,
    /// Rebuild attempts since the last good frame; the first ones retry on
    /// the same window, repeated failure escalates to a window recreate.
    gpu_resets: u32,
    last_renderer_reset: Option<Instant>,
    frame_bg: [u8; 3],
    prev_offset: usize,
    prev_cursor: Option<Point>,
    /// Cell indices shattered proactively on Backspace/Delete; used to dedupe
    /// against the diff-based erase detection for ~200ms.
    predicted_erase: Vec<(usize, Instant)>,
    last_frame: Instant,
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
            render_failures: 0,
            gpu_resets: 0,
            last_renderer_reset: None,
            frame_bg,
            prev_offset: usize::MAX,
            prev_cursor: None,
            predicted_erase: Vec::new(),
            last_frame: Instant::now(),
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

    fn build_settings_overlay(&self) -> (Vec<Instance>, Vec<Line>, Vec<settings_ui::Hit>) {
        let r = self.renderer.as_ref().unwrap();
        let w = self.window.as_ref().unwrap().inner_size();
        settings_ui::build(&self.cfg, w.width as f32, w.height as f32, r.cell_w, r.cell_h, r.font_families())
    }

    fn build_overlay(&self) -> (Vec<Instance>, Vec<Line>, Vec<settings_ui::Hit>) {
        if self.show_settings {
            return self.build_settings_overlay();
        }
        let r = self.renderer.as_ref().unwrap();
        let size = self.window.as_ref().unwrap().inner_size();
        let anchor = (self.last_cursor_px.0 - r.cell_w / 2.0, self.last_cursor_px.1 - r.cell_h / 2.0);
        let (quads, lines) = self.ime_preedit.overlay(
            anchor,
            (r.cell_w, r.cell_h),
            (size.width as f32, size.height as f32),
            parse_rgb(&self.cfg.foreground),
            parse_rgb(&self.cfg.background),
        );
        (quads, lines, Vec::new())
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
            // Rebuild on the same window first; only escalate to a full
            // window recreate when rebuilds keep failing.
            self.gpu_resets += 1;
            if self.gpu_resets >= 3 {
                self.gpu_resets = 0;
                self.recreate_window_and_renderer(el);
            } else {
                self.recreate_renderer();
            }
            self.render_failures = 0;
            self.last_renderer_reset = Some(Instant::now());
        }
    }

    /// After a device loss the window's compositor binding can be tied to
    /// the dead adapter — a fresh surface on the same HWND may present to
    /// nothing. Recreate the whole window; the PTY/shell keeps running
    /// because TermCore is independent of the renderer.
    fn recreate_window_and_renderer(&mut self, el: &ActiveEventLoop) {
        let Some(old_win) = self.window.take() else { return };
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
        let Some(window) = self.window.clone() else { return };
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
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    r.resize(size.width, size.height)
                }));
                self.renderer = Some(r);
                self.snapshot.clear();
                self.dirty_lines.iter_mut().for_each(|d| *d = true);
                self.resize_terminal_to_window();
                self.dirty = true;
                self.window.as_ref().unwrap().request_redraw();
                eprintln!("dopaterm: renderer re-initialized after GPU change");
            }
            Ok(Err(e)) => eprintln!("dopaterm: renderer re-init failed: {e}"),
            Err(_) => eprintln!("dopaterm: renderer re-init panicked"),
        }
    }

    fn resize_terminal_to_window(&mut self) {
        let Some(r) = self.renderer.as_ref() else { return };
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
        let scale = self.cfg.intensity.scale();
        self.fx.set_scale(scale);
        if scale <= 0.0 {
            return;
        }
        if self.cfg.effects.sparks {
            self.fx.push(Box::new(fx::builtin::Sparks::new()));
        }
        if self.cfg.effects.shatter {
            self.fx.push(Box::new(fx::builtin::Shatter::new()));
        }
        if self.cfg.effects.cursor_trail {
            self.fx.push(Box::new(fx::builtin::CursorTrail::new()));
        }
        if self.cfg.effects.fireworks {
            self.fx.push(Box::new(fx::builtin::Fireworks::new()));
        }
        if self.cfg.effects.wait_pulse {
            self.fx.push(Box::new(fx::builtin::WaitPulse::new()));
        }
        if self.cfg.effects.confetti {
            self.fx.push(Box::new(fx::builtin::Confetti::new()));
        }
        if self.cfg.effects.output_rain {
            self.fx.push(Box::new(fx::builtin::OutputRain::new()));
        }
        self.fx.push(Box::new(fx::builtin::BellFlash::new()));
        if self.cfg.effects.process_error {
            self.fx.push(Box::new(fx::builtin::ProcessError::new()));
        }
        let lua_dir = self.cfg.effects.lua_dir.clone()
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
            && term.child_pid.and_then(crate::mouse_inject::input_mode)
                .is_some_and(crate::mouse_inject::native_tracking_enabled)
        {
            return mode | TermMode::MOUSE_MOTION;
        }
        mode
    }

    fn send_mouse_event(&self, term: &TermCore, button: u8, col: usize, line: usize, release: bool) {
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
                let _ = writeln!(f, "btn={button} col={col} line={line} release={release} seq={seq:?}");
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
                    let flags = if is_motion { crate::mouse_inject::MOUSE_MOVED } else { 0 };
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
                    pid, col as i16, line as i16, button_state, ctrl, event_flags,
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
        let Some(term) = self.term.as_ref() else { return };
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
        let Ok(mut cb) = arboard::Clipboard::new() else { return };
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
        let Some(term) = self.term.as_ref() else { return };
        // Move to a known cell and generate press / release / wheel events.
        self.mouse_cell = (4, 2);
        self.send_mouse_event(term, 0, self.mouse_cell.0, self.mouse_cell.1, false);
        self.send_mouse_event(term, 0, self.mouse_cell.0, self.mouse_cell.1, true);
        self.mouse_cell = (6, 3);
        self.send_mouse_event(term, 64, self.mouse_cell.0, self.mouse_cell.1, false);
        eprintln!("dopaterm: synthetic mouse events injected; exiting in 500ms");
        self.pending_exit = Some(Instant::now() + Duration::from_millis(500));
    }

    fn handle_settings_click(&mut self) {
        let (mx, my) = self.mouse_px;
        let mut needs_font_reload = false;
        let mut needs_effects = false;
        for hit in &self.settings_hits {
            let (x, y, w, h) = hit.rect;
            if mx >= x && mx < x + w && my >= y && my < y + h {
                match &hit.action {
                    settings_ui::Action::Close => {
                        self.show_settings = false;
                    }
                    settings_ui::Action::Intensity(_) |
                    settings_ui::Action::Toggle(_) => {
                        settings_ui::apply_action(&mut self.cfg, &hit.action, self.renderer.as_ref().unwrap().font_families());
                        needs_effects = true;
                    }
                    settings_ui::Action::FontSize(_) |
                    settings_ui::Action::FontNext => {
                        settings_ui::apply_action(&mut self.cfg, &hit.action, self.renderer.as_ref().unwrap().font_families());
                        needs_font_reload = true;
                    }
                    settings_ui::Action::ShellNext |
                    settings_ui::Action::Theme(_) => {
                        settings_ui::apply_action(&mut self.cfg, &hit.action, self.renderer.as_ref().unwrap().font_families());
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
                break;
            }
        }
        self.sync_ime();
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
                let (ww, wh) = self.window.as_ref()
                    .map(|w| {
                        let s = w.inner_size();
                        (s.width as f32, s.height as f32)
                    })
                    .unwrap_or((960.0, 600.0));
                self.fx.event(&FxEvent::PtyOutput { n: 1, w: ww, h: wh });
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
                    term.write(fmt(alacritty_terminal::vte::ansi::Rgb { r: 0, g: 0, b: 0 }).as_bytes());
                }
            }
            Event::TextAreaSizeRequest(fmt) => {
                if let Some(r) = &self.renderer {
                    let w = self.window.as_ref().unwrap().inner_size();
                    term.write(fmt(WindowSize {
                        num_cols: (w.width as f32 / r.cell_w) as u16,
                        num_lines: (w.height as f32 / r.cell_h) as u16,
                        cell_width: r.cell_w as u16,
                        cell_height: r.cell_h as u16,
                    }).as_bytes());
                }
            }
            Event::Bell => {
                let (x, y) = self.last_cursor_px;
                let r = self.renderer.as_ref().unwrap();
                self.fx.event(&FxEvent::Bell { x, y, w: r.cell_w, h: r.cell_h });
                self.dirty = true;
            }
            Event::ChildExit(status) => {
                if !status.success() {
                    let code = status.code().unwrap_or(1);
                    let (cx, cy) = self.last_cursor_px;
                    let (ww, wh) = self.window.as_ref()
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

        if std::env::var_os("DOPA_DEBUG").is_some() {
            let nonspace = snap.iter().filter(|c| !matches!(c.0, ' ' | '\0')).count();
            eprintln!("dopaterm: nonspace={} scrolled={} rows={}", nonspace, scrolled, rows);
        }

        // Mass changes = scroll/redraw; suppress shatter storms.
        if !scrolled {
            let erased = find_erased(&self.snapshot, &snap);
            if std::env::var_os("DOPA_DEBUG").is_some() && !erased.is_empty() {
                eprintln!("dopaterm: erased {:?}", erased.iter().map(|e| e.1).collect::<String>());
            }
            self.predicted_erase.retain(|(_, at)| at.elapsed() < Duration::from_secs(1));
            for (i, ch_, fg_) in erased {
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
        if self.snapshot.is_empty() {
            self.dirty_lines.iter_mut().for_each(|d| *d = true);
        } else {
            for (i, (old, new)) in self.snapshot.iter().zip(&snap).enumerate() {
                if *old != *new {
                    let line = i / cols;
                    if line < rows {
                        self.dirty_lines[line] = true;
                    }
                }
            }
        }
        if scrolled {
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
                let (c, mut fg, bg, bold) = snap[i * cols + col];
                if c == '\0' {
                    continue;
                }
                if p_eq(cursor_point, i, col) && cursor.shape != CursorShape::Hidden {
                    fg = bg;
                }
                if span_key != Some((fg, bold)) {
                    if let Some((f, b)) = span_key.take() {
                        if !text.is_empty() {
                            spans.push(Span { text: std::mem::take(&mut text), fg: f, bold: b });
                        }
                    }
                    span_key = Some((fg, bold));
                }
                text.push(c);
            }
            if let Some((f, b)) = span_key {
                if !text.is_empty() {
                    spans.push(Span { text, fg: f, bold: b });
                }
            }
            self.lines[i] = Some(Line { top: i as f32 * ch, left: 0.0, spans });
        }

        // Rebuild cell background + cursor quads when content changed.
        if any_dirty || background_changed {
            self.bgs.clear();
            for i in 0..rows {
                for col in 0..cols {
                    let (_, fg, bg, _) = snap[i * cols + col];
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

        // Cursor move events.
        if let Some(prev) = self.prev_cursor {
            if prev != cursor_point {
                let (px, py, w, h) = self.cell_rect(prev);
                let dx = (cursor_point.column.0 as i32 - prev.column.0 as i32) as f32 * cw;
                let dy = (cursor_point.line.0 - prev.line.0) as f32 * ch;
                self.fx.event(&FxEvent::CursorMoved { x: px + w / 2.0, y: py + h / 2.0, w, h, dx, dy });
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
        if fired {
            self.dirty = true;
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        }
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
            (0..=8).map(|d| base_col - d).take_while(|c| *c >= 0).collect()
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
        if self.anchor <= self.head { (self.anchor, self.head) } else { (self.head, self.anchor) }
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
            let (c, ..) = snap[vis * cols + col];
            if c != '\0' {
                line.push(c);
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
    let rgba =
        image::RgbaImage::from_raw(img.width as u32, img.height as u32, img.bytes.to_vec())?;
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
    p_eq(cursor, i, col)
        || (col > 0 && snap[i * cols + col].0 == '\0' && p_eq(cursor, i, col - 1))
}

type SnapCell = (char, [u8; 4], [u8; 4], bool);

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
    let mut snap = vec![(' ', fg, bg, false); cols * rows];
    for indexed in content.display_iter {
        let line = indexed.point.line.0 + content.display_offset as i32;
        if line < 0 || line as usize >= rows {
            continue;
        }
        let cell = indexed.cell;
        let (fg, bg) = cell_colors(cell, content.colors, default_fg, default_bg);
        let ch = if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            '\0'
        } else if cell.c == '\t' {
            ' '
        } else {
            cell.c
        };
        snap[line as usize * cols + indexed.point.column.0] =
            (ch, fg, bg, cell.flags.contains(Flags::BOLD));
    }
    snap
}

/// Cells that went non-space -> space between snapshots (candidate erases).
/// Returns (cell_index, erased_char, old_fg). Suppressed when more than a
/// third of the grid changed (scroll/redraw storms aren't deletions).
fn find_erased(old: &[SnapCell], new: &[SnapCell]) -> Vec<(usize, char, [u8; 4])> {
    if old.len() != new.len() || new.is_empty() {
        return Vec::new();
    }
    let mut changed = 0usize;
    let mut out = Vec::new();
    for (i, (o, n)) in old.iter().zip(new.iter()).enumerate() {
        if o.0 != n.0 {
            changed += 1;
            if !matches!(o.0, ' ' | '\0') && matches!(n.0, ' ' | '\0') {
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
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::vte::ansi::Processor;
    use alacritty_terminal::Term;

    fn cell(c: char) -> SnapCell {
        (c, [255; 4], [0, 0, 0, 255], false)
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
        assert_eq!(snap[..4].iter().map(|cell| cell.0).filter(|ch| *ch != '\0').collect::<String>(), "日本");
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
        let sel = Selection { anchor: (1, 1), head: (2, 3), dragging: false };
        assert_eq!(selected_text(&snap, 6, 3, 0, &sel), "Xc\ntail");

        // Reversed drag order selects the same region.
        let rev = Selection { anchor: (2, 3), head: (1, 1), dragging: false };
        assert_eq!(selected_text(&snap, 6, 3, 0, &rev), "Xc\ntail");

        // Buffer-line coordinates account for the scroll offset.
        let top = Selection { anchor: (0, 0), head: (0, 3), dragging: false };
        assert_eq!(selected_text(&snap, 6, 3, 1, &top), "aXc");
    }

    #[test]
    fn dropped_file_path_is_quoted_when_it_has_whitespace() {
        assert_eq!(
            drop_path_text(std::path::Path::new("/tmp/a b.png"), false),
            "\"/tmp/a b.png\""
        );
        assert_eq!(drop_path_text(std::path::Path::new("/tmp/ab.png"), false), "/tmp/ab.png");
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
        assert_eq!(snap[..9].iter().map(|cell| cell.0).collect::<String>(), "A       B");
    }

    #[test]
    fn spacer_removal_does_not_spawn_a_glyph_particle() {
        let mut old = vec![cell(' '); 12];
        old[2] = cell('\0');
        assert!(find_erased(&old, &vec![cell(' '); 12]).is_empty());
    }

    #[test]
    fn doomed_cell_reads_grid() {
        let mut t = Term::new(
            alacritty_terminal::term::Config::default(),
            &Dims { cols: 80, lines: 24 },
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
        let er = find_erased(&old, &new);
        assert_eq!(er.len(), 1);
        assert_eq!(er[0].0, 2);
        assert_eq!(er[0].1, 'x');
    }

    #[test]
    fn overwrite_is_not_erase() {
        let old: Vec<SnapCell> = vec![cell('a'), cell(' ')];
        let new: Vec<SnapCell> = vec![cell('b'), cell(' ')];
        assert!(find_erased(&old, &new).is_empty());
    }

    #[test]
    fn full_repaint_suppressed() {
        // 2/4 cells changed incl. erases -> exceeds 1/3 threshold.
        let old: Vec<SnapCell> = vec![cell('a'), cell('b'), cell('c'), cell('d')];
        let new: Vec<SnapCell> = vec![cell(' '), cell('x'), cell(' '), cell('y')];
        assert!(find_erased(&old, &new).is_empty());
    }

    #[test]
    fn size_mismatch_ignored() {
        let old: Vec<SnapCell> = vec![cell('a')];
        let new: Vec<SnapCell> = vec![cell(' '), cell(' ')];
        assert!(find_erased(&old, &new).is_empty());
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
        let renderer = pollster::block_on(Renderer::new(
            Arc::clone(&window),
            self.cfg.font_size,
            self.cfg.font_family.clone(),
        ))
        .expect("init renderer");
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
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(200));
            if wake_proxy.send_event(UserEvent::Wake).is_err() {
                break;
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
                if event.state != ElementState::Pressed || event.repeat {
                    return;
                }
                if event.logical_key == Key::Named(NamedKey::F1)
                    || (self.mods.control_key() && self.mods.shift_key()
                        && matches!(&event.logical_key, Key::Character(c) if c == "," || c == "<"))
                {
                    self.show_settings = !self.show_settings;
                    self.sync_ime();
                    self.dirty = true;
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                    return;
                }
                if self.show_settings {
                    if event.logical_key == Key::Named(NamedKey::Escape) {
                        self.show_settings = false;
                        self.sync_ime();
                        self.dirty = true;
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
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
                // Paste / copy shortcuts.
                if self.mods.control_key() && self.mods.shift_key() {
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
                    self.fx.event(&FxEvent::Key { kind: d.kind, ch: d.ch, x, y });
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
                    if d.kind == KeyKind::Enter {
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
                let Some(r) = self.renderer.as_ref() else { return };
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
                let starting_composition = cfg!(windows) && matches!(&ime, winit::event::Ime::Enabled);
                if let Some(text) = self.ime_preedit.update(ime) {
                    term.write(text.as_bytes());
                    term.term.lock().scroll_display(Scroll::Bottom);
                    let (x, y) = self.last_cursor_px;
                    self.fx.event(&FxEvent::Key { kind: KeyKind::Char, ch: text.chars().next(), x, y });
                }
                self.ime_composing = starting_composition || self.ime_preedit.active();
                self.last_activity = Instant::now();
                self.last_input = self.last_activity;
                self.dirty = true;
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::Focused(false) => {
                self.ime_preedit.clear();
                self.ime_composing = false;
                self.mods = ModifiersState::empty();
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
                    MouseScrollDelta::PixelDelta(p) => {
                        -(p.y as f32 / 20.0).round() as i32
                    }
                };
                if lines != 0 {
                    term.term.lock().scroll_display(Scroll::Delta(lines));
                    self.dirty = true;
                    self.window.as_ref().unwrap().request_redraw();
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
                        r.render(clear, &self.draw_quads, &self.lines, &self.dirty_lines, &fx_instances, &overlay_bg, &overlay_lines)
                    }));
                    match result {
                        Ok(Ok(())) => {
                            self.render_failures = 0;
                            self.gpu_resets = 0;
                            rendered = true;
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
                if animating || self.dirty || self.shot_path.is_some() {
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
                            self.fx.event(&FxEvent::Key { kind: crate::input::KeyKind::Char, ch: Some('x'), x, y });
                        }
                        self.fx.event(&FxEvent::Erased {
                            ch: 'x',
                            x: x + cw * 3.0,
                            y,
                            w: cw,
                            h: ch,
                            color: [1.0, 0.5, 0.3, 1.0],
                        });
                        self.fx.event(&FxEvent::CursorMoved {
                            x,
                            y,
                            w: cw,
                            h: ch,
                            dx: cw * 8.0,
                            dy: ch * 2.0,
                        });
                        self.fx.event(&FxEvent::Bell { x: x + cw * 8.0, y: y + ch * 2.0, w: cw, h: ch });
                        self.fx.event(&FxEvent::CommandDone { x: x + 380.0, y: y + 340.0 });
                        self.fx.event(&FxEvent::Waiting { x, y });
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
                        if std::env::var_os("DOPA_IME_DEMO").is_some() {
                            let text = "日本語入力中".to_string();
                            let end = text.len();
                            self.ime_preedit.update(winit::event::Ime::Preedit(text, Some((end, end))));
                        }
                        self.build_frame();
                        self.fx.tick(0.05);
                        let fx_instances: Vec<Instance> = self.fx.instances().to_vec();
                        let w = self.window.as_ref().unwrap().inner_size();
                        let clear = crate::colors::srgb_to_linear(crate::colors::to_f32_3(self.frame_bg));
                        let (overlay_bg, overlay_lines, hits) = self.build_overlay();
                        self.settings_hits = hits;
                        self.draw_quads.clear();
                        self.draw_quads.extend_from_slice(&self.bgs);
                        self.draw_quads.extend_from_slice(&self.sel_quads);
                        let r = self.renderer.as_mut().unwrap();
                        match r.screenshot(w.width, w.height, clear, &self.draw_quads, &self.lines, &fx_instances, &overlay_bg, &overlay_lines) {
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
