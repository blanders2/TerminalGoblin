//! A terminal pane: an alacritty_terminal `Term` plus the PTY event loop feeding it.

use std::borrow::Cow;
use std::collections::HashMap;
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

/// How a pane is started beyond its profile: a different command (e.g. `ssh host ...`) and extra
/// environment (e.g. PatchGoblin's `PATCHGOBLIN_TAB`). Splits and duplicates reuse it.
#[derive(Clone, Debug, Default)]
pub struct PaneLaunch {
    pub command: Option<(String, Vec<String>)>,
    pub env: HashMap<String, String>,
}

#[derive(Debug)]
pub enum UserEvent {
    Term(PaneId, Event),
    ConfigChanged,
    Ipc(crate::ipc::Request),
    /// A remote folder listing for the settings folder browser: (canonical dir, subfolders).
    RemoteDir {
        req: u64,
        result: Result<(String, Vec<String>), String>,
    },
}

/// List the subfolders of `dir` on `host` over ssh in the background, sending a
/// [`UserEvent::RemoteDir`] when done. Key/agent auth only (`BatchMode`), with an overall deadline.
pub fn list_remote_dir(proxy: EventLoopProxy<UserEvent>, req: u64, host: String, dir: String) {
    let send = move |result| {
        let _ = proxy.send_event(UserEvent::RemoteDir { req, result });
    };
    if !crate::config::valid_ssh_host(&host) {
        send(Err(format!("invalid host {host:?}")));
        return;
    }
    std::thread::Builder::new()
        .name("remote dir".into())
        .spawn(move || send(run_remote_ls(&host, &dir)))
        .ok();
}

fn run_remote_ls(host: &str, dir: &str) -> Result<(String, Vec<String>), String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let dir = crate::config::remote_path_arg(if dir.trim().is_empty() { "~" } else { dir.trim() });
    let mut command = Command::new("ssh");
    command
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=8", "--", host])
        .arg(format!("cd {dir} && pwd && ls -1Ap"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // ssh is a console program: don't flash a console window.
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        std::os::windows::process::CommandExt::creation_flags(&mut command, CREATE_NO_WINDOW);
    }
    let mut child = command.spawn().map_err(|e| format!("could not run ssh: {e}"))?;
    // Read both pipes on their own threads so a chatty child can't block on a full pipe.
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = String::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_string(&mut buf);
            }
            buf
        })
    };
    let out = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let err = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("timed out connecting to {host}"));
            }
            Err(e) => return Err(e.to_string()),
        }
    };
    let stdout = out.join().unwrap_or_default();
    let stderr = err.join().unwrap_or_default();
    if !status.success() {
        let msg = stderr.trim();
        return Err(if msg.is_empty() {
            format!("ssh {status}")
        } else {
            msg.to_string()
        });
    }
    Ok(parse_remote_ls(&stdout))
}

/// `pwd` followed by `ls -1Ap` output: the folder and its sorted subfolders.
fn parse_remote_ls(out: &str) -> (String, Vec<String>) {
    let mut lines = out.lines();
    let cwd = lines.next().unwrap_or("").trim().to_string();
    let mut dirs: Vec<String> = lines
        .filter_map(|l| l.strip_suffix('/'))
        .filter(|d| !d.is_empty())
        .map(str::to_string)
        .collect();
    dirs.sort_by_key(|d| d.to_lowercase());
    (cwd, dirs)
}

#[cfg(test)]
mod tests {
    #[test]
    fn remote_listing_keeps_folders() {
        let (cwd, dirs) = super::parse_remote_ls("/home/me\n.config/\nb.txt\nZed/\napp/\n");
        assert_eq!(cwd, "/home/me");
        assert_eq!(dirs, [".config", "app", "Zed"]);
    }
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
    pub launch: PaneLaunch,
    /// Set when a launched command failed: the pane stays open showing why.
    pub exited: Option<String>,
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
        launch: PaneLaunch,
        grid: GridSize,
        cell: (f32, f32),
        proxy: EventLoopProxy<UserEvent>,
    ) -> anyhow::Result<Pane> {
        let mut env = profile.env.clone();
        env.extend(launch.env.clone());
        env.insert("TERM_PROGRAM".into(), "TerminalGoblin".into());
        env.insert("TERM_PROGRAM_VERSION".into(), env!("CARGO_PKG_VERSION").into());
        env.insert("TERMINALGOBLIN_PANE_ID".into(), id.to_string());
        if let Some(sock) = crate::ipc::socket_path() {
            env.insert("TERMINALGOBLIN_SOCKET".into(), sock.to_string_lossy().into_owned());
        }
        let working_directory = cwd.or_else(|| profile.cwd.clone()).or_else(dirs::home_dir);
        let (program, args) = launch
            .command
            .clone()
            .unwrap_or_else(|| (profile.command.clone(), profile.args.clone()));
        // The program goes into the command line as is, so quote paths like `C:\Program Files\...`.
        #[cfg(windows)]
        let program = if program.contains(' ') && !program.starts_with('"') {
            format!("\"{program}\"")
        } else {
            program
        };
        let options = tty::Options {
            shell: Some(tty::Shell::new(program, args)),
            working_directory,
            drain_on_exit: true,
            env,
            #[cfg(windows)]
            escape_args: true,
        };
        let window_size = window_size(grid, cell);
        let pty = tty::new(&options, window_size, id)?;
        #[cfg(not(windows))]
        let child_pid = pty.child().id();
        #[cfg(windows)]
        let child_pid = pty.child_watcher().pid().map_or(0, |p| p.get());
        let listener = EventProxy { id, proxy };
        let term = Arc::new(FairMutex::new(Term::new(term_config(cfg), &grid, listener.clone())));
        // Drain on exit so a failing command's last words (e.g. ssh's error) reach the screen.
        let event_loop = EventLoop::new(term.clone(), listener, pty, true, false)?;
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
            launch,
            exited: None,
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

    /// Write text to the screen as if the program had printed it.
    pub fn print(&self, text: &str) {
        let mut parser: alacritty_terminal::vte::ansi::Processor = Default::default();
        parser.advance(&mut *self.term.lock(), text.as_bytes());
    }

    /// Process id of the program running in the pane.
    pub fn pid(&self) -> u32 {
        self.child_pid
    }

    /// The shell's current working directory, used to open splits in the same place.
    #[cfg(not(windows))]
    pub fn cwd(&self) -> Option<PathBuf> {
        std::fs::read_link(format!("/proc/{}/cwd", self.child_pid)).ok()
    }

    /// Windows has no way to read another process's working directory, so splits open in the
    /// profile's folder.
    #[cfg(windows)]
    pub fn cwd(&self) -> Option<PathBuf> {
        None
    }

    pub fn display_title(&self) -> &str {
        if let Some(label) = self.label.as_deref().filter(|l| !l.trim().is_empty()) {
            return label;
        }
        if !self.title.trim().is_empty() {
            return &self.title;
        }
        // Until the program sets a title, name the tab after what it runs.
        match &self.launch.command {
            Some((program, _)) => program.rsplit('/').next().unwrap_or(program),
            None => &self.profile_name,
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
