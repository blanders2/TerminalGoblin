//! Font loading, shaping, glyph rasterization and the glyph atlas.

use std::collections::HashMap;
use std::rc::Rc;

use cosmic_text::{
    Attrs, Buffer, CacheKey, Family, FontSystem, Metrics, Shaping, Style, SwashCache, SwashContent, Weight, fontdb,
};

use crate::renderer::{ATLAS_SIZE, Instance, Rect, Upload};
use crate::theme::Color;

#[derive(Clone, Copy, Debug)]
struct AtlasGlyph {
    uv: [f32; 4],
    left: i32,
    top: i32,
    color: bool,
}

#[derive(Clone, Copy, Debug)]
struct Placed {
    key: CacheKey,
    x: i32,
    y: i32,
}

struct Row {
    y: u32,
    h: u32,
    x: u32,
}

/// Shelf-packed RGBA glyph atlas. Mask glyphs are stored as white with coverage in alpha.
struct Atlas {
    rows: Vec<Row>,
    next_y: u32,
    map: HashMap<CacheKey, Option<AtlasGlyph>>,
    uploads: Vec<Upload>,
}

impl Atlas {
    fn new() -> Atlas {
        Atlas {
            rows: Vec::new(),
            next_y: 0,
            map: HashMap::new(),
            uploads: Vec::new(),
        }
    }

    fn clear(&mut self) {
        self.rows.clear();
        self.next_y = 0;
        self.map.clear();
    }

    fn alloc(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        let (w, h) = (w + 1, h + 1);
        if w > ATLAS_SIZE || h > ATLAS_SIZE {
            return None;
        }
        if let Some(row) = self
            .rows
            .iter_mut()
            .find(|r| r.h >= h && r.h <= h + h / 3 + 2 && r.x + w <= ATLAS_SIZE)
        {
            let pos = (row.x, row.y);
            row.x += w;
            return Some(pos);
        }
        if self.next_y + h > ATLAS_SIZE {
            return None;
        }
        let row = Row {
            y: self.next_y,
            h,
            x: w,
        };
        self.next_y += h;
        let pos = (0, row.y);
        self.rows.push(row);
        Some(pos)
    }

    fn get(&mut self, fs: &mut FontSystem, swash: &mut SwashCache, key: CacheKey) -> Option<AtlasGlyph> {
        if let Some(g) = self.map.get(&key) {
            return *g;
        }
        let entry = self.rasterize(fs, swash, key);
        self.map.insert(key, entry);
        entry
    }

    fn rasterize(&mut self, fs: &mut FontSystem, swash: &mut SwashCache, key: CacheKey) -> Option<AtlasGlyph> {
        let image = swash.get_image_uncached(fs, key)?;
        let (w, h) = (image.placement.width, image.placement.height);
        if w == 0 || h == 0 {
            return None;
        }
        let (rgba, color) = match image.content {
            SwashContent::Mask => (
                image.data.iter().flat_map(|&a| [255, 255, 255, a]).collect::<Vec<u8>>(),
                false,
            ),
            SwashContent::Color => {
                // Premultiply so the shader can composite it directly.
                let mut d = image.data.clone();
                for px in d.as_chunks_mut::<4>().0 {
                    let a = px[3] as u32;
                    for c in &mut px[..3] {
                        *c = ((*c as u32 * a + 127) / 255) as u8;
                    }
                }
                (d, true)
            }
            SwashContent::SubpixelMask => (
                image
                    .data
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .flat_map(|p| [255, 255, 255, ((p[0] as u32 + p[1] as u32 + p[2] as u32) / 3) as u8])
                    .collect(),
                false,
            ),
        };
        let (x, y) = match self.alloc(w, h) {
            Some(p) => p,
            None => {
                // Atlas full: start over. Glyphs re-rasterize lazily on the following frames.
                self.clear();
                self.alloc(w, h)?
            }
        };
        self.uploads.push(Upload { x, y, w, h, rgba });
        Some(AtlasGlyph {
            uv: [x as f32, y as f32, w as f32, h as f32],
            left: image.placement.left,
            top: image.placement.top,
            color,
        })
    }
}

pub struct Shaped {
    glyphs: Vec<Placed>,
    pub width: f32,
}

pub const BOLD: u8 = 1;
pub const ITALIC: u8 = 2;

/// Shaped glyphs for one terminal font size.
#[derive(Default)]
struct TermSize {
    glyphs: HashMap<(char, u8), Rc<[Placed]>>,
    clusters: HashMap<(String, u8), Rc<[Placed]>>,
}

pub struct TextSystem {
    fs: FontSystem,
    swash: SwashCache,
    atlas: Atlas,
    term_family: String,
    ui_family: String,
    /// Terminal font advance and line height per pixel of font size.
    em_w: f32,
    em_h: f32,
    line_height: f32,
    /// Glyph caches keyed by font size in physical pixels (panes can have different sizes).
    sizes: HashMap<u32, TermSize>,
    ui_cache: HashMap<(String, u32, bool), Rc<Shaped>>,
}

impl TextSystem {
    pub fn new(term_family: &str, ui_family: &str, line_height: f32) -> TextSystem {
        let fs = FontSystem::new();
        let mut t = TextSystem {
            fs,
            swash: SwashCache::new(),
            atlas: Atlas::new(),
            term_family: String::new(),
            ui_family: String::new(),
            em_w: 0.6,
            em_h: 1.2,
            line_height,
            sizes: HashMap::new(),
            ui_cache: HashMap::new(),
        };
        t.set_fonts(term_family, ui_family, line_height);
        t
    }

    fn resolve_family(&self, wanted: &str, generic: Family<'static>) -> String {
        let db = self.fs.db();
        let found = db.faces().find_map(|f| {
            f.families
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(wanted))
                .map(|(name, _)| name.clone())
        });
        if let Some(name) = found {
            return name;
        }
        log::warn!("font {wanted:?} not found, using the system default");
        let id = db.query(&fontdb::Query {
            families: &[generic],
            ..Default::default()
        });
        id.and_then(|id| db.face(id))
            .and_then(|f| f.families.first().map(|(n, _)| n.clone()))
            .unwrap_or_default()
    }

    /// Installed font families, sorted; `monospace_only` limits it to fixed-width fonts.
    pub fn families(&self, monospace_only: bool) -> Vec<String> {
        let mut names: Vec<String> = self
            .fs
            .db()
            .faces()
            .filter(|f| !monospace_only || f.monospaced)
            .filter_map(|f| f.families.first().map(|(n, _)| n.clone()))
            // Emoji fonts claim to be monospaced but can't render text.
            .filter(|n| !monospace_only || !n.to_lowercase().contains("emoji"))
            .collect();
        names.sort_by_key(|n| n.to_lowercase());
        names.dedup();
        names
    }

    /// (Re)configure fonts.
    pub fn set_fonts(&mut self, term_family: &str, ui_family: &str, line_height: f32) {
        self.term_family = self.resolve_family(term_family, Family::Monospace);
        self.ui_family = self.resolve_family(ui_family, Family::SansSerif);
        self.line_height = line_height;
        self.sizes.clear();
        self.ui_cache.clear();

        let id = self.fs.db().query(&fontdb::Query {
            families: &[Family::Name(&self.term_family), Family::Monospace],
            ..Default::default()
        });
        let metrics = id.and_then(|id| self.fs.get_font(id, Weight::NORMAL)).map(|font| {
            let f = font.as_swash();
            let m = f.metrics(&[]);
            let upem = m.units_per_em.max(1) as f32;
            let adv = f.glyph_metrics(&[]).advance_width(f.charmap().map('M'));
            (adv / upem, (m.ascent + m.descent + m.leading) / upem)
        });
        (self.em_w, self.em_h) = metrics.unwrap_or((0.6, 1.2));
        log::info!("terminal font {:?}", self.term_family);
    }

    /// Cell size in physical pixels for a font size in physical pixels.
    pub fn cell(&self, size_px: f32) -> (f32, f32) {
        (
            (self.em_w * size_px).round().max(1.0),
            (self.em_h * size_px * self.line_height).ceil().max(1.0),
        )
    }

    pub fn take_uploads(&mut self) -> Vec<Upload> {
        std::mem::take(&mut self.atlas.uploads)
    }

    fn shape(fs: &mut FontSystem, text: &str, attrs: &Attrs, size: f32, line_h: f32) -> (Vec<Placed>, f32) {
        let mut buf = Buffer::new(fs, Metrics::new(size, line_h));
        buf.set_size(None, None);
        buf.set_text(text, attrs, Shaping::Advanced, None);
        buf.shape_until_scroll(fs, false);
        let mut out = Vec::new();
        let mut width: f32 = 0.0;
        for run in buf.layout_runs() {
            width = width.max(run.line_w);
            for g in run.glyphs {
                let p = g.physical((0.0, run.line_y), 1.0);
                out.push(Placed {
                    key: p.cache_key,
                    x: p.x,
                    y: p.y,
                });
            }
        }
        (out, width)
    }

    fn term_attrs(family: &str, style: u8) -> Attrs<'_> {
        Attrs::new()
            .family(Family::Name(family))
            .weight(if style & BOLD != 0 {
                Weight::BOLD
            } else {
                Weight::NORMAL
            })
            .style(if style & ITALIC != 0 {
                Style::Italic
            } else {
                Style::Normal
            })
    }

    fn emit(&mut self, out: &mut Vec<Instance>, glyphs: &[Placed], x: f32, y: f32, color: Color, clip: Rect) {
        for p in glyphs {
            let Some(g) = self.atlas.get(&mut self.fs, &mut self.swash, p.key) else {
                continue;
            };
            let r = Rect::new(x + (p.x + g.left) as f32, y + (p.y - g.top) as f32, g.uv[2], g.uv[3]);
            out.push(Instance::glyph(r, g.uv, color, g.color).clip(clip));
        }
    }

    /// Draw one terminal cell's text, at font size `size_px`, with its top-left corner at (x, y).
    pub fn draw_cell(
        &mut self,
        out: &mut Vec<Instance>,
        size_px: f32,
        x: f32,
        y: f32,
        c: char,
        zerowidth: Option<&[char]>,
        style: u8,
        color: Color,
        clip: Rect,
    ) {
        let (cw, ch) = self.cell(size_px);
        if zerowidth.is_none() && draw_builtin(out, c, x, y, cw, ch, color, clip) {
            return;
        }
        if !self.sizes.contains_key(&size_px.to_bits()) && self.sizes.len() >= 8 {
            // Many distinct sizes in play: drop the caches rather than grow without bound.
            self.sizes.clear();
        }
        let cache = self.sizes.entry(size_px.to_bits()).or_default();
        let glyphs = match zerowidth {
            None => match cache.glyphs.get(&(c, style)) {
                Some(g) => g.clone(),
                None => {
                    let attrs = Self::term_attrs(&self.term_family, style);
                    let (g, _) = Self::shape(&mut self.fs, c.encode_utf8(&mut [0; 4]), &attrs, size_px, ch);
                    let g: Rc<[Placed]> = g.into();
                    cache.glyphs.insert((c, style), g.clone());
                    g
                }
            },
            Some(zw) => {
                let mut s = String::with_capacity(8);
                s.push(c);
                s.extend(zw);
                match cache.clusters.get(&(s.clone(), style)) {
                    Some(g) => g.clone(),
                    None => {
                        let attrs = Self::term_attrs(&self.term_family, style);
                        let (g, _) = Self::shape(&mut self.fs, &s, &attrs, size_px, ch);
                        let g: Rc<[Placed]> = g.into();
                        if cache.clusters.len() > 4096 {
                            cache.clusters.clear();
                        }
                        cache.clusters.insert((s, style), g.clone());
                        g
                    }
                }
            }
        };
        self.emit(out, &glyphs, x, y, color, clip);
    }

    pub fn ui_shape(&mut self, text: &str, size: f32, bold: bool) -> Rc<Shaped> {
        let key = (text.to_string(), size.to_bits(), bold);
        if let Some(s) = self.ui_cache.get(&key) {
            return s.clone();
        }
        let attrs = Attrs::new().family(Family::Name(&self.ui_family)).weight(if bold {
            Weight::SEMIBOLD
        } else {
            Weight::NORMAL
        });
        let (glyphs, width) = Self::shape(&mut self.fs, text, &attrs, size, (size * 1.3).ceil());
        let shaped = Rc::new(Shaped { glyphs, width });
        if self.ui_cache.len() > 2048 {
            self.ui_cache.clear();
        }
        self.ui_cache.insert(key, shaped.clone());
        shaped
    }

    pub fn measure(&mut self, text: &str, size: f32, bold: bool) -> f32 {
        self.ui_shape(text, size, bold).width
    }

    /// Draw UI text vertically centred on `cy`, returning its width.
    pub fn draw_ui(
        &mut self,
        out: &mut Vec<Instance>,
        text: &str,
        x: f32,
        cy: f32,
        size: f32,
        bold: bool,
        color: Color,
        clip: Rect,
    ) -> f32 {
        let shaped = self.ui_shape(text, size, bold);
        let line_h = (size * 1.3).ceil();
        self.emit(out, &shaped.glyphs, x.round(), (cy - line_h / 2.0).round(), color, clip);
        shaped.width
    }

    /// Longest prefix of `text` (plus an ellipsis) that fits in `max_w`.
    pub fn ellipsize(&mut self, text: &str, size: f32, bold: bool, max_w: f32) -> String {
        if self.measure(text, size, bold) <= max_w {
            return text.to_string();
        }
        let chars: Vec<char> = text.chars().collect();
        let (mut lo, mut hi) = (0usize, chars.len());
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            let candidate: String = chars[..mid].iter().collect::<String>() + "…";
            if self.measure(&candidate, size, bold) <= max_w {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        if lo == 0 {
            return String::new();
        }
        chars[..lo].iter().collect::<String>().trim_end().to_string() + "…"
    }

    /// Like [`Self::ellipsize`], but keeps the end of the text (for paths).
    pub fn ellipsize_start(&mut self, text: &str, size: f32, bold: bool, max_w: f32) -> String {
        if self.measure(text, size, bold) <= max_w {
            return text.to_string();
        }
        let chars: Vec<char> = text.chars().collect();
        let (mut lo, mut hi) = (0usize, chars.len());
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            let candidate: String = "…".to_string() + &chars[chars.len() - mid..].iter().collect::<String>();
            if self.measure(&candidate, size, bold) <= max_w {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        "…".to_string() + &chars[chars.len() - lo..].iter().collect::<String>()
    }
}

/// Box-drawing line weights (up, right, down, left): 0 none, 1 light, 2 heavy, 3 double.
fn box_lines(c: char) -> Option<[u8; 4]> {
    const T: [[u8; 4]; 128] = [
        [0, 1, 0, 1],
        [0, 2, 0, 2],
        [1, 0, 1, 0],
        [2, 0, 2, 0],
        [9; 4],
        [9; 4],
        [9; 4],
        [9; 4], // 2500
        [9; 4],
        [9; 4],
        [9; 4],
        [9; 4],
        [0, 1, 1, 0],
        [0, 2, 1, 0],
        [0, 1, 2, 0],
        [0, 2, 2, 0], // 2508
        [0, 0, 1, 1],
        [0, 0, 1, 2],
        [0, 0, 2, 1],
        [0, 0, 2, 2],
        [1, 1, 0, 0],
        [1, 2, 0, 0],
        [2, 1, 0, 0],
        [2, 2, 0, 0], // 2510
        [1, 0, 0, 1],
        [1, 0, 0, 2],
        [2, 0, 0, 1],
        [2, 0, 0, 2],
        [1, 1, 1, 0],
        [1, 2, 1, 0],
        [2, 1, 1, 0],
        [1, 1, 2, 0], // 2518
        [2, 1, 2, 0],
        [2, 2, 1, 0],
        [1, 2, 2, 0],
        [2, 2, 2, 0],
        [1, 0, 1, 1],
        [1, 0, 1, 2],
        [2, 0, 1, 1],
        [1, 0, 2, 1], // 2520
        [2, 0, 2, 1],
        [2, 0, 1, 2],
        [1, 0, 2, 2],
        [2, 0, 2, 2],
        [0, 1, 1, 1],
        [0, 1, 1, 2],
        [0, 2, 1, 1],
        [0, 2, 1, 2], // 2528
        [0, 1, 2, 1],
        [0, 1, 2, 2],
        [0, 2, 2, 1],
        [0, 2, 2, 2],
        [1, 1, 0, 1],
        [1, 1, 0, 2],
        [1, 2, 0, 1],
        [1, 2, 0, 2], // 2530
        [2, 1, 0, 1],
        [2, 1, 0, 2],
        [2, 2, 0, 1],
        [2, 2, 0, 2],
        [1, 1, 1, 1],
        [1, 1, 1, 2],
        [1, 2, 1, 1],
        [1, 2, 1, 2], // 2538
        [2, 1, 1, 1],
        [1, 1, 2, 1],
        [2, 1, 2, 1],
        [2, 1, 1, 2],
        [2, 2, 1, 1],
        [1, 1, 2, 2],
        [1, 2, 2, 1],
        [2, 2, 1, 2], // 2540
        [1, 2, 2, 2],
        [2, 1, 2, 2],
        [2, 2, 2, 1],
        [2, 2, 2, 2],
        [9; 4],
        [9; 4],
        [9; 4],
        [9; 4], // 2548
        [0, 3, 0, 3],
        [3, 0, 3, 0],
        [0, 3, 1, 0],
        [0, 1, 3, 0],
        [0, 3, 3, 0],
        [0, 0, 1, 3],
        [0, 0, 3, 1],
        [0, 0, 3, 3], // 2550
        [1, 3, 0, 0],
        [3, 1, 0, 0],
        [3, 3, 0, 0],
        [1, 0, 0, 3],
        [3, 0, 0, 1],
        [3, 0, 0, 3],
        [1, 3, 1, 0],
        [3, 1, 3, 0], // 2558
        [3, 3, 3, 0],
        [1, 0, 1, 3],
        [3, 0, 3, 1],
        [3, 0, 3, 3],
        [0, 3, 1, 3],
        [0, 1, 3, 1],
        [0, 3, 3, 3],
        [1, 3, 0, 3], // 2560
        [3, 1, 0, 1],
        [3, 3, 0, 3],
        [1, 3, 1, 3],
        [3, 1, 3, 1],
        [3, 3, 3, 3],
        [8; 4],
        [8; 4],
        [8; 4], // 2568
        [8; 4],
        [9; 4],
        [9; 4],
        [9; 4],
        [0, 0, 0, 1],
        [1, 0, 0, 0],
        [0, 1, 0, 0],
        [0, 0, 1, 0], // 2570
        [0, 0, 0, 2],
        [2, 0, 0, 0],
        [0, 2, 0, 0],
        [0, 0, 2, 0],
        [0, 2, 0, 1],
        [1, 0, 2, 0],
        [0, 1, 0, 2],
        [2, 0, 1, 0], // 2578
    ];
    let i = (c as u32).checked_sub(0x2500)? as usize;
    T.get(i).copied()
}

/// Render box-drawing and block-element characters geometrically so they tile seamlessly.
fn draw_builtin(out: &mut Vec<Instance>, c: char, x: f32, y: f32, w: f32, h: f32, color: Color, clip: Rect) -> bool {
    let cell = Rect::new(x, y, w, h);
    let mut push = |r: Rect, col: Color| out.push(Instance::rect(r, col).clip(clip));
    let light = (w / 8.0).round().max(1.0);
    let heavy = (light * 2.0).max(2.0);
    let cx = (x + (w - light) / 2.0).round();
    let cy = (y + (h - light) / 2.0).round();

    if let Some(lines) = box_lines(c) {
        if lines.iter().any(|&l| l >= 3) {
            if lines[0] == 8 {
                // Rounded corners: draw a quarter of a rounded outline clipped to the cell.
                let r = (w.min(h) / 2.0).round();
                let (ox, oy) = match c {
                    '╭' => (cx, cy),
                    '╮' => (cx + light - 4.0 * w, cy),
                    '╯' => (cx + light - 4.0 * w, cy + light - 4.0 * h),
                    _ => (cx, cy + light - 4.0 * h), // '╰'
                };
                let cell_clip = intersect(cell, clip);
                out.push(
                    Instance::rounded(Rect::new(ox, oy, 4.0 * w, 4.0 * h), [0.0; 4], r)
                        .border(light, color)
                        .clip(cell_clip),
                );
                return true;
            }
            // Doubles and dashes look right from the font.
            return false;
        }
        let thick = |l: u8| if l == 2 { heavy } else { light };
        // Each arm runs from the cell edge to (and through) the centre.
        let [up, right, down, left] = lines;
        let vx = |l: u8| (x + (w - thick(l)) / 2.0).round();
        let hy = |l: u8| (y + (h - thick(l)) / 2.0).round();
        // Arms meet so that they exactly cover the band of the crossing lines.
        let horiz: Vec<u8> = [left, right].into_iter().filter(|&l| l > 0).collect();
        let vert: Vec<u8> = [up, down].into_iter().filter(|&l| l > 0).collect();
        let h_top = horiz.iter().map(|&l| hy(l)).reduce(f32::min);
        let h_bot = horiz.iter().map(|&l| hy(l) + thick(l)).reduce(f32::max);
        let v_left = vert.iter().map(|&l| vx(l)).reduce(f32::min);
        let v_right = vert.iter().map(|&l| vx(l) + thick(l)).reduce(f32::max);
        if up > 0 {
            let end = h_bot.unwrap_or(hy(up) + thick(up));
            push(Rect::new(vx(up), y, thick(up), end - y), color);
        }
        if down > 0 {
            let start = h_top.unwrap_or(hy(down));
            push(Rect::new(vx(down), start, thick(down), y + h - start), color);
        }
        if left > 0 {
            let end = v_right.unwrap_or(vx(left) + thick(left));
            push(Rect::new(x, hy(left), end - x, thick(left)), color);
        }
        if right > 0 {
            let start = v_left.unwrap_or(vx(right));
            push(Rect::new(start, hy(right), x + w - start, thick(right)), color);
        }
        return true;
    }

    let cp = c as u32;
    let frac = |n: f32| (n / 8.0 * h).round();
    match cp {
        0x2580 => push(Rect::new(x, y, w, frac(4.0)), color),
        0x2581..=0x2588 => {
            let fh = frac((cp - 0x2580) as f32);
            push(Rect::new(x, y + h - fh, w, fh), color)
        }
        0x2589..=0x258f => {
            let fw = (w * (0x2590 - cp) as f32 / 8.0).round();
            push(Rect::new(x, y, fw, h), color)
        }
        0x2590 => push(Rect::new(x + (w / 2.0).round(), y, w - (w / 2.0).round(), h), color),
        0x2591..=0x2593 => push(cell, crate::theme::alpha(color, (cp - 0x2590) as f32 * 0.25)),
        0x2594 => push(Rect::new(x, y, w, frac(1.0)), color),
        0x2595 => {
            let fw = (w / 8.0).round().max(1.0);
            push(Rect::new(x + w - fw, y, fw, h), color)
        }
        0x2596..=0x259f => {
            // Quadrants as bits: upper-left, upper-right, lower-left, lower-right.
            const Q: [u8; 10] = [
                0b0010, 0b0001, 0b1000, 0b1011, 0b1001, 0b1110, 0b1101, 0b0100, 0b0110, 0b0111,
            ];
            let bits = Q[(cp - 0x2596) as usize];
            let hw = (w / 2.0).round();
            let hh = (h / 2.0).round();
            if bits & 0b1000 != 0 {
                push(Rect::new(x, y, hw, hh), color);
            }
            if bits & 0b0100 != 0 {
                push(Rect::new(x + hw, y, w - hw, hh), color);
            }
            if bits & 0b0010 != 0 {
                push(Rect::new(x, y + hh, hw, h - hh), color);
            }
            if bits & 0b0001 != 0 {
                push(Rect::new(x + hw, y + hh, w - hw, h - hh), color);
            }
        }
        _ => return false,
    }
    true
}

pub fn intersect(a: Rect, b: Rect) -> Rect {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    let r = a.right().min(b.right());
    let btm = a.bottom().min(b.bottom());
    Rect::new(x, y, (r - x).max(0.0), (btm - y).max(0.0))
}
