//! The settings window: appearance, terminal behaviour and keybindings, saved to the config file.

use winit::keyboard::{Key, ModifiersState, NamedKey};

use crate::app::State;
use crate::config::{self, Action, KeyCombo};
use crate::renderer::{Instance, Rect};
use crate::theme::{self, alpha, mix};
use crate::ui::{self, Cmd, MenuEntry};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Appearance,
    Terminal,
    Keys,
}

impl Page {
    const ALL: [Page; 3] = [Page::Appearance, Page::Terminal, Page::Keys];
    fn title(self) -> &'static str {
        match self {
            Page::Appearance => "Appearance",
            Page::Terminal => "Terminal",
            Page::Keys => "Keybindings",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    Scheme,
    FontFamily,
    UiFont,
    FontSize,
    LineHeight,
    Opacity,
    TabBarOpacity,
    Blur,
    Padding,
    PaneHeaders,
    UnfocusedDim,
    Decorations,
    CursorStyle,
    CursorBlink,
    Scrollback,
    CopyOnSelect,
    BoldBright,
    DefaultProfile,
}

pub struct Settings {
    pub page: Page,
    pub scroll: f32,
    /// Keybinding being recorded: the action, and the shortcut it replaces (None when adding).
    pub capture: Option<(Action, Option<KeyCombo>)>,
    /// Slider being dragged; the config is saved when it's released.
    pub slider: Option<Setting>,
    pub hover: Option<Hit>,
}

impl Settings {
    pub fn new() -> Settings {
        Settings {
            page: Page::Appearance,
            scroll: 0.0,
            capture: None,
            slider: None,
            hover: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Hit {
    Backdrop,
    Panel,
    Close,
    Nav(Page),
    OpenFile,
    ResetAllKeys,
    Slider(Setting, Rect),
    Toggle(Setting),
    Segment(Setting, &'static str),
    Select(Setting, Rect),
    Chip(Action, KeyCombo),
    ChipRemove(Action, KeyCombo),
    AddChip(Action),
    ResetAction(Action),
}

enum Control {
    Slider {
        min: f32,
        max: f32,
        step: f32,
        value: f32,
        text: String,
    },
    Toggle(bool),
    Segments(&'static [(&'static str, &'static str)], String),
    Select(String),
}

enum Item {
    Section(&'static str),
    Row {
        label: &'static str,
        hint: &'static str,
        setting: Setting,
        control: Control,
    },
    Keys {
        label: String,
        action: Action,
    },
    KeysHeader,
}

/// Geometry shared by drawing and hit testing.
struct Layout {
    panel: Rect,
    sidebar: Rect,
    content: Rect,
    close: Rect,
    open_file: Rect,
    nav: Vec<(Page, Rect)>,
    /// Items with their (scrolled) rectangles.
    items: Vec<(Item, Rect)>,
    content_h: f32,
}

const ROW_H: f32 = 50.0;
const SECTION_H: f32 = 38.0;
const KEY_ROW_H: f32 = 38.0;
const CONTROL_W: f32 = 270.0;

const CURSOR_STYLES: &[(&str, &str)] = &[("block", "Block"), ("beam", "Beam"), ("underline", "Underline")];
const PANE_HEADERS: &[(&str, &str)] = &[("auto", "Auto"), ("always", "Always"), ("never", "Never")];
const DECORATIONS: &[(&str, &str)] = &[("client", "Tabs in titlebar"), ("system", "System")];

impl State {
    // ---------------------------------------------------------------------------------------------
    // Model

    fn items(&self, page: Page) -> Vec<Item> {
        let c = &self.cfg;
        let slider = |min: f32, max: f32, step: f32, value: f32, text: String| Control::Slider {
            min,
            max,
            step,
            value,
            text,
        };
        let pct = |v: f32| format!("{:.0}%", v * 100.0);
        let row = |label, hint, setting, control| Item::Row {
            label,
            hint,
            setting,
            control,
        };
        match page {
            Page::Appearance => vec![
                Item::Section("Colors"),
                row(
                    "Color scheme",
                    "",
                    Setting::Scheme,
                    Control::Select(c.color_scheme.clone()),
                ),
                Item::Section("Text"),
                row(
                    "Terminal font",
                    "",
                    Setting::FontFamily,
                    Control::Select(c.font_family.clone()),
                ),
                row(
                    "Text size",
                    "Default for new panes. Ctrl+= / Ctrl+- resize a single pane.",
                    Setting::FontSize,
                    slider(6.0, 36.0, 0.5, c.font_size, format!("{} px", c.font_size)),
                ),
                row(
                    "Line height",
                    "",
                    Setting::LineHeight,
                    slider(0.8, 2.0, 0.05, c.line_height, format!("{:.2}×", c.line_height)),
                ),
                row(
                    "Interface font",
                    "Tabs, menus and headers",
                    Setting::UiFont,
                    Control::Select(c.ui_font_family.clone()),
                ),
                Item::Section("Window"),
                row(
                    "Opacity",
                    "",
                    Setting::Opacity,
                    slider(0.1, 1.0, 0.01, c.opacity, pct(c.opacity)),
                ),
                row(
                    "Tab bar opacity",
                    "",
                    Setting::TabBarOpacity,
                    slider(0.1, 1.0, 0.01, c.tab_bar_opacity(), pct(c.tab_bar_opacity())),
                ),
                row(
                    "Background blur",
                    "If the compositor supports it",
                    Setting::Blur,
                    Control::Toggle(c.blur),
                ),
                row(
                    "Padding",
                    "",
                    Setting::Padding,
                    slider(0.0, 32.0, 1.0, c.padding, format!("{} px", c.padding)),
                ),
                row(
                    "Decorations",
                    "",
                    Setting::Decorations,
                    Control::Segments(
                        DECORATIONS,
                        if c.client_decorations() { "client" } else { "system" }.into(),
                    ),
                ),
                Item::Section("Panes"),
                row(
                    "Pane headers",
                    "Auto shows them once a pane has a name or color",
                    Setting::PaneHeaders,
                    Control::Segments(PANE_HEADERS, c.pane_headers.clone()),
                ),
                row(
                    "Dim unfocused panes",
                    "",
                    Setting::UnfocusedDim,
                    slider(0.0, 0.6, 0.01, c.unfocused_pane_dim, pct(c.unfocused_pane_dim)),
                ),
            ],
            Page::Terminal => vec![
                Item::Section("Cursor"),
                row(
                    "Cursor shape",
                    "",
                    Setting::CursorStyle,
                    Control::Segments(CURSOR_STYLES, c.cursor_style.clone()),
                ),
                row(
                    "Blinking cursor",
                    "",
                    Setting::CursorBlink,
                    Control::Toggle(c.cursor_blink),
                ),
                Item::Section("Behavior"),
                row(
                    "Default shell",
                    "Used for new tabs",
                    Setting::DefaultProfile,
                    Control::Select(
                        self.profiles
                            .get(c.default_profile_index(&self.profiles))
                            .map(|p| p.name.clone())
                            .unwrap_or_default(),
                    ),
                ),
                row(
                    "Scrollback",
                    "",
                    Setting::Scrollback,
                    slider(
                        1000.0,
                        100_000.0,
                        1000.0,
                        c.scrollback as f32,
                        format!("{} lines", group_digits(c.scrollback)),
                    ),
                ),
                row(
                    "Copy on select",
                    "Also copy selections to the clipboard",
                    Setting::CopyOnSelect,
                    Control::Toggle(c.copy_on_select),
                ),
                row(
                    "Bold text in bright colors",
                    "",
                    Setting::BoldBright,
                    Control::Toggle(c.bold_is_bright),
                ),
            ],
            Page::Keys => {
                let mut v = vec![Item::KeysHeader];
                let mut section = "";
                for (sec, action, label) in config::action_catalog() {
                    if sec != section {
                        v.push(Item::Section(sec));
                        section = sec;
                    }
                    v.push(Item::Keys { label, action });
                }
                v
            }
        }
    }

    fn settings_layout(&self) -> Option<Layout> {
        let st = self.settings.as_ref()?;
        let s = self.scale;
        let (w, h) = (self.size.width as f32, self.size.height as f32);
        let pw = (860.0 * s).min(w - 32.0 * s);
        let ph = (600.0 * s).min(h - 32.0 * s);
        let panel = Rect::new(((w - pw) / 2.0).round(), ((h - ph) / 2.0).round(), pw, ph);
        let sidebar = Rect::new(panel.x, panel.y, (190.0 * s).round(), panel.h);
        let content = Rect::new(
            sidebar.right(),
            panel.y + 52.0 * s,
            panel.right() - sidebar.right(),
            panel.h - 52.0 * s,
        );
        let close = Rect::new(panel.right() - 44.0 * s, panel.y + 10.0 * s, 32.0 * s, 32.0 * s);
        let nav = Page::ALL
            .iter()
            .enumerate()
            .map(|(i, p)| {
                (
                    *p,
                    Rect::new(
                        sidebar.x + 10.0 * s,
                        sidebar.y + (64.0 + i as f32 * 38.0) * s,
                        sidebar.w - 20.0 * s,
                        34.0 * s,
                    ),
                )
            })
            .collect();
        let open_file = Rect::new(
            sidebar.x + 10.0 * s,
            sidebar.bottom() - 46.0 * s,
            sidebar.w - 20.0 * s,
            34.0 * s,
        );

        let mut y = content.y + 4.0 * s - st.scroll;
        let mut items = Vec::new();
        let x = content.x + 24.0 * s;
        let iw = content.w - 48.0 * s;
        for item in self.items(st.page) {
            let ih = match item {
                Item::Section(_) => SECTION_H,
                Item::Row { hint, .. } => {
                    if hint.is_empty() {
                        ROW_H - 6.0
                    } else {
                        ROW_H + 4.0
                    }
                }
                Item::Keys { .. } => KEY_ROW_H,
                Item::KeysHeader => 56.0,
            } * s;
            items.push((item, Rect::new(x, y, iw, ih)));
            y += ih;
        }
        let content_h = y + st.scroll - content.y + 16.0 * s;
        Some(Layout {
            panel,
            sidebar,
            content,
            close,
            open_file,
            nav,
            items,
            content_h,
        })
    }

    /// Control geometry for a row: the area right-aligned in the row.
    fn control_rect(&self, row: Rect) -> Rect {
        let s = self.scale;
        let w = (CONTROL_W * s).min(row.w * 0.55);
        Rect::new(row.right() - w, row.center_y() - 15.0 * s, w, 30.0 * s)
    }

    /// Key chip rectangles for an action row: (combo, chip, remove button), then the add button.
    fn key_chips(&mut self, action: Action, row: Rect) -> (Vec<(KeyCombo, Rect, Rect)>, Rect, Option<Rect>) {
        let s = self.scale;
        let font = (ui::UI_FONT - 1.0) * s;
        let capture = self.settings.as_ref().and_then(|st| st.capture.clone());
        let combos: Vec<KeyCombo> = self
            .keybindings
            .iter()
            .filter(|(_, a)| *a == action)
            .map(|(k, _)| k.clone())
            .collect();
        let mut x = row.x + (row.w * 0.42).max(220.0 * s);
        let h = 26.0 * s;
        let y = row.center_y() - h / 2.0;
        let mut chips = Vec::new();
        for k in combos {
            let label = match &capture {
                Some((a, Some(old))) if *a == action && *old == k => "Press keys…".to_string(),
                _ => k.display(),
            };
            let w = (self.text.measure(&label, font, false) + 34.0 * s).round();
            let chip = Rect::new(x, y, w, h);
            let remove = Rect::new(chip.right() - 22.0 * s, y + 3.0 * s, 20.0 * s, h - 6.0 * s);
            chips.push((k, chip, remove));
            x += w + 6.0 * s;
        }
        let adding = matches!(&capture, Some((a, None)) if *a == action);
        let add_w = if adding {
            self.text.measure("Press keys…", font, false) + 20.0 * s
        } else {
            h
        };
        let add = Rect::new(x, y, add_w.round(), h);
        let defaults: Vec<KeyCombo> = self
            .default_bindings
            .iter()
            .filter(|(_, a)| *a == action)
            .map(|(k, _)| k.clone())
            .collect();
        let mut current: Vec<KeyCombo> = self
            .keybindings
            .iter()
            .filter(|(_, a)| *a == action)
            .map(|(k, _)| k.clone())
            .collect();
        let mut defaults_sorted = defaults;
        current.sort_by_key(|k| k.to_config());
        defaults_sorted.sort_by_key(|k| k.to_config());
        let reset = (current != defaults_sorted).then(|| Rect::new(row.right() - 54.0 * s, y, 54.0 * s, h));
        (chips, add, reset)
    }

    fn settings_hit(&mut self, x: f32, y: f32) -> Option<Hit> {
        let lay = self.settings_layout()?;
        if !lay.panel.contains(x, y) {
            return Some(Hit::Backdrop);
        }
        if lay.close.contains(x, y) {
            return Some(Hit::Close);
        }
        for (p, r) in &lay.nav {
            if r.contains(x, y) {
                return Some(Hit::Nav(*p));
            }
        }
        if lay.open_file.contains(x, y) {
            return Some(Hit::OpenFile);
        }
        if !lay.content.contains(x, y) {
            return Some(Hit::Panel);
        }
        for (item, rect) in &lay.items {
            if !rect.contains(x, y) {
                continue;
            }
            match item {
                Item::Row { setting, control, .. } => {
                    let cr = self.control_rect(*rect);
                    match control {
                        Control::Slider { .. } => {
                            let track = slider_track(cr, self.scale);
                            if Rect::new(track.x - 8.0, cr.y, track.w + 16.0, cr.h).contains(x, y) {
                                return Some(Hit::Slider(*setting, track));
                            }
                        }
                        Control::Toggle(_) => {
                            if toggle_rect(cr, self.scale).inset(-4.0).contains(x, y) {
                                return Some(Hit::Toggle(*setting));
                            }
                        }
                        Control::Segments(opts, _) => {
                            for (i, (value, _)) in opts.iter().enumerate() {
                                if segment_rect(cr, i, opts.len()).contains(x, y) {
                                    return Some(Hit::Segment(*setting, value));
                                }
                            }
                        }
                        Control::Select(_) => {
                            if cr.contains(x, y) {
                                return Some(Hit::Select(*setting, cr));
                            }
                        }
                    }
                }
                Item::Keys { action, .. } => {
                    let (chips, add, reset) = self.key_chips(*action, *rect);
                    for (k, chip, remove) in chips {
                        if remove.contains(x, y) {
                            return Some(Hit::ChipRemove(*action, k));
                        }
                        if chip.contains(x, y) {
                            return Some(Hit::Chip(*action, k));
                        }
                    }
                    if add.contains(x, y) {
                        return Some(Hit::AddChip(*action));
                    }
                    if reset.is_some_and(|r| r.contains(x, y)) {
                        return Some(Hit::ResetAction(*action));
                    }
                }
                Item::KeysHeader => {
                    if reset_all_rect(*rect, self.scale).contains(x, y) {
                        return Some(Hit::ResetAllKeys);
                    }
                }
                Item::Section(_) => {}
            }
        }
        Some(Hit::Panel)
    }

    // ---------------------------------------------------------------------------------------------
    // Changing settings

    /// `--demo-settings <page>`: open the window on a page, optionally recording a shortcut.
    pub(crate) fn demo_settings(&mut self, page: &str) {
        self.open_settings();
        if let Some(st) = self.settings.as_mut() {
            st.page = match page {
                "terminal" => Page::Terminal,
                "keys" | "keys-capture" => Page::Keys,
                _ => Page::Appearance,
            };
            if page == "keys-capture" {
                st.capture = Some((Action::NewTab, None));
            }
        }
        if page == "fonts"
            && let Some(lay) = self.settings_layout()
            && let Some(rect) = lay.items.iter().find_map(|(item, r)| match item {
                Item::Row {
                    setting: Setting::FontFamily,
                    ..
                } => Some(*r),
                _ => None,
            })
        {
            let cr = self.control_rect(rect);
            self.open_picker(Setting::FontFamily, cr);
        }
    }

    pub(crate) fn open_settings(&mut self) {
        self.menu = None;
        self.edit = None;
        self.settings = Some(Settings::new());
        self.window.request_redraw();
    }

    pub(crate) fn close_settings(&mut self) {
        if let Some(st) = self.settings.take()
            && st.slider.is_some()
        {
            self.save_settings();
        }
        self.window.request_redraw();
    }

    /// Persist the current config and keybindings to disk.
    pub(crate) fn save_settings(&mut self) {
        self.cfg.keybindings = config::binding_overrides(&self.keybindings).into_iter().collect();
        match config::save(&self.cfg, &self.keybindings) {
            Ok(text) => self.last_saved = Some(text),
            Err(e) => self.show_toast(format!("Couldn't save settings: {e}")),
        }
    }

    fn set_slider(&mut self, setting: Setting, track: Rect, x: f32) {
        let Some((min, max, step)) = self.items_slider_range(setting) else {
            return;
        };
        let t = ((x - track.x) / track.w).clamp(0.0, 1.0);
        let v = ((min + t * (max - min)) / step).round() * step;
        let v = v.clamp(min, max);
        let c = &mut self.cfg;
        match setting {
            Setting::FontSize => c.font_size = v,
            Setting::LineHeight => c.line_height = v,
            Setting::Opacity => c.opacity = v,
            Setting::TabBarOpacity => c.tab_bar_opacity = Some(v),
            Setting::Padding => c.padding = v,
            Setting::UnfocusedDim => c.unfocused_pane_dim = v,
            Setting::Scrollback => c.scrollback = v as usize,
            _ => return,
        }
        self.apply_setting(setting);
    }

    fn items_slider_range(&self, setting: Setting) -> Option<(f32, f32, f32)> {
        let page = self.settings.as_ref()?.page;
        self.items(page).into_iter().find_map(|item| match item {
            Item::Row {
                setting: s,
                control: Control::Slider { min, max, step, .. },
                ..
            } if s == setting => Some((min, max, step)),
            _ => None,
        })
    }

    fn toggle(&mut self, setting: Setting) {
        let c = &mut self.cfg;
        match setting {
            Setting::Blur => c.blur = !c.blur,
            Setting::CursorBlink => c.cursor_blink = !c.cursor_blink,
            Setting::CopyOnSelect => c.copy_on_select = !c.copy_on_select,
            Setting::BoldBright => c.bold_is_bright = !c.bold_is_bright,
            _ => return,
        }
        self.apply_setting(setting);
        self.save_settings();
    }

    /// Set a text-valued setting (segments and pickers).
    pub(crate) fn set_setting(&mut self, setting: Setting, value: String) {
        let c = &mut self.cfg;
        match setting {
            Setting::Scheme => c.color_scheme = value,
            Setting::FontFamily => c.font_family = value,
            Setting::UiFont => c.ui_font_family = value,
            Setting::PaneHeaders => c.pane_headers = value,
            Setting::Decorations => c.decorations = value,
            Setting::CursorStyle => c.cursor_style = value,
            Setting::DefaultProfile => c.default_profile = Some(value),
            _ => return,
        }
        self.apply_setting(setting);
        self.save_settings();
    }

    /// Make a changed setting take effect immediately.
    fn apply_setting(&mut self, setting: Setting) {
        match setting {
            Setting::Scheme => {
                self.scheme = self.cfg.scheme(None);
                self.ui = theme::UiPalette::from_scheme(&self.scheme);
            }
            Setting::FontFamily | Setting::UiFont | Setting::LineHeight => {
                self.text
                    .set_fonts(&self.cfg.font_family, &self.cfg.ui_font_family, self.cfg.line_height);
            }
            Setting::FontSize => self.font_size = self.cfg.font_size,
            Setting::Opacity => {
                self.opacity = self.cfg.opacity;
                self.transparency_on = self.renderer.supports_transparency;
            }
            Setting::Blur => self.window.set_blur(self.cfg.blur),
            Setting::Decorations => self.window.set_decorations(!self.cfg.client_decorations()),
            Setting::CursorStyle | Setting::CursorBlink | Setting::Scrollback => {
                let tc = crate::pane::term_config(&self.cfg);
                for p in self.ws.panes.values() {
                    p.term.lock().set_options(tc.clone());
                }
            }
            _ => {}
        }
        self.after_structure_change();
    }

    fn open_picker(&mut self, setting: Setting, at: Rect) {
        let current = match setting {
            Setting::Scheme => self.cfg.color_scheme.clone(),
            Setting::FontFamily => self.text_family_current(true),
            Setting::UiFont => self.text_family_current(false),
            Setting::DefaultProfile => self
                .profiles
                .get(self.cfg.default_profile_index(&self.profiles))
                .map(|p| p.name.clone())
                .unwrap_or_default(),
            _ => return,
        };
        let options: Vec<String> = match setting {
            Setting::Scheme => self.schemes.iter().map(|s| s.name.clone()).collect(),
            Setting::FontFamily => self.text.families(true),
            Setting::UiFont => self.text.families(false),
            Setting::DefaultProfile => self.profiles.iter().map(|p| p.name.clone()).collect(),
            _ => return,
        };
        let entries = options
            .into_iter()
            .map(|o| {
                let checked = o.eq_ignore_ascii_case(&current);
                MenuEntry::choice(o.clone(), checked, Cmd::SetSetting(setting, o))
            })
            .collect();
        self.open_menu(entries, at.x, at.bottom() + 4.0 * self.scale);
        if let Some(m) = self.menu.as_mut() {
            m.rect.w = m.rect.w.max(at.w);
        }
    }

    fn text_family_current(&self, terminal: bool) -> String {
        if terminal {
            self.cfg.font_family.clone()
        } else {
            self.cfg.ui_font_family.clone()
        }
    }

    fn remove_binding(&mut self, action: Action, combo: &KeyCombo) {
        self.keybindings.retain(|(k, a)| !(k == combo && *a == action));
        self.save_settings();
    }

    fn reset_action(&mut self, action: Action) {
        self.keybindings.retain(|(_, a)| *a != action);
        let defaults: Vec<(KeyCombo, Action)> = self
            .default_bindings
            .iter()
            .filter(|(_, a)| *a == action)
            .cloned()
            .collect();
        for (k, a) in defaults {
            // A default shortcut that was reassigned elsewhere comes back to this action.
            self.keybindings.retain(|(bk, _)| *bk != k);
            self.keybindings.push((k, a));
        }
        self.save_settings();
    }

    /// Finish recording a shortcut. Returns false if the key can't be used and recording continues.
    fn capture_key(&mut self, key: &Key, unmodified: &Key, mods: ModifiersState) -> bool {
        let Some((action, old)) = self.settings.as_ref().and_then(|s| s.capture.clone()) else {
            return true;
        };
        let plain = !(mods.control_key() || mods.alt_key() || mods.super_key() || mods.shift_key());
        match key {
            Key::Named(NamedKey::Escape) if plain => return true,
            Key::Named(NamedKey::Backspace | NamedKey::Delete) if plain => {
                if let Some(old) = old {
                    self.remove_binding(action, &old);
                }
                return true;
            }
            _ => {}
        }
        let Some(combo) = KeyCombo::from_key(mods, unmodified) else {
            return false; // a lone modifier: keep waiting
        };
        let is_text = combo.key.chars().count() == 1 || combo.key == "space";
        if is_text && !(combo.ctrl || combo.alt || combo.logo) {
            self.show_toast("Add Ctrl, Alt or Super: plain keys are needed for typing".into());
            return false;
        }
        let catalog = config::action_catalog();
        let label = |a: Action| {
            catalog
                .iter()
                .find(|(_, x, _)| *x == a)
                .map(|(_, _, l)| l.clone())
                .unwrap_or_default()
        };
        if let Some((_, prev)) = self
            .keybindings
            .iter()
            .find(|(k, a)| *k == combo && *a != action)
            .cloned()
        {
            self.show_toast(format!(
                "{} moved from \"{}\" to \"{}\"",
                combo.display(),
                label(prev),
                label(action)
            ));
        }
        self.keybindings.retain(|(k, _)| *k != combo);
        match old {
            Some(old) => {
                if let Some(entry) = self.keybindings.iter_mut().find(|(k, a)| *k == old && *a == action) {
                    entry.0 = combo;
                } else {
                    self.keybindings.push((combo, action));
                }
            }
            None => self.keybindings.push((combo, action)),
        }
        self.save_settings();
        true
    }

    // ---------------------------------------------------------------------------------------------
    // Input

    /// Keyboard input while the settings window is open. Returns true when it was consumed.
    pub(crate) fn settings_key(&mut self, key: &Key, unmodified: &Key, mods: ModifiersState) -> bool {
        let Some(st) = self.settings.as_ref() else { return false };
        if st.capture.is_some() {
            if self.capture_key(key, unmodified, mods)
                && let Some(st) = self.settings.as_mut()
            {
                st.capture = None;
            }
            self.window.request_redraw();
            return true;
        }
        if matches!(key, Key::Named(NamedKey::Escape)) {
            self.close_settings();
            return true;
        }
        // Let the settings shortcut close the window again; swallow everything else.
        let opens_settings = self
            .keybindings
            .iter()
            .any(|(k, a)| *a == Action::OpenSettings && k.matches(mods, unmodified));
        if opens_settings {
            self.close_settings();
        }
        true
    }

    pub(crate) fn settings_mouse_move(&mut self, x: f32, y: f32) {
        if let Some(setting) = self.settings.as_ref().and_then(|s| s.slider) {
            if let Some(track) = self.slider_track_for(setting) {
                self.set_slider(setting, track, x);
            }
            return;
        }
        let hit = self.settings_hit(x, y);
        if let Some(st) = self.settings.as_mut()
            && st.hover != hit
        {
            st.hover = hit;
            self.window.request_redraw();
        }
    }

    fn slider_track_for(&self, setting: Setting) -> Option<Rect> {
        let lay = self.settings_layout()?;
        lay.items.iter().find_map(|(item, rect)| match item {
            Item::Row { setting: s, .. } if *s == setting => Some(slider_track(self.control_rect(*rect), self.scale)),
            _ => None,
        })
    }

    pub(crate) fn settings_mouse_button(&mut self, pressed: bool) {
        let (x, y) = self.mouse;
        if !pressed {
            if let Some(st) = self.settings.as_mut()
                && st.slider.take().is_some()
            {
                self.save_settings();
            }
            return;
        }
        let Some(hit) = self.settings_hit(x, y) else { return };
        // Clicking anywhere cancels a pending shortcut recording.
        let was_capturing = self.settings.as_mut().and_then(|s| s.capture.take()).is_some();
        match hit {
            Hit::Backdrop | Hit::Close => self.close_settings(),
            Hit::Nav(p) => {
                if let Some(st) = self.settings.as_mut() {
                    st.page = p;
                    st.scroll = 0.0;
                }
            }
            Hit::OpenFile => self.open_config_file(),
            Hit::ResetAllKeys => {
                self.keybindings = self.default_bindings.clone();
                self.save_settings();
                self.show_toast("Keybindings reset to defaults".into());
            }
            Hit::Slider(setting, track) => {
                if let Some(st) = self.settings.as_mut() {
                    st.slider = Some(setting);
                }
                self.set_slider(setting, track, x);
            }
            Hit::Toggle(setting) => self.toggle(setting),
            Hit::Segment(setting, value) => self.set_setting(setting, value.to_string()),
            Hit::Select(setting, rect) => self.open_picker(setting, rect),
            Hit::Chip(action, combo) => {
                if !was_capturing && let Some(st) = self.settings.as_mut() {
                    st.capture = Some((action, Some(combo)));
                }
            }
            Hit::ChipRemove(action, combo) => self.remove_binding(action, &combo),
            Hit::AddChip(action) => {
                if !was_capturing && let Some(st) = self.settings.as_mut() {
                    st.capture = Some((action, None));
                }
            }
            Hit::ResetAction(action) => self.reset_action(action),
            Hit::Panel => {}
        }
        self.window.request_redraw();
    }

    pub(crate) fn settings_scroll(&mut self, dy: f32) {
        let max = self
            .settings_layout()
            .map(|l| (l.content_h - l.content.h).max(0.0))
            .unwrap_or(0.0);
        if let Some(st) = self.settings.as_mut() {
            st.scroll = (st.scroll + dy).clamp(0.0, max);
            st.hover = None;
        }
        self.window.request_redraw();
    }

    // ---------------------------------------------------------------------------------------------
    // Drawing

    pub(crate) fn draw_settings(&mut self, out: &mut Vec<Instance>) {
        let Some(lay) = self.settings_layout() else { return };
        let s = self.scale;
        let ui = self.ui.clone();
        let (page, hover, capture) = match &self.settings {
            Some(st) => (st.page, st.hover.clone(), st.capture.clone()),
            None => return,
        };
        let (w, h) = (self.size.width as f32, self.size.height as f32);
        out.push(Instance::rect(Rect::new(0.0, 0.0, w, h), [0.0, 0.0, 0.0, 0.45]));
        let p = lay.panel;
        out.push(Instance::shadow(
            Rect::new(p.x, p.y + 10.0 * s, p.w, p.h),
            [0.0, 0.0, 0.0, 0.5],
            14.0 * s,
            24.0 * s,
        ));
        let panel_bg = alpha(ui.menu_bg, 1.0);
        out.push(Instance::rounded(p, panel_bg, 14.0 * s).border(1.0, ui.border));
        out.push(Instance::rounded_corners(
            lay.sidebar,
            mix(panel_bg, [0.0, 0.0, 0.0, 1.0], 0.18),
            [14.0 * s, 0.0, 0.0, 14.0 * s],
        ));

        let font = ui::UI_FONT * s;
        self.text.draw_ui(
            out,
            "Settings",
            lay.sidebar.x + 22.0 * s,
            lay.sidebar.y + 32.0 * s,
            17.0 * s,
            true,
            ui.text,
            lay.sidebar,
        );
        for (pg, r) in &lay.nav {
            let selected = *pg == page;
            if selected {
                out.push(Instance::rounded(*r, alpha(ui.accent, 0.16), 8.0 * s));
            } else if hover == Some(Hit::Nav(*pg)) {
                out.push(Instance::rounded(*r, ui.hover, 8.0 * s));
            }
            let color = if selected { ui.accent } else { ui.text };
            self.text
                .draw_ui(out, pg.title(), r.x + 12.0 * s, r.center_y(), font, selected, color, *r);
        }
        let of = lay.open_file;
        if hover == Some(Hit::OpenFile) {
            out.push(Instance::rounded(of, ui.hover, 8.0 * s));
        }
        self.text.draw_ui(
            out,
            "Open config file",
            of.x + 12.0 * s,
            of.center_y(),
            font - 0.5 * s,
            false,
            ui.text_dim,
            of,
        );

        // Header and close button.
        self.text.draw_ui(
            out,
            page.title(),
            lay.content.x + 24.0 * s,
            p.y + 28.0 * s,
            15.0 * s,
            true,
            ui.text,
            p,
        );
        if hover == Some(Hit::Close) {
            out.push(Instance::rounded(lay.close, ui.hover, 8.0 * s));
        }
        let fx = self.text.measure("×", 22.0 * s, false);
        self.text.draw_ui(
            out,
            "×",
            lay.close.x + (lay.close.w - fx) / 2.0,
            lay.close.center_y(),
            22.0 * s,
            false,
            ui.text_dim,
            lay.close,
        );

        let first = out.len();
        for (item, rect) in &lay.items {
            if rect.bottom() < lay.content.y || rect.y > lay.content.bottom() {
                continue;
            }
            match item {
                Item::Section(title) => {
                    self.text.draw_ui(
                        out,
                        &title.to_uppercase(),
                        rect.x,
                        rect.bottom() - 12.0 * s,
                        font - 2.0 * s,
                        true,
                        ui.text_dim,
                        *rect,
                    );
                }
                Item::Row {
                    label,
                    hint,
                    setting,
                    control,
                } => {
                    out.push(Instance::rect(
                        Rect::new(rect.x, rect.bottom() - 1.0, rect.w, 1.0),
                        alpha(ui.border, 0.6),
                    ));
                    let ly = if hint.is_empty() {
                        rect.center_y()
                    } else {
                        rect.center_y() - 8.0 * s
                    };
                    self.text
                        .draw_ui(out, label, rect.x, ly, font + 0.5 * s, false, ui.text, *rect);
                    if !hint.is_empty() {
                        let cr = self.control_rect(*rect);
                        let hint_clip = Rect::new(rect.x, rect.y, cr.x - rect.x - 12.0 * s, rect.h);
                        let hint = self.text.ellipsize(hint, font - 1.5 * s, false, hint_clip.w);
                        self.text.draw_ui(
                            out,
                            &hint,
                            rect.x,
                            rect.center_y() + 10.0 * s,
                            font - 1.5 * s,
                            false,
                            ui.text_dim,
                            hint_clip,
                        );
                    }
                    self.draw_control(out, *rect, *setting, control, &hover);
                }
                Item::KeysHeader => {
                    let msg = "Click a shortcut to change it, or + to add one. Esc cancels, Backspace removes.";
                    let r = reset_all_rect(*rect, s);
                    let clip = Rect::new(rect.x, rect.y, r.x - rect.x - 10.0 * s, rect.h);
                    let msg = self.text.ellipsize(msg, font - 1.0 * s, false, clip.w);
                    self.text.draw_ui(
                        out,
                        &msg,
                        rect.x,
                        rect.center_y(),
                        font - 1.0 * s,
                        false,
                        ui.text_dim,
                        clip,
                    );
                    self.draw_button(out, r, "Reset all", hover == Some(Hit::ResetAllKeys));
                }
                Item::Keys { label, action } => {
                    let row = *rect;
                    if hover.as_ref().is_some_and(|h| hit_action(h) == Some(*action)) {
                        out.push(Instance::rounded(row.inset(-4.0 * s), alpha(ui.text, 0.03), 6.0 * s));
                    }
                    self.text
                        .draw_ui(out, label, row.x, row.center_y(), font, false, ui.text, row);
                    let (chips, add, reset) = self.key_chips(*action, row);
                    let small = font - 1.0 * s;
                    for (k, chip, remove) in chips {
                        let recording = matches!(&capture, Some((a, Some(old))) if a == action && *old == k);
                        let hovered = matches!(&hover, Some(Hit::Chip(a, c)) | Some(Hit::ChipRemove(a, c)) if a == action && *c == k);
                        let (bg, fg) = if recording {
                            (alpha(ui.accent, 0.25), ui.accent)
                        } else {
                            (alpha(ui.text, if hovered { 0.12 } else { 0.07 }), ui.text)
                        };
                        let mut inst = Instance::rounded(chip, bg, 6.0 * s);
                        if recording {
                            inst = inst.border(1.0, ui.accent);
                        }
                        out.push(inst);
                        let label = if recording {
                            "Press keys…".to_string()
                        } else {
                            k.display()
                        };
                        self.text
                            .draw_ui(out, &label, chip.x + 10.0 * s, chip.center_y(), small, false, fg, chip);
                        if hovered && !recording {
                            let over_x = matches!(&hover, Some(Hit::ChipRemove(..)));
                            if over_x {
                                out.push(Instance::rounded(remove, alpha(ui.danger, 0.8), 5.0 * s));
                            }
                            let xw = self.text.measure("×", 16.0 * s, false);
                            self.text.draw_ui(
                                out,
                                "×",
                                remove.x + (remove.w - xw) / 2.0,
                                remove.center_y(),
                                16.0 * s,
                                false,
                                if over_x { [1.0; 4] } else { ui.text_dim },
                                remove,
                            );
                        }
                    }
                    let adding = matches!(&capture, Some((a, None)) if a == action);
                    let add_hover = hover == Some(Hit::AddChip(*action));
                    if adding {
                        out.push(Instance::rounded(add, alpha(ui.accent, 0.25), 6.0 * s).border(1.0, ui.accent));
                        self.text.draw_ui(
                            out,
                            "Press keys…",
                            add.x + 10.0 * s,
                            add.center_y(),
                            small,
                            false,
                            ui.accent,
                            add,
                        );
                    } else {
                        let col = if add_hover { ui.text } else { ui.text_dim };
                        out.push(Instance::rounded(add, [0.0; 4], 6.0 * s).border(1.0, alpha(col, 0.5)));
                        let pw = self.text.measure("+", 15.0 * s, false);
                        self.text.draw_ui(
                            out,
                            "+",
                            add.x + (add.w - pw) / 2.0,
                            add.center_y(),
                            15.0 * s,
                            false,
                            col,
                            add,
                        );
                    }
                    if let Some(r) = reset {
                        let col = if hover == Some(Hit::ResetAction(*action)) {
                            ui.accent
                        } else {
                            ui.text_dim
                        };
                        let rw = self.text.measure("Reset", small, false);
                        self.text
                            .draw_ui(out, "Reset", r.right() - rw, r.center_y(), small, false, col, r);
                    }
                }
            }
        }
        let clip = lay.content.inset(1.0);
        for inst in &mut out[first..] {
            *inst = inst.clip_within(clip);
        }
        // Scroll position.
        let max = (lay.content_h - lay.content.h).max(0.0);
        if max > 0.0 {
            let scroll = self.settings.as_ref().map(|s| s.scroll).unwrap_or(0.0);
            let track_h = lay.content.h - 16.0 * s;
            let thumb = (lay.content.h / lay.content_h * track_h).max(24.0 * s);
            let ty = lay.content.y + 8.0 * s + scroll / max * (track_h - thumb);
            out.push(Instance::rounded(
                Rect::new(lay.content.right() - 8.0 * s, ty, 4.0 * s, thumb),
                alpha(ui.text, 0.25),
                2.0 * s,
            ));
        }
    }

    fn draw_button(&mut self, out: &mut Vec<Instance>, r: Rect, label: &str, hovered: bool) {
        let s = self.scale;
        let ui = self.ui.clone();
        out.push(Instance::rounded(
            r,
            alpha(ui.text, if hovered { 0.12 } else { 0.07 }),
            7.0 * s,
        ));
        let font = (ui::UI_FONT - 0.5) * s;
        let tw = self.text.measure(label, font, false);
        self.text.draw_ui(
            out,
            label,
            r.x + (r.w - tw) / 2.0,
            r.center_y(),
            font,
            false,
            ui.text,
            r,
        );
    }

    fn draw_control(
        &mut self,
        out: &mut Vec<Instance>,
        row: Rect,
        setting: Setting,
        control: &Control,
        hover: &Option<Hit>,
    ) {
        let s = self.scale;
        let ui = self.ui.clone();
        let cr = self.control_rect(row);
        let font = (ui::UI_FONT - 0.5) * s;
        match control {
            Control::Slider {
                min, max, value, text, ..
            } => {
                let track = slider_track(cr, s);
                let t = ((value - min) / (max - min)).clamp(0.0, 1.0);
                let th = (4.0 * s).round();
                let ty = (track.center_y() - th / 2.0).round();
                out.push(Instance::rounded(
                    Rect::new(track.x, ty, track.w, th),
                    alpha(ui.text, 0.15),
                    th / 2.0,
                ));
                out.push(Instance::rounded(
                    Rect::new(track.x, ty, track.w * t, th),
                    ui.accent,
                    th / 2.0,
                ));
                let active = matches!(hover, Some(Hit::Slider(h, _)) if *h == setting)
                    || self.settings.as_ref().is_some_and(|st| st.slider == Some(setting));
                let k = if active { 18.0 * s } else { 16.0 * s };
                let kx = track.x + track.w * t - k / 2.0;
                out.push(Instance::shadow(
                    Rect::new(kx, track.center_y() - k / 2.0 + 1.0 * s, k, k),
                    [0.0, 0.0, 0.0, 0.35],
                    k / 2.0,
                    3.0 * s,
                ));
                out.push(Instance::rounded(
                    Rect::new(kx, track.center_y() - k / 2.0, k, k),
                    [0.96, 0.97, 0.98, 1.0],
                    k / 2.0,
                ));
                let tw = self.text.measure(text, font, false);
                self.text
                    .draw_ui(out, text, cr.right() - tw, cr.center_y(), font, false, ui.text_dim, cr);
            }
            Control::Toggle(on) => {
                let r = toggle_rect(cr, s);
                let hovered = *hover == Some(Hit::Toggle(setting));
                let bg = if *on {
                    ui.accent
                } else {
                    alpha(ui.text, if hovered { 0.28 } else { 0.2 })
                };
                out.push(Instance::rounded(r, bg, r.h / 2.0));
                let k = r.h - 6.0 * s;
                let kx = if *on { r.right() - 3.0 * s - k } else { r.x + 3.0 * s };
                out.push(Instance::rounded(Rect::new(kx, r.y + 3.0 * s, k, k), [1.0; 4], k / 2.0));
            }
            Control::Segments(opts, current) => {
                out.push(Instance::rounded(cr, alpha(ui.text, 0.07), 8.0 * s));
                for (i, (value, label)) in opts.iter().enumerate() {
                    let r = segment_rect(cr, i, opts.len());
                    let selected = current.eq_ignore_ascii_case(value);
                    if selected {
                        out.push(Instance::rounded(r.inset(3.0 * s), alpha(ui.accent, 0.9), 6.0 * s));
                    } else if *hover == Some(Hit::Segment(setting, value)) {
                        out.push(Instance::rounded(r.inset(3.0 * s), ui.hover, 6.0 * s));
                    }
                    let col = if selected {
                        theme::contrast_text(ui.accent)
                    } else {
                        ui.text
                    };
                    let label = self.text.ellipsize(label, font, selected, r.w - 8.0 * s);
                    let tw = self.text.measure(&label, font, selected);
                    self.text.draw_ui(
                        out,
                        &label,
                        r.x + (r.w - tw) / 2.0,
                        r.center_y(),
                        font,
                        selected,
                        col,
                        r,
                    );
                }
            }
            Control::Select(value) => {
                let hovered = matches!(hover, Some(Hit::Select(h, _)) if *h == setting);
                out.push(
                    Instance::rounded(cr, alpha(ui.text, if hovered { 0.1 } else { 0.06 }), 8.0 * s)
                        .border(1.0, alpha(ui.border, 1.0)),
                );
                let v = self.text.ellipsize(value, font, false, cr.w - 40.0 * s);
                self.text
                    .draw_ui(out, &v, cr.x + 12.0 * s, cr.center_y(), font, false, ui.text, cr);
                let cw = self.text.measure("▾", font, false);
                self.text.draw_ui(
                    out,
                    "▾",
                    cr.right() - 14.0 * s - cw,
                    cr.center_y(),
                    font,
                    false,
                    ui.text_dim,
                    cr,
                );
            }
        }
    }
}

fn slider_track(cr: Rect, s: f32) -> Rect {
    let label_w = 84.0 * s;
    Rect::new(
        cr.x + 8.0 * s,
        cr.center_y() - 9.0 * s,
        cr.w - label_w - 16.0 * s,
        18.0 * s,
    )
}

fn toggle_rect(cr: Rect, s: f32) -> Rect {
    let (w, h) = (42.0 * s, 24.0 * s);
    Rect::new(cr.right() - w, cr.center_y() - h / 2.0, w, h)
}

fn segment_rect(cr: Rect, i: usize, n: usize) -> Rect {
    let w = cr.w / n as f32;
    Rect::new(cr.x + i as f32 * w, cr.y, w, cr.h)
}

fn reset_all_rect(row: Rect, s: f32) -> Rect {
    Rect::new(row.right() - 96.0 * s, row.center_y() - 15.0 * s, 96.0 * s, 30.0 * s)
}

fn hit_action(h: &Hit) -> Option<Action> {
    match h {
        Hit::Chip(a, _) | Hit::ChipRemove(a, _) | Hit::AddChip(a) | Hit::ResetAction(a) => Some(*a),
        _ => None,
    }
}

fn group_digits(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}
