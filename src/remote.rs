//! Handling remote-control requests (see `ipc.rs`): listing, launching and finding tabs.

use serde_json::{Value, json};
use winit::window::UserAttentionType;

use crate::app::State;
use crate::config::expand_tilde;
use crate::ipc::LaunchSpec;
use crate::layout::{Axis, Node};
use crate::pane::{PaneId, PaneLaunch};
use crate::theme::{self, Color};
use crate::workspace::Tab;

fn color_json(c: Option<Color>) -> Value {
    c.map(|c| {
        json!(format!(
            "#{:02x}{:02x}{:02x}",
            (c[0] * 255.0).round() as u8,
            (c[1] * 255.0).round() as u8,
            (c[2] * 255.0).round() as u8
        ))
    })
    .unwrap_or(Value::Null)
}

/// `#rrggbb` → Some(color); `none`/empty → Ok(None).
fn parse_color(s: Option<&str>) -> Result<Option<Color>, String> {
    match s.map(str::trim) {
        None | Some("") | Some("none") => Ok(None),
        Some(c) => theme::hex(c)
            .map(Some)
            .ok_or_else(|| format!("bad color {c:?} (use #rrggbb)")),
    }
}

fn err(msg: impl Into<String>) -> Value {
    json!({"ok": false, "error": msg.into()})
}

impl State {
    pub(crate) fn handle_ipc(&mut self, body: Value) -> Value {
        let cmd = body.get("cmd").and_then(Value::as_str).unwrap_or("").to_string();
        let target = body.get("match").and_then(Value::as_str).map(str::to_string);
        let reply = match cmd.as_str() {
            "ls" => self.ipc_ls(),
            "version" => json!({"ok": true, "version": env!("CARGO_PKG_VERSION")}),
            // Test aid: render the next frame to a PNG, then exit (like `--screenshot`).
            "debug-screenshot" => match body.get("path").and_then(Value::as_str) {
                Some(path) => {
                    self.screenshot = Some((path.into(), std::time::Instant::now()));
                    self.window.request_redraw();
                    json!({"ok": true})
                }
                None => err("missing \"path\""),
            },
            "launch" => match serde_json::from_value::<LaunchSpec>(body) {
                Ok(spec) => self.launch(spec).unwrap_or_else(err),
                Err(e) => err(format!("bad launch request: {e}")),
            },
            "focus" | "close" | "set-tab-title" | "set-tab-color" => {
                let Some(m) = target else {
                    return err("missing \"match\"");
                };
                let Some((ti, pane)) = self.find_match(&m) else {
                    return json!({"ok": true, "found": false});
                };
                let tab_id = self.ws.tabs[ti].id;
                let single = matches!(self.ws.tabs[ti].root, Node::Leaf(_));
                match cmd.as_str() {
                    "focus" => {
                        self.activate_tab(ti);
                        self.focus_pane(pane);
                        self.raise_window();
                    }
                    "close" => self.close_pane(pane),
                    "set-tab-title" => {
                        let title = body
                            .get("title")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        let title = (!title.is_empty()).then_some(title);
                        // A pane sharing its tab carries its own name (shown in its header).
                        if single {
                            self.ws.tabs[ti].custom_title = title;
                        } else if let Some(p) = self.ws.panes.get_mut(&pane) {
                            p.label = title;
                        }
                    }
                    _ => {
                        let color = match parse_color(body.get("color").and_then(Value::as_str)) {
                            Ok(c) => c,
                            Err(e) => return err(e),
                        };
                        if single {
                            self.ws.tabs[ti].color = color;
                        } else if let Some(p) = self.ws.panes.get_mut(&pane) {
                            p.color = color;
                        }
                    }
                }
                self.after_structure_change();
                json!({"ok": true, "found": true, "tab_id": tab_id, "pane_id": pane})
            }
            "" => err("missing \"cmd\""),
            other => err(format!("unknown command {other:?}")),
        };
        if self.exit {
            // The last tab was closed by request.
            self.window.request_redraw();
        }
        reply
    }

    fn raise_window(&self) {
        // Wayland compositors may refuse focus requests from a background app; asking for
        // attention at least highlights the window.
        self.window.focus_window();
        if !self.focused {
            self.window
                .request_user_attention(Some(UserAttentionType::Informational));
        }
    }

    fn ipc_ls(&self) -> Value {
        let tabs: Vec<Value> = self
            .ws
            .tabs
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let panes: Vec<Value> = t
                    .root
                    .panes()
                    .into_iter()
                    .filter_map(|id| {
                        let p = self.ws.panes.get(&id)?;
                        Some(json!({
                            "id": id,
                            "title": p.title,
                            "label": p.label,
                            "color": color_json(p.color),
                            "cwd": p.cwd(),
                            "env": p.launch.env,
                            "command": p.launch.command.as_ref().map(|(c, a)| {
                                std::iter::once(c.clone()).chain(a.iter().cloned()).collect::<Vec<_>>()
                            }),
                            "profile": p.profile_name,
                            "pid": p.pid(),
                            "focused": t.focused == id,
                            "font_size": p.font_size,
                        }))
                    })
                    .collect();
                json!({
                    "id": t.id,
                    "index": i,
                    "title": self.ws.tab_title(t),
                    "custom_title": t.custom_title,
                    "color": color_json(t.color),
                    "active": i == self.ws.active,
                    "group": t.group.and_then(|g| self.ws.group(g)).map(|g| g.name.clone()),
                    "hidden": self.ws.is_hidden(t),
                    "panes": panes,
                })
            })
            .collect();
        json!({
            "ok": true,
            "pid": std::process::id(),
            "version": env!("CARGO_PKG_VERSION"),
            "socket": crate::ipc::socket_path(),
            "focused": self.focused,
            "last_focus": self.focused_at,
            "tabs": tabs,
        })
    }

    /// `env:KEY=VALUE`, `id:PANE`, `tab:TAB` or `title:TEXT` → (tab index, pane).
    pub(crate) fn find_match(&self, m: &str) -> Option<(usize, PaneId)> {
        let (kind, value) = m.split_once(':')?;
        let pane = match kind {
            "env" => {
                let (k, v) = value.split_once('=')?;
                // Oldest first, so the original tab wins over its splits and duplicates.
                let mut ids: Vec<&PaneId> = self.ws.panes.keys().collect();
                ids.sort();
                ids.into_iter()
                    .find(|id| self.ws.panes[id].launch.env.get(k).is_some_and(|x| x == v))
                    .copied()?
            }
            "id" => value.parse().ok().filter(|id| self.ws.panes.contains_key(id))?,
            "tab" => {
                let id: u64 = value.parse().ok()?;
                self.ws.tabs.iter().find(|t| t.id == id)?.focused
            }
            "title" => {
                if let Some(t) = self
                    .ws
                    .tabs
                    .iter()
                    .find(|t| self.ws.tab_title(t).eq_ignore_ascii_case(value))
                {
                    t.focused
                } else {
                    *self
                        .ws
                        .panes
                        .iter()
                        .find(|(_, p)| p.display_title().eq_ignore_ascii_case(value))?
                        .0
                }
            }
            _ => return None,
        };
        Some((self.ws.tab_of_pane(pane)?, pane))
    }

    /// Open a tab (or split) described by `spec`.
    pub(crate) fn launch(&mut self, spec: LaunchSpec) -> Result<Value, String> {
        let profile = match &spec.profile {
            Some(name) => self
                .profiles
                .iter()
                .position(|p| p.name.eq_ignore_ascii_case(name))
                .ok_or_else(|| format!("no profile named {name:?}"))?,
            None => self.cfg.default_profile_index(&self.profiles),
        };
        let color = parse_color(spec.color.as_deref())?;
        let title = spec
            .title
            .as_deref()
            .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "));
        let title = title.filter(|t| !t.is_empty());
        let launch = PaneLaunch {
            command: spec.argv.split_first().map(|(c, a)| (c.clone(), a.to_vec())),
            env: spec.env.clone(),
        };
        let cwd = spec.cwd.as_deref().map(expand_tilde);
        if let Some(dir) = &cwd
            && spec.argv.is_empty()
            && !dir.is_dir()
        {
            return Err(format!("directory does not exist: {}", dir.display()));
        }

        let split = match spec.kind.as_deref() {
            None | Some("tab") => None,
            Some("split-right") => Some(Axis::Horizontal),
            Some("split-down") => Some(Axis::Vertical),
            Some(other) => return Err(format!("unknown launch type {other:?}")),
        };

        let (tab_id, pane) = match (split, self.ws.active_tab().map(|t| t.focused)) {
            (Some(axis), Some(focused)) => {
                let rect = self
                    .pane_rects
                    .iter()
                    .find(|(id, _)| *id == focused)
                    .map(|(_, r)| *r)
                    .unwrap_or(self.content_rect());
                let pane = self
                    .spawn_pane(profile, cwd, rect, None, launch)
                    .map_err(|e| e.to_string())?;
                if let Some(p) = self.ws.panes.get_mut(&pane) {
                    p.label = title;
                    p.color = color;
                }
                let tab = self.ws.active_tab_mut().ok_or("no active tab")?;
                tab.zoomed = false;
                tab.root.split(focused, pane, axis);
                tab.focused = pane;
                (tab.id, pane)
            }
            _ => {
                let area = self.content_rect();
                let pane = self
                    .spawn_pane(profile, cwd, area, None, launch)
                    .map_err(|e| e.to_string())?;
                let id = self.ws.next_id();
                let index = self.ws.insert_tab(Tab {
                    id,
                    root: Node::Leaf(pane),
                    focused: pane,
                    custom_title: title,
                    color: color.or_else(|| self.profiles.get(profile).and_then(|p| p.tab_color)),
                    group: None,
                    zoomed: false,
                    bell: false,
                });
                match spec.group.as_deref().map(str::trim).filter(|g| !g.is_empty()) {
                    Some(name) => {
                        let existing = self
                            .ws
                            .groups
                            .iter()
                            .find(|g| g.name.eq_ignore_ascii_case(name))
                            .map(|g| g.id);
                        let index = self.ws.tab_index(id).unwrap_or(index);
                        match existing {
                            Some(gid) => self.ws.add_to_group(index, gid),
                            None => {
                                let c = self.next_group_color();
                                self.ws.new_group(index, name.to_string(), c);
                            }
                        }
                    }
                    None => {
                        // Opened from outside: don't silently join whatever group is active.
                        if let Some(i) = self.ws.tab_index(id)
                            && self.ws.tabs[i].group.is_some()
                        {
                            self.ws.ungroup_tab(i);
                        }
                    }
                }
                if let Some(i) = self.ws.tab_index(id) {
                    self.activate_tab(i);
                }
                (id, pane)
            }
        };
        self.after_structure_change();
        if spec.focus != Some(false) {
            self.raise_window();
        }
        Ok(json!({"ok": true, "found": true, "tab_id": tab_id, "pane_id": pane, "pid": std::process::id()}))
    }
}
