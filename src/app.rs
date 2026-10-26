//! Application state and event handling.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use alacritty_terminal::event::Event as TermEvent;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::{ClipboardType, TermMode};
use alacritty_terminal::vte::ansi::Rgb;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, Ime, MouseButton, MouseScrollDelta, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
#[cfg(not(windows))]
use winit::platform::wayland::WindowAttributesExtWayland;
use winit::window::{CursorIcon, Fullscreen, ResizeDirection, UserAttentionType, Window, WindowId};

use crate::clipboard::Clipboard;
use crate::config::{self, Action, Config, KeyCombo, Profile};
use crate::input::{self, MouseButtonCode};
use crate::layout::{self, Axis, Dir, Divider, Node};
use crate::pane::{GridSize, Pane, PaneId, PaneLaunch, UserEvent};
use crate::renderer::{Instance, Rect, Renderer};
use crate::text::TextSystem;
use crate::theme::{self, Scheme, UiPalette};
use crate::ui::{self, Cmd, EditTarget, Hit, InlineEdit, Menu, MenuEntry, MenuHit, SwatchTarget, TabBarLayout};
use crate::workspace::{Tab, TabId, Workspace};

const BLINK: Duration = Duration::from_millis(530);
const BLINK_IDLE_STOP: Duration = Duration::from_secs(30);
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

#[derive(Default)]
pub struct Options {
    pub screenshot: Option<PathBuf>,
    pub demo: bool,
    pub demo_menu: bool,
    pub demo_dock: bool,
    pub demo_settings: Option<String>,
    /// First tab to open instead of the default shell (from the command line).
    pub launch: Option<crate::ipc::LaunchSpec>,
}

pub struct App {
    proxy: EventLoopProxy<UserEvent>,
    cfg: Config,
    opts: Options,
    startup_error: Option<String>,
    st: Option<State>,
}

impl App {
    pub fn new(proxy: EventLoopProxy<UserEvent>, opts: Options) -> App {
        let (cfg, startup_error) = Config::load();
        crate::desktop::sync_actions(&cfg.locations);
        App {
            proxy,
            cfg,
            opts,
            startup_error,
            st: None,
        }
    }
}

pub enum Drag {
    None,
    Tab {
        id: TabId,
        start: (f32, f32),
        moved: bool,
        /// The tab that was active before this one was grabbed: dragging down onto the content
        /// shows it again so the dragged tab can be docked into one of its panes.
        prev_active: Option<TabId>,
        dock: Option<(PaneId, Dir)>,
    },
    Divider(Divider),
    Selection(PaneId),
}

pub struct State {
    pub window: Arc<Window>,
    pub renderer: Renderer,
    pub text: TextSystem,
    pub ws: Workspace,
    pub cfg: Config,
    pub profiles: Vec<Profile>,
    pub keybindings: Vec<(KeyCombo, Action)>,
    pub default_bindings: Vec<(KeyCombo, Action)>,
    pub settings: Option<crate::settings::Settings>,
    /// Text of the config file as we last wrote it, so our own saves aren't "reloaded".
    pub last_saved: Option<String>,
    pub schemes: Vec<Scheme>,
    pub scheme: Scheme,
    pub ui: UiPalette,
    pub(crate) proxy: EventLoopProxy<UserEvent>,
    pub(crate) clipboard: Option<Clipboard>,
    pub scale: f32,
    pub size: PhysicalSize<u32>,
    pub mods: ModifiersState,
    pub mouse: (f32, f32),
    pub focused: bool,
    pub font_size: f32,
    pub opacity: f32,
    pub transparency_on: bool,
    pub menu: Option<Menu>,
    pub edit: Option<InlineEdit>,
    pub drag: Drag,
    last_click: Option<(Instant, f32, f32, u32)>,
    pub hover: Option<Hit>,
    pub last_input: Instant,
    pub toast: Option<(String, Instant)>,
    pub pane_rects: Vec<(PaneId, Rect)>,
    pub dividers: Vec<Divider>,
    /// Whether the active tab's panes show name/color headers.
    pub headers: bool,
    pub instances: Vec<Instance>,
    cursor_icon: CursorIcon,
    /// Last cell reported to a program tracking mouse motion.
    last_report_cell: Option<(PaneId, usize, usize)>,
    reported_button: Option<MouseButtonCode>,
    pub screenshot: Option<(PathBuf, Instant)>,
    /// When the window last gained focus (seconds since the epoch), so `@ launch` picks the
    /// most recently used window.
    pub focused_at: f64,
    pub exit: bool,
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.st.is_some() {
            return;
        }
        if let Err(e) = crate::ipc::serve(self.proxy.clone()) {
            log::warn!("remote control unavailable: {e}");
        }
        match State::new(
            event_loop,
            self.cfg.clone(),
            self.proxy.clone(),
            self.opts.launch.take(),
        ) {
            Ok(mut st) => {
                if let Some(e) = self.startup_error.take() {
                    st.show_toast(e);
                }
                if let Some(path) = self.opts.screenshot.take() {
                    st.screenshot = Some((path, Instant::now() + Duration::from_millis(2500)));
                }
                if self.opts.demo {
                    st.demo(self.opts.demo_menu, self.opts.demo_dock);
                }
                if let Some(page) = self.opts.demo_settings.take() {
                    st.demo_settings(&page);
                }
                spawn_config_watcher(self.proxy.clone());
                self.st = Some(st);
            }
            Err(e) => {
                log::error!("failed to start: {e:#}");
                eprintln!("terminalgoblin: {e:#}");
                event_loop.exit();
            }
        }
    }

    fn new_events(&mut self, _event_loop: &ActiveEventLoop, cause: StartCause) {
        if let (StartCause::ResumeTimeReached { .. }, Some(st)) = (cause, &self.st) {
            st.window.request_redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(st) = self.st.as_mut() else { return };
        st.handle_window_event(event);
        if st.exit {
            event_loop.exit();
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        let Some(st) = self.st.as_mut() else { return };
        match event {
            UserEvent::Term(id, ev) => st.handle_term_event(id, ev),
            UserEvent::ConfigChanged => st.reload_config(),
            UserEvent::Ipc(req) => {
                let reply = st.handle_ipc(req.body);
                let _ = req.reply.send(reply);
            }
            UserEvent::RemoteDir { req, result } => st.remote_dir_listed(req, result),
        }
        if st.exit {
            event_loop.exit();
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        crate::ipc::cleanup();
        // Tear down while the Wayland connection is still alive: the clipboard and GPU surface
        // hold raw pointers into it.
        if let Some(mut st) = self.st.take() {
            st.ws.tabs.clear();
            st.ws.panes.clear();
            drop(st.clipboard.take());
            drop(st);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(st) = self.st.as_ref() else { return };
        let mut next: Option<Instant> = st.screenshot.as_ref().map(|(_, at)| *at);
        let now = Instant::now();
        if st.blinking() {
            let since = now - st.last_input;
            let phases = since.as_millis() / BLINK.as_millis() + 1;
            next = Some(st.last_input + BLINK * phases as u32);
        }
        if let Some((_, at)) = &st.toast {
            // Wake once when the fade-out starts; the fade then animates itself.
            let fade_start = *at + Duration::from_millis(1800);
            let end = if now < fade_start {
                fade_start
            } else {
                *at + Duration::from_millis(2200)
            };
            next = Some(next.map_or(end, |n| n.min(end)));
        }
        event_loop.set_control_flow(match next {
            Some(t) => ControlFlow::WaitUntil(t),
            None => ControlFlow::Wait,
        });
    }
}

fn spawn_config_watcher(proxy: EventLoopProxy<UserEvent>) {
    std::thread::Builder::new()
        .name("config watcher".into())
        .spawn(move || {
            let path = config::config_path();
            let stamp = |p: &PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok();
            let mut last = stamp(&path);
            loop {
                std::thread::sleep(Duration::from_millis(700));
                let now = stamp(&path);
                if now != last {
                    last = now;
                    if proxy.send_event(UserEvent::ConfigChanged).is_err() {
                        return;
                    }
                }
            }
        })
        .ok();
}

impl State {
    pub(crate) fn new(
        event_loop: &ActiveEventLoop,
        cfg: Config,
        proxy: EventLoopProxy<UserEvent>,
        initial: Option<crate::ipc::LaunchSpec>,
    ) -> anyhow::Result<State> {
        let attrs = Window::default_attributes()
            .with_title("TerminalGoblin")
            .with_transparent(true)
            .with_blur(cfg.blur)
            .with_decorations(!cfg.client_decorations())
            .with_inner_size(LogicalSize::new(1060.0, 680.0))
            .with_min_inner_size(LogicalSize::new(380.0, 220.0));
        #[cfg(not(windows))]
        let attrs = attrs.with_name("terminalgoblin", "terminalgoblin");
        let window = Arc::new(event_loop.create_window(attrs)?);
        window.set_ime_allowed(true);
        let renderer = Renderer::new(window.clone())?;
        let scale = window.scale_factor() as f32;
        let text = TextSystem::new(&cfg.font_family, &cfg.ui_font_family, cfg.line_height);

        let clipboard = Clipboard::new(&window);

        let schemes = cfg.schemes();
        let scheme = cfg.scheme(None);
        let ui = UiPalette::from_scheme(&scheme);
        let mut st = State {
            size: window.inner_size(),
            window,
            renderer,
            text,
            ws: Workspace::default(),
            profiles: cfg.profiles(),
            keybindings: cfg.keybindings(),
            default_bindings: Config::default().keybindings(),
            settings: None,
            last_saved: None,
            schemes,
            scheme,
            ui,
            proxy,
            clipboard,
            scale,
            mods: ModifiersState::empty(),
            mouse: (0.0, 0.0),
            focused: true,
            font_size: cfg.font_size,
            opacity: cfg.opacity,
            transparency_on: true,
            menu: None,
            edit: None,
            drag: Drag::None,
            last_click: None,
            hover: None,
            last_input: Instant::now(),
            toast: None,
            pane_rects: Vec::new(),
            dividers: Vec::new(),
            headers: false,
            instances: Vec::new(),
            cursor_icon: CursorIcon::Default,
            last_report_cell: None,
            reported_button: None,
            screenshot: None,
            focused_at: unix_now(),
            exit: false,
            cfg,
        };
        if !st.renderer.supports_transparency {
            st.transparency_on = false;
        }
        match initial.map(|spec| st.launch(spec)) {
            Some(Ok(_)) => {}
            other => {
                if let Some(Err(e)) = other {
                    st.show_toast(format!("Could not open the requested tab: {e}"));
                }
                let profile = st.cfg.default_profile_index(&st.profiles);
                st.new_tab(profile, None)?;
            }
        }
        st.window.request_redraw();
        Ok(st)
    }

    /// Sample layout for `--demo`: tabs, colors, a group and splits.
    pub(crate) fn demo(&mut self, with_menu: bool, with_dock: bool) {
        let p = self.cfg.default_profile_index(&self.profiles);
        let first = self.ws.tabs[0].id;
        self.run_cmd(Cmd::SetTabColor(first, Some(theme::c("#7ee787"))));
        if let Some(t) = self.ws.tabs.get_mut(0) {
            t.custom_title = Some("goblin".into());
        }
        self.split(Some(Axis::Horizontal));
        self.split(Some(Axis::Vertical));
        let demo_cmd = concat!(
            "clear; printf '\\e[1;32m╭─ TerminalGoblin ─────────────╮\\e[0m\\n",
            "\\e[1;32m│\\e[0m \\e[31mred \\e[32mgreen \\e[33myellow \\e[34mblue \\e[35mmagenta \\e[36mcyan \\e[1;32m│\\e[0m\\n",
            "\\e[1;32m╰──────────────────────────────╯\\e[0m\\n\\e[1mbold\\e[0m \\e[3mitalic\\e[0m \\e[4munderline\\e[0m \\e[7m inverse \\e[0m ▁▂▃▄▅▆▇█ ✻ ⏺ 日本語\\n'\r",
        );
        let panes = self.ws.tabs[0].root.panes();
        if let Some(pane) = panes.first().and_then(|id| self.ws.panes.get(id)) {
            let cmd = std::env::var("TG_DEMO_CMD")
                .map(|c| c + "\r")
                .unwrap_or_else(|_| demo_cmd.to_string());
            pane.write(cmd.into_bytes());
        }
        if let Some(pane) = panes.get(1).and_then(|id| self.ws.panes.get(id)) {
            pane.write(b"ls -la --color=always /etc | head -30\r".to_vec());
        }
        self.new_tab_or_toast(p, None);
        self.ws.tabs[1].custom_title = Some("build".into());
        self.ws.tabs[1].color = Some(theme::c("#f5a35c"));
        self.new_tab_or_toast(p, None);
        self.ws.tabs[2].custom_title = Some("api server".into());
        self.group_tab(2);
        self.edit = None;
        if let Some(g) = self.ws.groups.first_mut() {
            g.name = "Servers".into();
        }
        self.new_tab_or_toast(p, None);
        self.ws.tabs[3].custom_title = Some("db".into());
        self.new_tab_or_toast(p, None);
        self.activate_tab(0);
        // Show the "build" tab inside the first one, and name a pane.
        let build = self.ws.tabs[1].id;
        self.run_cmd(Cmd::JoinTab(build, Dir::Down));
        let first_pane = self.ws.tabs[0].root.panes()[0];
        if let Some(p) = self.ws.panes.get_mut(&first_pane) {
            p.label = Some("editor".into());
            p.font_size = Some(19.0);
        }
        if with_dock {
            self.relayout();
            let db = self
                .ws
                .tabs
                .iter()
                .find(|t| t.custom_title.as_deref() == Some("db"))
                .map(|t| t.id);
            let target = self.pane_rects.get(1).copied();
            if let (Some(db), Some((pane, r))) = (db, target) {
                self.mouse = (r.right() - 60.0, r.center_y());
                self.drag = Drag::Tab {
                    id: db,
                    start: (0.0, 0.0),
                    moved: true,
                    prev_active: None,
                    dock: Some((pane, Dir::Right)),
                };
            }
        }
        if with_menu {
            self.relayout();
            let bar = self.tabbar();
            if let Some(r) = bar.tab_rect(1) {
                self.mouse = (r.x + 40.0, r.center_y());
                self.open_tab_menu(1, r.x + 40.0, r.bottom());
            }
        }
    }

    // ---------------------------------------------------------------------------------------------
    // Geometry

    pub fn s(&self) -> f32 {
        self.scale
    }

    pub fn csd(&self) -> bool {
        self.cfg.client_decorations()
    }

    pub fn is_maximized(&self) -> bool {
        self.window.is_maximized() || self.window.fullscreen().is_some()
    }

    pub fn tabbar(&mut self) -> TabBarLayout {
        let w = self.size.width as f32;
        let s = self.scale;
        let controls = self.csd();
        ui::layout_tabbar(&self.ws, &mut self.text, w, s, controls, self.edit.as_ref())
    }

    pub fn content_rect(&self) -> Rect {
        let top = (ui::BAR_H * self.scale).round();
        Rect::new(
            0.0,
            top,
            self.size.width as f32,
            (self.size.height as f32 - top).max(1.0),
        )
    }

    pub fn pane_gap(&self) -> f32 {
        (2.0 * self.scale).round().max(1.0)
    }

    pub fn padding(&self) -> f32 {
        (self.cfg.padding * self.scale).round()
    }

    pub(crate) fn grid_for(&self, rect: Rect, cell: (f32, f32)) -> GridSize {
        let pad = self.padding();
        let cols = ((rect.w - 2.0 * pad) / cell.0).floor().max(2.0) as usize;
        let rows = ((rect.h - 2.0 * pad) / cell.1).floor().max(1.0) as usize;
        GridSize { cols, rows }
    }

    /// A pane's terminal font size in physical pixels.
    pub fn pane_font_px(&self, id: PaneId) -> f32 {
        let logical = self
            .ws
            .panes
            .get(&id)
            .and_then(|p| p.font_size)
            .unwrap_or(self.font_size);
        logical * self.scale
    }

    pub fn pane_cell(&self, id: PaneId) -> (f32, f32) {
        self.text.cell(self.pane_font_px(id))
    }

    pub(crate) fn default_cell(&self) -> (f32, f32) {
        self.text.cell(self.font_size * self.scale)
    }

    pub fn tab_shows_headers(&self, tab: &Tab) -> bool {
        if tab.zoomed || matches!(tab.root, Node::Leaf(_)) {
            return false;
        }
        match self.cfg.pane_headers.as_str() {
            "never" | "off" | "none" => false,
            "always" | "on" => true,
            _ => tab.root.panes().iter().any(|id| {
                self.ws
                    .panes
                    .get(id)
                    .is_some_and(|p| p.label.is_some() || p.color.is_some())
            }),
        }
    }

    pub fn header_height(&self) -> f32 {
        (ui::PANE_HEADER * self.scale).round()
    }

    pub(crate) fn body_of(&self, rect: Rect, headers: bool) -> Rect {
        if !headers {
            return rect;
        }
        let h = self.header_height();
        Rect::new(rect.x, rect.y + h, rect.w, (rect.h - h).max(1.0))
    }

    /// The terminal area of a pane in the active tab (below its header, if any).
    pub fn pane_body(&self, rect: Rect) -> Rect {
        self.body_of(rect, self.headers)
    }

    pub fn pane_header(&self, rect: Rect) -> Option<Rect> {
        self.headers
            .then(|| Rect::new(rect.x, rect.y, rect.w, self.header_height()))
    }

    pub(crate) fn header_at(&self, x: f32, y: f32) -> Option<(PaneId, Rect)> {
        let (id, rect) = self.pane_at(x, y)?;
        self.pane_header(rect).filter(|h| h.contains(x, y)).map(|h| (id, h))
    }

    /// Compute pane rectangles for every tab and resize terminals to match.
    pub fn relayout(&mut self) {
        let area = self.content_rect();
        let gap = self.pane_gap();
        let mut active_rects = Vec::new();
        let mut active_dividers = Vec::new();
        let mut active_headers = false;
        let mut resizes = Vec::new();
        for (i, tab) in self.ws.tabs.iter().enumerate() {
            let headers = self.tab_shows_headers(tab);
            let mut rects = Vec::new();
            let mut dividers = Vec::new();
            if tab.zoomed {
                rects.push((tab.focused, area));
            } else {
                tab.root.layout(area, gap, &mut rects, &mut dividers);
            }
            for (id, r) in &rects {
                let cell = self.pane_cell(*id);
                resizes.push((*id, self.grid_for(self.body_of(*r, headers), cell), cell));
            }
            if i == self.ws.active {
                active_rects = rects;
                active_dividers = dividers;
                active_headers = headers;
            }
        }
        for (id, grid, cell) in resizes {
            if let Some(p) = self.ws.panes.get_mut(&id) {
                p.resize(grid, cell);
            }
        }
        self.pane_rects = active_rects;
        self.dividers = active_dividers;
        self.headers = active_headers;
    }

    pub(crate) fn pane_at(&self, x: f32, y: f32) -> Option<(PaneId, Rect)> {
        self.pane_rects.iter().find(|(_, r)| r.contains(x, y)).copied()
    }

    /// Grid point under (x, y) within a pane, clamped to its visible area.
    pub(crate) fn grid_point(&self, pane: PaneId, rect: Rect, x: f32, y: f32) -> Option<(Point, Side, usize, usize)> {
        let p = self.ws.panes.get(&pane)?;
        let rect = self.pane_body(rect);
        let pad = self.padding();
        let (cw, ch) = self.pane_cell(pane);
        let fx = ((x - rect.x - pad) / cw).max(0.0);
        let fy = ((y - rect.y - pad) / ch).max(0.0);
        let col = (fx as usize).min(p.grid.cols - 1);
        let row = (fy as usize).min(p.grid.rows - 1);
        let side = if fx.fract() < 0.5 { Side::Left } else { Side::Right };
        let offset = p.term.lock().grid().display_offset() as i32;
        Some((Point::new(Line(row as i32 - offset), Column(col)), side, col, row))
    }

    pub(crate) fn edge_direction(&self, x: f32, y: f32) -> Option<ResizeDirection> {
        if !self.csd() || self.is_maximized() {
            return None;
        }
        let b = ui::RESIZE_BORDER * self.scale;
        let (w, h) = (self.size.width as f32, self.size.height as f32);
        let (l, r, t, btm) = (x < b, x > w - b, y < b, y > h - b);
        Some(match (l, r, t, btm) {
            (true, _, true, _) => ResizeDirection::NorthWest,
            (_, true, true, _) => ResizeDirection::NorthEast,
            (true, _, _, true) => ResizeDirection::SouthWest,
            (_, true, _, true) => ResizeDirection::SouthEast,
            (true, ..) => ResizeDirection::West,
            (_, true, ..) => ResizeDirection::East,
            (_, _, true, _) => ResizeDirection::North,
            (_, _, _, true) => ResizeDirection::South,
            _ => return None,
        })
    }

    // ---------------------------------------------------------------------------------------------
    // Tabs and panes

    pub(crate) fn spawn_pane(
        &mut self,
        profile: usize,
        cwd: Option<PathBuf>,
        approx: Rect,
        font_size: Option<f32>,
        launch: PaneLaunch,
    ) -> anyhow::Result<PaneId> {
        let id = self.ws.next_id();
        let profile_def = self
            .profiles
            .get(profile)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no such profile"))?;
        let cell = self.text.cell(font_size.unwrap_or(self.font_size) * self.scale);
        let grid = self.grid_for(approx, cell);
        let mut pane = Pane::spawn(
            id,
            &self.cfg,
            profile,
            &profile_def,
            cwd,
            launch,
            grid,
            cell,
            self.proxy.clone(),
        )?;
        pane.font_size = font_size;
        self.ws.panes.insert(id, pane);
        Ok(id)
    }

    pub fn new_tab(&mut self, profile: usize, cwd: Option<PathBuf>) -> anyhow::Result<()> {
        self.open_tab_with(profile, cwd, PaneLaunch::default(), None, None)
            .map(|_| ())
    }

    /// Open a tab after the active one and switch to it. The color defaults to the profile's.
    pub(crate) fn open_tab_with(
        &mut self,
        profile: usize,
        cwd: Option<PathBuf>,
        launch: PaneLaunch,
        title: Option<String>,
        color: Option<theme::Color>,
    ) -> anyhow::Result<(TabId, PaneId)> {
        let area = self.content_rect();
        let pane = self.spawn_pane(profile, cwd, area, None, launch)?;
        let tab_id = self.ws.next_id();
        let color = color.or_else(|| self.profiles.get(profile).and_then(|p| p.tab_color));
        self.ws.insert_tab(Tab {
            id: tab_id,
            root: Node::Leaf(pane),
            focused: pane,
            custom_title: title,
            color,
            group: None,
            zoomed: false,
            bell: false,
        });
        self.after_structure_change();
        Ok((tab_id, pane))
    }

    /// Open saved location `i` in a new tab: a local folder, or an ssh session in a remote one.
    pub(crate) fn open_location(&mut self, i: usize) {
        if let Err(e) = self.try_open_location(i) {
            self.show_toast(e);
        }
    }

    pub(crate) fn try_open_location(&mut self, i: usize) -> Result<(TabId, PaneId), String> {
        let loc = self.cfg.locations.get(i).cloned().ok_or("no such location")?;
        let mut profile = self.cfg.default_profile_index(&self.profiles);
        if let Some(name) = loc.profile.as_deref().filter(|n| !n.trim().is_empty()) {
            match self.profiles.iter().position(|p| p.name.eq_ignore_ascii_case(name)) {
                Some(p) => profile = p,
                None => self.show_toast(format!("Profile \"{name}\" not found; using the default")),
            }
        }
        let color = loc.tab_color.as_deref().and_then(theme::hex);
        let title = Some(loc.name.clone()).filter(|n| !n.trim().is_empty());
        let result = match loc.remote_host() {
            Some(host) => {
                if !config::valid_ssh_host(host) {
                    return Err(format!("Invalid SSH host \"{host}\""));
                }
                let launch = PaneLaunch {
                    command: Some(config::ssh_launch(host, Some(&loc.path))),
                    ..Default::default()
                };
                self.open_tab_with(profile, None, launch, title, color)
            }
            None => {
                let dir = config::expand_tilde(loc.path.trim());
                if !dir.is_dir() {
                    return Err(format!("Folder not found: {}", dir.display()));
                }
                self.open_tab_with(profile, Some(dir), PaneLaunch::default(), title, color)
            }
        };
        result.map_err(|e| format!("Could not open {}: {e}", loc.name))
    }

    /// Open a new tab with `ssh host`, in the remote home folder.
    pub(crate) fn open_ssh_host(&mut self, host: &str) {
        if !config::valid_ssh_host(host) {
            self.show_toast(format!("Invalid SSH host \"{host}\""));
            return;
        }
        let profile = self.cfg.default_profile_index(&self.profiles);
        let launch = PaneLaunch {
            command: Some(config::ssh_launch(host, None)),
            ..Default::default()
        };
        if let Err(e) = self.open_tab_with(profile, None, launch, Some(host.to_string()), None) {
            self.show_toast(format!("Could not open {host}: {e}"));
        }
    }

    pub(crate) fn new_tab_or_toast(&mut self, profile: usize, cwd: Option<PathBuf>) {
        if let Err(e) = self.new_tab(profile, cwd) {
            let name = self
                .profiles
                .get(profile)
                .map(|p| p.command.clone())
                .unwrap_or_default();
            self.show_toast(format!("Could not start {name}: {e}"));
        }
    }

    pub(crate) fn split(&mut self, axis: Option<Axis>) {
        let Some(tab) = self.ws.active_tab() else {
            return;
        };
        let focused = tab.focused;
        let rect = self
            .pane_rects
            .iter()
            .find(|(id, _)| *id == focused)
            .map(|(_, r)| *r)
            .unwrap_or(self.content_rect());
        let axis = axis.unwrap_or(if rect.w >= rect.h * 1.6 {
            Axis::Horizontal
        } else {
            Axis::Vertical
        });
        // A split runs what the focused pane runs: the same shell, or the same ssh session.
        let (profile, cwd, font_size, launch) = match self.ws.panes.get(&focused) {
            Some(p) => (p.profile, p.cwd(), p.font_size, p.launch.clone()),
            None => (
                self.cfg.default_profile_index(&self.profiles),
                None,
                None,
                PaneLaunch::default(),
            ),
        };
        let half = match axis {
            Axis::Horizontal => Rect::new(rect.x, rect.y, rect.w / 2.0, rect.h),
            Axis::Vertical => Rect::new(rect.x, rect.y, rect.w, rect.h / 2.0),
        };
        match self.spawn_pane(profile, cwd, half, font_size, launch) {
            Ok(new) => {
                let tab = self.ws.active_tab_mut().unwrap();
                tab.zoomed = false;
                tab.root.split(focused, new, axis);
                tab.focused = new;
                self.after_structure_change();
            }
            Err(e) => self.show_toast(format!("Could not split: {e}")),
        }
    }

    /// Run a pane's command again in the same place, keeping its name, color and text size.
    pub(crate) fn restart_pane(&mut self, id: PaneId) {
        let Some(old) = self.ws.panes.get(&id) else { return };
        let (profile, cwd, font_size, launch) = (old.profile, old.cwd(), old.font_size, old.launch.clone());
        let (label, color) = (old.label.clone(), old.color);
        let rect = self
            .pane_rects
            .iter()
            .find(|(p, _)| *p == id)
            .map(|(_, r)| *r)
            .unwrap_or(self.content_rect());
        match self.spawn_pane(profile, cwd, rect, font_size, launch) {
            Ok(new) => {
                if let Some(p) = self.ws.panes.get_mut(&new) {
                    p.label = label;
                    p.color = color;
                }
                if let Some(ti) = self.ws.tab_of_pane(id) {
                    let tab = &mut self.ws.tabs[ti];
                    tab.root.replace(id, new);
                    if tab.focused == id {
                        tab.focused = new;
                    }
                }
                self.ws.panes.remove(&id);
                self.after_structure_change();
            }
            Err(e) => self.show_toast(format!("Could not restart: {e}")),
        }
    }

    pub fn close_pane(&mut self, id: PaneId) {
        let Some(ti) = self.ws.tab_of_pane(id) else {
            self.ws.panes.remove(&id);
            return;
        };
        let tab = &mut self.ws.tabs[ti];
        if matches!(tab.root, Node::Leaf(p) if p == id) {
            self.close_tab_index(ti);
            return;
        }
        tab.root.remove(id);
        tab.zoomed = false;
        if tab.focused == id {
            // Focus the pane that took the closed pane's place.
            tab.focused = tab.root.panes().into_iter().next().unwrap_or(0);
        }
        self.ws.panes.remove(&id);
        self.after_structure_change();
    }

    pub(crate) fn close_tab_index(&mut self, index: usize) {
        drop(self.ws.remove_tab(index));
        if self.ws.tabs.is_empty() {
            self.exit = true;
            return;
        }
        if let Some(EditTarget::Tab(t)) = self.edit.as_ref().map(|e| e.target)
            && self.ws.tab_index(t).is_none()
        {
            self.edit = None;
        }
        self.after_structure_change();
    }

    pub(crate) fn after_structure_change(&mut self) {
        self.relayout();
        self.update_window_title();
        self.last_input = Instant::now();
        self.window.request_redraw();
    }

    pub(crate) fn update_window_title(&self) {
        if let Some(tab) = self.ws.active_tab() {
            let title = self.ws.tab_title(tab);
            self.window
                .set_title(if title.is_empty() { "TerminalGoblin" } else { &title });
        }
    }

    pub(crate) fn focus_pane(&mut self, id: PaneId) {
        let Some(ti) = self.ws.tab_of_pane(id) else {
            return;
        };
        let old = self.ws.tabs[ti].focused;
        if old == id {
            return;
        }
        self.send_focus(old, false);
        self.ws.tabs[ti].focused = id;
        self.send_focus(id, true);
        self.update_window_title();
        self.last_input = Instant::now();
        self.window.request_redraw();
    }

    pub(crate) fn send_focus(&self, id: PaneId, on: bool) {
        if let Some(p) = self.ws.panes.get(&id)
            && p.term.lock().mode().contains(TermMode::FOCUS_IN_OUT)
        {
            p.write(if on { &b"\x1b[I"[..] } else { &b"\x1b[O"[..] });
        }
    }

    pub(crate) fn activate_tab(&mut self, index: usize) {
        if index >= self.ws.tabs.len() {
            return;
        }
        let old = self.ws.active_tab().map(|t| t.focused);
        self.ws.activate(index);
        let new = self.ws.active_tab().map(|t| t.focused);
        if old != new {
            if let Some(o) = old {
                self.send_focus(o, false);
            }
            if let Some(n) = new {
                self.send_focus(n, true);
            }
        }
        self.after_structure_change();
    }

    pub(crate) fn tab_index_by_id(&self, id: TabId) -> Option<usize> {
        self.ws.tab_index(id)
    }

    pub(crate) fn start_rename_tab(&mut self, index: usize) {
        let Some(tab) = self.ws.tabs.get(index) else {
            return;
        };
        let current = self.ws.tab_title(tab);
        self.edit = Some(InlineEdit::new(EditTarget::Tab(tab.id), current));
        self.menu = None;
        self.window.request_redraw();
    }

    pub(crate) fn commit_edit(&mut self) {
        let Some(edit) = self.edit.take() else { return };
        let text = edit.text.trim().to_string();
        match edit.target {
            EditTarget::Tab(id) => {
                if let Some(i) = self.ws.tab_index(id) {
                    self.ws.tabs[i].custom_title = (!text.is_empty()).then_some(text);
                }
            }
            EditTarget::Group(id) => {
                if let Some(g) = self.ws.group_mut(id) {
                    g.name = text;
                }
            }
            EditTarget::Pane(id) => {
                if let Some(p) = self.ws.panes.get_mut(&id) {
                    p.label = (!text.is_empty()).then_some(text);
                }
            }
            EditTarget::Field => {}
        }
        self.update_window_title();
        self.window.request_redraw();
    }

    pub(crate) fn next_group_color(&self) -> theme::Color {
        let used: Vec<theme::Color> = self.ws.groups.iter().map(|g| g.color).collect();
        theme::TAB_COLORS
            .iter()
            .map(|(_, h)| theme::c(h))
            .find(|c| !used.contains(c))
            .unwrap_or_else(|| theme::c(theme::TAB_COLORS[self.ws.groups.len() % theme::TAB_COLORS.len()].1))
    }

    pub(crate) fn group_tab(&mut self, index: usize) {
        let color = self.next_group_color();
        let n = self.ws.groups.len() + 1;
        let gid = self.ws.new_group(index, format!("Group {n}"), color);
        // Name it right away, like a browser does.
        let label = self.ws.group(gid).map(|g| g.name.clone()).unwrap_or_default();
        self.edit = Some(InlineEdit::new(EditTarget::Group(gid), label));
        self.after_structure_change();
    }

    // ---------------------------------------------------------------------------------------------
    // Actions

    pub fn run_action(&mut self, action: Action) {
        let s = self.scale;
        match action {
            Action::Unbound => {}
            Action::NewTab => {
                let p = self.cfg.default_profile_index(&self.profiles);
                self.new_tab_or_toast(p, None);
            }
            Action::NewTabProfile(i) => {
                if i < self.profiles.len() {
                    self.new_tab_or_toast(i, None);
                }
            }
            Action::DuplicateTab => {
                if let Some(id) = self.ws.active_tab().map(|t| t.id) {
                    self.run_cmd(Cmd::DuplicateTab(id));
                }
            }
            Action::CloseTab => {
                let i = self.ws.active;
                self.close_tab_index(i);
            }
            Action::ClosePane => {
                if let Some(id) = self.ws.active_tab().map(|t| t.focused) {
                    self.close_pane(id);
                }
            }
            Action::NextTab => {
                self.ws.cycle(true);
                self.after_structure_change();
            }
            Action::PrevTab => {
                self.ws.cycle(false);
                self.after_structure_change();
            }
            Action::SwitchToTab(i) => self.activate_tab(i),
            Action::MoveTabLeft => {
                let a = self.ws.active;
                if a > 0 {
                    self.ws.move_tab(a, a - 1);
                    self.after_structure_change();
                }
            }
            Action::MoveTabRight => {
                let a = self.ws.active;
                if a + 1 < self.ws.tabs.len() {
                    self.ws.move_tab(a, a + 1);
                    self.after_structure_change();
                }
            }
            Action::RenameTab => {
                let a = self.ws.active;
                self.start_rename_tab(a);
            }
            Action::SplitRight => self.split(Some(Axis::Horizontal)),
            Action::SplitDown => self.split(Some(Axis::Vertical)),
            Action::SplitAuto => self.split(None),
            Action::FocusLeft | Action::FocusRight | Action::FocusUp | Action::FocusDown => {
                let dir = match action {
                    Action::FocusLeft => Dir::Left,
                    Action::FocusRight => Dir::Right,
                    Action::FocusUp => Dir::Up,
                    _ => Dir::Down,
                };
                if let Some(from) = self.ws.active_tab().map(|t| t.focused)
                    && let Some(to) = layout::neighbor(&self.pane_rects, from, dir)
                {
                    self.focus_pane(to);
                }
            }
            Action::ResizeLeft | Action::ResizeRight | Action::ResizeUp | Action::ResizeDown => {
                let dir = match action {
                    Action::ResizeLeft => Dir::Left,
                    Action::ResizeRight => Dir::Right,
                    Action::ResizeUp => Dir::Up,
                    _ => Dir::Down,
                };
                if let Some(tab) = self.ws.active_tab_mut() {
                    let f = tab.focused;
                    tab.root.resize(f, dir, 0.04);
                }
                self.after_structure_change();
            }
            Action::TogglePaneZoom => {
                if let Some(tab) = self.ws.active_tab_mut()
                    && !matches!(tab.root, Node::Leaf(_))
                {
                    tab.zoomed = !tab.zoomed;
                }
                self.after_structure_change();
            }
            Action::MovePaneToNewTab => {
                if let Some(id) = self.ws.active_tab().map(|t| t.focused) {
                    self.run_cmd(Cmd::BreakOutPane(id));
                }
            }
            Action::RenamePane => {
                if let Some(id) = self.ws.active_tab().map(|t| t.focused) {
                    self.run_cmd(Cmd::RenamePane(id));
                }
            }
            Action::NewGroup => {
                let a = self.ws.active;
                if self.ws.tabs.get(a).is_some_and(|t| t.group.is_none()) {
                    self.group_tab(a);
                }
            }
            Action::Ungroup => {
                let a = self.ws.active;
                self.ws.ungroup_tab(a);
                self.after_structure_change();
            }
            Action::ToggleGroupCollapse => {
                if let Some(g) = self.ws.active_tab().and_then(|t| t.group) {
                    self.ws.toggle_collapse(g);
                    self.after_structure_change();
                }
            }
            Action::Copy => self.copy_selection(false),
            Action::Paste => self.paste(ClipboardType::Clipboard),
            Action::SelectAll => {
                if let Some(p) = self.ws.focused_pane() {
                    let mut term = p.term.lock();
                    let top = term.grid().topmost_line();
                    let bottom = term.grid().bottommost_line();
                    let last = term.grid().last_column();
                    let mut sel = Selection::new(SelectionType::Simple, Point::new(top, Column(0)), Side::Left);
                    sel.update(Point::new(bottom, last), Side::Right);
                    term.selection = Some(sel);
                }
                self.window.request_redraw();
            }
            Action::ScrollUp => self.scroll_focused(Scroll::Delta(1)),
            Action::ScrollDown => self.scroll_focused(Scroll::Delta(-1)),
            Action::ScrollPageUp => self.scroll_focused(Scroll::PageUp),
            Action::ScrollPageDown => self.scroll_focused(Scroll::PageDown),
            Action::ScrollTop => self.scroll_focused(Scroll::Top),
            Action::ScrollBottom => self.scroll_focused(Scroll::Bottom),
            Action::ClearScrollback => {
                if let Some(p) = self.ws.focused_pane() {
                    let mut term = p.term.lock();
                    term.grid_mut().clear_history();
                    term.scroll_display(Scroll::Bottom);
                }
                self.window.request_redraw();
            }
            Action::FontBigger | Action::FontSmaller | Action::FontReset => {
                if let Some(id) = self.ws.active_tab().map(|t| t.focused) {
                    let delta = match action {
                        Action::FontBigger => Some(1.0),
                        Action::FontSmaller => Some(-1.0),
                        _ => None,
                    };
                    self.change_pane_font(id, delta);
                }
            }
            Action::OpacityUp => self.set_opacity(self.opacity + 0.05),
            Action::OpacityDown => self.set_opacity(self.opacity - 0.05),
            Action::ToggleTransparency => {
                if !self.renderer.supports_transparency {
                    self.show_toast("Transparency isn't supported by this GPU surface".into());
                } else {
                    self.transparency_on = !self.transparency_on;
                    let msg = if self.transparency_on {
                        format!("Transparency on ({:.0}%)", self.opacity * 100.0)
                    } else {
                        "Transparency off".to_string()
                    };
                    self.show_toast(msg);
                }
            }
            Action::NextColorScheme => {
                let i = self
                    .schemes
                    .iter()
                    .position(|sc| sc.name == self.scheme.name)
                    .unwrap_or(0);
                let next = self.schemes[(i + 1) % self.schemes.len()].name.clone();
                self.run_cmd(Cmd::SetScheme(next));
            }
            Action::ToggleFullscreen => {
                let fs = self.window.fullscreen().is_some();
                self.window
                    .set_fullscreen(if fs { None } else { Some(Fullscreen::Borderless(None)) });
            }
            Action::OpenSettings => {
                if self.settings.is_some() {
                    self.close_settings();
                } else {
                    self.open_settings();
                }
            }
            Action::ReloadConfig => self.reload_config(),
            Action::NewTabMenu => {
                let r = self.tabbar().dropdown;
                self.open_new_tab_menu(r.x, r.bottom() + 4.0 * s);
            }
            Action::Quit => self.exit = true,
            Action::OpenLocation(i) => self.open_location(i),
        }
    }

    /// Pane actions that can't do anything let the key through to the program, like Windows
    /// Terminal does (so e.g. Alt+Left still reaches the shell when there's a single pane).
    pub(crate) fn action_applies(&self, action: Action) -> bool {
        let Some(tab) = self.ws.active_tab() else { return true };
        let split = !matches!(tab.root, Node::Leaf(_));
        let dir = match action {
            Action::FocusLeft => Dir::Left,
            Action::FocusRight => Dir::Right,
            Action::FocusUp => Dir::Up,
            Action::FocusDown => Dir::Down,
            Action::ResizeLeft
            | Action::ResizeRight
            | Action::ResizeUp
            | Action::ResizeDown
            | Action::TogglePaneZoom
            | Action::MovePaneToNewTab => return split,
            _ => return true,
        };
        layout::neighbor(&self.pane_rects, tab.focused, dir).is_some()
    }

    pub fn run_cmd(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Action(a) => self.run_action(a),
            Cmd::NewTabWithProfile(i) => self.new_tab_or_toast(i, None),
            Cmd::CloseTab(id) => {
                if let Some(i) = self.tab_index_by_id(id) {
                    self.close_tab_index(i);
                }
            }
            Cmd::CloseOtherTabs(id) => {
                let others: Vec<TabId> = self.ws.tabs.iter().map(|t| t.id).filter(|t| *t != id).collect();
                for t in others {
                    if let Some(i) = self.tab_index_by_id(t) {
                        drop(self.ws.remove_tab(i));
                    }
                }
                self.after_structure_change();
            }
            Cmd::DuplicateTab(id) => {
                let Some(i) = self.tab_index_by_id(id) else {
                    return;
                };
                let focused = self.ws.tabs[i].focused;
                let (profile, cwd, launch, font_size) = match self.ws.panes.get(&focused) {
                    Some(p) => (p.profile, p.cwd(), p.launch.clone(), p.font_size),
                    None => (
                        self.cfg.default_profile_index(&self.profiles),
                        None,
                        PaneLaunch::default(),
                        None,
                    ),
                };
                let (color, group, title) = (
                    self.ws.tabs[i].color,
                    self.ws.tabs[i].group,
                    self.ws.tabs[i].custom_title.clone(),
                );
                self.ws.active = i;
                let area = self.content_rect();
                match self.spawn_pane(profile, cwd, area, font_size, launch) {
                    Ok(pane) => {
                        let id = self.ws.next_id();
                        self.ws.insert_tab(Tab {
                            id,
                            root: Node::Leaf(pane),
                            focused: pane,
                            custom_title: title,
                            color,
                            group,
                            zoomed: false,
                            bell: false,
                        });
                    }
                    Err(e) => self.show_toast(format!("Could not duplicate the tab: {e}")),
                }
                self.after_structure_change();
            }
            Cmd::RenameTab(id) => {
                if let Some(i) = self.tab_index_by_id(id) {
                    self.start_rename_tab(i);
                }
            }
            Cmd::SetTabColor(id, color) => {
                if let Some(i) = self.tab_index_by_id(id) {
                    self.ws.tabs[i].color = color;
                }
            }
            Cmd::NewGroupWithTab(id) => {
                if let Some(i) = self.tab_index_by_id(id) {
                    self.group_tab(i);
                }
            }
            Cmd::AddTabToGroup(id, gid) => {
                if let Some(i) = self.tab_index_by_id(id) {
                    self.ws.add_to_group(i, gid);
                }
            }
            Cmd::RemoveTabFromGroup(id) => {
                if let Some(i) = self.tab_index_by_id(id) {
                    self.ws.ungroup_tab(i);
                }
            }
            Cmd::RenameGroup(gid) => {
                let name = self.ws.group(gid).map(|g| g.name.clone()).unwrap_or_default();
                self.edit = Some(InlineEdit::new(EditTarget::Group(gid), name));
            }
            Cmd::SetGroupColor(gid, color) => {
                if let Some(g) = self.ws.group_mut(gid) {
                    g.color = color;
                }
            }
            Cmd::ToggleGroup(gid) => self.ws.toggle_collapse(gid),
            Cmd::Ungroup(gid) => self.ws.dissolve_group(gid),
            Cmd::CloseGroup(gid) => {
                let ids: Vec<TabId> = self
                    .ws
                    .tabs
                    .iter()
                    .filter(|t| t.group == Some(gid))
                    .map(|t| t.id)
                    .collect();
                for t in ids {
                    if let Some(i) = self.tab_index_by_id(t) {
                        self.close_tab_index(i);
                        if self.exit {
                            return;
                        }
                    }
                }
            }
            Cmd::SplitPane(id, axis) => {
                self.focus_pane(id);
                self.split(Some(axis));
            }
            Cmd::ClosePane(id) => self.close_pane(id),
            Cmd::JoinTab(tab, dir) => {
                let Some(target) = self.ws.active_tab().map(|t| t.focused) else {
                    return;
                };
                self.run_cmd(Cmd::DockTab(tab, target, dir));
            }
            Cmd::DockTab(tab, target, dir) => {
                let old_focus = self.ws.active_tab().map(|t| t.focused);
                if self.ws.join_tab(tab, target, dir).is_some() {
                    if let Some(o) = old_focus {
                        self.send_focus(o, false);
                    }
                    if let Some(n) = self.ws.active_tab().map(|t| t.focused) {
                        self.send_focus(n, true);
                    }
                }
            }
            Cmd::BreakOutPane(id) => {
                if self.ws.break_out(id).is_none() {
                    self.show_toast("This pane is already the only one in its tab".into());
                }
            }
            Cmd::RenamePane(id) => {
                let Some(ti) = self.ws.tab_of_pane(id) else { return };
                if matches!(self.ws.tabs[ti].root, Node::Leaf(_)) {
                    // A lone pane is named by its tab.
                    self.start_rename_tab(ti);
                } else {
                    self.focus_pane(id);
                    let current = self
                        .ws
                        .panes
                        .get(&id)
                        .map(|p| p.display_title().to_string())
                        .unwrap_or_default();
                    // Naming a pane gives it a label, which also makes headers appear.
                    if let Some(p) = self.ws.panes.get_mut(&id)
                        && p.label.is_none()
                    {
                        p.label = Some(current.clone());
                    }
                    self.edit = Some(InlineEdit::new(EditTarget::Pane(id), current));
                }
            }
            Cmd::SetSetting(setting, value) => self.set_setting(setting, value),
            Cmd::SetPaneColor(id, color) => {
                if let Some(p) = self.ws.panes.get_mut(&id) {
                    p.color = color;
                }
            }
            Cmd::SetScheme(name) => {
                self.cfg.color_scheme = name.clone();
                self.scheme = self.cfg.scheme(None);
                self.ui = UiPalette::from_scheme(&self.scheme);
                self.show_toast(format!("Color scheme: {name}"));
            }
            Cmd::OpenLocation(i) => self.open_location(i),
            Cmd::OpenSshHost(host) => self.open_ssh_host(&host),
            Cmd::ManageLocations => self.open_settings_page(crate::settings::Page::Locations),
            Cmd::AddLocation => {
                self.open_settings_page(crate::settings::Page::Locations);
                self.edit_location(None);
            }
        }
        self.after_structure_change();
    }

    pub(crate) fn open_config_file(&mut self) {
        let path = config::config_path();
        if let Err(e) = open_external(path.as_os_str()) {
            self.show_toast(format!("Could not open {}: {e}", path.display()));
        }
    }

    /// The first shortcut bound to `action`, for menu hints.
    pub(crate) fn hint(&self, action: Action) -> String {
        self.keybindings
            .iter()
            .find(|(_, a)| *a == action)
            .map(|(k, _)| k.display())
            .unwrap_or_default()
    }

    pub(crate) fn scroll_focused(&mut self, scroll: Scroll) {
        if let Some(p) = self.ws.focused_pane() {
            p.term.lock().scroll_display(scroll);
        }
        self.window.request_redraw();
    }

    /// Grow/shrink one pane's text by `delta` logical pixels, or reset it to the default.
    pub(crate) fn change_pane_font(&mut self, id: PaneId, delta: Option<f32>) {
        let default = self.font_size;
        let Some(p) = self.ws.panes.get_mut(&id) else { return };
        p.font_size = delta
            .map(|d| (p.font_size.unwrap_or(default) + d).clamp(6.0, 72.0))
            .filter(|s| *s != default);
        let size = p.font_size.unwrap_or(default);
        self.after_structure_change();
        let msg = if p_is_default(size, default) {
            format!("Text size {size} (default)")
        } else {
            format!("Text size {size} for this pane · Ctrl+0 resets")
        };
        self.show_toast(msg);
    }

    pub(crate) fn set_opacity(&mut self, opacity: f32) {
        if !self.renderer.supports_transparency {
            self.show_toast("Transparency isn't supported by this GPU surface".into());
            return;
        }
        self.opacity = (opacity * 100.0).round().clamp(10.0, 100.0) / 100.0;
        self.transparency_on = true;
        self.show_toast(format!("Opacity {:.0}%", self.opacity * 100.0));
    }

    pub fn effective_opacity(&self) -> f32 {
        if self.transparency_on { self.opacity } else { 1.0 }
    }

    pub fn tab_bar_opacity(&self) -> f32 {
        if !self.transparency_on {
            return 1.0;
        }
        let base = self.cfg.tab_bar_opacity();
        // Follow live opacity changes relative to the configured value.
        (base * self.opacity / self.cfg.opacity.max(0.01)).clamp(0.05, 1.0)
    }

    pub fn show_toast(&mut self, msg: String) {
        self.toast = Some((msg, Instant::now()));
        self.window.request_redraw();
    }

    pub fn blinking(&self) -> bool {
        if !self.focused || !self.cfg.cursor_blink || self.edit.is_some() {
            return false;
        }
        if self.last_input.elapsed() > BLINK_IDLE_STOP {
            return false;
        }
        self.ws
            .focused_pane()
            .is_some_and(|p| p.term.lock().cursor_style().blinking)
    }

    pub fn cursor_blink_visible(&self) -> bool {
        !self.blinking() || (self.last_input.elapsed().as_millis() / BLINK.as_millis()).is_multiple_of(2)
    }

    pub(crate) fn reload_config(&mut self) {
        // Our own save just wrote the file; nothing to reload.
        if self.last_saved.is_some() && std::fs::read_to_string(config::config_path()).ok() == self.last_saved {
            return;
        }
        let (cfg, err) = Config::load();
        if let Some(e) = err {
            self.show_toast(e);
            return;
        }
        let font_changed = cfg.font_family != self.cfg.font_family
            || cfg.ui_font_family != self.cfg.ui_font_family
            || cfg.font_size != self.cfg.font_size
            || cfg.line_height != self.cfg.line_height;
        self.opacity = cfg.opacity;
        self.window.set_blur(cfg.blur);
        self.profiles = cfg.profiles();
        self.keybindings = cfg.keybindings();
        self.schemes = cfg.schemes();
        self.scheme = cfg.scheme(None);
        self.ui = UiPalette::from_scheme(&self.scheme);
        let tc = crate::pane::term_config(&cfg);
        for p in self.ws.panes.values() {
            p.term.lock().set_options(tc.clone());
        }
        crate::desktop::sync_actions(&cfg.locations);
        self.cfg = cfg;
        if font_changed {
            self.font_size = self.cfg.font_size;
            self.text
                .set_fonts(&self.cfg.font_family, &self.cfg.ui_font_family, self.cfg.line_height);
        }
        self.after_structure_change();
        self.show_toast("Settings reloaded".into());
    }

    pub fn pane_scheme(&self, pane: &Pane) -> &Scheme {
        pane.scheme
            .as_deref()
            .and_then(|n| self.schemes.iter().find(|s| s.name.eq_ignore_ascii_case(n)))
            .unwrap_or(&self.scheme)
    }

    // ---------------------------------------------------------------------------------------------
    // Clipboard

    pub(crate) fn copy_selection(&mut self, primary_only: bool) {
        let Some(text) = self.ws.focused_pane().and_then(|p| p.term.lock().selection_to_string()) else {
            return;
        };
        if text.is_empty() {
            return;
        }
        if let Some(cb) = &self.clipboard {
            cb.store_primary(text.clone());
            if !primary_only {
                cb.store(text);
            }
        }
    }

    pub(crate) fn paste(&mut self, ty: ClipboardType) {
        let Some(cb) = &self.clipboard else { return };
        let text = match ty {
            ClipboardType::Clipboard => cb.load(),
            ClipboardType::Selection => cb.load_primary(),
        };
        let Ok(text) = text else { return };
        if let Some(p) = self.ws.focused_pane() {
            let mode = *p.term.lock().mode();
            p.term.lock().scroll_display(Scroll::Bottom);
            p.write(input::paste_bytes(&text, mode));
        }
    }

    // ---------------------------------------------------------------------------------------------
    // Menus

    pub(crate) fn open_new_tab_menu(&mut self, x: f32, y: f32) {
        let mut entries = vec![MenuEntry::Label("New tab".into())];
        for (i, p) in self.profiles.iter().enumerate() {
            let hint = if i < 5 {
                self.hint(Action::NewTabProfile(i))
            } else {
                String::new()
            };
            entries.push(MenuEntry::item(&p.name, hint, Cmd::NewTabWithProfile(i)));
        }
        entries.push(MenuEntry::Separator);
        entries.push(MenuEntry::item(
            if self.transparency_on {
                "Disable transparency"
            } else {
                "Enable transparency"
            },
            self.hint(Action::ToggleTransparency),
            Cmd::Action(Action::ToggleTransparency),
        ));
        entries.push(MenuEntry::item(
            format!("Color scheme: {}", self.scheme.name),
            self.hint(Action::NextColorScheme),
            Cmd::Action(Action::NextColorScheme),
        ));
        entries.push(MenuEntry::Separator);
        entries.push(MenuEntry::item(
            "Settings",
            self.hint(Action::OpenSettings),
            Cmd::Action(Action::OpenSettings),
        ));
        entries.push(MenuEntry::item(
            "Reload settings",
            self.hint(Action::ReloadConfig),
            Cmd::Action(Action::ReloadConfig),
        ));
        self.open_menu(entries, x, y);
    }

    pub(crate) fn open_locations_menu(&mut self, x: f32, y: f32) {
        let mut entries = vec![MenuEntry::Label("Locations".into())];
        if self.cfg.locations.is_empty() {
            entries.push(MenuEntry::Label("No locations yet".into()));
        }
        for (i, loc) in self.cfg.locations.iter().enumerate() {
            let hint = self.hint(Action::OpenLocation(i));
            // The host goes in the dimmed hint column, before any shortcut.
            let hint = match (loc.remote_host(), hint.is_empty()) {
                (Some(h), true) => h.to_string(),
                (Some(h), false) => format!("{h}  ·  {hint}"),
                (None, _) => hint,
            };
            entries.push(MenuEntry::item(&loc.name, hint, Cmd::OpenLocation(i)));
        }
        // Hosts from ~/.ssh/config, read fresh each time, minus those saved for their home folder.
        let saved_home = |h: &str| {
            self.cfg
                .locations
                .iter()
                .any(|l| l.remote_host() == Some(h) && matches!(l.path.trim(), "" | "~" | "~/"))
        };
        let hosts: Vec<String> = config::ssh_hosts().into_iter().filter(|h| !saved_home(h)).collect();
        if !hosts.is_empty() {
            entries.push(MenuEntry::Separator);
            entries.push(MenuEntry::Label("SSH hosts".into()));
            for h in hosts {
                entries.push(MenuEntry::item(&h, "", Cmd::OpenSshHost(h.clone())));
            }
        }
        entries.push(MenuEntry::Separator);
        entries.push(MenuEntry::item("Add location…", "", Cmd::AddLocation));
        entries.push(MenuEntry::item("Edit locations…", "", Cmd::ManageLocations));
        self.open_menu(entries, x, y);
    }

    pub(crate) fn open_tab_menu(&mut self, index: usize, x: f32, y: f32) {
        let Some(tab) = self.ws.tabs.get(index) else {
            return;
        };
        let id = tab.id;
        let mut entries = vec![
            MenuEntry::item("Rename tab", self.hint(Action::RenameTab), Cmd::RenameTab(id)),
            MenuEntry::item("Duplicate tab", self.hint(Action::DuplicateTab), Cmd::DuplicateTab(id)),
            MenuEntry::Label("Tab color".into()),
            MenuEntry::Swatches {
                target: SwatchTarget::Tab(id),
                current: tab.color,
            },
            MenuEntry::Separator,
        ];
        if index != self.ws.active {
            entries.push(MenuEntry::item(
                "Show beside current tab",
                "",
                Cmd::JoinTab(id, Dir::Right),
            ));
            entries.push(MenuEntry::item(
                "Show below current tab",
                "",
                Cmd::JoinTab(id, Dir::Down),
            ));
            entries.push(MenuEntry::Separator);
        }
        match tab.group {
            Some(gid) => {
                let name = self.ws.group(gid).map(|g| g.name.clone()).unwrap_or_default();
                entries.push(MenuEntry::item(
                    format!("Remove from \"{name}\""),
                    self.hint(Action::Ungroup),
                    Cmd::RemoveTabFromGroup(id),
                ));
            }
            None => entries.push(MenuEntry::item(
                "Add to new group",
                self.hint(Action::NewGroup),
                Cmd::NewGroupWithTab(id),
            )),
        }
        for g in &self.ws.groups {
            if tab.group != Some(g.id) {
                entries.push(MenuEntry::item(
                    format!("Move to \"{}\"", g.name),
                    "",
                    Cmd::AddTabToGroup(id, g.id),
                ));
            }
        }
        entries.push(MenuEntry::Separator);
        if self.ws.tabs.len() > 1 {
            entries.push(MenuEntry::item("Close other tabs", "", Cmd::CloseOtherTabs(id)));
        }
        entries.push(MenuEntry::danger(
            "Close tab",
            self.hint(Action::CloseTab),
            Cmd::CloseTab(id),
        ));
        self.open_menu(entries, x, y);
    }

    pub(crate) fn open_group_menu(&mut self, gid: crate::workspace::GroupId, x: f32, y: f32) {
        let Some(g) = self.ws.group(gid) else { return };
        let entries = vec![
            MenuEntry::item("Rename group", "", Cmd::RenameGroup(gid)),
            MenuEntry::item(
                if g.collapsed { "Expand group" } else { "Collapse group" },
                "",
                Cmd::ToggleGroup(gid),
            ),
            MenuEntry::Label("Group color".into()),
            MenuEntry::Swatches {
                target: SwatchTarget::Group(gid),
                current: Some(g.color),
            },
            MenuEntry::Separator,
            MenuEntry::item("Ungroup", "", Cmd::Ungroup(gid)),
            MenuEntry::danger("Close group", "", Cmd::CloseGroup(gid)),
        ];
        self.open_menu(entries, x, y);
    }

    pub(crate) fn open_pane_menu(&mut self, pane: PaneId, x: f32, y: f32) {
        let has_sel = self
            .ws
            .panes
            .get(&pane)
            .and_then(|p| p.term.lock().selection.as_ref().map(|s| !s.is_empty()))
            .unwrap_or(false);
        let multi = self.pane_rects.len() > 1 || self.ws.active_tab().is_some_and(|t| t.zoomed);
        let mut entries = Vec::new();
        if has_sel {
            entries.push(MenuEntry::item(
                "Copy",
                self.hint(Action::Copy),
                Cmd::Action(Action::Copy),
            ));
        }
        entries.push(MenuEntry::item(
            "Paste",
            self.hint(Action::Paste),
            Cmd::Action(Action::Paste),
        ));
        entries.push(MenuEntry::item(
            "Select all",
            self.hint(Action::SelectAll),
            Cmd::Action(Action::SelectAll),
        ));
        entries.push(MenuEntry::Separator);
        entries.push(MenuEntry::item(
            "Split right",
            self.hint(Action::SplitRight),
            Cmd::SplitPane(pane, Axis::Horizontal),
        ));
        entries.push(MenuEntry::item(
            "Split down",
            self.hint(Action::SplitDown),
            Cmd::SplitPane(pane, Axis::Vertical),
        ));
        if multi {
            let zoomed = self.ws.active_tab().is_some_and(|t| t.zoomed);
            entries.push(MenuEntry::item(
                if zoomed { "Unzoom pane" } else { "Zoom pane" },
                self.hint(Action::TogglePaneZoom),
                Cmd::Action(Action::TogglePaneZoom),
            ));
            entries.push(MenuEntry::item(
                "Move to new tab",
                self.hint(Action::MovePaneToNewTab),
                Cmd::BreakOutPane(pane),
            ));
            entries.push(MenuEntry::Separator);
            entries.push(MenuEntry::item(
                "Rename pane",
                self.hint(Action::RenamePane),
                Cmd::RenamePane(pane),
            ));
            entries.push(MenuEntry::Label("Pane color".into()));
            entries.push(MenuEntry::Swatches {
                target: SwatchTarget::Pane(pane),
                current: self.ws.panes.get(&pane).and_then(|p| p.color),
            });
        }
        entries.push(MenuEntry::Separator);
        entries.push(MenuEntry::item(
            "Clear scrollback",
            self.hint(Action::ClearScrollback),
            Cmd::Action(Action::ClearScrollback),
        ));
        entries.push(MenuEntry::danger(
            "Close pane",
            self.hint(Action::ClosePane),
            Cmd::ClosePane(pane),
        ));
        self.open_menu(entries, x, y);
    }

    pub(crate) fn open_menu(&mut self, entries: Vec<MenuEntry>, x: f32, y: f32) {
        let win = (self.size.width as f32, self.size.height as f32);
        let s = self.scale;
        let mut menu = Menu::new(entries, x, y, &mut self.text, s, win);
        menu.update_hover(self.mouse.0, self.mouse.1, s);
        self.menu = Some(menu);
        self.window.request_redraw();
    }

    // ---------------------------------------------------------------------------------------------
    // Events

    pub(crate) fn handle_window_event(&mut self, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => self.exit = true,
            WindowEvent::RedrawRequested => self.draw(),
            WindowEvent::Resized(size) => {
                if size != self.size {
                    self.menu = None;
                }
                self.size = size;
                self.renderer.resize(size.width, size.height);
                self.relayout();
                self.window.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.scale = scale_factor as f32;
                self.relayout();
                self.window.request_redraw();
            }
            WindowEvent::Focused(f) => {
                self.focused = f;
                if f {
                    self.focused_at = unix_now();
                }
                if let Some(id) = self.ws.active_tab().map(|t| t.focused) {
                    self.send_focus(id, f);
                }
                if !f {
                    self.mods = ModifiersState::empty();
                }
                self.last_input = Instant::now();
                self.window.request_redraw();
            }
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::KeyboardInput {
                event, is_synthetic, ..
            } => {
                if event.state == ElementState::Pressed && !is_synthetic {
                    let unmodified = event.key_without_modifiers();
                    self.on_key(&event.logical_key, &unmodified, event.text.as_deref());
                }
            }
            WindowEvent::Ime(Ime::Commit(text)) => {
                if self.settings_text(&text) {
                    self.window.request_redraw();
                } else if let Some(edit) = self.edit.as_mut() {
                    edit.insert(&text);
                    self.window.request_redraw();
                } else if let Some(p) = self.ws.focused_pane() {
                    p.term.lock().scroll_display(Scroll::Bottom);
                    p.write(text.into_bytes());
                }
            }
            WindowEvent::CursorMoved { position, .. } => self.on_mouse_move(position),
            WindowEvent::CursorLeft { .. } => {
                if self.hover.take().is_some() {
                    self.window.request_redraw();
                }
            }
            WindowEvent::MouseInput { state, button, .. } => self.on_mouse_button(state, button),
            WindowEvent::MouseWheel { delta, .. } => self.on_wheel(delta),
            _ => {}
        }
    }

    pub(crate) fn handle_term_event(&mut self, id: PaneId, ev: TermEvent) {
        match ev {
            TermEvent::Wakeup => {
                if self.pane_rects.iter().any(|(p, _)| *p == id) {
                    self.window.request_redraw();
                }
            }
            TermEvent::Title(t) => {
                if let Some(p) = self.ws.panes.get_mut(&id) {
                    p.title = t;
                }
                self.update_window_title();
                self.window.request_redraw();
            }
            TermEvent::ResetTitle => {
                if let Some(p) = self.ws.panes.get_mut(&id) {
                    p.title.clear();
                }
                self.update_window_title();
                self.window.request_redraw();
            }
            TermEvent::ClipboardStore(ty, text) => {
                if let Some(cb) = &self.clipboard {
                    match ty {
                        ClipboardType::Clipboard => cb.store(text),
                        ClipboardType::Selection => cb.store_primary(text),
                    }
                }
            }
            TermEvent::ClipboardLoad(ty, format) => {
                let text = self.clipboard.as_ref().and_then(|cb| match ty {
                    ClipboardType::Clipboard => cb.load().ok(),
                    ClipboardType::Selection => cb.load_primary().ok(),
                });
                if let (Some(text), Some(p)) = (text, self.ws.panes.get(&id)) {
                    p.write(format(&text).into_bytes());
                }
            }
            TermEvent::ColorRequest(index, format) => {
                let Some(p) = self.ws.panes.get(&id) else {
                    return;
                };
                let scheme = self.pane_scheme(p);
                let color = p.term.lock().colors()[index]
                    .map(|c| [c.r as f32 / 255.0, c.g as f32 / 255.0, c.b as f32 / 255.0, 1.0]);
                let c = color.unwrap_or_else(|| match index {
                    256 => scheme.fg,
                    257 => scheme.bg,
                    258 => scheme.cursor,
                    i if i < 256 => scheme.indexed(i as u8),
                    _ => scheme.fg,
                });
                let rgb = Rgb {
                    r: (c[0] * 255.0) as u8,
                    g: (c[1] * 255.0) as u8,
                    b: (c[2] * 255.0) as u8,
                };
                p.write(format(rgb).into_bytes());
            }
            TermEvent::TextAreaSizeRequest(format) => {
                if let Some(p) = self.ws.panes.get(&id) {
                    p.write(format(crate::pane::window_size(p.grid, self.pane_cell(id))).into_bytes());
                }
            }
            TermEvent::PtyWrite(text) => {
                if let Some(p) = self.ws.panes.get(&id) {
                    p.write(text.into_bytes());
                }
            }
            TermEvent::CursorBlinkingChange => self.window.request_redraw(),
            TermEvent::MouseCursorDirty => {}
            TermEvent::Bell => {
                if let Some(ti) = self.ws.tab_of_pane(id)
                    && (ti != self.ws.active || !self.focused)
                {
                    self.ws.tabs[ti].bell = ti != self.ws.active;
                    self.window
                        .request_user_attention(Some(UserAttentionType::Informational));
                    self.window.request_redraw();
                }
            }
            TermEvent::ChildExit(status) => {
                let Some(p) = self.ws.panes.get_mut(&id) else { return };
                // Shells close their pane however they exit; a launched command (ssh, a
                // PatchGoblin project, `-e`) that fails stays open so its error can be read.
                if status.success() || p.launch.command.is_none() {
                    self.close_pane(id);
                    return;
                }
                let why = match status.code() {
                    Some(code) => format!("exited with code {code}"),
                    None => "was terminated".to_string(),
                };
                p.print(&format!(
                    "\r\n\x1b[0;2m[process {why} · Enter closes · r restarts]\x1b[0m"
                ));
                p.exited = Some(why);
                self.window.request_redraw();
            }
            TermEvent::Exit => {
                if self.ws.panes.get(&id).is_some_and(|p| p.exited.is_none()) {
                    self.close_pane(id);
                }
            }
        }
    }

    pub(crate) fn on_key(&mut self, key: &Key, unmodified: &Key, text: Option<&str>) {
        self.last_input = Instant::now();

        // A failed command's pane: Enter/Esc/q close it, r runs it again.
        if self.edit.is_none()
            && self.menu.is_none()
            && self.settings.is_none()
            && let Some(id) = self.ws.active_tab().map(|t| t.focused)
            && self.ws.panes.get(&id).is_some_and(|p| p.exited.is_some())
        {
            match key {
                Key::Named(NamedKey::Enter | NamedKey::Escape) => self.close_pane(id),
                Key::Character(c) if c.eq_ignore_ascii_case("q") => self.close_pane(id),
                Key::Character(c) if c.eq_ignore_ascii_case("r") => self.restart_pane(id),
                _ => {}
            }
            return;
        }

        if self.edit.is_some() {
            self.on_edit_key(key, text);
            return;
        }

        if let Some(menu) = self.menu.as_mut() {
            match key {
                Key::Named(NamedKey::Escape) => self.menu = None,
                Key::Named(NamedKey::ArrowDown) | Key::Named(NamedKey::Tab) => menu.step(true),
                Key::Named(NamedKey::ArrowUp) => menu.step(false),
                Key::Named(NamedKey::Enter) => {
                    if let Some(MenuEntry::Item { cmd, .. }) = menu.hovered.and_then(|h| menu.entries.get(h)).cloned() {
                        self.menu = None;
                        self.run_cmd(cmd);
                    }
                }
                _ => {}
            }
            self.window.request_redraw();
            return;
        }

        if self.settings_key(key, unmodified, text, self.mods) {
            return;
        }

        let action = self
            .keybindings
            .iter()
            .find(|(k, _)| k.matches(self.mods, unmodified))
            .map(|(_, a)| *a)
            .filter(|a| self.action_applies(*a));
        if let Some(action) = action {
            self.run_action(action);
            return;
        }

        let Some(pane) = self.ws.focused_pane() else {
            return;
        };
        let mode = *pane.term.lock().mode();
        if let Some(bytes) = input::encode_key(key, unmodified, text, self.mods, mode) {
            {
                let mut term = pane.term.lock();
                term.scroll_display(Scroll::Bottom);
                term.selection = None;
            }
            pane.write(bytes);
            self.window.request_redraw();
        }
    }

    pub(crate) fn on_edit_key(&mut self, key: &Key, text: Option<&str>) {
        let ctrl = self.mods.control_key();
        let Some(edit) = self.edit.as_mut() else {
            return;
        };
        match key {
            Key::Named(NamedKey::Enter) => {
                self.commit_edit();
                return;
            }
            Key::Named(NamedKey::Escape) => self.edit = None,
            Key::Named(NamedKey::Backspace) => edit.backspace(),
            Key::Named(NamedKey::Delete) => edit.delete(),
            Key::Named(NamedKey::ArrowLeft) => edit.left(),
            Key::Named(NamedKey::ArrowRight) => edit.right(),
            Key::Named(NamedKey::Home) => edit.home(),
            Key::Named(NamedKey::End) => edit.end(),
            Key::Character(c) if ctrl && c.eq_ignore_ascii_case("a") => edit.all_selected = true,
            Key::Character(c) if ctrl && c.eq_ignore_ascii_case("v") => {
                if let Some(t) = self.clipboard.as_ref().and_then(|cb| cb.load().ok()) {
                    edit.insert(t.lines().next().unwrap_or(""));
                }
            }
            _ => {
                if let Some(t) = text.filter(|_| !ctrl) {
                    edit.insert(t);
                }
            }
        }
        self.after_structure_change();
    }

    pub(crate) fn set_cursor_icon(&mut self, icon: CursorIcon) {
        if icon != self.cursor_icon {
            self.cursor_icon = icon;
            self.window.set_cursor(icon);
        }
    }

    pub(crate) fn on_mouse_move(&mut self, pos: PhysicalPosition<f64>) {
        let (x, y) = (pos.x as f32, pos.y as f32);
        self.mouse = (x, y);
        let s = self.scale;

        if let Some(menu) = self.menu.as_mut() {
            if menu.update_hover(x, y, s) {
                self.window.request_redraw();
            }
            self.set_cursor_icon(CursorIcon::Default);
            return;
        }

        if self.settings.is_some() {
            self.settings_mouse_move(x, y);
            self.set_cursor_icon(CursorIcon::Default);
            return;
        }

        match &mut self.drag {
            Drag::Divider(div) => {
                let div = div.clone();
                let ratio = match div.axis {
                    Axis::Horizontal => (x - div.area.x) / div.area.w,
                    Axis::Vertical => (y - div.area.y) / div.area.h,
                };
                if let Some(tab) = self.ws.active_tab_mut() {
                    tab.root.set_ratio(&div.path, ratio);
                }
                self.relayout();
                // The divider's area is unchanged by moving it, so keep dragging the same one.
                self.window.request_redraw();
                return;
            }
            Drag::Tab { .. } => {
                self.drag_tab(x, y);
                return;
            }
            Drag::Selection(pane) => {
                let pane = *pane;
                if let Some((_, rect)) = self.pane_rects.iter().find(|(id, _)| *id == pane).copied() {
                    // Scroll while dragging past the top or bottom edge.
                    if let Some(p) = self.ws.panes.get(&pane) {
                        if y < rect.y {
                            p.term.lock().scroll_display(Scroll::Delta(1));
                        } else if y > rect.bottom() {
                            p.term.lock().scroll_display(Scroll::Delta(-1));
                        }
                    }
                    if let Some((point, side, ..)) = self.grid_point(pane, rect, x, y)
                        && let Some(p) = self.ws.panes.get(&pane)
                        && let Some(sel) = p.term.lock().selection.as_mut()
                    {
                        sel.update(point, side);
                    }
                    self.window.request_redraw();
                }
                return;
            }
            Drag::None => {}
        }

        // Mouse motion reporting.
        if let Some((pane, rect)) = self.pane_at(x, y) {
            let mode = self
                .ws
                .panes
                .get(&pane)
                .map(|p| *p.term.lock().mode())
                .unwrap_or(TermMode::empty());
            let report_motion = mode.contains(TermMode::MOUSE_MOTION)
                || (mode.contains(TermMode::MOUSE_DRAG) && self.reported_button.is_some());
            if report_motion
                && !self.mods.shift_key()
                && let Some((_, _, col, row)) = self.grid_point(pane, rect, x, y)
                && self.last_report_cell != Some((pane, col, row))
            {
                self.last_report_cell = Some((pane, col, row));
                let button = self.reported_button.unwrap_or(MouseButtonCode::Right);
                // "No button" motion is encoded as button 3.
                let bytes = if self.reported_button.is_none() {
                    let mut b = input::mouse_report(MouseButtonCode::Left, true, true, col, row, self.mods, mode);
                    fix_no_button_motion(&mut b, mode);
                    b
                } else {
                    input::mouse_report(button, true, true, col, row, self.mods, mode)
                };
                if let Some(p) = self.ws.panes.get(&pane) {
                    p.write(bytes);
                }
            }
        }

        let bar = self.tabbar();
        let hit = if y < bar.bar.h { bar.hit(x, y) } else { None };
        if hit != self.hover {
            self.hover = hit;
            self.window.request_redraw();
        }

        let icon = if let Some(dir) = self.edge_direction(x, y) {
            match dir {
                ResizeDirection::North | ResizeDirection::South => CursorIcon::NsResize,
                ResizeDirection::East | ResizeDirection::West => CursorIcon::EwResize,
                ResizeDirection::NorthWest | ResizeDirection::SouthEast => CursorIcon::NwseResize,
                ResizeDirection::NorthEast | ResizeDirection::SouthWest => CursorIcon::NeswResize,
            }
        } else if let Some(d) = self.dividers.iter().find(|d| d.rect.inset(-3.0 * s).contains(x, y)) {
            if d.axis == Axis::Horizontal {
                CursorIcon::ColResize
            } else {
                CursorIcon::RowResize
            }
        } else if self.header_at(x, y).is_some() {
            CursorIcon::Default
        } else if self.pane_at(x, y).is_some() {
            CursorIcon::Text
        } else {
            CursorIcon::Default
        };
        self.set_cursor_icon(icon);
    }

    /// Dragging a tab: reorder within the tab bar, or dock into a pane of the previous tab.
    pub(crate) fn drag_tab(&mut self, x: f32, y: f32) {
        let s = self.scale;
        let Drag::Tab {
            id,
            start,
            moved,
            prev_active,
            ..
        } = self.drag
        else {
            return;
        };
        let moved = moved || (x - start.0).abs() > 6.0 * s || (y - start.1).abs() > 6.0 * s;
        let mut dock = None;
        if moved {
            let bar_h = (ui::BAR_H * s).round();
            match prev_active.and_then(|p| self.ws.tab_index(p)) {
                Some(pi) if y >= bar_h => {
                    // Over the content: show the previous tab so this one can dock into it.
                    if pi != self.ws.active {
                        self.ws.activate(pi);
                        self.relayout();
                    }
                    dock = self.pane_at(x, y).map(|(pane, rect)| (pane, dock_side(rect, x, y)));
                }
                _ => {
                    if let Some(di) = self.ws.tab_index(id)
                        && di != self.ws.active
                    {
                        self.ws.activate(di);
                        self.relayout();
                    }
                    if y < bar_h {
                        let bar = self.tabbar();
                        let target = bar.items.iter().find_map(|it| match it {
                            ui::BarItem::Tab { index, rect, .. } if rect.contains(x, rect.y + 1.0) => Some(*index),
                            _ => None,
                        });
                        if let (Some(from), Some(to)) = (self.ws.tab_index(id), target)
                            && from != to
                        {
                            self.ws.move_tab(from, to);
                        }
                    }
                }
            }
            self.window.request_redraw();
        }
        self.drag = Drag::Tab {
            id,
            start,
            moved,
            prev_active,
            dock,
        };
    }

    pub(crate) fn click_count(&mut self, x: f32, y: f32) -> u32 {
        let now = Instant::now();
        let s = self.scale;
        let count = match self.last_click {
            Some((t, lx, ly, n)) if now - t < DOUBLE_CLICK && (lx - x).abs() < 4.0 * s && (ly - y).abs() < 4.0 * s => {
                n % 3 + 1
            }
            _ => 1,
        };
        self.last_click = Some((now, x, y, count));
        count
    }

    pub(crate) fn on_mouse_button(&mut self, state: ElementState, button: MouseButton) {
        let (x, y) = self.mouse;
        let s = self.scale;
        let pressed = state == ElementState::Pressed;
        self.last_input = Instant::now();

        if let Some(menu) = &self.menu {
            if pressed {
                match menu.hit(x, y, s) {
                    MenuHit::Entry(i) => {
                        if let Some(MenuEntry::Item { cmd, .. }) = menu.entries.get(i).cloned() {
                            self.menu = None;
                            self.run_cmd(cmd);
                        }
                    }
                    MenuHit::Swatch(i, color) => {
                        let target = match menu.entries.get(i) {
                            Some(MenuEntry::Swatches { target, .. }) => Some(*target),
                            _ => None,
                        };
                        self.menu = None;
                        match (target, color) {
                            (Some(SwatchTarget::Tab(t)), c) => self.run_cmd(Cmd::SetTabColor(t, c)),
                            (Some(SwatchTarget::Group(g)), Some(c)) => self.run_cmd(Cmd::SetGroupColor(g, c)),
                            (Some(SwatchTarget::Pane(p)), c) => self.run_cmd(Cmd::SetPaneColor(p, c)),
                            _ => {}
                        }
                    }
                    MenuHit::Inside => {}
                    MenuHit::Outside => self.menu = None,
                }
                self.window.request_redraw();
            }
            return;
        }

        if self.settings.is_some() {
            if button == MouseButton::Left {
                self.settings_mouse_button(pressed);
            }
            return;
        }

        if !pressed {
            self.on_mouse_release(button);
            return;
        }

        let bar = self.tabbar();
        let in_bar = y < bar.bar.h;
        if self.edit.is_some() {
            let editing_rect = match self.edit.as_ref().map(|e| e.target) {
                Some(EditTarget::Tab(t)) => self.ws.tab_index(t).and_then(|i| bar.tab_rect(i)),
                Some(EditTarget::Group(g)) => bar.group_rect(g),
                Some(EditTarget::Pane(p)) => self
                    .pane_rects
                    .iter()
                    .find(|(id, _)| *id == p)
                    .and_then(|(_, r)| self.pane_header(*r)),
                Some(EditTarget::Field) | None => None,
            };
            if editing_rect.is_some_and(|r| r.contains(x, y)) {
                return;
            }
            self.commit_edit();
        }

        if button == MouseButton::Left
            && let Some(dir) = self.edge_direction(x, y)
        {
            let _ = self.window.drag_resize_window(dir);
            return;
        }

        if in_bar {
            let clicks = if button == MouseButton::Left {
                self.click_count(x, y)
            } else {
                1
            };
            let bar = self.tabbar();
            match (bar.hit(x, y), button) {
                (Some(Hit::Tab(i)), MouseButton::Left) => {
                    if clicks == 2 {
                        self.start_rename_tab(i);
                    } else {
                        let id = self.ws.tabs[i].id;
                        let prev_active = self.ws.active_tab().map(|t| t.id).filter(|p| *p != id);
                        self.activate_tab(i);
                        self.drag = Drag::Tab {
                            id,
                            start: (x, y),
                            moved: false,
                            prev_active,
                            dock: None,
                        };
                    }
                }
                (Some(Hit::Tab(i)), MouseButton::Middle) | (Some(Hit::TabClose(i)), MouseButton::Left) => {
                    self.close_tab_index(i)
                }
                (Some(Hit::Tab(i)) | Some(Hit::TabClose(i)), MouseButton::Right) => {
                    self.open_tab_menu(i, x, y + 4.0 * s)
                }
                (Some(Hit::Group(g)), MouseButton::Left) => {
                    if clicks == 2 {
                        self.run_cmd(Cmd::RenameGroup(g));
                    } else {
                        self.ws.toggle_collapse(g);
                        self.after_structure_change();
                    }
                }
                (Some(Hit::Group(g)), MouseButton::Right) => self.open_group_menu(g, x, y + 4.0 * s),
                (Some(Hit::NewTab), MouseButton::Left) => self.run_action(Action::NewTab),
                (Some(Hit::NewTab), MouseButton::Right) | (Some(Hit::NewTabMenu), _) => {
                    let r = bar.dropdown;
                    self.open_new_tab_menu(r.x, r.bottom() + 4.0 * s);
                }
                (Some(Hit::Locations), MouseButton::Left) => {
                    let r = bar.locations;
                    self.open_locations_menu(r.x, r.bottom() + 4.0 * s);
                }
                (Some(Hit::Minimize), MouseButton::Left) => self.window.set_minimized(true),
                (Some(Hit::Maximize), MouseButton::Left) => self.window.set_maximized(!self.window.is_maximized()),
                (Some(Hit::Close), MouseButton::Left) => self.exit = true,
                (Some(Hit::DragArea), MouseButton::Left) => {
                    if clicks == 2 {
                        self.window.set_maximized(!self.window.is_maximized());
                    } else {
                        let _ = self.window.drag_window();
                    }
                }
                (Some(Hit::DragArea), MouseButton::Right) => {
                    self.window.show_window_menu(PhysicalPosition::new(x as f64, y as f64));
                }
                _ => {}
            }
            self.window.request_redraw();
            return;
        }

        if button == MouseButton::Left
            && let Some(d) = self.dividers.iter().find(|d| d.rect.inset(-3.0 * s).contains(x, y))
        {
            self.drag = Drag::Divider(d.clone());
            return;
        }

        if let Some((pane, _)) = self.header_at(x, y) {
            let clicks = if button == MouseButton::Left {
                self.click_count(x, y)
            } else {
                1
            };
            self.focus_pane(pane);
            match button {
                MouseButton::Left if clicks == 2 => self.run_cmd(Cmd::RenamePane(pane)),
                MouseButton::Middle => self.close_pane(pane),
                MouseButton::Right => self.open_pane_menu(pane, x, y),
                _ => {}
            }
            self.window.request_redraw();
            return;
        }

        let Some((pane, rect)) = self.pane_at(x, y) else {
            return;
        };
        self.focus_pane(pane);
        let Some(p) = self.ws.panes.get(&pane) else {
            return;
        };
        let mode = *p.term.lock().mode();

        if mode.intersects(TermMode::MOUSE_MODE) && !self.mods.shift_key() {
            let code = match button {
                MouseButton::Left => MouseButtonCode::Left,
                MouseButton::Middle => MouseButtonCode::Middle,
                MouseButton::Right => MouseButtonCode::Right,
                _ => return,
            };
            if let Some((_, _, col, row)) = self.grid_point(pane, rect, x, y) {
                p.write(input::mouse_report(code, true, false, col, row, self.mods, mode));
                self.reported_button = Some(code);
                self.last_report_cell = Some((pane, col, row));
            }
            return;
        }

        match button {
            MouseButton::Left if self.mods.control_key() => {
                // Ctrl+click opens OSC 8 hyperlinks.
                let Some((point, ..)) = self.grid_point(pane, rect, x, y) else {
                    return;
                };
                let uri = self
                    .ws
                    .panes
                    .get(&pane)
                    .and_then(|p| p.term.lock().grid()[point].hyperlink().map(|h| h.uri().to_string()));
                if let Some(uri) = uri
                    && let Err(e) = open_link(&uri)
                {
                    self.show_toast(format!("Could not open link: {e}"));
                }
            }
            MouseButton::Left => {
                let clicks = self.click_count(x, y);
                let Some((point, side, ..)) = self.grid_point(pane, rect, x, y) else {
                    return;
                };
                let ty = match clicks {
                    2 => SelectionType::Semantic,
                    3 => SelectionType::Lines,
                    _ if self.mods.alt_key() => SelectionType::Block,
                    _ => SelectionType::Simple,
                };
                let Some(p) = self.ws.panes.get(&pane) else {
                    return;
                };
                let mut term = p.term.lock();
                if clicks == 1 && self.mods.shift_key() && term.selection.is_some() {
                    if let Some(sel) = term.selection.as_mut() {
                        sel.update(point, side);
                    }
                } else {
                    term.selection = Some(Selection::new(ty, point, side));
                }
                drop(term);
                self.drag = Drag::Selection(pane);
                self.window.request_redraw();
            }
            MouseButton::Middle => self.paste(ClipboardType::Selection),
            MouseButton::Right => self.open_pane_menu(pane, x, y),
            _ => {}
        }
    }

    pub(crate) fn on_mouse_release(&mut self, button: MouseButton) {
        let (x, y) = self.mouse;
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::Selection(pane) => {
                let empty = self
                    .ws
                    .panes
                    .get(&pane)
                    .and_then(|p| p.term.lock().selection.as_ref().map(|s| s.is_empty()))
                    .unwrap_or(true);
                if empty {
                    if let Some(p) = self.ws.panes.get(&pane) {
                        p.term.lock().selection = None;
                    }
                } else {
                    let copy_to_clipboard = self.cfg.copy_on_select;
                    self.copy_selection(!copy_to_clipboard);
                }
                self.window.request_redraw();
            }
            Drag::Tab {
                id,
                dock: Some((pane, dir)),
                ..
            } => self.run_cmd(Cmd::DockTab(id, pane, dir)),
            Drag::Tab { id, moved: true, .. } => {
                // Dropped somewhere that isn't a pane: end up on the dragged tab.
                if let Some(i) = self.ws.tab_index(id) {
                    self.activate_tab(i);
                }
            }
            Drag::Divider(_) | Drag::Tab { .. } => self.window.request_redraw(),
            Drag::None => {}
        }

        if let Some(code) = self.reported_button.take() {
            let pressed_code = match button {
                MouseButton::Left => MouseButtonCode::Left,
                MouseButton::Middle => MouseButtonCode::Middle,
                MouseButton::Right => MouseButtonCode::Right,
                _ => code,
            };
            if let Some((pane, rect)) = self.pane_at(x, y).or_else(|| {
                let id = self.ws.active_tab()?.focused;
                self.pane_rects.iter().find(|(p, _)| *p == id).copied()
            }) && let (Some(p), Some((_, _, col, row))) =
                (self.ws.panes.get(&pane), self.grid_point(pane, rect, x, y))
            {
                let mode = *p.term.lock().mode();
                p.write(input::mouse_report(
                    pressed_code,
                    false,
                    false,
                    col,
                    row,
                    self.mods,
                    mode,
                ));
            }
        }
    }

    pub(crate) fn on_wheel(&mut self, delta: MouseScrollDelta) {
        let (x, y) = self.mouse;
        if let Some(menu) = self.menu.as_mut() {
            // Scroll a long menu; any other wheel action closes it.
            if menu.max_scroll > 0.0 && menu.rect.contains(x, y) {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, dy) => -dy * 3.0 * ui::MENU_ROW * self.scale / 2.0,
                    MouseScrollDelta::PixelDelta(p) => -p.y as f32,
                };
                menu.scroll_by(dy);
                menu.update_hover(x, y, self.scale);
                self.window.request_redraw();
                return;
            }
            self.menu = None;
        }
        if self.settings.is_some() {
            let dy = match delta {
                MouseScrollDelta::LineDelta(_, dy) => -dy * 48.0 * self.scale,
                MouseScrollDelta::PixelDelta(p) => -p.y as f32,
            };
            self.settings_scroll(dy);
            return;
        }
        let lines_f = match delta {
            MouseScrollDelta::LineDelta(_, dy) => dy * 3.0,
            MouseScrollDelta::PixelDelta(p) => p.y as f32 / self.default_cell().1,
        };

        if self.mods.control_key() && self.mods.shift_key() {
            let step = if lines_f > 0.0 { 0.05 } else { -0.05 };
            if lines_f != 0.0 {
                self.set_opacity(self.opacity + step);
            }
            return;
        }
        if self.mods.control_key() {
            if lines_f != 0.0
                && let Some((pane, _)) = self.pane_at(x, y)
            {
                self.change_pane_font(pane, Some(lines_f.signum()));
            }
            return;
        }
        if y < (ui::BAR_H * self.scale) {
            if lines_f.abs() >= 1.0 {
                self.ws.cycle(lines_f < 0.0);
                self.after_structure_change();
            }
            return;
        }

        let Some((pane, rect)) = self.pane_at(x, y) else {
            return;
        };
        let Some((_, _, col, row)) = self.grid_point(pane, rect, x, y) else {
            return;
        };
        let mods = self.mods;
        let Some(p) = self.ws.panes.get_mut(&pane) else {
            return;
        };
        p.scroll_accum += lines_f;
        let lines = p.scroll_accum.trunc() as i32;
        p.scroll_accum -= lines as f32;
        if lines == 0 {
            return;
        }
        let mode = *p.term.lock().mode();
        if mode.intersects(TermMode::MOUSE_MODE) && !mods.shift_key() {
            let code = if lines > 0 {
                MouseButtonCode::WheelUp
            } else {
                MouseButtonCode::WheelDown
            };
            let mut bytes = Vec::new();
            for _ in 0..lines.unsigned_abs() {
                bytes.extend(input::mouse_report(code, true, false, col, row, mods, mode));
            }
            p.write(bytes);
        } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) && !mods.shift_key() {
            let seq: &[u8] = match (lines > 0, mode.contains(TermMode::APP_CURSOR)) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1b[A",
                (false, true) => b"\x1bOB",
                (false, false) => b"\x1b[B",
            };
            p.write(seq.repeat(lines.unsigned_abs() as usize));
        } else {
            p.term.lock().scroll_display(Scroll::Delta(lines));
            self.window.request_redraw();
        }
    }
}

/// Open a file or URL with the desktop's default application.
#[cfg(not(windows))]
fn open_external(target: &std::ffi::OsStr) -> std::io::Result<()> {
    std::process::Command::new("xdg-open").arg(target).spawn().map(drop)
}

/// Open a file or URL with the desktop's default application.
#[cfg(windows)]
fn open_external(target: &std::ffi::OsStr) -> std::io::Result<()> {
    std::process::Command::new("rundll32")
        .arg("url.dll,FileProtocolHandler")
        .arg(target)
        .spawn()
        .map(drop)
}

/// Open a hyperlink a program printed.
#[cfg(not(windows))]
fn open_link(uri: &str) -> std::io::Result<()> {
    open_external(uri.as_ref())
}

/// Open a hyperlink a program printed. Windows would also run a program given its path, so only
/// web and mail links are opened.
#[cfg(windows)]
fn open_link(uri: &str) -> std::io::Result<()> {
    let lower = uri.to_ascii_lowercase();
    if !["http://", "https://", "mailto:"].iter().any(|s| lower.starts_with(s)) {
        return Err(std::io::Error::other("only web and mail links can be opened"));
    }
    open_external(uri.as_ref())
}

fn unix_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn p_is_default(size: f32, default: f32) -> bool {
    (size - default).abs() < f32::EPSILON
}

/// Which edge of `rect` the point is nearest to: where a dropped tab will dock.
fn dock_side(rect: Rect, x: f32, y: f32) -> Dir {
    let fx = ((x - rect.x) / rect.w).clamp(0.0, 1.0);
    let fy = ((y - rect.y) / rect.h).clamp(0.0, 1.0);
    [
        (fx, Dir::Left),
        (1.0 - fx, Dir::Right),
        (fy, Dir::Up),
        (1.0 - fy, Dir::Down),
    ]
    .into_iter()
    .min_by(|a, b| a.0.total_cmp(&b.0))
    .map(|(_, d)| d)
    .unwrap_or(Dir::Right)
}

/// Motion with no button pressed uses button code 3 (+32 for motion).
fn fix_no_button_motion(bytes: &mut Vec<u8>, mode: TermMode) {
    if mode.contains(TermMode::SGR_MOUSE) {
        // "\x1b[<32;..." -> "\x1b[<35;..."
        if let Some(rest) = bytes.strip_prefix(b"\x1b[<") {
            let end = rest.iter().position(|&b| b == b';').unwrap_or(0);
            let code: u8 = std::str::from_utf8(&rest[..end])
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(32);
            let mut out = format!("\x1b[<{}", code + 3).into_bytes();
            out.extend_from_slice(&rest[end..]);
            *bytes = out;
        }
    } else if bytes.len() > 3 {
        bytes[3] += 3;
    }
}
