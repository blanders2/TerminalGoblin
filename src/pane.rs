//! A terminal pane: an alacritty_terminal `Term` plus the PTY event loop feeding it.

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{self, Term};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::{CursorShape, CursorStyle};
use winit::event_loop::EventLoopProxy;

use crate::config::{Config, Profile};

pub type PaneId = u64;

#[derive(Debug)]
pub enum UserEvent {
    Term(PaneId, Event),
    ConfigChanged,
}

#[derive(Clone)]
pub struct EventProxy {
    id: PaneId,
    proxy: EventLoopProxy<UserEvent>,
}

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        let _ = self.proxy.send_event(UserEvent::Term(self.id, event));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridSize {
    pub cols: usize,
    pub rows: usize,
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

pub struct Pane {
    pub term: Arc<FairMutex<Term<EventProxy>>>,
    notifier: EventLoopSender,
    pub profile: usize,
    pub profile_name: String,
    pub scheme: Option<String>,
    pub title: String,
    /// Name and color carried over from a tab that was joined into another one.
    pub label: Option<String>,
    pub color: Option<crate::theme::Color>,
    /// Font size override in logical pixels; `None` uses the configured default.
    pub font_size: Option<f32>,
    pub grid: GridSize,
    child_pid: u32,
    /// Accumulated fractional scroll from touchpads.
    pub scroll_accum: f32,
}

pub fn term_config(cfg: &Config) -> term::Config {
    let shape = match cfg.cursor_style.as_str() {
        "beam" | "bar" => CursorShape::Beam,
        "underline" => CursorShape::Underline,
        _ => CursorShape::Block,
    };
    term::Config {
        scrolling_history: cfg.scrollback,
        default_cursor_style: CursorStyle {
            shape,
            blinking: cfg.cursor_blink,
        },
        // Lets programs tell e.g. Shift+Enter from Enter (Claude Code uses it for newlines).
        kitty_keyboard: true,
        ..Default::default()
    }
}

impl Pane {
    pub fn spawn(
        id: PaneId,
        cfg: &Config,
        profile_index: usize,
        profile: &Profile,
        cwd: Option<PathBuf>,
        grid: GridSize,
        cell: (f32, f32),
        proxy: EventLoopProxy<UserEvent>,
    ) -> anyhow::Result<Pane> {
        let mut env = profile.env.clone();
        env.insert("TERM_PROGRAM".into(), "TerminalGoblin".into());
        env.insert("TERM_PROGRAM_VERSION".into(), env!("CARGO_PKG_VERSION").into());
        let working_directory = cwd.or_else(|| profile.cwd.clone()).or_else(dirs::home_dir);
        let options = tty::Options {
            shell: Some(tty::Shell::new(profile.command.clone(), profile.args.clone())),
            working_directory,
            drain_on_exit: false,
            env,
        };
        let window_size = window_size(grid, cell);
        let pty = tty::new(&options, window_size, id)?;
        let child_pid = pty.child().id();
        let listener = EventProxy { id, proxy };
        let term = Arc::new(FairMutex::new(Term::new(term_config(cfg), &grid, listener.clone())));
        let event_loop = EventLoop::new(term.clone(), listener, pty, false, false)?;
        let notifier = event_loop.channel();
        event_loop.spawn();
        Ok(Pane {
            term,
            notifier,
            profile: profile_index,
            profile_name: profile.name.clone(),
            scheme: profile.scheme.clone(),
            title: String::new(),
            label: None,
            color: None,
            font_size: None,
            grid,
            child_pid,
            scroll_accum: 0.0,
        })
    }

    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        let bytes = bytes.into();
        if !bytes.is_empty() {
            let _ = self.notifier.send(Msg::Input(bytes));
        }
    }

    pub fn resize(&mut self, grid: GridSize, cell: (f32, f32)) {
        if grid == self.grid {
            return;
        }
        self.grid = grid;
        self.term.lock().resize(grid);
        let _ = self.notifier.send(Msg::Resize(window_size(grid, cell)));
    }

    /// The shell's current working directory, used to open splits in the same place.
    pub fn cwd(&self) -> Option<PathBuf> {
        std::fs::read_link(format!("/proc/{}/cwd", self.child_pid)).ok()
    }

    pub fn display_title(&self) -> &str {
        if let Some(label) = self.label.as_deref().filter(|l| !l.trim().is_empty()) {
            return label;
        }
        if self.title.trim().is_empty() {
            &self.profile_name
        } else {
            &self.title
        }
    }
}

impl Drop for Pane {
    fn drop(&mut self) {
        let _ = self.notifier.send(Msg::Shutdown);
    }
}

pub fn window_size(grid: GridSize, cell: (f32, f32)) -> WindowSize {
    WindowSize {
        num_lines: grid.rows as u16,
        num_cols: grid.cols as u16,
        cell_width: cell.0 as u16,
        cell_height: cell.1 as u16,
    }
}
