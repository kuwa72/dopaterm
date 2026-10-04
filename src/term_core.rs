//! Terminal core: PTY + Term + alacritty_terminal's own event loop thread.

use std::borrow::Cow;
use std::io;
use std::sync::Arc;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::Config as TermConfig;
use alacritty_terminal::tty;
use alacritty_terminal::Term;
use winit::event_loop::EventLoopProxy;

/// Messages delivered to the winit event loop.
pub enum UserEvent {
    /// Something happened inside Term (Wakeup, PtyWrite, Title, ...).
    Term(Event),
    /// Wake request that does not carry a payload.
    Wake,
}

#[derive(Clone)]
pub struct EventProxy {
    proxy: EventLoopProxy<UserEvent>,
}

impl EventProxy {
    pub fn new(proxy: EventLoopProxy<UserEvent>) -> Self {
        Self { proxy }
    }
}

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        let _ = self.proxy.send_event(UserEvent::Term(event));
    }
}

/// Grid dimensions for `Term::new` / `resize`.
#[derive(Clone, Copy)]
pub struct Dims {
    pub cols: usize,
    pub lines: usize,
}

impl Dimensions for Dims {
    fn total_lines(&self) -> usize {
        self.lines
    }
    fn screen_lines(&self) -> usize {
        self.lines
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

pub struct TermCore {
    pub term: Arc<FairMutex<Term<EventProxy>>>,
    pub sender: EventLoopSender,
    pub child_pid: Option<u32>,
}

impl TermCore {
    pub fn spawn(
        window_size: WindowSize,
        shell: Option<tty::Shell>,
        proxy: EventProxy,
    ) -> io::Result<Self> {
        let mut options = tty::Options::default();
        options.shell = shell;
        options.drain_on_exit = true;

        #[cfg(windows)]
        let (pty, child_pid) = {
            let pty = tty::new(&options, window_size, 0)?;
            let pid = pty.child_watcher().pid().map(|p| p.get());
            (pty, pid)
        };
        #[cfg(not(windows))]
        let (pty, child_pid) = {
            let pty = tty::new(&options, window_size, 0)?;
            (pty, None)
        };
        let dims = Dims { cols: window_size.num_cols as usize, lines: window_size.num_lines as usize };

        let term_config = TermConfig { scrolling_history: 10_000, ..Default::default() };
        let term = Arc::new(FairMutex::new(Term::new(term_config, &dims, proxy.clone())));

        let event_loop = EventLoop::new(Arc::clone(&term), proxy, pty, true, false)?;
        let sender = event_loop.channel();
        let _handle = event_loop.spawn();
        Ok(Self { term, sender, child_pid })
    }

    pub fn write(&self, bytes: &[u8]) {
        let _ = self.sender.send(Msg::Input(Cow::Owned(bytes.to_vec())));
    }

    pub fn resize(&self, window_size: WindowSize) {
        let _ = self.sender.send(Msg::Resize(window_size));
        self.term.lock().resize(Dims {
            cols: window_size.num_cols as usize,
            lines: window_size.num_lines as usize,
        });
    }

    pub fn shutdown(&self) {
        let _ = self.sender.send(Msg::Shutdown);
    }
}
