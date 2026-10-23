//! User configuration (`~/.config/terminalgoblin/config.toml`), shell profiles and keybindings.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use winit::keyboard::{Key, ModifiersState, NamedKey};

use crate::theme::{self, Scheme, SchemeDef};

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    pub font_family: String,
    pub font_size: f32,
    pub line_height: f32,
    pub ui_font_family: String,
    pub color_scheme: String,
    pub opacity: f32,
    pub tab_bar_opacity: Option<f32>,
    pub blur: bool,
    pub padding: f32,
    pub cursor_style: String,
    pub cursor_blink: bool,
    pub scrollback: usize,
    pub copy_on_select: bool,
    pub decorations: String,
    pub default_profile: Option<String>,
    pub detect_shells: bool,
    pub unfocused_pane_dim: f32,
    pub pane_headers: String,
    pub bold_is_bright: bool,
    pub profiles: Vec<ProfileDef>,
    pub color_schemes: Vec<SchemeDef>,
    pub keybindings: HashMap<String, String>,
    pub locations: Vec<LocationDef>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            // cosmic-term's default font, which is what Claude Code renders in on COSMIC.
            font_family: "Noto Sans Mono".into(),
            font_size: 14.0,
            line_height: 1.0,
            // COSMIC's interface font.
            ui_font_family: "Open Sans".into(),
            color_scheme: "Goblin".into(),
            opacity: 0.94,
            tab_bar_opacity: None,
            blur: true,
            padding: 8.0,
            cursor_style: "block".into(),
            cursor_blink: true,
            scrollback: 10_000,
            copy_on_select: false,
            decorations: "client".into(),
            default_profile: None,
            detect_shells: true,
            unfocused_pane_dim: 0.12,
            pane_headers: "auto".into(),
            bold_is_bright: false,
            profiles: Vec::new(),
            color_schemes: Vec::new(),
            keybindings: HashMap::new(),
            locations: Vec::new(),
        }
    }
}

/// A saved place to open a tab in: a local folder, or a folder on an SSH host.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct LocationDef {
    pub name: String,
    /// SSH host (as in `~/.ssh/config`); `None` or blank means a local folder.
    pub host: Option<String>,
    pub path: String,
    /// Shortcut that opens it, e.g. `ctrl+alt+h`.
    pub key: Option<String>,
    /// Name of the profile to use (`ProfileDef.name`); the default profile when unset.
    pub profile: Option<String>,
    /// `#rrggbb`
    pub tab_color: Option<String>,
}

impl LocationDef {
    pub fn remote_host(&self) -> Option<&str> {
        self.host.as_deref().map(str::trim).filter(|h| !h.is_empty())
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct ProfileDef {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub color_scheme: Option<String>,
    pub tab_color: Option<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct Profile {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub scheme: Option<String>,
    pub tab_color: Option<theme::Color>,
    pub env: HashMap<String, String>,
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("terminalgoblin")
}

/// `$TERMINALGOBLIN_CONFIG`, or `~/.config/terminalgoblin/config.toml`.
pub fn config_path() -> PathBuf {
    std::env::var_os("TERMINALGOBLIN_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| config_dir().join("config.toml"))
}

impl Config {
    /// Load the config file, writing a commented default one first if none exists.
    pub fn load() -> (Config, Option<String>) {
        let path = config_path();
        if !path.exists()
            && let Err(e) = write_default(&path)
        {
            log::warn!("could not write default config to {}: {e}", path.display());
        }
        match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str::<Config>(&text) {
                Ok(cfg) => (cfg.sanitized(), None),
                Err(e) => {
                    log::error!("config error: {e}");
                    (Config::default(), Some(format!("Config error: {}", e.message())))
                }
            },
            Err(_) => (Config::default(), None),
        }
    }

    fn sanitized(mut self) -> Config {
        self.font_size = self.font_size.clamp(4.0, 96.0);
        self.line_height = self.line_height.clamp(0.6, 3.0);
        self.opacity = self.opacity.clamp(0.05, 1.0);
        self.padding = self.padding.clamp(0.0, 64.0);
        self.unfocused_pane_dim = self.unfocused_pane_dim.clamp(0.0, 0.8);
        self
    }

    pub fn client_decorations(&self) -> bool {
        !matches!(self.decorations.as_str(), "system" | "server" | "none")
    }

    pub fn tab_bar_opacity(&self) -> f32 {
        self.tab_bar_opacity.unwrap_or(self.opacity).clamp(0.05, 1.0)
    }

    /// All schemes: built-ins first, user-defined ones override by name.
    pub fn schemes(&self) -> Vec<Scheme> {
        let mut defs = theme::builtin_schemes();
        for user in &self.color_schemes {
            if let Some(existing) = defs.iter_mut().find(|d| d.name.eq_ignore_ascii_case(&user.name)) {
                *existing = user.clone();
            } else {
                defs.push(user.clone());
            }
        }
        defs.iter().map(Scheme::from_def).collect()
    }

    pub fn scheme(&self, name: Option<&str>) -> Scheme {
        let schemes = self.schemes();
        let want = name.unwrap_or(&self.color_scheme);
        schemes
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(want))
            .or_else(|| schemes.iter().find(|s| s.name.eq_ignore_ascii_case(&self.color_scheme)))
            .cloned()
            .unwrap_or_else(|| schemes[0].clone())
    }

    /// Configured profiles, followed by any shells in `/etc/shells` they don't already cover
    /// (unless `detect_shells = false`), so shells installed after the config was written show up.
    pub fn profiles(&self) -> Vec<Profile> {
        let configured: Vec<Profile> = self
            .profiles
            .iter()
            .map(|p| Profile {
                name: p.name.clone(),
                command: expand_tilde(&p.command).to_string_lossy().into_owned(),
                args: p.args.clone(),
                cwd: p.cwd.as_deref().map(expand_tilde),
                scheme: p.color_scheme.clone(),
                tab_color: p.tab_color.as_deref().and_then(theme::hex),
                env: p.env.clone(),
            })
            .collect();
        let detected = if self.detect_shells || configured.is_empty() {
            detect_shells()
        } else {
            Vec::new()
        };
        let out = merge_detected(configured, detected);
        if out.is_empty() { vec![fallback_shell()] } else { out }
    }

    pub fn default_profile_index(&self, profiles: &[Profile]) -> usize {
        if let Some(name) = &self.default_profile
            && let Some(i) = profiles.iter().position(|p| p.name.eq_ignore_ascii_case(name))
        {
            return i;
        }
        let login_shell = std::env::var("SHELL").unwrap_or_default();
        let login_base = Path::new(&login_shell).file_name().map(|s| s.to_owned());
        profiles
            .iter()
            .position(|p| Path::new(&p.command).file_name().map(|s| s.to_owned()) == login_base)
            .unwrap_or(0)
    }

    pub fn keybindings(&self) -> Vec<(KeyCombo, Action)> {
        let mut map: Vec<(KeyCombo, Action)> = Vec::new();
        let mut add = |combo: &str, action: &str| {
            let (Some(k), Some(a)) = (KeyCombo::parse(combo), Action::parse(action)) else {
                log::warn!("ignoring keybinding {combo:?} = {action:?}");
                return;
            };
            map.retain(|(existing, _)| *existing != k);
            if a != Action::Unbound {
                map.push((k, a));
            }
        };
        for (combo, action) in DEFAULT_KEYBINDINGS {
            add(combo, action);
        }
        for (combo, action) in &self.keybindings {
            add(combo, action);
        }
        // Location shortcuts live with their location and win over any other use of the key.
        for (i, loc) in self.locations.iter().enumerate() {
            let Some(key) = loc.key.as_deref().filter(|k| !k.trim().is_empty()) else {
                continue;
            };
            let Some(k) = KeyCombo::parse(key) else {
                log::warn!("ignoring key {key:?} of location {:?}", loc.name);
                continue;
            };
            map.retain(|(existing, _)| *existing != k);
            map.push((k, Action::OpenLocation(i)));
        }
        map
    }
}

pub fn expand_tilde(p: &str) -> PathBuf {
    if p == "~" {
        return dirs::home_dir().unwrap_or_default();
    }
    if let Some(rest) = p.strip_prefix("~/") {
        return dirs::home_dir().unwrap_or_default().join(rest);
    }
    PathBuf::from(p)
}

/// Quote `s` for a POSIX shell.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The `ssh` command for a tab on `host`, starting in `path` (the remote home when `None`).
/// A leading `~` stays unquoted so the remote shell expands it.
pub fn ssh_launch(host: &str, path: Option<&str>) -> (String, Vec<String>) {
    let mut args = vec!["-t".to_string(), "--".to_string(), host.to_string()];
    if let Some(path) = path.map(str::trim).filter(|p| !p.is_empty()) {
        // `$SHELL` is expanded by the remote shell, giving the user's own login shell there.
        args.push(format!("cd {} && exec $SHELL -l", remote_path_arg(path)));
    }
    ("ssh".to_string(), args)
}

/// `path` quoted for a remote shell, leaving a leading `~` unquoted so it expands.
pub fn remote_path_arg(path: &str) -> String {
    if path == "~" {
        "~".to_string()
    } else if let Some(rest) = path.strip_prefix("~/") {
        if rest.is_empty() {
            "~/".to_string()
        } else {
            format!("~/{}", shell_quote(rest))
        }
    } else {
        shell_quote(path)
    }
}

/// Whether `host` is safe to hand to ssh as a host name (not an option, no spaces).
pub fn valid_ssh_host(host: &str) -> bool {
    !host.is_empty() && !host.starts_with('-') && !host.chars().any(char::is_whitespace)
}

/// Concrete host names from `~/.ssh/config`, in file order. Patterns (`*`, `?`, `!`) are skipped,
/// and `Include`d files aren't read.
pub fn ssh_hosts() -> Vec<String> {
    let Some(path) = dirs::home_dir().map(|h| h.join(".ssh/config")) else {
        return Vec::new();
    };
    parse_ssh_hosts(&std::fs::read_to_string(path).unwrap_or_default())
}

fn parse_ssh_hosts(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let Some(keyword) = line.get(..4) else { continue };
        let rest = &line[4..];
        if !keyword.eq_ignore_ascii_case("host") || !rest.starts_with(|c: char| c.is_whitespace() || c == '=') {
            continue;
        }
        let rest = rest.trim_start().trim_start_matches('=');
        let rest = rest.split('#').next().unwrap_or("");
        for name in rest.split_whitespace().map(|n| n.trim_matches('"')) {
            if name.is_empty() || name.contains(['*', '?', '!']) || !valid_ssh_host(name) {
                continue;
            }
            if !out.iter().any(|h| h == name) {
                out.push(name.to_string());
            }
        }
    }
    out
}

fn detect_shells() -> Vec<Profile> {
    const SKIP: &[&str] = &["sh", "dash", "rbash", "git-shell", "nologin", "false", "screen", "tmux"];
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    let listed = std::fs::read_to_string("/etc/shells").unwrap_or_default();
    let login = std::env::var("SHELL").ok();
    let candidates = login.iter().cloned().chain(
        listed
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(String::from),
    );
    for path in candidates {
        let Some(base) = Path::new(&path).file_name().and_then(|b| b.to_str()).map(String::from) else {
            continue;
        };
        if SKIP.contains(&base.as_str()) || seen.contains(&base) || !Path::new(&path).exists() {
            continue;
        }
        seen.push(base.clone());
        let mut name = base.clone();
        if let Some(first) = name.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        out.push(Profile {
            name,
            command: path,
            args: Vec::new(),
            cwd: None,
            scheme: None,
            tab_color: None,
            env: HashMap::new(),
        });
    }
    out
}

/// `$SHELL` (or `/bin/sh`), for when there are no profiles at all.
fn fallback_shell() -> Profile {
    Profile {
        name: "Shell".into(),
        command: std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()),
        args: Vec::new(),
        cwd: None,
        scheme: None,
        tab_color: None,
        env: HashMap::new(),
    }
}

/// Appends the detected shells whose program isn't already used by a configured profile.
/// Programs are compared by file name, so `/bin/fish` covers a detected `/usr/bin/fish`.
fn merge_detected(mut configured: Vec<Profile>, detected: Vec<Profile>) -> Vec<Profile> {
    let base = |p: &Profile| Path::new(&p.command).file_name().map(|s| s.to_owned());
    for d in detected {
        if !configured.iter().any(|c| base(c) == base(&d)) {
            configured.push(d);
        }
    }
    configured
}

/// Everything a key binding or menu entry can trigger on the active tab/pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Unbound,
    NewTab,
    NewTabProfile(usize),
    DuplicateTab,
    CloseTab,
    ClosePane,
    NextTab,
    PrevTab,
    SwitchToTab(usize),
    MoveTabLeft,
    MoveTabRight,
    RenameTab,
    SplitRight,
    SplitDown,
    SplitAuto,
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    ResizeLeft,
    ResizeRight,
    ResizeUp,
    ResizeDown,
    TogglePaneZoom,
    MovePaneToNewTab,
    RenamePane,
    NewGroup,
    Ungroup,
    ToggleGroupCollapse,
    Copy,
    Paste,
    SelectAll,
    ScrollUp,
    ScrollDown,
    ScrollPageUp,
    ScrollPageDown,
    ScrollTop,
    ScrollBottom,
    ClearScrollback,
    FontBigger,
    FontSmaller,
    FontReset,
    OpacityUp,
    OpacityDown,
    ToggleTransparency,
    NextColorScheme,
    ToggleFullscreen,
    OpenSettings,
    ReloadConfig,
    NewTabMenu,
    Quit,
    /// Open `[[locations]]` entry N. Bound through the location's own `key`, not `[keybindings]`.
    OpenLocation(usize),
}

/// Every bindable action, grouped for the keybindings editor: (section, action, label).
pub fn action_catalog() -> Vec<(&'static str, Action, String)> {
    use Action::*;
    let mut v: Vec<(&'static str, Action, String)> = Vec::new();
    let mut add = |section: &'static str, a: Action, label: &str| v.push((section, a, label.to_string()));
    add("Tabs", NewTab, "New tab");
    add("Tabs", NewTabMenu, "New tab menu");
    for i in 0..5 {
        add("Tabs", NewTabProfile(i), &format!("New tab with profile {}", i + 1));
    }
    add("Tabs", DuplicateTab, "Duplicate tab");
    add("Tabs", CloseTab, "Close tab");
    add("Tabs", NextTab, "Next tab");
    add("Tabs", PrevTab, "Previous tab");
    for i in 0..9 {
        add("Tabs", SwitchToTab(i), &format!("Go to tab {}", i + 1));
    }
    add("Tabs", MoveTabLeft, "Move tab left");
    add("Tabs", MoveTabRight, "Move tab right");
    add("Tabs", RenameTab, "Rename tab");
    add("Panes", SplitRight, "Split right");
    add("Panes", SplitDown, "Split down");
    add("Panes", SplitAuto, "Split (automatic direction)");
    add("Panes", ClosePane, "Close pane");
    add("Panes", FocusLeft, "Focus pane left");
    add("Panes", FocusRight, "Focus pane right");
    add("Panes", FocusUp, "Focus pane above");
    add("Panes", FocusDown, "Focus pane below");
    add("Panes", ResizeLeft, "Resize pane left");
    add("Panes", ResizeRight, "Resize pane right");
    add("Panes", ResizeUp, "Resize pane up");
    add("Panes", ResizeDown, "Resize pane down");
    add("Panes", TogglePaneZoom, "Zoom pane");
    add("Panes", MovePaneToNewTab, "Move pane to new tab");
    add("Panes", RenamePane, "Rename pane");
    add("Groups", NewGroup, "Add tab to new group");
    add("Groups", Ungroup, "Remove tab from group");
    add("Groups", ToggleGroupCollapse, "Collapse/expand group");
    add("Text", Copy, "Copy");
    add("Text", Paste, "Paste");
    add("Text", SelectAll, "Select all");
    add("Text", ScrollUp, "Scroll up");
    add("Text", ScrollDown, "Scroll down");
    add("Text", ScrollPageUp, "Scroll page up");
    add("Text", ScrollPageDown, "Scroll page down");
    add("Text", ScrollTop, "Scroll to top");
    add("Text", ScrollBottom, "Scroll to bottom");
    add("Text", ClearScrollback, "Clear scrollback");
    add("Text", FontBigger, "Pane text bigger");
    add("Text", FontSmaller, "Pane text smaller");
    add("Text", FontReset, "Reset pane text size");
    add("Window", OpacityUp, "More opaque");
    add("Window", OpacityDown, "More transparent");
    add("Window", ToggleTransparency, "Toggle transparency");
    add("Window", NextColorScheme, "Next color scheme");
    add("Window", ToggleFullscreen, "Fullscreen");
    add("Window", OpenSettings, "Settings");
    add("Window", ReloadConfig, "Reload config file");
    add("Window", Quit, "Quit");
    v
}

impl Action {
    /// The name used in the config file.
    pub fn name(self) -> String {
        use Action::*;
        match self {
            NewTabProfile(i) => return format!("new_tab_profile_{}", i + 1),
            SwitchToTab(i) => return format!("switch_to_tab_{}", i + 1),
            OpenLocation(i) => return format!("open_location_{}", i + 1),
            _ => {}
        }
        match self {
            Unbound => "unbound",
            NewTab => "new_tab",
            DuplicateTab => "duplicate_tab",
            CloseTab => "close_tab",
            ClosePane => "close_pane",
            NextTab => "next_tab",
            PrevTab => "prev_tab",
            MoveTabLeft => "move_tab_left",
            MoveTabRight => "move_tab_right",
            RenameTab => "rename_tab",
            SplitRight => "split_right",
            SplitDown => "split_down",
            SplitAuto => "split_auto",
            FocusLeft => "focus_left",
            FocusRight => "focus_right",
            FocusUp => "focus_up",
            FocusDown => "focus_down",
            ResizeLeft => "resize_left",
            ResizeRight => "resize_right",
            ResizeUp => "resize_up",
            ResizeDown => "resize_down",
            TogglePaneZoom => "toggle_pane_zoom",
            MovePaneToNewTab => "move_pane_to_new_tab",
            RenamePane => "rename_pane",
            NewGroup => "new_group",
            Ungroup => "ungroup",
            ToggleGroupCollapse => "toggle_group_collapse",
            Copy => "copy",
            Paste => "paste",
            SelectAll => "select_all",
            ScrollUp => "scroll_up",
            ScrollDown => "scroll_down",
            ScrollPageUp => "scroll_page_up",
            ScrollPageDown => "scroll_page_down",
            ScrollTop => "scroll_top",
            ScrollBottom => "scroll_bottom",
            ClearScrollback => "clear_scrollback",
            FontBigger => "font_bigger",
            FontSmaller => "font_smaller",
            FontReset => "font_reset",
            OpacityUp => "opacity_up",
            OpacityDown => "opacity_down",
            ToggleTransparency => "toggle_transparency",
            NextColorScheme => "next_color_scheme",
            ToggleFullscreen => "toggle_fullscreen",
            OpenSettings => "open_settings",
            ReloadConfig => "reload_config",
            NewTabMenu => "new_tab_menu",
            Quit => "quit",
            NewTabProfile(_) | SwitchToTab(_) | OpenLocation(_) => unreachable!(),
        }
        .to_string()
    }

    pub fn parse(s: &str) -> Option<Action> {
        use Action::*;
        let s = s.trim().to_ascii_lowercase().replace('-', "_");
        if let Some(n) = s.strip_prefix("new_tab_profile_") {
            return n
                .parse::<usize>()
                .ok()
                .filter(|n| *n >= 1)
                .map(|n| NewTabProfile(n - 1));
        }
        if let Some(n) = s.strip_prefix("switch_to_tab_") {
            return n.parse::<usize>().ok().filter(|n| *n >= 1).map(|n| SwitchToTab(n - 1));
        }
        if let Some(n) = s.strip_prefix("open_location_") {
            return n.parse::<usize>().ok().filter(|n| *n >= 1).map(|n| OpenLocation(n - 1));
        }
        Some(match s.as_str() {
            "unbound" | "none" => Unbound,
            "new_tab" => NewTab,
            "duplicate_tab" => DuplicateTab,
            "close_tab" => CloseTab,
            "close_pane" => ClosePane,
            "next_tab" => NextTab,
            "prev_tab" | "previous_tab" => PrevTab,
            "move_tab_left" => MoveTabLeft,
            "move_tab_right" => MoveTabRight,
            "rename_tab" => RenameTab,
            "split_right" | "split_vertical" => SplitRight,
            "split_down" | "split_horizontal" => SplitDown,
            "split_auto" | "duplicate_pane" => SplitAuto,
            "focus_left" => FocusLeft,
            "focus_right" => FocusRight,
            "focus_up" => FocusUp,
            "focus_down" => FocusDown,
            "resize_left" => ResizeLeft,
            "resize_right" => ResizeRight,
            "resize_up" => ResizeUp,
            "resize_down" => ResizeDown,
            "toggle_pane_zoom" | "zoom_pane" => TogglePaneZoom,
            "move_pane_to_new_tab" | "break_out_pane" => MovePaneToNewTab,
            "rename_pane" => RenamePane,
            "new_group" => NewGroup,
            "ungroup" | "remove_from_group" => Ungroup,
            "toggle_group_collapse" => ToggleGroupCollapse,
            "copy" => Copy,
            "paste" => Paste,
            "select_all" => SelectAll,
            "scroll_up" => ScrollUp,
            "scroll_down" => ScrollDown,
            "scroll_page_up" => ScrollPageUp,
            "scroll_page_down" => ScrollPageDown,
            "scroll_top" => ScrollTop,
            "scroll_bottom" => ScrollBottom,
            "clear_scrollback" => ClearScrollback,
            "font_bigger" | "increase_font_size" => FontBigger,
            "font_smaller" | "decrease_font_size" => FontSmaller,
            "font_reset" | "reset_font_size" => FontReset,
            "opacity_up" => OpacityUp,
            "opacity_down" => OpacityDown,
            "toggle_transparency" => ToggleTransparency,
            "next_color_scheme" => NextColorScheme,
            "toggle_fullscreen" => ToggleFullscreen,
            "open_settings" => OpenSettings,
            "reload_config" => ReloadConfig,
            "new_tab_menu" => NewTabMenu,
            "quit" => Quit,
            _ => return None,
        })
    }
}

/// Defaults modelled on Windows Terminal.
const DEFAULT_KEYBINDINGS: &[(&str, &str)] = &[
    ("ctrl+shift+t", "new_tab"),
    ("ctrl+shift+space", "new_tab_menu"),
    ("ctrl+shift+1", "new_tab_profile_1"),
    ("ctrl+shift+2", "new_tab_profile_2"),
    ("ctrl+shift+3", "new_tab_profile_3"),
    ("ctrl+shift+4", "new_tab_profile_4"),
    ("ctrl+shift+5", "new_tab_profile_5"),
    ("ctrl+shift+d", "duplicate_tab"),
    ("ctrl+shift+w", "close_pane"),
    ("ctrl+shift+q", "close_tab"),
    ("ctrl+tab", "next_tab"),
    ("ctrl+shift+tab", "prev_tab"),
    ("ctrl+pagedown", "next_tab"),
    ("ctrl+pageup", "prev_tab"),
    ("ctrl+alt+1", "switch_to_tab_1"),
    ("ctrl+alt+2", "switch_to_tab_2"),
    ("ctrl+alt+3", "switch_to_tab_3"),
    ("ctrl+alt+4", "switch_to_tab_4"),
    ("ctrl+alt+5", "switch_to_tab_5"),
    ("ctrl+alt+6", "switch_to_tab_6"),
    ("ctrl+alt+7", "switch_to_tab_7"),
    ("ctrl+alt+8", "switch_to_tab_8"),
    ("ctrl+alt+9", "switch_to_tab_9"),
    ("ctrl+shift+pageup", "move_tab_left"),
    ("ctrl+shift+pagedown", "move_tab_right"),
    ("ctrl+shift+r", "rename_tab"),
    ("alt+shift+equal", "split_right"),
    ("alt+shift+plus", "split_right"),
    ("alt+shift+minus", "split_down"),
    ("alt+shift+d", "split_auto"),
    ("alt+left", "focus_left"),
    ("alt+right", "focus_right"),
    ("alt+up", "focus_up"),
    ("alt+down", "focus_down"),
    ("alt+shift+left", "resize_left"),
    ("alt+shift+right", "resize_right"),
    ("alt+shift+up", "resize_up"),
    ("alt+shift+down", "resize_down"),
    ("alt+shift+z", "toggle_pane_zoom"),
    ("alt+shift+n", "move_pane_to_new_tab"),
    ("alt+shift+r", "rename_pane"),
    ("ctrl+shift+g", "new_group"),
    ("ctrl+shift+u", "ungroup"),
    ("ctrl+shift+c", "copy"),
    ("ctrl+shift+v", "paste"),
    ("shift+insert", "paste"),
    ("ctrl+shift+a", "select_all"),
    ("ctrl+shift+up", "scroll_up"),
    ("ctrl+shift+down", "scroll_down"),
    ("shift+pageup", "scroll_page_up"),
    ("shift+pagedown", "scroll_page_down"),
    ("ctrl+shift+home", "scroll_top"),
    ("ctrl+shift+end", "scroll_bottom"),
    ("ctrl+shift+k", "clear_scrollback"),
    ("ctrl+equal", "font_bigger"),
    ("ctrl+plus", "font_bigger"),
    ("ctrl+minus", "font_smaller"),
    ("ctrl+0", "font_reset"),
    ("ctrl+shift+alt+up", "opacity_up"),
    ("ctrl+shift+alt+down", "opacity_down"),
    ("ctrl+shift+o", "toggle_transparency"),
    ("ctrl+shift+s", "next_color_scheme"),
    ("f11", "toggle_fullscreen"),
    ("ctrl+comma", "open_settings"),
    ("ctrl+shift+comma", "reload_config"),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyCombo {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub logo: bool,
    /// Lower-cased key name: a single character or a named key like `pageup`.
    pub key: String,
}

impl KeyCombo {
    pub fn parse(s: &str) -> Option<KeyCombo> {
        let mut combo = KeyCombo {
            ctrl: false,
            shift: false,
            alt: false,
            logo: false,
            key: String::new(),
        };
        for part in s.to_ascii_lowercase().split('+').map(str::trim) {
            match part {
                "ctrl" | "control" => combo.ctrl = true,
                "shift" => combo.shift = true,
                "alt" => combo.alt = true,
                "super" | "logo" | "win" | "meta" => combo.logo = true,
                "" => combo.key = "plus".into(),
                k => combo.key = normalize_key_name(k),
            }
        }
        (!combo.key.is_empty()).then_some(combo)
    }

    /// Match a key event. `unmodified` is the key with modifiers stripped (Shift+= gives `=`).
    pub fn matches(&self, mods: ModifiersState, unmodified: &Key) -> bool {
        self.ctrl == mods.control_key()
            && self.shift == mods.shift_key()
            && self.alt == mods.alt_key()
            && self.logo == mods.super_key()
            && key_name(unmodified).is_some_and(|k| k == self.key)
    }
}

impl KeyCombo {
    /// Build a combo from a key press; `None` for keys that can't be bound (lone modifiers).
    pub fn from_key(mods: ModifiersState, unmodified: &Key) -> Option<KeyCombo> {
        Some(KeyCombo {
            ctrl: mods.control_key(),
            shift: mods.shift_key(),
            alt: mods.alt_key(),
            logo: mods.super_key(),
            key: key_name(unmodified)?,
        })
    }

    /// Config-file form, e.g. `ctrl+shift+t`.
    pub fn to_config(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if self.ctrl {
            parts.push("ctrl");
        }
        if self.shift {
            parts.push("shift");
        }
        if self.alt {
            parts.push("alt");
        }
        if self.logo {
            parts.push("super");
        }
        parts.push(&self.key);
        parts.join("+")
    }

    /// Human form, e.g. `Ctrl+Shift+T`.
    pub fn display(&self) -> String {
        let key = match self.key.as_str() {
            "equal" => "=".to_string(),
            "minus" => "-".to_string(),
            "plus" => "+".to_string(),
            "comma" => ",".to_string(),
            "period" => ".".to_string(),
            "slash" => "/".to_string(),
            "backslash" => "\\".to_string(),
            "semicolon" => ";".to_string(),
            "quote" => "'".to_string(),
            "backtick" => "`".to_string(),
            "bracketleft" => "[".to_string(),
            "bracketright" => "]".to_string(),
            "pageup" => "PgUp".to_string(),
            "pagedown" => "PgDn".to_string(),
            "up" => "↑".to_string(),
            "down" => "↓".to_string(),
            "left" => "←".to_string(),
            "right" => "→".to_string(),
            "escape" => "Esc".to_string(),
            k if k.len() == 1 => k.to_uppercase(),
            k if k.starts_with('f') && k[1..].parse::<u8>().is_ok() => k.to_uppercase(),
            k => {
                let mut c = k.chars();
                c.next()
                    .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                    .unwrap_or_default()
            }
        };
        let mut s = String::new();
        if self.ctrl {
            s.push_str("Ctrl+");
        }
        if self.shift {
            s.push_str("Shift+");
        }
        if self.alt {
            s.push_str("Alt+");
        }
        if self.logo {
            s.push_str("Super+");
        }
        s + &key
    }
}

/// The `[keybindings]` entries needed to turn the defaults into `bindings`. Location shortcuts
/// are left out: they're saved with their location and re-applied over these on load.
pub fn binding_overrides(bindings: &[(KeyCombo, Action)]) -> Vec<(String, String)> {
    let defaults = Config::default().keybindings();
    let mut out = Vec::new();
    for (k, a) in &defaults {
        if !bindings.iter().any(|(bk, _)| bk == k) {
            out.push((k.to_config(), "unbound".to_string()));
        } else if let Some((_, ba)) = bindings.iter().find(|(bk, _)| bk == k)
            && ba != a
            && !matches!(ba, Action::OpenLocation(_))
        {
            out.push((k.to_config(), ba.name()));
        }
    }
    for (k, a) in bindings {
        if matches!(a, Action::OpenLocation(_)) {
            continue;
        }
        if !defaults.iter().any(|(dk, _)| dk == k) {
            out.push((k.to_config(), a.name()));
        }
    }
    out.sort();
    out
}

/// Write `cfg` (and the key bindings) back to the config file, keeping its comments and layout.
/// Returns the text written.
pub fn save(cfg: &Config, bindings: &[(KeyCombo, Action)]) -> anyhow::Result<String> {
    save_to(&config_path(), cfg, bindings)
}

fn save_to(path: &Path, cfg: &Config, bindings: &[(KeyCombo, Action)]) -> anyhow::Result<String> {
    use toml_edit::{DocumentMut, Item, Table, Value};
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut doc: DocumentMut = text.parse()?;

    fn set(doc: &mut DocumentMut, key: &str, value: Value) {
        // Keep the existing entry's comments/whitespace when overwriting it.
        match doc.get_mut(key).and_then(Item::as_value_mut) {
            Some(old) => {
                let decor = old.decor().clone();
                *old = value;
                *old.decor_mut() = decor;
            }
            None => {
                doc.insert(key, Item::Value(value));
            }
        }
    }
    set(&mut doc, "font_family", cfg.font_family.as_str().into());
    set(&mut doc, "font_size", (cfg.font_size as f64).into());
    set(&mut doc, "line_height", round2(cfg.line_height).into());
    set(&mut doc, "ui_font_family", cfg.ui_font_family.as_str().into());
    set(&mut doc, "color_scheme", cfg.color_scheme.as_str().into());
    set(&mut doc, "opacity", round2(cfg.opacity).into());
    match cfg.tab_bar_opacity {
        Some(v) => set(&mut doc, "tab_bar_opacity", round2(v).into()),
        None => {
            doc.remove("tab_bar_opacity");
        }
    }
    set(&mut doc, "blur", cfg.blur.into());
    set(&mut doc, "padding", (cfg.padding as f64).into());
    set(&mut doc, "cursor_style", cfg.cursor_style.as_str().into());
    set(&mut doc, "cursor_blink", cfg.cursor_blink.into());
    set(&mut doc, "scrollback", (cfg.scrollback as i64).into());
    set(&mut doc, "copy_on_select", cfg.copy_on_select.into());
    set(&mut doc, "unfocused_pane_dim", round2(cfg.unfocused_pane_dim).into());
    set(&mut doc, "pane_headers", cfg.pane_headers.as_str().into());
    set(&mut doc, "bold_is_bright", cfg.bold_is_bright.into());
    set(&mut doc, "decorations", cfg.decorations.as_str().into());
    if let Some(p) = &cfg.default_profile {
        set(&mut doc, "default_profile", p.as_str().into());
    }

    let overrides = binding_overrides(bindings);
    let table = doc
        .entry("keybindings")
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("[keybindings] is not a table"))?;
    let stale: Vec<String> = table.iter().map(|(k, _)| k.to_string()).collect();
    for k in stale {
        table.remove(&k);
    }
    for (k, a) in overrides {
        table.insert(&k, toml_edit::value(a));
    }

    // Locations, with each one's shortcut taken from the live bindings.
    let mut locations = cfg.locations.clone();
    for (i, loc) in locations.iter_mut().enumerate() {
        loc.key = bindings
            .iter()
            .find(|(_, a)| *a == Action::OpenLocation(i))
            .map(|(k, _)| k.to_config());
    }
    if locations.is_empty() {
        doc.remove("locations");
    } else {
        let mut tables = toml_edit::ArrayOfTables::new();
        for loc in &locations {
            let mut t = Table::new();
            t.insert("name", toml_edit::value(loc.name.as_str()));
            let optional = [
                ("host", loc.remote_host()),
                ("path", Some(loc.path.as_str())),
                ("key", loc.key.as_deref()),
                ("profile", loc.profile.as_deref()),
                ("tab_color", loc.tab_color.as_deref()),
            ];
            for (k, v) in optional {
                if let Some(v) = v {
                    t.insert(k, toml_edit::value(v));
                }
            }
            tables.push(t);
        }
        doc.insert("locations", Item::ArrayOfTables(tables));
    }

    let out = doc.to_string();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, &out)?;
    Ok(out)
}

fn round2(v: f32) -> f64 {
    (v as f64 * 100.0).round() / 100.0
}

fn normalize_key_name(k: &str) -> String {
    match k {
        "=" => "equal",
        "-" => "minus",
        "," => "comma",
        "." => "period",
        "/" => "slash",
        "\\" => "backslash",
        ";" => "semicolon",
        "'" => "quote",
        "`" => "backtick",
        "[" => "bracketleft",
        "]" => "bracketright",
        "esc" => "escape",
        "return" => "enter",
        "pgup" => "pageup",
        "pgdn" => "pagedown",
        "del" => "delete",
        "ins" => "insert",
        "arrowup" => "up",
        "arrowdown" => "down",
        "arrowleft" => "left",
        "arrowright" => "right",
        other => other,
    }
    .to_string()
}

pub fn key_name(key: &Key) -> Option<String> {
    Some(match key {
        Key::Character(s) => {
            let s = s.to_lowercase();
            match s.as_str() {
                "+" => "plus".into(),
                " " => "space".into(),
                _ => normalize_key_name(&s),
            }
        }
        Key::Named(n) => match n {
            NamedKey::Tab => "tab",
            NamedKey::Enter => "enter",
            NamedKey::Space => "space",
            NamedKey::Escape => "escape",
            NamedKey::Backspace => "backspace",
            NamedKey::Delete => "delete",
            NamedKey::Insert => "insert",
            NamedKey::Home => "home",
            NamedKey::End => "end",
            NamedKey::PageUp => "pageup",
            NamedKey::PageDown => "pagedown",
            NamedKey::ArrowUp => "up",
            NamedKey::ArrowDown => "down",
            NamedKey::ArrowLeft => "left",
            NamedKey::ArrowRight => "right",
            NamedKey::F1 => "f1",
            NamedKey::F2 => "f2",
            NamedKey::F3 => "f3",
            NamedKey::F4 => "f4",
            NamedKey::F5 => "f5",
            NamedKey::F6 => "f6",
            NamedKey::F7 => "f7",
            NamedKey::F8 => "f8",
            NamedKey::F9 => "f9",
            NamedKey::F10 => "f10",
            NamedKey::F11 => "f11",
            NamedKey::F12 => "f12",
            _ => return None,
        }
        .into(),
        _ => return None,
    })
}

fn write_default(path: &Path) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut text = DEFAULT_CONFIG.to_string();
    let shells = detect_shells();
    text.push_str("\n# Shell profiles. Detected on first run; edit freely.\n# Shells installed later are added to the menu while detect_shells = true.\n");
    text.push_str("# Each profile appears in the new-tab (▾) menu.\n");
    for p in &shells {
        text.push_str(&format!(
            "\n[[profiles]]\nname = {:?}\ncommand = {:?}\n",
            p.name, p.command
        ));
        text.push_str("# args = []\n# cwd = \"~\"\n# color_scheme = \"Goblin\"\n# tab_color = \"#7ee787\"\n");
    }
    std::fs::write(path, text)
}

const DEFAULT_CONFIG: &str = r##"# TerminalGoblin configuration.
# Saved changes apply immediately.

# Terminal font. Noto Sans Mono is cosmic-term's default, so it's what Claude Code renders in there.
font_family = "Noto Sans Mono"
font_size = 14.0          # logical pixels
line_height = 1.0         # multiplier on the font's natural line height
ui_font_family = "Open Sans"

# Built-in schemes: Goblin, Campbell, One Half Dark, Tokyo Night,
# Catppuccin Mocha, Dracula, Nord, Gruvbox Dark.
color_scheme = "Goblin"

# Transparency. 1.0 is opaque. Ctrl+Shift+Scroll or Ctrl+Shift+Alt+Up/Down changes it live,
# Ctrl+Shift+O toggles it.
opacity = 0.94
# tab_bar_opacity = 0.9   # defaults to `opacity`
blur = true               # asks the compositor to blur behind the window, if it supports it

padding = 8.0
cursor_style = "block"    # block | beam | underline
cursor_blink = true
scrollback = 10000
copy_on_select = false
unfocused_pane_dim = 0.12 # darken inactive split panes (0 disables)
# Name/color headers on split panes: "auto" (when a pane has a name or color), "always", "never".
pane_headers = "auto"
bold_is_bright = false

# "client" draws tabs in the titlebar like Windows Terminal; "system" uses compositor decorations.
decorations = "client"

# default_profile = "Fish"
# Also list shells from /etc/shells that no profile below uses, such as ones installed later.
detect_shells = true

# Keybindings override the defaults. Use "unbound" to remove one.
# Actions: new_tab, new_tab_menu, new_tab_profile_N, duplicate_tab, close_tab, close_pane,
# next_tab, prev_tab, switch_to_tab_N, move_tab_left, move_tab_right, rename_tab,
# split_right, split_down, split_auto, focus_left/right/up/down, resize_left/right/up/down,
# toggle_pane_zoom, move_pane_to_new_tab, rename_pane, new_group, ungroup, toggle_group_collapse, copy, paste, select_all,
# scroll_up/down, scroll_page_up/down, scroll_top/bottom, clear_scrollback,
# font_bigger, font_smaller, font_reset, opacity_up, opacity_down, toggle_transparency,
# next_color_scheme, toggle_fullscreen, open_settings, reload_config, quit,
# open_location_N (better set with the location's own `key`, see below)
[keybindings]
# "ctrl+shift+t" = "new_tab"
# "alt+shift+equal" = "split_right"

# Custom color schemes:
# [[color_schemes]]
# name = "My Scheme"
# foreground = "#d7dce2"
# background = "#101318"
# cursor = "#7ee787"
# selection = "#2b4434"
# accent = "#7ee787"
# black = "#1b1f27"
# red = "#f0717a"
# green = "#7ee787"
# yellow = "#f2cc60"
# blue = "#6cb6ff"
# magenta = "#d2a8ff"
# cyan = "#56d4dd"
# white = "#c9d1d9"
# bright_black = "#5c6370"
# bright_red = "#ff8f96"
# bright_green = "#a6f3a8"
# bright_yellow = "#ffe08a"
# bright_blue = "#91cbff"
# bright_magenta = "#e2c5ff"
# bright_cyan = "#8de8ee"
# bright_white = "#f5f7fa"

# Locations: folders to open from the Locations menu in the tab bar, on this machine or over SSH.
# Hosts from ~/.ssh/config are listed there automatically.
# [[locations]]
# name = "Web server"
# host = "web1"              # an SSH host; leave out for a local folder
# path = "~/sites/app"
# key = "ctrl+alt+h"         # optional shortcut
# profile = "Bash"           # optional profile name (default profile otherwise)
# tab_color = "#6cb6ff"
"##;

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::SmolStr;

    #[test]
    fn default_config_file_parses() {
        let mut text = config_with_location_example();
        text.push_str("\n[[profiles]]\nname = \"Bash\"\ncommand = \"/bin/bash\"\n");
        let cfg: Config = toml::from_str(&text).expect("default config must parse");
        assert_eq!(cfg.font_family, Config::default().font_family);
        assert_eq!(cfg.profiles.len(), 1);
        assert_eq!(cfg.locations.len(), 1);
    }

    fn shell(name: &str, command: &str) -> Profile {
        Profile {
            name: name.into(),
            command: command.into(),
            args: Vec::new(),
            cwd: None,
            scheme: None,
            tab_color: None,
            env: HashMap::new(),
        }
    }

    #[test]
    fn detected_shells_merge_after_configured() {
        let configured = vec![shell("My Bash", "/bin/bash"), shell("Server", "/usr/bin/ssh")];
        let detected = vec![shell("Bash", "/usr/bin/bash"), shell("Fish", "/usr/bin/fish")];
        let names: Vec<_> = merge_detected(configured, detected)
            .into_iter()
            .map(|p| p.name)
            .collect();
        assert_eq!(names, ["My Bash", "Server", "Fish"]);
    }

    #[test]
    fn every_default_binding_parses() {
        for (combo, action) in DEFAULT_KEYBINDINGS {
            assert!(KeyCombo::parse(combo).is_some(), "{combo}");
            assert!(Action::parse(action).is_some(), "{action}");
        }
    }

    #[test]
    fn combos_match_unmodified_keys() {
        let split = KeyCombo::parse("alt+shift+equal").unwrap();
        let mods = ModifiersState::ALT | ModifiersState::SHIFT;
        assert!(split.matches(mods, &Key::Character(SmolStr::new("="))));
        assert!(!split.matches(ModifiersState::ALT, &Key::Character(SmolStr::new("="))));
        let tab = KeyCombo::parse("Ctrl+Shift+T").unwrap();
        assert!(tab.matches(
            ModifiersState::CONTROL | ModifiersState::SHIFT,
            &Key::Character(SmolStr::new("T"))
        ));
        let next = KeyCombo::parse("ctrl+tab").unwrap();
        assert!(next.matches(ModifiersState::CONTROL, &Key::Named(NamedKey::Tab)));
    }

    #[test]
    fn action_names_round_trip() {
        for (_, a, _) in action_catalog() {
            assert_eq!(Action::parse(&a.name()), Some(a), "{}", a.name());
        }
    }

    #[test]
    fn combo_formats() {
        let k = KeyCombo::parse("alt+shift+equal").unwrap();
        assert_eq!(k.to_config(), "shift+alt+equal");
        assert_eq!(KeyCombo::parse(&k.to_config()), Some(k.clone()));
        assert_eq!(k.display(), "Shift+Alt+=");
        assert_eq!(KeyCombo::parse("ctrl+pageup").unwrap().display(), "Ctrl+PgUp");
    }

    #[test]
    fn overrides_reproduce_bindings() {
        let mut b = Config::default().keybindings();
        b.retain(|(_, a)| *a != Action::NewTab);
        b.push((KeyCombo::parse("ctrl+t").unwrap(), Action::NewTab));
        let mut cfg = Config::default();
        cfg.keybindings = binding_overrides(&b).into_iter().collect();
        let mut got = cfg.keybindings();
        let key = |x: &(KeyCombo, Action)| x.0.to_config();
        got.sort_by_key(key);
        b.sort_by_key(key);
        assert_eq!(got, b);
        assert!(binding_overrides(&Config::default().keybindings()).is_empty());
    }

    #[test]
    fn save_keeps_comments_and_round_trips() {
        let path = std::env::temp_dir().join(format!("terminalgoblin-test-{}.toml", std::process::id()));
        std::fs::write(&path, DEFAULT_CONFIG).unwrap();
        let mut cfg: Config = toml::from_str(DEFAULT_CONFIG).unwrap();
        cfg.font_size = 17.0;
        cfg.color_scheme = "Nord".into();
        let mut b = cfg.keybindings();
        b.push((KeyCombo::parse("ctrl+t").unwrap(), Action::NewTab));
        let text = save_to(&path, &cfg, &b).unwrap();
        std::fs::remove_file(&path).ok();
        assert!(
            text.contains("font_size = 17.0          # logical pixels"),
            "trailing comments survive"
        );
        assert!(text.contains("# Built-in schemes"), "standalone comments survive");
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.font_size, 17.0);
        assert_eq!(back.color_scheme, "Nord");
        assert!(
            back.keybindings()
                .contains(&(KeyCombo::parse("ctrl+t").unwrap(), Action::NewTab))
        );
    }

    /// DEFAULT_CONFIG with its `[[locations]]` example uncommented.
    fn config_with_location_example() -> String {
        let mut in_example = false;
        DEFAULT_CONFIG
            .lines()
            .map(|l| {
                if l == "# [[locations]]" {
                    in_example = true;
                }
                if in_example && let Some(rest) = l.strip_prefix("# ") {
                    return rest.to_string();
                }
                l.to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn location_example_parses() {
        let cfg: Config = toml::from_str(&config_with_location_example()).unwrap();
        assert_eq!(
            cfg.locations,
            [LocationDef {
                name: "Web server".into(),
                host: Some("web1".into()),
                path: "~/sites/app".into(),
                key: Some("ctrl+alt+h".into()),
                profile: Some("Bash".into()),
                tab_color: Some("#6cb6ff".into()),
            }]
        );
    }

    #[test]
    fn save_round_trips_locations() {
        let path = std::env::temp_dir().join(format!("terminalgoblin-loc-test-{}.toml", std::process::id()));
        std::fs::write(&path, DEFAULT_CONFIG).unwrap();
        let mut cfg: Config = toml::from_str(DEFAULT_CONFIG).unwrap();
        cfg.locations = vec![
            LocationDef {
                name: "Home".into(),
                path: "~/src".into(),
                profile: Some("Fish".into()),
                tab_color: Some("#7ee787".into()),
                ..Default::default()
            },
            LocationDef {
                name: "Web".into(),
                host: Some("web1".into()),
                path: "/var/www".into(),
                ..Default::default()
            },
        ];
        let mut b = cfg.keybindings();
        b.push((KeyCombo::parse("ctrl+alt+w").unwrap(), Action::OpenLocation(1)));
        let text = save_to(&path, &cfg, &b).unwrap();
        std::fs::remove_file(&path).ok();
        let back: Config = toml::from_str(&text).unwrap();
        assert!(
            !back.keybindings.values().any(|a| a.starts_with("open_location")),
            "location keys stay out of [keybindings]"
        );
        assert_eq!(back.locations[0], cfg.locations[0]);
        assert_eq!(back.locations[1].key.as_deref(), Some("ctrl+alt+w"));
        assert_eq!(back.locations[1].host.as_deref(), Some("web1"));
        assert!(
            back.keybindings()
                .contains(&(KeyCombo::parse("ctrl+alt+w").unwrap(), Action::OpenLocation(1)))
        );
    }

    #[test]
    fn location_keys_bind_and_win() {
        let cfg = Config {
            locations: vec![
                LocationDef {
                    name: "a".into(),
                    path: "/".into(),
                    key: Some("ctrl+shift+t".into()),
                    ..Default::default()
                },
                LocationDef {
                    name: "b".into(),
                    path: "/".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let b = cfg.keybindings();
        let t = KeyCombo::parse("ctrl+shift+t").unwrap();
        assert_eq!(
            b.iter().filter(|(k, _)| *k == t).map(|(_, a)| *a).collect::<Vec<_>>(),
            [Action::OpenLocation(0)]
        );
        assert!(!b.iter().any(|(_, a)| *a == Action::OpenLocation(1)));
        // The default it displaced isn't written as an override, so it comes back if the key goes.
        assert!(binding_overrides(&b).is_empty());
    }

    #[test]
    fn open_location_names_round_trip() {
        let a = Action::OpenLocation(2);
        assert_eq!(a.name(), "open_location_3");
        assert_eq!(Action::parse("open_location_3"), Some(a));
        assert_eq!(Action::parse("open_location_0"), None);
    }

    #[test]
    fn ssh_launch_quotes_paths() {
        let args = |p: Option<&str>| ssh_launch("web1", p).1;
        assert_eq!(args(None), ["-t", "--", "web1"]);
        assert_eq!(args(Some("")), ["-t", "--", "web1"]);
        assert_eq!(args(Some("/srv/my site"))[3], "cd '/srv/my site' && exec $SHELL -l");
        assert_eq!(args(Some("/tmp/it's"))[3], "cd '/tmp/it'\\''s' && exec $SHELL -l");
        assert_eq!(args(Some("~/a b"))[3], "cd ~/'a b' && exec $SHELL -l");
        assert_eq!(args(Some("~"))[3], "cd ~ && exec $SHELL -l");
        assert_eq!(ssh_launch("web1", None).0, "ssh");
    }

    #[test]
    fn ssh_config_hosts() {
        let text = "Host *\n  User me\nHost web1 web2 # comment\n  HostName 10.0.0.1\n\
                    host=db\nHost !bastion jump\nHost web1\nMatch host x\nHost dev?\n";
        assert_eq!(parse_ssh_hosts(text), ["web1", "web2", "db", "jump"]);
    }

    #[test]
    fn ssh_host_validation() {
        assert!(valid_ssh_host("user@web1"));
        assert!(!valid_ssh_host("-oProxyCommand=touch /tmp/x"));
        assert!(!valid_ssh_host(""));
        assert!(!valid_ssh_host("a b"));
    }

    #[test]
    fn user_bindings_override_and_unbind() {
        let mut cfg = Config::default();
        cfg.keybindings.insert("ctrl+shift+t".into(), "unbound".into());
        cfg.keybindings.insert("ctrl+t".into(), "new_tab".into());
        let b = cfg.keybindings();
        assert!(!b.iter().any(|(k, _)| *k == KeyCombo::parse("ctrl+shift+t").unwrap()));
        assert!(
            b.iter()
                .any(|(k, a)| *k == KeyCombo::parse("ctrl+t").unwrap() && *a == Action::NewTab)
        );
    }
}
