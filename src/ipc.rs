//! Remote control: a Unix socket per running instance, and the `terminalgoblin @ ...` client.
//!
//! Each instance listens on `$XDG_RUNTIME_DIR/terminalgoblin/<pid>.sock`. A request is one line
//! of JSON (`{"cmd": "ls"}`, `{"cmd": "launch", ...}`, ...) and the reply is one line of JSON.
//! This is what lets tools like PatchGoblin open named, colored tabs and find them again.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::mpsc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use winit::event_loop::EventLoopProxy;

use crate::pane::UserEvent;

const REPLY_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_REQUEST: u64 = 1 << 20;

static SOCKET: OnceLock<PathBuf> = OnceLock::new();

/// A request from a client, with the channel its reply goes back on.
pub struct Request {
    pub body: Value,
    pub reply: mpsc::Sender<Value>,
}

impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Request({})", self.body)
    }
}

/// What to open: used by `@ launch`, the startup flags and the `launch` request alike.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LaunchSpec {
    /// Command and arguments; empty runs the default profile's shell.
    pub argv: Vec<String>,
    pub cwd: Option<String>,
    /// Tab title. Kept even when the shell or a remote session sets its own.
    pub title: Option<String>,
    /// Tab color, `#rrggbb`.
    pub color: Option<String>,
    pub env: HashMap<String, String>,
    /// Put the tab in the group with this name (created if needed).
    pub group: Option<String>,
    /// Profile name to use instead of the default one (ignored when `argv` is given).
    pub profile: Option<String>,
    /// `tab` (default), `split-right` or `split-down`.
    #[serde(rename = "type")]
    pub kind: Option<String>,
    /// Bring the window to the front. Defaults to true.
    pub focus: Option<bool>,
}

impl LaunchSpec {
    pub fn is_empty(&self) -> bool {
        self.argv.is_empty()
            && self.cwd.is_none()
            && self.title.is_none()
            && self.color.is_none()
            && self.env.is_empty()
            && self.group.is_none()
            && self.profile.is_none()
            && self.kind.is_none()
    }
}

pub fn socket_dir() -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    runtime.join("terminalgoblin")
}

/// This instance's socket, once listening.
pub fn socket_path() -> Option<&'static Path> {
    SOCKET.get().map(PathBuf::as_path)
}

/// Start listening. Requests arrive on the event loop as `UserEvent::Ipc`.
pub fn serve(proxy: EventLoopProxy<UserEvent>) -> std::io::Result<PathBuf> {
    let dir = socket_dir();
    std::fs::create_dir_all(&dir)?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    let path = dir.join(format!("{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    let _ = SOCKET.set(path.clone());
    std::thread::Builder::new().name("ipc".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            let proxy = proxy.clone();
            let _ = std::thread::Builder::new()
                .name("ipc client".into())
                .spawn(move || handle_client(stream, proxy));
        }
    })?;
    Ok(path)
}

pub fn cleanup() {
    if let Some(p) = socket_path() {
        let _ = std::fs::remove_file(p);
    }
}

fn handle_client(stream: UnixStream, proxy: EventLoopProxy<UserEvent>) {
    let _ = stream.set_read_timeout(Some(REPLY_TIMEOUT));
    let mut line = String::new();
    let Ok(read) = stream.try_clone() else { return };
    if BufReader::new(read.take(MAX_REQUEST)).read_line(&mut line).is_err() {
        return;
    }
    let reply = match serde_json::from_str::<Value>(&line) {
        Ok(body) => {
            let (tx, rx) = mpsc::channel();
            if proxy.send_event(UserEvent::Ipc(Request { body, reply: tx })).is_err() {
                json!({"ok": false, "error": "TerminalGoblin is shutting down"})
            } else {
                rx.recv_timeout(REPLY_TIMEOUT)
                    .unwrap_or_else(|_| json!({"ok": false, "error": "no reply from TerminalGoblin"}))
            }
        }
        Err(e) => json!({"ok": false, "error": format!("bad request: {e}")}),
    };
    let mut stream = stream;
    let _ = writeln!(stream, "{reply}");
}

// -------------------------------------------------------------------------------------------------
// Client

/// Send one request to one instance.
pub fn request(socket: &Path, body: &Value) -> std::io::Result<Value> {
    let mut stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(REPLY_TIMEOUT))?;
    writeln!(stream, "{body}")?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    serde_json::from_str(&line).map_err(std::io::Error::other)
}

/// Sockets of the running instances. Stale sockets left by crashed instances are removed.
pub fn instances() -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(socket_dir()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "sock") {
            continue;
        }
        match UnixStream::connect(&path) {
            Ok(_) => out.push(path),
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                let _ = std::fs::remove_file(&path);
            }
            Err(_) => {}
        }
    }
    out
}

/// `ls` from every instance, most recently focused first.
fn list_all() -> Vec<(PathBuf, Value)> {
    let mut all: Vec<(PathBuf, Value)> = instances()
        .into_iter()
        .filter_map(|s| request(&s, &json!({"cmd": "ls"})).ok().map(|v| (s, v)))
        .collect();
    let focus = |v: &Value| v.get("last_focus").and_then(Value::as_f64).unwrap_or(0.0);
    all.sort_by(|a, b| focus(&b.1).total_cmp(&focus(&a.1)));
    all
}

/// Forward a launch to the most recently focused instance. `None` when nothing is running.
pub fn forward_launch(spec: &LaunchSpec) -> Option<Value> {
    let (socket, _) = list_all().into_iter().next()?;
    let mut body = serde_json::to_value(spec).ok()?;
    body["cmd"] = json!("launch");
    request(&socket, &body).ok()
}

/// Start a new instance whose first tab is `spec`, and wait until it answers.
fn start_instance(spec: &LaunchSpec) -> Result<Value, String> {
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let before: Vec<PathBuf> = instances();
    let json = serde_json::to_string(spec).map_err(|e| e.to_string())?;
    let mut command = std::process::Command::new(exe);
    // Started from a background service (e.g. PatchGoblin under systemd), the session's display
    // may not be in our environment: find the compositor's socket ourselves.
    if std::env::var_os("WAYLAND_DISPLAY").is_none()
        && let Some(display) = find_wayland_display()
    {
        command.env("WAYLAND_DISPLAY", display);
    }
    command
        .args(["--new-instance", "--launch-json", &json])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|e| format!("could not start TerminalGoblin: {e}"))?;
    for _ in 0..50 {
        std::thread::sleep(Duration::from_millis(100));
        if let Some(sock) = instances().into_iter().find(|s| !before.contains(s)) {
            let mut reply = request(&sock, &json!({"cmd": "ls"})).map_err(|e| e.to_string())?;
            reply["started"] = json!(true);
            reply["ok"] = json!(true);
            return Ok(reply);
        }
    }
    Err("TerminalGoblin started but didn't open its control socket".into())
}

/// The first `wayland-N` socket in the runtime directory.
fn find_wayland_display() -> Option<String> {
    use std::os::unix::fs::FileTypeExt;
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
    let mut names: Vec<String> = std::fs::read_dir(runtime)
        .ok()?
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_socket()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with("wayland-"))
        .collect();
    names.sort();
    names.into_iter().next()
}

const CLIENT_HELP: &str = "\
Control running TerminalGoblin windows.

  terminalgoblin @ ls
      JSON list of instances, their tabs and panes (with each pane's launch environment).
  terminalgoblin @ launch [--cwd DIR] [--title T] [--color #RRGGBB] [--env K=V]...
                          [--group NAME] [--profile NAME] [--type tab|split-right|split-down]
                          [--no-focus] [-- COMMAND ARGS...]
      Open a tab (or split) in the most recently focused window, starting TerminalGoblin
      if it isn't running. A --title stays put even if the shell sets its own.
  terminalgoblin @ focus --match MATCH
  terminalgoblin @ set-tab-title --match MATCH TITLE
  terminalgoblin @ set-tab-color --match MATCH #RRGGBB|none
  terminalgoblin @ close --match MATCH

MATCH is env:KEY=VALUE (a pane's launch environment), id:PANE_ID, tab:TAB_ID or title:TEXT.
Inside a TerminalGoblin pane, $TERMINALGOBLIN_PANE_ID and $TERMINALGOBLIN_SOCKET are set.
";

/// Parse launch flags (shared by `@ launch` and startup). Returns the spec and leftover args.
pub fn parse_launch_flags(args: &[String]) -> Result<(LaunchSpec, Vec<String>), String> {
    let mut spec = LaunchSpec::default();
    let mut rest = Vec::new();
    let mut i = 0;
    let value = |i: &mut usize, name: &str| -> Result<String, String> {
        *i += 1;
        args.get(*i).cloned().ok_or_else(|| format!("{name} needs a value"))
    };
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "--" | "-e" | "--command" => {
                spec.argv = args[i + 1..].to_vec();
                break;
            }
            "--cwd" | "--working-directory" | "-d" => spec.cwd = Some(value(&mut i, a)?),
            "--title" | "-T" => spec.title = Some(value(&mut i, a)?),
            "--color" | "--tab-color" => spec.color = Some(value(&mut i, a)?),
            "--group" => spec.group = Some(value(&mut i, a)?),
            "--profile" => spec.profile = Some(value(&mut i, a)?),
            "--type" => spec.kind = Some(value(&mut i, a)?),
            "--no-focus" => spec.focus = Some(false),
            "--env" => {
                let kv = value(&mut i, a)?;
                let (k, v) = kv.split_once('=').ok_or("--env takes KEY=VALUE")?;
                spec.env.insert(k.to_string(), v.to_string());
            }
            _ => rest.push(args[i].clone()),
        }
        i += 1;
    }
    Ok((spec, rest))
}

/// `terminalgoblin @ ...`. Returns the process exit code.
pub fn client_main(args: &[String]) -> i32 {
    let print = |v: &Value| println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
    let fail = |msg: &str| {
        eprintln!("terminalgoblin: {msg}");
        1
    };
    let Some(cmd) = args.first().map(String::as_str) else {
        print!("{CLIENT_HELP}");
        return 0;
    };
    let rest = &args[1..];
    let take_match = |rest: &[String]| -> Result<(String, Vec<String>), String> {
        let mut m = None;
        let mut other = Vec::new();
        let mut it = rest.iter();
        while let Some(a) = it.next() {
            if a == "--match" || a == "-m" {
                m = it.next().cloned();
            } else {
                other.push(a.clone());
            }
        }
        m.map(|m| (m, other)).ok_or_else(|| "--match is required".to_string())
    };
    match cmd {
        "help" | "--help" | "-h" => {
            print!("{CLIENT_HELP}");
            0
        }
        "ls" => {
            let all: Vec<Value> = list_all().into_iter().map(|(_, v)| v).collect();
            print(&Value::Array(all));
            0
        }
        "launch" => {
            let (spec, extra) = match parse_launch_flags(rest) {
                Ok(x) => x,
                Err(e) => return fail(&e),
            };
            if let Some(bad) = extra.first() {
                return fail(&format!("unknown option {bad}"));
            }
            let reply = match forward_launch(&spec) {
                Some(r) => Ok(r),
                None => start_instance(&spec),
            };
            match reply {
                Ok(r) if r.get("ok").and_then(Value::as_bool) == Some(true) => {
                    print(&r);
                    0
                }
                Ok(r) => fail(r.get("error").and_then(Value::as_str).unwrap_or("launch failed")),
                Err(e) => fail(&e),
            }
        }
        "focus" | "close" | "set-tab-title" | "set-tab-color" => {
            let (m, other) = match take_match(rest) {
                Ok(x) => x,
                Err(e) => return fail(&e),
            };
            let mut body = json!({"cmd": cmd, "match": m});
            match cmd {
                "set-tab-title" => body["title"] = json!(other.join(" ")),
                "set-tab-color" => match other.first() {
                    Some(c) => body["color"] = json!(c),
                    None => return fail("set-tab-color needs a color"),
                },
                _ => {}
            }
            // Ask every instance; the one that has the match acts on it.
            for (sock, _) in list_all() {
                if let Ok(r) = request(&sock, &body)
                    && r.get("found").and_then(Value::as_bool) == Some(true)
                {
                    print(&r);
                    return 0;
                }
            }
            fail(&format!("no tab or pane matches {m}"))
        }
        other => fail(&format!("unknown command {other:?} (try `terminalgoblin @ help`)")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn launch_flags() {
        let (spec, rest) = parse_launch_flags(&s(&[
            "--cwd",
            "/p",
            "--title",
            "My app",
            "--color",
            "#12ab9f",
            "--env",
            "PATCHGOBLIN_TAB=p1",
            "--odd",
            "--",
            "ssh",
            "-t",
            "host",
            "sh -c 'cd /p'",
        ]))
        .unwrap();
        assert_eq!(spec.cwd.as_deref(), Some("/p"));
        assert_eq!(spec.title.as_deref(), Some("My app"));
        assert_eq!(spec.color.as_deref(), Some("#12ab9f"));
        assert_eq!(spec.env.get("PATCHGOBLIN_TAB").map(String::as_str), Some("p1"));
        assert_eq!(spec.argv, s(&["ssh", "-t", "host", "sh -c 'cd /p'"]));
        assert_eq!(rest, s(&["--odd"]));
        assert!(!spec.is_empty());
        assert!(parse_launch_flags(&s(&["--env", "novalue"])).is_err());
        let (e, _) = parse_launch_flags(&s(&["-e", "htop"])).unwrap();
        assert_eq!(e.argv, s(&["htop"]));
    }

    #[test]
    fn spec_json_round_trip() {
        let (spec, _) = parse_launch_flags(&s(&["--type", "split-right", "--no-focus", "--", "ls"])).unwrap();
        let v = serde_json::to_value(&spec).unwrap();
        assert_eq!(v["type"], "split-right");
        let back: LaunchSpec = serde_json::from_value(v).unwrap();
        assert_eq!(back.kind.as_deref(), Some("split-right"));
        assert_eq!(back.focus, Some(false));
        let minimal: LaunchSpec = serde_json::from_str(r#"{"cmd":"launch","title":"x"}"#).unwrap();
        assert_eq!(minimal.title.as_deref(), Some("x"));
    }
}
