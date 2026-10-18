//! Building the frame: window chrome, tab bar, terminal panes, menus and toasts.

use std::time::Duration;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, CursorShape, Rgb};

use crate::app::{Drag, State};
use crate::layout::Dir;
use crate::pane::PaneId;
use crate::renderer::{Instance, Rect};
use crate::text::{BOLD, ITALIC};
use crate::theme::{self, Color, Scheme, alpha, mix};
use crate::ui::{self, BarItem, EditTarget, Hit, MenuEntry, TabBarLayout};

const WINDOW_RADIUS: f32 = 10.0;
const TAB_RADIUS: f32 = 8.0;

fn rgb(c: Rgb) -> Color {
    [c.r as f32 / 255.0, c.g as f32 / 255.0, c.b as f32 / 255.0, 1.0]
}

fn resolve(color: AnsiColor, colors: &Colors, scheme: &Scheme) -> Color {
    match color {
        AnsiColor::Spec(c) => rgb(c),
        AnsiColor::Indexed(i) => colors[i as usize].map(rgb).unwrap_or_else(|| scheme.indexed(i)),
        AnsiColor::Named(n) => {
            let i = n as usize;
            if let Some(c) = colors[i] {
                return rgb(c);
            }
            match i {
                0..=15 => scheme.ansi[i],
                256 | 267 => scheme.fg,
                257 => scheme.bg,
                258 => scheme.cursor,
                259..=266 => mix(scheme.ansi[i - 259], scheme.bg, 0.35),
                _ => mix(scheme.fg, scheme.bg, 0.35),
            }
        }
    }
}

fn brighten(color: AnsiColor) -> AnsiColor {
    match color {
        AnsiColor::Named(n) if (n as usize) < 8 => AnsiColor::Indexed(n as u8 + 8),
        AnsiColor::Indexed(i) if i < 8 => AnsiColor::Indexed(i + 8),
        c => c,
    }
}

impl State {
    pub fn draw(&mut self) {
        self.relayout();
        let mut out = std::mem::take(&mut self.instances);
        out.clear();

        let s = self.s();
        let (w, h) = (self.size.width as f32, self.size.height as f32);
        let rounded = self.csd() && !self.is_maximized();
        let r = if rounded { WINDOW_RADIUS * s } else { 0.0 };
        let bar = self.tabbar();

        // Title bar background. Translucent layers must not stack, so the area under the active
        // tab is left out and only its rounded corners are filled in.
        let bar_bg = alpha(self.ui.titlebar, self.tab_bar_opacity());
        let bar_shape = Instance::rounded_corners(bar.bar, bar_bg, [r, r, 0.0, 0.0]);
        match bar.tab_rect(self.ws.active) {
            Some(t) => {
                let tr = TAB_RADIUS * s;
                out.push(bar_shape.clip(Rect::new(0.0, 0.0, t.x, bar.bar.h)));
                out.push(bar_shape.clip(Rect::new(t.right(), 0.0, w - t.right(), bar.bar.h)));
                out.push(bar_shape.clip(Rect::new(t.x, 0.0, t.w, t.y)));
                out.push(Instance::corner_filler(
                    Rect::new(t.x, t.y, tr, tr),
                    bar_bg,
                    (t.x + tr, t.y + tr),
                    tr,
                ));
                out.push(Instance::corner_filler(
                    Rect::new(t.right() - tr, t.y, tr, tr),
                    bar_bg,
                    (t.right() - tr, t.y + tr),
                    tr,
                ));
            }
            None => out.push(bar_shape),
        }

        // Gaps between split panes.
        let gap_color = alpha(mix(self.scheme.bg, self.scheme.fg, 0.10), self.effective_opacity());
        for d in &self.dividers {
            out.push(Instance::rect(d.rect, gap_color));
        }

        self.draw_tabbar(&mut out, &bar);

        let focused_pane = self.ws.active_tab().map(|t| t.focused);
        let multi = self.pane_rects.len() > 1;
        let rects = self.pane_rects.clone();
        for (id, rect) in rects {
            let mut radii = [0.0; 4];
            if rect.bottom() >= h - 0.5 {
                if rect.x <= 0.5 {
                    radii[3] = r;
                }
                if rect.right() >= w - 0.5 {
                    radii[2] = r;
                }
            }
            self.draw_pane(&mut out, id, rect, Some(id) == focused_pane, multi, radii);
        }

        // Dragged divider highlight.
        if let Drag::Divider(d) = &self.drag
            && let Some(cur) = self.dividers.iter().find(|x| x.path == d.path)
        {
            out.push(Instance::rect(cur.rect, alpha(self.ui.accent, 0.8)));
        }

        self.draw_dock_preview(&mut out);
        self.draw_settings(&mut out);
        self.draw_toast(&mut out);
        self.draw_menu(&mut out);

        if rounded {
            out.push(
                Instance::rounded(Rect::new(0.0, 0.0, w, h), [0.0; 4], r).border(1.0, alpha(self.scheme.fg, 0.14)),
            );
        }

        let uploads = self.text.take_uploads();
        self.renderer.upload(&uploads);
        if self
            .screenshot
            .as_ref()
            .is_some_and(|(_, at)| std::time::Instant::now() >= *at)
        {
            let (path, _) = self.screenshot.take().unwrap();
            match self.renderer.screenshot(&out, &path) {
                Ok(()) => log::info!("saved screenshot to {}", path.display()),
                Err(e) => log::error!("screenshot failed: {e:#}"),
            }
            self.exit = true;
        }
        self.renderer.render(&self.window, &out);
        self.instances = out;

        if let Some((_, at)) = &self.toast
            && at.elapsed() > Duration::from_millis(2200)
        {
            self.toast = None;
            self.window.request_redraw();
        }
    }

    fn active_surface(&self) -> Color {
        let scheme = self
            .ws
            .focused_pane()
            .map(|p| self.pane_scheme(p))
            .unwrap_or(&self.scheme);
        alpha(scheme.bg, self.effective_opacity())
    }

    fn draw_tabbar(&mut self, out: &mut Vec<Instance>, bar: &TabBarLayout) {
        let s = self.s();
        let ui = self.ui.clone();
        let surface = self.active_surface();
        let hover = self.hover;
        let dragging_tab = matches!(self.drag, Drag::Tab { moved: true, .. });

        // Group underlines, beneath the tabs.
        for g in &self.ws.groups {
            let Some(chip) = bar.group_rect(g.id) else {
                continue;
            };
            let last = bar
                .items
                .iter()
                .filter_map(|it| match it {
                    BarItem::Tab { index, rect, .. } if self.ws.tabs[*index].group == Some(g.id) => Some(rect.right()),
                    _ => None,
                })
                .fold(chip.right(), f32::max);
            let lh = (2.0 * s).round();
            out.push(Instance::rounded(
                Rect::new(chip.x, bar.bar.bottom() - lh, last - chip.x, lh),
                g.color,
                lh / 2.0,
            ));
        }

        let tab_items: Vec<(usize, Rect, Rect)> = bar
            .items
            .iter()
            .filter_map(|it| match it {
                BarItem::Tab { index, rect, close } => Some((*index, *rect, *close)),
                _ => None,
            })
            .collect();

        for (pos, &(index, rect, close)) in tab_items.iter().enumerate() {
            let tab = &self.ws.tabs[index];
            let active = index == self.ws.active;
            let hovered = matches!(hover, Some(Hit::Tab(i)) | Some(Hit::TabClose(i)) if i == index) && !dragging_tab;
            let tr = TAB_RADIUS * s;

            if active {
                // Same fill as the terminal below so the tab flows into it.
                out.push(Instance::rounded_corners(rect, surface, [tr, tr, 0.0, 0.0]));
                if let Some(c) = tab.color {
                    // A colored cap that follows the tab's rounded top edge.
                    let cap = Rect::new(rect.x, rect.y, rect.w, tr);
                    out.push(
                        Instance::rounded_corners(rect, [0.0; 4], [tr, tr, 0.0, 0.0])
                            .border((2.5 * s).round(), c)
                            .clip(cap),
                    );
                }
            } else {
                let inset = Rect::new(rect.x + 2.0 * s, rect.y + 1.0 * s, rect.w - 4.0 * s, rect.h - 7.0 * s);
                if let Some(c) = tab.color {
                    out.push(Instance::rounded(
                        inset,
                        alpha(c, if hovered { 0.32 } else { 0.22 }),
                        7.0 * s,
                    ));
                } else if hovered {
                    out.push(Instance::rounded(inset, ui.hover, 7.0 * s));
                }
                // Separator to the next tab when neither side is highlighted.
                if let Some(&(next, nrect, _)) = tab_items.get(pos + 1) {
                    let next_hovered = matches!(hover, Some(Hit::Tab(i)) | Some(Hit::TabClose(i)) if i == next);
                    let adjacent = (nrect.x - rect.right()).abs() < 4.0 * s;
                    if adjacent
                        && next != self.ws.active
                        && !hovered
                        && !next_hovered
                        && tab.color.is_none()
                        && self.ws.tabs[next].color.is_none()
                    {
                        let sh = 16.0 * s;
                        let sx = ((rect.right() + nrect.x) / 2.0).round();
                        out.push(Instance::rect(
                            Rect::new(sx, rect.center_y() - sh / 2.0 - 2.0 * s, 1.0, sh),
                            ui.border,
                        ));
                    }
                }
            }

            let editing = self
                .edit
                .as_ref()
                .filter(|e| e.target == EditTarget::Tab(tab.id))
                .map(|e| (e.text.clone(), e.cursor, e.all_selected));
            let cy = rect.center_y() - if active { 0.0 } else { 2.0 * s };
            let font = ui::UI_FONT * s;
            let show_close = (active || hovered) && editing.is_none();
            let mut x = rect.x + 12.0 * s;
            let clip = rect;

            if let Some((text, cursor, all)) = editing {
                let field = Rect::new(rect.x + 7.0 * s, cy - 11.0 * s, rect.w - 14.0 * s, 22.0 * s);
                self.draw_text_field(out, field, &text, cursor, all);
                continue;
            }

            if tab.bell {
                let d = 6.0 * s;
                out.push(Instance::rounded(Rect::new(x, cy - d / 2.0, d, d), ui.accent, d / 2.0));
                x += d + 6.0 * s;
            }
            if tab.zoomed {
                let sq = 8.0 * s;
                out.push(
                    Instance::rounded(Rect::new(x, cy - sq / 2.0, sq, sq), [0.0; 4], 2.0 * s)
                        .border(1.2 * s, if active { ui.text } else { ui.text_dim }),
                );
                x += sq + 6.0 * s;
            }
            let right_pad = if show_close { 34.0 * s } else { 12.0 * s };
            let avail = rect.right() - right_pad - x;
            let title = self.ws.tab_title(tab);
            let title = self.text.ellipsize(&title, font, active, avail);
            let color = if active || hovered { ui.text } else { ui.text_dim };
            self.text.draw_ui(out, &title, x, cy, font, active, color, clip);

            if show_close {
                let close_hovered = matches!(hover, Some(Hit::TabClose(i)) if i == index);
                let cr = Rect::new(close.x, cy - close.h / 2.0, close.w, close.h);
                if close_hovered {
                    out.push(Instance::rounded(cr, ui.pressed, cr.w / 2.0));
                }
                self.draw_icon_x(out, cr, if close_hovered { ui.text } else { ui.text_dim }, 8.0 * s);
            }
        }

        // Group chips.
        for item in &bar.items {
            let BarItem::Group { id, rect, label } = item else {
                continue;
            };
            let Some(g) = self.ws.group(*id) else {
                continue;
            };
            let hovered = matches!(hover, Some(Hit::Group(h)) if h == *id);
            let color = if hovered { mix(g.color, [1.0; 4], 0.15) } else { g.color };
            let color = if g.collapsed {
                mix(color, ui.titlebar, 0.15)
            } else {
                color
            };
            out.push(Instance::rounded(*rect, color, 6.0 * s));
            let fg = theme::contrast_text(g.color);
            let editing = self
                .edit
                .as_ref()
                .filter(|e| e.target == EditTarget::Group(*id))
                .map(|e| (e.cursor, e.all_selected));
            let font = ui::CHIP_FONT * s;
            let tx = rect.x + 11.0 * s;
            if let Some((cursor, all)) = editing {
                let w = self.text.measure(label, font, true);
                if all && !label.is_empty() {
                    out.push(Instance::rect(
                        Rect::new(tx - 1.0, rect.y + 4.0 * s, w + 2.0, rect.h - 8.0 * s),
                        alpha(fg, 0.25),
                    ));
                }
                let cx = tx + self.text.measure(&label[..cursor.min(label.len())], font, true);
                out.push(Instance::rect(
                    Rect::new(cx.round(), rect.y + 4.0 * s, (1.5 * s).max(1.0), rect.h - 8.0 * s),
                    fg,
                ));
            }
            self.text
                .draw_ui(out, label, tx, rect.center_y(), font, true, fg, *rect);
        }

        // New tab and dropdown buttons.
        let nt_hover = hover == Some(Hit::NewTab);
        let dd_hover = hover == Some(Hit::NewTabMenu);
        if nt_hover {
            out.push(Instance::rounded(bar.new_tab, ui.hover, 7.0 * s));
        }
        if dd_hover {
            out.push(Instance::rounded(bar.dropdown, ui.hover, 7.0 * s));
        }
        let c = bar.new_tab;
        let (len, th) = ((12.0 * s).round(), (1.5 * s).max(1.0).round());
        let col = if nt_hover { ui.text } else { ui.text_dim };
        let (mx, my) = ((c.x + c.w / 2.0).round(), (c.y + c.h / 2.0).round());
        out.push(Instance::rect(
            Rect::new(mx - (len / 2.0).round(), my - (th / 2.0).round(), len, th),
            col,
        ));
        out.push(Instance::rect(
            Rect::new(mx - (th / 2.0).round(), my - (len / 2.0).round(), th, len),
            col,
        ));
        self.draw_chevron(out, bar.dropdown, if dd_hover { ui.text } else { ui.text_dim });

        // Window controls.
        if let Some([min, max, close]) = bar.controls {
            let col = |h: bool| if h { ui.text } else { ui.text_dim };
            let hmin = hover == Some(Hit::Minimize);
            let hmax = hover == Some(Hit::Maximize);
            let hclose = hover == Some(Hit::Close);
            for (r, hv) in [(min, hmin), (max, hmax)] {
                if hv {
                    out.push(Instance::rounded(r.inset(2.0 * s), ui.hover, 7.0 * s));
                }
            }
            if hclose {
                out.push(Instance::rounded(close.inset(2.0 * s), ui.danger, 7.0 * s));
            }
            let lw = (1.2 * s).max(1.0);
            let ic = (10.0 * s).round();
            let (cx, cy) = (min.x + min.w / 2.0, min.y + min.h / 2.0);
            out.push(Instance::rect(
                Rect::new((cx - ic / 2.0).round(), cy.round(), ic, lw.round()),
                col(hmin),
            ));
            let (cx, cy) = (max.x + max.w / 2.0, max.y + max.h / 2.0);
            if self.window.is_maximized() {
                let sq = (8.0 * s).round();
                let back = Rect::new(
                    (cx - sq / 2.0 + 2.0 * s).round(),
                    (cy - sq / 2.0 - 2.0 * s).round(),
                    sq,
                    sq,
                );
                let front = Rect::new(
                    (cx - sq / 2.0 - 1.0 * s).round(),
                    (cy - sq / 2.0 + 1.0 * s).round(),
                    sq,
                    sq,
                );
                out.push(Instance::rounded(back, [0.0; 4], 1.5 * s).border(lw, col(hmax)));
                let fill = if hmax {
                    mix(ui.titlebar, ui.text, 0.07)
                } else {
                    ui.titlebar
                };
                out.push(Instance::rounded(front, fill, 1.5 * s).border(lw, col(hmax)));
            } else {
                let sq = ic;
                let r = Rect::new((cx - sq / 2.0).round(), (cy - sq / 2.0).round(), sq, sq);
                out.push(Instance::rounded(r, [0.0; 4], 1.5 * s).border(lw, col(hmax)));
            }
            self.draw_icon_x(out, close, if hclose { [1.0; 4] } else { ui.text_dim }, 10.0 * s);
        }
    }

    /// An "×" drawn from glyphs so it stays crisp at any scale.
    fn draw_icon_x(&mut self, out: &mut Vec<Instance>, r: Rect, color: Color, size: f32) {
        let font = size * 1.7;
        let w = self.text.measure("×", font, false);
        self.text.draw_ui(
            out,
            "×",
            r.x + (r.w - w) / 2.0,
            r.y + r.h / 2.0 - size * 0.05,
            font,
            false,
            color,
            r,
        );
    }

    /// Down chevron made of stepped rects.
    fn draw_chevron(&mut self, out: &mut Vec<Instance>, r: Rect, color: Color) {
        let s = self.s();
        let px = (1.0 * s).max(1.0).round();
        let steps = (4.0 * s).round().max(3.0) as i32;
        let cx = (r.x + r.w / 2.0).round();
        let cy = (r.y + r.h / 2.0 - steps as f32 * px / 2.0).round();
        for i in 0..steps {
            let y = cy + i as f32 * px;
            let half = (steps - i) as f32 * px;
            out.push(Instance::rect(Rect::new(cx - half, y, px * 1.5, px * 1.5), color));
            out.push(Instance::rect(
                Rect::new(cx + half - px * 1.5, y, px * 1.5, px * 1.5),
                color,
            ));
        }
        out.push(Instance::rect(
            Rect::new(cx - px, cy + steps as f32 * px, px * 2.0, px * 1.5),
            color,
        ));
    }

    fn draw_text_field(&mut self, out: &mut Vec<Instance>, field: Rect, text: &str, cursor: usize, all: bool) {
        let s = self.s();
        let ui = self.ui.clone();
        out.push(Instance::rounded(field, alpha(ui.titlebar, 0.9), 5.0 * s).border((1.2 * s).max(1.0), ui.accent));
        let font = ui::UI_FONT * s;
        let tx = field.x + 7.0 * s;
        let tw = self.text.measure(text, font, false);
        if all && !text.is_empty() {
            out.push(
                Instance::rect(
                    Rect::new(tx - 1.0, field.y + 4.0 * s, tw + 2.0, field.h - 8.0 * s),
                    alpha(ui.accent, 0.35),
                )
                .clip(field),
            );
        }
        self.text
            .draw_ui(out, text, tx, field.center_y(), font, false, ui.text, field);
        let cx = tx + self.text.measure(&text[..cursor.min(text.len())], font, false);
        out.push(
            Instance::rect(
                Rect::new(cx.round(), field.y + 4.0 * s, (1.5 * s).max(1.0), field.h - 8.0 * s),
                ui.text,
            )
            .clip(field),
        );
    }

    fn draw_pane(
        &mut self,
        out: &mut Vec<Instance>,
        id: PaneId,
        rect: Rect,
        focused: bool,
        multi: bool,
        radii: [f32; 4],
    ) {
        let s = self.s();
        let Some(pane) = self.ws.panes.get(&id) else {
            return;
        };
        let scheme = self.pane_scheme(pane).clone();
        let opacity = self.effective_opacity();
        out.push(Instance::rounded_corners(rect, alpha(scheme.bg, opacity), radii));
        let (label, title, pane_color) = (pane.label.clone(), pane.title.clone(), pane.color);

        if let Some(header) = self.pane_header(rect) {
            self.draw_pane_header(out, id, header, focused, label.as_deref(), &title, pane_color);
        }
        let full = rect;
        let rect = self.pane_body(rect);
        let Some(pane) = self.ws.panes.get(&id) else {
            return;
        };

        let pad = self.padding();
        let (cw, ch) = self.pane_cell(id);
        let font_px = self.pane_font_px(id);
        let ox = (rect.x + pad).round();
        let oy = (rect.y + pad).round();
        let clip = rect;
        let bold_is_bright = self.cfg.bold_is_bright;
        let show_cursor = self.focused && focused && self.cursor_blink_visible();
        let hollow = !(self.focused && focused);

        let term_arc = pane.term.clone();
        let term = term_arc.lock();
        let content = term.renderable_content();
        let colors = content.colors;
        let offset = content.display_offset as i32;
        let selection = content.selection;
        let cursor = content.cursor;
        let cursor_row = cursor.point.line.0 + offset;
        let cursor_visible =
            cursor.shape != CursorShape::Hidden && cursor_row >= 0 && (cursor_row as usize) < term.screen_lines();
        let history = term.grid().history_size();
        let rows = term.screen_lines();

        let mut bgs: Vec<Instance> = Vec::new();
        let mut fgs: Vec<Instance> = Vec::new();
        // Pending run of identical backgrounds on the current row.
        let mut run: Option<(i32, f32, f32, Color)> = None;
        let flush = |run: &mut Option<(i32, f32, f32, Color)>, bgs: &mut Vec<Instance>| {
            if let Some((row, x0, x1, c)) = run.take() {
                bgs.push(Instance::rect(Rect::new(x0, oy + row as f32 * ch, x1 - x0, ch), c).clip(clip));
            }
        };

        // Position and width (in cells) of the cursor, if it's on screen.
        let mut cursor_cell: Option<(f32, f32, f32)> = None;

        for item in content.display_iter {
            let cell = item.cell;
            let point = item.point;
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            let row = point.line.0 + offset;
            let col = point.column.0;
            let x = ox + col as f32 * cw;
            let y = oy + row as f32 * ch;
            let width = if cell.flags.contains(Flags::WIDE_CHAR) {
                2.0
            } else {
                1.0
            };

            let fg_color = if bold_is_bright && cell.flags.contains(Flags::BOLD) {
                brighten(cell.fg)
            } else {
                cell.fg
            };
            let mut fg = resolve(fg_color, colors, &scheme);
            let mut bg = resolve(cell.bg, colors, &scheme);
            let mut has_bg = cell.bg != AnsiColor::Named(alacritty_terminal::vte::ansi::NamedColor::Background);
            if cell.flags.contains(Flags::DIM) {
                fg = mix(fg, scheme.bg, 0.4);
            }
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
                has_bg = true;
            }
            if selection.is_some_and(|sel| sel.contains(point)) {
                bg = scheme.selection;
                has_bg = true;
            }
            let is_cursor = cursor_visible && point == cursor.point;
            if is_cursor && show_cursor && !hollow && cursor.shape == CursorShape::Block {
                let fill = colors[258].map(rgb).unwrap_or(scheme.cursor);
                bg = fill;
                has_bg = true;
                fg = if theme::luminance(fill) > 0.5 {
                    scheme.bg
                } else {
                    scheme.fg
                };
            }

            if has_bg {
                let x1 = x + cw * width;
                match &mut run {
                    Some((r, _, end, c)) if *r == row && (*end - x).abs() < 0.5 && *c == bg => *end = x1,
                    _ => {
                        flush(&mut run, &mut bgs);
                        run = Some((row, x, x1, bg));
                    }
                }
            } else {
                flush(&mut run, &mut bgs);
            }

            let style =
                (cell.flags.contains(Flags::BOLD) as u8 * BOLD) | (cell.flags.contains(Flags::ITALIC) as u8 * ITALIC);
            if is_cursor {
                cursor_cell = Some((x, y, width));
            }
            if cell.c != ' ' && cell.c != '\t' && !cell.flags.contains(Flags::HIDDEN) {
                self.text
                    .draw_cell(&mut fgs, font_px, x, y, cell.c, cell.zerowidth(), style, fg, clip);
            }

            let lw = (ch / 16.0).round().max(1.0);
            if cell.flags.intersects(Flags::ALL_UNDERLINES) {
                let uc = cell
                    .underline_color()
                    .map(|c| resolve(c, colors, &scheme))
                    .unwrap_or(fg);
                let uy = y + ch - lw * 2.0;
                fgs.push(Instance::rect(Rect::new(x, uy, cw * width, lw), uc).clip(clip));
                if cell.flags.contains(Flags::DOUBLE_UNDERLINE) {
                    fgs.push(Instance::rect(Rect::new(x, uy - lw * 2.0, cw * width, lw), uc).clip(clip));
                }
            }
            if cell.flags.contains(Flags::STRIKEOUT) {
                fgs.push(Instance::rect(Rect::new(x, (y + ch / 2.0).round(), cw * width, lw), fg).clip(clip));
            }
        }
        flush(&mut run, &mut bgs);

        // Cursor shapes other than a solid block are drawn on top of the text.
        if let Some((x, y, width)) = cursor_cell {
            let cc = colors[258].map(rgb).unwrap_or(scheme.cursor);
            let lw = (2.0 * s).round().max(1.0);
            if hollow {
                fgs.push(
                    Instance::rect(Rect::new(x, y, cw * width, ch), [0.0; 4])
                        .border(lw.min(1.5 * s).max(1.0), cc)
                        .clip(clip),
                );
            } else if show_cursor {
                match cursor.shape {
                    CursorShape::Beam => fgs.push(Instance::rect(Rect::new(x, y, lw, ch), cc).clip(clip)),
                    CursorShape::Underline => {
                        fgs.push(Instance::rect(Rect::new(x, y + ch - lw, cw * width, lw), cc).clip(clip))
                    }
                    CursorShape::HollowBlock => fgs.push(
                        Instance::rect(Rect::new(x, y, cw * width, ch), [0.0; 4])
                            .border(lw / 2.0, cc)
                            .clip(clip),
                    ),
                    _ => {}
                }
            }
        }
        drop(term);

        out.extend(bgs);
        out.extend(fgs);

        // Scrollback position indicator.
        if offset > 0 && history > 0 {
            let total = (history + rows) as f32;
            let track = Rect::new(rect.right() - 7.0 * s, rect.y + 4.0 * s, 4.0 * s, rect.h - 8.0 * s);
            let thumb_h = (rows as f32 / total * track.h).max(24.0 * s);
            let top_frac = (history as f32 - offset as f32) / (total - rows as f32).max(1.0);
            let ty = track.y + top_frac * (track.h - thumb_h);
            out.push(Instance::rounded(
                Rect::new(track.x, ty, track.w, thumb_h),
                alpha(scheme.fg, 0.35),
                track.w / 2.0,
            ));
        }

        let rect = full;
        if multi {
            if !focused && self.cfg.unfocused_pane_dim > 0.0 {
                out.push(Instance::rounded_corners(
                    rect,
                    [0.0, 0.0, 0.0, self.cfg.unfocused_pane_dim],
                    radii,
                ));
            }
            if focused {
                let bw = (1.5 * s).max(1.0);
                out.push(Instance::rounded_corners(rect, [0.0; 4], radii).border(bw, alpha(self.ui.accent, 0.55)));
            }
        }
    }

    /// Slim bar above a split pane showing its name and color.
    fn draw_pane_header(
        &mut self,
        out: &mut Vec<Instance>,
        id: PaneId,
        header: Rect,
        focused: bool,
        label: Option<&str>,
        title: &str,
        color: Option<Color>,
    ) {
        let s = self.s();
        let ui = self.ui.clone();
        out.push(Instance::rect(
            header,
            alpha(ui.text, if focused { 0.07 } else { 0.035 }),
        ));
        out.push(Instance::rect(
            Rect::new(header.x, header.bottom() - 1.0, header.w, 1.0),
            ui.border,
        ));
        if let Some(c) = color {
            out.push(Instance::rect(
                Rect::new(header.x, header.y, header.w, (2.0 * s).round()),
                c,
            ));
        }

        let editing = self
            .edit
            .as_ref()
            .filter(|e| e.target == EditTarget::Pane(id))
            .map(|e| (e.text.clone(), e.cursor, e.all_selected));
        if let Some((text, cursor, all)) = editing {
            let field = Rect::new(
                header.x + 6.0 * s,
                header.y + 3.0 * s,
                (header.w - 12.0 * s).min(320.0 * s),
                header.h - 6.0 * s,
            );
            self.draw_text_field(out, field, &text, cursor, all);
            return;
        }

        let cy = header.center_y() + if color.is_some() { 1.0 * s } else { 0.0 };
        let mut x = header.x + 10.0 * s;
        if let Some(c) = color {
            let d = (8.0 * s).round();
            out.push(Instance::rounded(
                Rect::new(x, (cy - d / 2.0).round(), d, d),
                c,
                d / 2.0,
            ));
            x += d + 7.0 * s;
        }
        let font = (ui::UI_FONT - 0.5) * s;
        let name = match label {
            Some(l) if !l.trim().is_empty() => l.to_string(),
            _ if !title.trim().is_empty() => title.to_string(),
            _ => self
                .ws
                .panes
                .get(&id)
                .map(|p| p.profile_name.clone())
                .unwrap_or_default(),
        };
        let avail = header.right() - 10.0 * s - x;
        let name = self.text.ellipsize(&name, font, true, avail);
        let w = self.text.draw_ui(
            out,
            &name,
            x,
            cy,
            font,
            true,
            if focused { ui.text } else { ui.text_dim },
            header,
        );
        // When the pane has a name, also show what's running in it.
        if label.is_some() && !title.trim().is_empty() && Some(title) != label {
            let tx = x + w + 8.0 * s;
            let rest = self.text.ellipsize(title, font, false, header.right() - 10.0 * s - tx);
            if !rest.is_empty() {
                self.text
                    .draw_ui(out, &rest, tx, cy, font, false, alpha(ui.text_dim, 0.8), header);
            }
        }
    }

    /// While a tab is dragged over the content: where it will dock, and a label at the cursor.
    fn draw_dock_preview(&mut self, out: &mut Vec<Instance>) {
        let Drag::Tab {
            id, moved: true, dock, ..
        } = self.drag
        else {
            return;
        };
        let s = self.s();
        let (mx, my) = self.mouse;
        if my < (ui::BAR_H * s).round() {
            return;
        }
        if let Some((pane, dir)) = dock
            && let Some((_, r)) = self.pane_rects.iter().find(|(p, _)| *p == pane).copied()
        {
            let half = match dir {
                Dir::Left => Rect::new(r.x, r.y, r.w / 2.0, r.h),
                Dir::Right => Rect::new(r.x + r.w / 2.0, r.y, r.w / 2.0, r.h),
                Dir::Up => Rect::new(r.x, r.y, r.w, r.h / 2.0),
                Dir::Down => Rect::new(r.x, r.y + r.h / 2.0, r.w, r.h / 2.0),
            };
            out.push(
                Instance::rounded(half.inset(5.0 * s), alpha(self.ui.accent, 0.18), 8.0 * s)
                    .border((1.5 * s).max(1.0), alpha(self.ui.accent, 0.85)),
            );
        }
        let Some(tab) = self.ws.tab_index(id).map(|i| &self.ws.tabs[i]) else {
            return;
        };
        let title = self.ws.tab_title(tab);
        let color = tab.color;
        let font = ui::UI_FONT * s;
        let title = self.text.ellipsize(&title, font, true, 220.0 * s);
        let tw = self.text.measure(&title, font, true);
        let dot = if color.is_some() { 14.0 * s } else { 0.0 };
        let pill = Rect::new(mx + 14.0 * s, my + 10.0 * s, tw + dot + 24.0 * s, 28.0 * s);
        let ui = self.ui.clone();
        out.push(Instance::shadow(pill, [0.0, 0.0, 0.0, 0.35], pill.h / 2.0, 8.0 * s));
        out.push(Instance::rounded(pill, ui.menu_bg, pill.h / 2.0).border(1.0, ui.border));
        if let Some(c) = color {
            let d = 8.0 * s;
            out.push(Instance::rounded(
                Rect::new(pill.x + 12.0 * s, pill.center_y() - d / 2.0, d, d),
                c,
                d / 2.0,
            ));
        }
        self.text.draw_ui(
            out,
            &title,
            pill.x + 12.0 * s + dot,
            pill.center_y(),
            font,
            true,
            ui.text,
            pill,
        );
    }

    fn draw_menu(&mut self, out: &mut Vec<Instance>) {
        let Some(menu) = self.menu.take() else { return };
        let s = self.s();
        let ui = self.ui.clone();
        let r = menu.rect;
        out.push(Instance::shadow(
            Rect::new(r.x, r.y + 6.0 * s, r.w, r.h),
            [0.0, 0.0, 0.0, 0.45],
            10.0 * s,
            14.0 * s,
        ));
        out.push(Instance::rounded(r, ui.menu_bg, 10.0 * s).border(1.0, ui.border));
        let font = ui::UI_FONT * s;
        let first = out.len();
        for (i, entry) in menu.entries.iter().enumerate() {
            let row = &menu.row(i);
            if row.bottom() < r.y || row.y > r.bottom() {
                continue;
            }
            match entry {
                MenuEntry::Item {
                    label,
                    hint,
                    danger,
                    checked,
                    ..
                } => {
                    if menu.hovered == Some(i) {
                        out.push(Instance::rounded(
                            *row,
                            if *danger { alpha(ui.danger, 0.85) } else { ui.menu_hover },
                            6.0 * s,
                        ));
                    }
                    let color = match (*danger, menu.hovered == Some(i)) {
                        (true, true) => [1.0; 4],
                        (true, false) => ui.danger,
                        _ => ui.text,
                    };
                    self.text.draw_ui(
                        out,
                        label,
                        row.x + 10.0 * s,
                        row.center_y(),
                        font,
                        *checked,
                        color,
                        *row,
                    );
                    if *checked {
                        let d = (6.0 * s).round();
                        out.push(Instance::rounded(
                            Rect::new(row.right() - 14.0 * s - d / 2.0, row.center_y() - d / 2.0, d, d),
                            ui.accent,
                            d / 2.0,
                        ));
                    }
                    if !hint.is_empty() {
                        let hf = font - 1.0 * s;
                        let hw = self.text.measure(hint, hf, false);
                        let hc = if *danger && menu.hovered == Some(i) {
                            [1.0, 1.0, 1.0, 0.8]
                        } else {
                            ui.text_dim
                        };
                        self.text.draw_ui(
                            out,
                            hint,
                            row.right() - 10.0 * s - hw,
                            row.center_y(),
                            hf,
                            false,
                            hc,
                            *row,
                        );
                    }
                }
                MenuEntry::Separator => {
                    out.push(Instance::rect(
                        Rect::new(row.x + 4.0 * s, row.center_y().round(), row.w - 8.0 * s, 1.0),
                        ui.border,
                    ));
                }
                MenuEntry::Label(l) => {
                    self.text.draw_ui(
                        out,
                        l,
                        row.x + 10.0 * s,
                        row.center_y() + 2.0 * s,
                        font - 1.5 * s,
                        true,
                        ui.text_dim,
                        *row,
                    );
                }
                MenuEntry::Swatches { target, current } => {
                    for (j, (sr, c)) in menu.swatch_rects(*row, *target, s).into_iter().enumerate() {
                        let selected = *current == c;
                        let hovered = menu.hovered == Some(i) && menu.hovered_swatch == Some(j);
                        if selected || hovered {
                            let ring = sr.inset(-3.0 * s);
                            let rc = if selected { ui.text } else { alpha(ui.text, 0.4) };
                            out.push(Instance::rounded(ring, [0.0; 4], ring.w / 2.0).border((1.5 * s).max(1.0), rc));
                        }
                        match c {
                            Some(c) => out.push(Instance::rounded(sr, c, sr.w / 2.0)),
                            None => {
                                out.push(
                                    Instance::rounded(sr, [0.0; 4], sr.w / 2.0).border((1.2 * s).max(1.0), ui.text_dim),
                                );
                                let lw = (1.2 * s).max(1.0);
                                out.push(Instance::rect(
                                    Rect::new(sr.x + 4.0 * s, sr.center_y() - lw / 2.0, sr.w - 8.0 * s, lw),
                                    ui.text_dim,
                                ));
                            }
                        }
                    }
                }
            }
        }
        // Scrolled rows must not spill outside the menu.
        let inner = r.inset(1.0);
        for inst in &mut out[first..] {
            *inst = inst.clip_within(inner);
        }
        if menu.max_scroll > 0.0 {
            let track_h = r.h - 16.0 * s;
            let thumb_h = (r.h / (r.h + menu.max_scroll) * track_h).max(20.0 * s);
            let ty = r.y + 8.0 * s + menu.scroll / menu.max_scroll * (track_h - thumb_h);
            out.push(Instance::rounded(
                Rect::new(r.right() - 6.0 * s, ty, 3.0 * s, thumb_h),
                alpha(ui.text, 0.3),
                1.5 * s,
            ));
        }
        self.menu = Some(menu);
    }

    fn draw_toast(&mut self, out: &mut Vec<Instance>) {
        let Some((msg, at)) = self.toast.clone() else {
            return;
        };
        let s = self.s();
        let elapsed = at.elapsed().as_secs_f32();
        let fade = if elapsed > 1.8 {
            (1.0 - (elapsed - 1.8) / 0.4).clamp(0.0, 1.0)
        } else {
            1.0
        };
        if fade <= 0.0 {
            return;
        }
        let font = ui::UI_FONT * s;
        let max_w = self.size.width as f32 - 48.0 * s;
        let msg = self.text.ellipsize(&msg, font, false, max_w - 32.0 * s);
        let tw = self.text.measure(&msg, font, false);
        let (w, h) = ((tw + 32.0 * s).round(), (34.0 * s).round());
        let r = Rect::new(
            ((self.size.width as f32 - w) / 2.0).round(),
            self.size.height as f32 - h - 28.0 * s,
            w,
            h,
        );
        let ui = self.ui.clone();
        out.push(Instance::shadow(
            Rect::new(r.x, r.y + 4.0 * s, r.w, r.h),
            [0.0, 0.0, 0.0, 0.35 * fade],
            h / 2.0,
            10.0 * s,
        ));
        out.push(Instance::rounded(r, alpha(ui.menu_bg, fade), h / 2.0).border(1.0, alpha(ui.border, fade)));
        self.text.draw_ui(
            out,
            &msg,
            r.x + 16.0 * s,
            r.center_y(),
            font,
            false,
            alpha(ui.text, fade),
            r,
        );
        if fade < 1.0 {
            self.window.request_redraw();
        }
    }
}
