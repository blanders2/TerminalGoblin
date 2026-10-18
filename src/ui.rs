//! Tab bar layout, context menus and inline renaming: geometry and hit testing.

use crate::config::Action;
use crate::layout::{Axis, Dir};
use crate::pane::PaneId;
use crate::renderer::Rect;
use crate::text::TextSystem;
use crate::theme::{self, Color};
use crate::workspace::{GroupId, TabId, Workspace};

pub const BAR_H: f32 = 40.0;
pub const TAB_TOP: f32 = 6.0;
pub const TAB_MAX_W: f32 = 232.0;
pub const TAB_MIN_W: f32 = 56.0;
pub const UI_FONT: f32 = 12.5;
pub const CHIP_FONT: f32 = 11.5;
pub const BUTTON: f32 = 30.0;
pub const CONTROL_W: f32 = 40.0;
pub const RESIZE_BORDER: f32 = 6.0;
pub const PANE_HEADER: f32 = 24.0;

/// Commands produced by menus and the tab bar.
#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    Action(Action),
    NewTabWithProfile(usize),
    CloseTab(TabId),
    CloseOtherTabs(TabId),
    DuplicateTab(TabId),
    RenameTab(TabId),
    SetTabColor(TabId, Option<Color>),
    NewGroupWithTab(TabId),
    AddTabToGroup(TabId, GroupId),
    RemoveTabFromGroup(TabId),
    RenameGroup(GroupId),
    SetGroupColor(GroupId, Color),
    ToggleGroup(GroupId),
    Ungroup(GroupId),
    CloseGroup(GroupId),
    SplitPane(PaneId, Axis),
    ClosePane(PaneId),
    /// Show tab beside the focused pane of the active tab.
    JoinTab(TabId, Dir),
    /// Move a tab's panes next to a specific pane.
    DockTab(TabId, PaneId, Dir),
    BreakOutPane(PaneId),
    RenamePane(PaneId),
    SetPaneColor(PaneId, Option<Color>),
    SetSetting(crate::settings::Setting, String),
    SetScheme(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    Tab(usize),
    TabClose(usize),
    Group(GroupId),
    NewTab,
    NewTabMenu,
    Minimize,
    Maximize,
    Close,
    DragArea,
}

#[derive(Clone, Debug)]
pub enum BarItem {
    Tab { index: usize, rect: Rect, close: Rect },
    Group { id: GroupId, rect: Rect, label: String },
}

pub struct TabBarLayout {
    pub bar: Rect,
    pub items: Vec<BarItem>,
    pub new_tab: Rect,
    pub dropdown: Rect,
    pub controls: Option<[Rect; 3]>,
}

impl TabBarLayout {
    pub fn tab_rect(&self, index: usize) -> Option<Rect> {
        self.items.iter().find_map(|it| match it {
            BarItem::Tab { index: i, rect, .. } if *i == index => Some(*rect),
            _ => None,
        })
    }

    pub fn group_rect(&self, gid: GroupId) -> Option<Rect> {
        self.items.iter().find_map(|it| match it {
            BarItem::Group { id, rect, .. } if *id == gid => Some(*rect),
            _ => None,
        })
    }

    pub fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        if let Some([min, max, close]) = self.controls {
            if min.contains(x, y) {
                return Some(Hit::Minimize);
            }
            if max.contains(x, y) {
                return Some(Hit::Maximize);
            }
            if close.contains(x, y) {
                return Some(Hit::Close);
            }
        }
        if self.new_tab.contains(x, y) {
            return Some(Hit::NewTab);
        }
        if self.dropdown.contains(x, y) {
            return Some(Hit::NewTabMenu);
        }
        for item in &self.items {
            match item {
                BarItem::Tab { index, rect, close } => {
                    if close.contains(x, y) {
                        return Some(Hit::TabClose(*index));
                    }
                    if rect.contains(x, y) {
                        return Some(Hit::Tab(*index));
                    }
                }
                BarItem::Group { id, rect, .. } => {
                    if rect.contains(x, y) {
                        return Some(Hit::Group(*id));
                    }
                }
            }
        }
        self.bar.contains(x, y).then_some(Hit::DragArea)
    }
}

pub fn group_label(ws: &Workspace, gid: GroupId, edit: Option<&InlineEdit>) -> String {
    if let Some(e) = edit.filter(|e| e.target == EditTarget::Group(gid)) {
        return e.text.clone();
    }
    let Some(g) = ws.group(gid) else {
        return String::new();
    };
    let name = if g.name.trim().is_empty() {
        "Group".to_string()
    } else {
        g.name.clone()
    };
    if g.collapsed {
        let n = ws.tabs.iter().filter(|t| t.group == Some(gid)).count();
        format!("{name}  {n}")
    } else {
        name
    }
}

pub fn layout_tabbar(
    ws: &Workspace,
    text: &mut TextSystem,
    width: f32,
    s: f32,
    controls: bool,
    edit: Option<&InlineEdit>,
) -> TabBarLayout {
    let bar = Rect::new(0.0, 0.0, width, (BAR_H * s).round());
    let controls_w = if controls { CONTROL_W * s * 3.0 + 8.0 * s } else { 0.0 };
    let left = 8.0 * s;
    let buttons_w = (BUTTON + 22.0 + 10.0) * s;
    let right_limit = width - controls_w - buttons_w - 12.0 * s;

    // Sequence of chips and visible tabs.
    enum Seq {
        Chip(GroupId, f32, String),
        Tab(usize),
    }
    let mut seq = Vec::new();
    let mut prev_group = None;
    for (i, tab) in ws.tabs.iter().enumerate() {
        if tab.group != prev_group {
            if let Some(gid) = tab.group {
                let label = group_label(ws, gid, edit);
                let w = (text.measure(&label, CHIP_FONT * s, true) + 22.0 * s).max(28.0 * s);
                seq.push(Seq::Chip(gid, w.round(), label));
            }
            prev_group = tab.group;
        }
        if !ws.is_hidden(tab) {
            seq.push(Seq::Tab(i));
        }
    }
    let gap = 2.0 * s;
    let chips_w: f32 = seq
        .iter()
        .map(|it| if let Seq::Chip(_, w, _) = it { w + 4.0 * s } else { 0.0 })
        .sum();
    let n_tabs = seq.iter().filter(|it| matches!(it, Seq::Tab(_))).count().max(1) as f32;
    let avail = (right_limit - left - chips_w).max(0.0);
    let tab_w = ((avail - gap * (n_tabs - 1.0)) / n_tabs)
        .clamp(TAB_MIN_W * s, TAB_MAX_W * s)
        .floor();

    let top = (TAB_TOP * s).round();
    let mut x = left;
    let mut items = Vec::new();
    for it in seq {
        match it {
            Seq::Chip(id, w, label) => {
                let h = 22.0 * s;
                let rect = Rect::new(x + 2.0 * s, (top + (bar.h - top - h) / 2.0).round(), w, h.round());
                items.push(BarItem::Group { id, rect, label });
                x += w + 4.0 * s;
            }
            Seq::Tab(index) => {
                let rect = Rect::new(x, top, tab_w, bar.h - top);
                let cs = 18.0 * s;
                let close = Rect::new(rect.right() - cs - 8.0 * s, rect.center_y() - cs / 2.0, cs, cs);
                items.push(BarItem::Tab { index, rect, close });
                x += tab_w + gap;
            }
        }
    }
    let bs = BUTTON * s;
    let by = top + (bar.h - top - bs) / 2.0;
    let new_tab = Rect::new(x + 4.0 * s, by, bs, bs);
    let dropdown = Rect::new(new_tab.right(), by, 22.0 * s, bs);
    let controls = controls.then(|| {
        let cw = CONTROL_W * s;
        let ch = 30.0 * s;
        let cy = (bar.h - ch) / 2.0;
        let x0 = width - 8.0 * s - 3.0 * cw;
        [
            Rect::new(x0, cy, cw, ch),
            Rect::new(x0 + cw, cy, cw, ch),
            Rect::new(x0 + 2.0 * cw, cy, cw, ch),
        ]
    });
    TabBarLayout {
        bar,
        items,
        new_tab,
        dropdown,
        controls,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditTarget {
    Tab(TabId),
    Group(GroupId),
    Pane(PaneId),
}

/// Single-line text field used for renaming tabs and groups.
pub struct InlineEdit {
    pub target: EditTarget,
    pub text: String,
    /// Cursor position as a byte index into `text`.
    pub cursor: usize,
    /// The whole text is selected (typing replaces it).
    pub all_selected: bool,
}

impl InlineEdit {
    pub fn new(target: EditTarget, text: String) -> InlineEdit {
        InlineEdit {
            target,
            cursor: text.len(),
            text,
            all_selected: true,
        }
    }

    pub fn insert(&mut self, s: &str) {
        if self.all_selected {
            self.text.clear();
            self.cursor = 0;
            self.all_selected = false;
        }
        let s: String = s.chars().filter(|c| !c.is_control()).collect();
        self.text.insert_str(self.cursor, &s);
        self.cursor += s.len();
    }

    pub fn backspace(&mut self) {
        if self.all_selected {
            self.text.clear();
            self.cursor = 0;
            self.all_selected = false;
            return;
        }
        if let Some((i, _)) = self.text[..self.cursor].char_indices().next_back() {
            self.text.replace_range(i..self.cursor, "");
            self.cursor = i;
        }
    }

    pub fn delete(&mut self) {
        if self.all_selected {
            self.backspace();
            return;
        }
        if let Some(c) = self.text[self.cursor..].chars().next() {
            self.text.replace_range(self.cursor..self.cursor + c.len_utf8(), "");
        }
    }

    pub fn left(&mut self) {
        self.all_selected = false;
        if let Some((i, _)) = self.text[..self.cursor].char_indices().next_back() {
            self.cursor = i;
        }
    }

    pub fn right(&mut self) {
        self.all_selected = false;
        if let Some(c) = self.text[self.cursor..].chars().next() {
            self.cursor += c.len_utf8();
        }
    }

    pub fn home(&mut self) {
        self.all_selected = false;
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.all_selected = false;
        self.cursor = self.text.len();
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SwatchTarget {
    Tab(TabId),
    Group(GroupId),
    Pane(PaneId),
}

#[derive(Clone, Debug)]
pub enum MenuEntry {
    Item {
        label: String,
        hint: String,
        cmd: Cmd,
        danger: bool,
        checked: bool,
    },
    Separator,
    Label(String),
    Swatches {
        target: SwatchTarget,
        current: Option<Color>,
    },
}

impl MenuEntry {
    pub fn item(label: impl Into<String>, hint: impl Into<String>, cmd: Cmd) -> MenuEntry {
        MenuEntry::Item {
            label: label.into(),
            hint: hint.into(),
            cmd,
            danger: false,
            checked: false,
        }
    }
    /// An option in a picker, with a check mark when it's the current choice.
    pub fn choice(label: impl Into<String>, checked: bool, cmd: Cmd) -> MenuEntry {
        MenuEntry::Item {
            label: label.into(),
            hint: String::new(),
            cmd,
            danger: false,
            checked,
        }
    }
    pub fn danger(label: impl Into<String>, hint: impl Into<String>, cmd: Cmd) -> MenuEntry {
        MenuEntry::Item {
            label: label.into(),
            hint: hint.into(),
            cmd,
            danger: true,
            checked: false,
        }
    }
}

pub struct Menu {
    pub rect: Rect,
    pub entries: Vec<MenuEntry>,
    /// Row rectangles when unscrolled; use [`Menu::row`] for on-screen positions.
    pub rows: Vec<Rect>,
    pub scroll: f32,
    pub max_scroll: f32,
    pub hovered: Option<usize>,
    pub hovered_swatch: Option<usize>,
}

pub enum MenuHit {
    Entry(usize),
    Swatch(usize, Option<Color>),
    Inside,
    Outside,
}

pub const MENU_ROW: f32 = 30.0;
pub const MENU_SEP: f32 = 9.0;
pub const MENU_SWATCH_ROW: f32 = 36.0;
pub const SWATCH: f32 = 18.0;
pub const SWATCH_GAP: f32 = 6.0;

pub fn swatch_colors(target: SwatchTarget) -> Vec<Option<Color>> {
    let mut v: Vec<Option<Color>> = Vec::new();
    if !matches!(target, SwatchTarget::Group(_)) {
        v.push(None);
    }
    v.extend(theme::TAB_COLORS.iter().map(|(_, h)| Some(theme::c(h))));
    v
}

impl Menu {
    pub fn new(entries: Vec<MenuEntry>, x: f32, y: f32, text: &mut TextSystem, s: f32, win: (f32, f32)) -> Menu {
        let pad = 6.0 * s;
        let mut width: f32 = 220.0 * s;
        for e in &entries {
            match e {
                MenuEntry::Item { label, hint, .. } => {
                    let w = text.measure(label, UI_FONT * s, false) + text.measure(hint, (UI_FONT - 1.0) * s, false);
                    width = width.max(w + 56.0 * s);
                }
                MenuEntry::Label(l) => width = width.max(text.measure(l, (UI_FONT - 1.0) * s, true) + 32.0 * s),
                MenuEntry::Swatches { target, .. } => {
                    let n = swatch_colors(*target).len() as f32;
                    width = width.max(n * (SWATCH + SWATCH_GAP) * s + 24.0 * s);
                }
                MenuEntry::Separator => {}
            }
        }
        let mut rows = Vec::new();
        let mut cy = pad;
        for e in &entries {
            let h = match e {
                MenuEntry::Separator => MENU_SEP,
                MenuEntry::Swatches { .. } => MENU_SWATCH_ROW,
                MenuEntry::Label(_) => 26.0,
                MenuEntry::Item { .. } => MENU_ROW,
            } * s;
            rows.push(Rect::new(pad, cy, width - 2.0 * pad, h));
            cy += h;
        }
        let content_h = cy + pad;
        let height = content_h.min(win.1 - 8.0 * s);
        let x = x.min(win.0 - width - 4.0 * s).max(4.0 * s).round();
        let y = if y + height > win.1 - 4.0 * s {
            (y - height).max(4.0 * s)
        } else {
            y
        }
        .round();
        for r in &mut rows {
            r.x += x;
            r.y += y;
        }
        let mut menu = Menu {
            rect: Rect::new(x, y, width, height),
            entries,
            rows,
            scroll: 0.0,
            max_scroll: (content_h - height).max(0.0),
            hovered: None,
            hovered_swatch: None,
        };
        // Long pickers open scrolled to the current choice.
        if let Some(i) = menu
            .entries
            .iter()
            .position(|e| matches!(e, MenuEntry::Item { checked: true, .. }))
        {
            let r = menu.rows[i];
            menu.scroll_by(r.center_y() - (menu.rect.y + menu.rect.h / 2.0));
        }
        menu
    }

    pub fn row(&self, i: usize) -> Rect {
        let r = self.rows[i];
        Rect::new(r.x, r.y - self.scroll, r.w, r.h)
    }

    pub fn scroll_by(&mut self, dy: f32) {
        self.scroll = (self.scroll + dy).clamp(0.0, self.max_scroll);
    }

    fn scroll_into_view(&mut self, i: usize) {
        let r = self.row(i);
        if r.y < self.rect.y {
            self.scroll_by(r.y - self.rect.y - 6.0);
        } else if r.bottom() > self.rect.bottom() {
            self.scroll_by(r.bottom() - self.rect.bottom() + 6.0);
        }
    }

    pub fn swatch_rects(&self, row: Rect, target: SwatchTarget, s: f32) -> Vec<(Rect, Option<Color>)> {
        let size = SWATCH * s;
        let gap = SWATCH_GAP * s;
        swatch_colors(target)
            .into_iter()
            .enumerate()
            .map(|(i, c)| {
                let x = row.x + 6.0 * s + i as f32 * (size + gap);
                (
                    Rect::new(x.round(), (row.center_y() - size / 2.0).round(), size, size),
                    c,
                )
            })
            .collect()
    }

    pub fn hit(&self, x: f32, y: f32, s: f32) -> MenuHit {
        if !self.rect.contains(x, y) {
            return MenuHit::Outside;
        }
        for (i, e) in self.entries.iter().enumerate() {
            let row = self.row(i);
            if !row.contains(x, y) {
                continue;
            }
            match e {
                MenuEntry::Item { .. } => return MenuHit::Entry(i),
                MenuEntry::Swatches { target, .. } => {
                    for (j, (r, c)) in self.swatch_rects(row, *target, s).into_iter().enumerate() {
                        if r.inset(-2.0 * s).contains(x, y) {
                            let _ = j;
                            return MenuHit::Swatch(i, c);
                        }
                    }
                }
                _ => {}
            }
        }
        MenuHit::Inside
    }

    pub fn update_hover(&mut self, x: f32, y: f32, s: f32) -> bool {
        let (h, sw) = match self.hit(x, y, s) {
            MenuHit::Entry(i) => (Some(i), None),
            MenuHit::Swatch(i, c) => {
                let idx = match &self.entries[i] {
                    MenuEntry::Swatches { target, .. } => swatch_colors(*target).iter().position(|x| *x == c),
                    _ => None,
                };
                (Some(i), idx)
            }
            _ => (None, None),
        };
        let changed = h != self.hovered || sw != self.hovered_swatch;
        self.hovered = h;
        self.hovered_swatch = sw;
        changed
    }

    /// Keyboard navigation over selectable items.
    pub fn step(&mut self, forward: bool) {
        let selectable: Vec<usize> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| matches!(e, MenuEntry::Item { .. }))
            .map(|(i, _)| i)
            .collect();
        if selectable.is_empty() {
            return;
        }
        let pos = self.hovered.and_then(|h| selectable.iter().position(|&i| i == h));
        let next = match (pos, forward) {
            (None, true) => 0,
            (None, false) => selectable.len() - 1,
            (Some(p), true) => (p + 1) % selectable.len(),
            (Some(p), false) => (p + selectable.len() - 1) % selectable.len(),
        };
        self.hovered = Some(selectable[next]);
        self.hovered_swatch = None;
        self.scroll_into_view(selectable[next]);
    }
}
