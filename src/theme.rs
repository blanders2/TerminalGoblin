//! Colors, built-in color schemes and the UI palette derived from them.

use serde::Deserialize;

/// Straight-alpha sRGB color, components in `0.0..=1.0`.
pub type Color = [f32; 4];

pub fn hex(s: &str) -> Option<Color> {
    let s = s.trim().trim_start_matches('#');
    let parse = |i: usize| u8::from_str_radix(s.get(i..i + 2)?, 16).ok();
    match s.len() {
        6 => Some([
            parse(0)? as f32 / 255.0,
            parse(2)? as f32 / 255.0,
            parse(4)? as f32 / 255.0,
            1.0,
        ]),
        8 => Some([
            parse(0)? as f32 / 255.0,
            parse(2)? as f32 / 255.0,
            parse(4)? as f32 / 255.0,
            parse(6)? as f32 / 255.0,
        ]),
        _ => None,
    }
}

/// Like [`hex`], for compile-time-known literals.
pub fn c(s: &str) -> Color {
    hex(s).unwrap_or([1.0, 0.0, 1.0, 1.0])
}

pub fn alpha(c: Color, a: f32) -> Color {
    [c[0], c[1], c[2], c[3] * a]
}

pub fn mix(a: Color, b: Color, t: f32) -> Color {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3] + (b[3] - a[3]) * t,
    ]
}

pub fn luminance(c: Color) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// Readable text color to put on top of `bg`.
pub fn contrast_text(bg: Color) -> Color {
    if luminance(bg) > 0.55 {
        c("#111318")
    } else {
        c("#f5f7fa")
    }
}

/// The palette offered for tab and group colors.
pub const TAB_COLORS: [(&str, &str); 9] = [
    ("Red", "#f0717a"),
    ("Orange", "#f5a35c"),
    ("Yellow", "#f2cc60"),
    ("Green", "#7ee787"),
    ("Teal", "#56d4dd"),
    ("Blue", "#6cb6ff"),
    ("Purple", "#b392f0"),
    ("Pink", "#f78fcf"),
    ("Grey", "#8b949e"),
];

#[derive(Clone, Debug, Deserialize)]
pub struct SchemeDef {
    pub name: String,
    pub foreground: String,
    pub background: String,
    pub cursor: Option<String>,
    pub selection: Option<String>,
    pub accent: Option<String>,
    pub black: String,
    pub red: String,
    pub green: String,
    pub yellow: String,
    pub blue: String,
    pub magenta: String,
    pub cyan: String,
    pub white: String,
    pub bright_black: String,
    pub bright_red: String,
    pub bright_green: String,
    pub bright_yellow: String,
    pub bright_blue: String,
    pub bright_magenta: String,
    pub bright_cyan: String,
    pub bright_white: String,
}

#[derive(Clone, Debug)]
pub struct Scheme {
    pub name: String,
    pub fg: Color,
    pub bg: Color,
    pub cursor: Color,
    pub selection: Color,
    pub accent: Color,
    /// The 16 ANSI colors.
    pub ansi: [Color; 16],
}

impl Scheme {
    pub fn from_def(d: &SchemeDef) -> Scheme {
        let fg = c(&d.foreground);
        let bg = c(&d.background);
        let ansi = [
            &d.black,
            &d.red,
            &d.green,
            &d.yellow,
            &d.blue,
            &d.magenta,
            &d.cyan,
            &d.white,
            &d.bright_black,
            &d.bright_red,
            &d.bright_green,
            &d.bright_yellow,
            &d.bright_blue,
            &d.bright_magenta,
            &d.bright_cyan,
            &d.bright_white,
        ]
        .map(|s| c(s));
        let cursor = d.cursor.as_deref().and_then(hex).unwrap_or(fg);
        let selection = d.selection.as_deref().and_then(hex).unwrap_or(mix(bg, fg, 0.25));
        let accent = d.accent.as_deref().and_then(hex).unwrap_or(ansi[12]);
        Scheme {
            name: d.name.clone(),
            fg,
            bg,
            cursor,
            selection,
            accent,
            ansi,
        }
    }

    /// Resolve one of the 256 indexed colors.
    pub fn indexed(&self, i: u8) -> Color {
        match i {
            0..=15 => self.ansi[i as usize],
            16..=231 => {
                let i = i - 16;
                let step = |v: u8| {
                    if v == 0 { 0.0 } else { (55.0 + v as f32 * 40.0) / 255.0 }
                };
                [step(i / 36), step((i / 6) % 6), step(i % 6), 1.0]
            }
            _ => {
                let v = (8.0 + (i - 232) as f32 * 10.0) / 255.0;
                [v, v, v, 1.0]
            }
        }
    }
}

/// Colors for the window chrome, derived from the active scheme.
#[derive(Clone, Debug)]
pub struct UiPalette {
    pub titlebar: Color,
    pub text: Color,
    pub text_dim: Color,
    pub hover: Color,
    pub pressed: Color,
    pub border: Color,
    pub accent: Color,
    pub menu_bg: Color,
    pub menu_hover: Color,
    pub danger: Color,
}

impl UiPalette {
    pub fn from_scheme(s: &Scheme) -> UiPalette {
        let dark = luminance(s.bg) < 0.5;
        let black = [0.0, 0.0, 0.0, 1.0];
        let white = [1.0, 1.0, 1.0, 1.0];
        let titlebar = if dark {
            mix(s.bg, black, 0.35)
        } else {
            mix(s.bg, black, 0.06)
        };
        let elevated = if dark {
            mix(s.bg, white, 0.06)
        } else {
            mix(s.bg, white, 0.6)
        };
        UiPalette {
            titlebar,
            text: s.fg,
            text_dim: mix(s.fg, titlebar, 0.45),
            hover: alpha(s.fg, 0.07),
            pressed: alpha(s.fg, 0.12),
            border: alpha(s.fg, 0.10),
            accent: s.accent,
            menu_bg: elevated,
            menu_hover: alpha(s.fg, 0.09),
            danger: c("#e5484d"),
        }
    }
}

macro_rules! scheme {
    ($name:expr, fg $fg:expr, bg $bg:expr, cursor $cur:expr, sel $sel:expr, accent $acc:expr,
     [$($a:expr),* $(,)?]) => {{
        let a = [$($a),*];
        SchemeDef {
            name: $name.into(),
            foreground: $fg.into(),
            background: $bg.into(),
            cursor: Some($cur.into()),
            selection: Some($sel.into()),
            accent: Some($acc.into()),
            black: a[0].into(), red: a[1].into(), green: a[2].into(), yellow: a[3].into(),
            blue: a[4].into(), magenta: a[5].into(), cyan: a[6].into(), white: a[7].into(),
            bright_black: a[8].into(), bright_red: a[9].into(), bright_green: a[10].into(),
            bright_yellow: a[11].into(), bright_blue: a[12].into(), bright_magenta: a[13].into(),
            bright_cyan: a[14].into(), bright_white: a[15].into(),
        }
    }};
}

pub fn builtin_schemes() -> Vec<SchemeDef> {
    vec![
        scheme!("Goblin", fg "#d7dce2", bg "#101318", cursor "#7ee787", sel "#2b4434", accent "#7ee787", [
            "#1b1f27", "#f0717a", "#7ee787", "#f2cc60", "#6cb6ff", "#d2a8ff", "#56d4dd", "#c9d1d9",
            "#5c6370", "#ff8f96", "#a6f3a8", "#ffe08a", "#91cbff", "#e2c5ff", "#8de8ee", "#f5f7fa",
        ]),
        scheme!("Campbell", fg "#cccccc", bg "#0c0c0c", cursor "#ffffff", sel "#3a3a3a", accent "#3b78ff", [
            "#0c0c0c", "#c50f1f", "#13a10e", "#c19c00", "#0037da", "#881798", "#3a96dd", "#cccccc",
            "#767676", "#e74856", "#16c60c", "#f9f1a5", "#3b78ff", "#b4009e", "#61d6d6", "#f2f2f2",
        ]),
        scheme!("One Half Dark", fg "#dcdfe4", bg "#282c34", cursor "#a3b3cc", sel "#474e5d", accent "#61afef", [
            "#282c34", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#dcdfe4",
            "#5a6374", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#dcdfe4",
        ]),
        scheme!("Tokyo Night", fg "#c0caf5", bg "#1a1b26", cursor "#c0caf5", sel "#33467c", accent "#7aa2f7", [
            "#15161e", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#a9b1d6",
            "#414868", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#c0caf5",
        ]),
        scheme!("Catppuccin Mocha", fg "#cdd6f4", bg "#1e1e2e", cursor "#f5e0dc", sel "#45475a", accent "#cba6f7", [
            "#45475a", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#bac2de",
            "#585b70", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#a6adc8",
        ]),
        scheme!("Dracula", fg "#f8f8f2", bg "#282a36", cursor "#f8f8f2", sel "#44475a", accent "#bd93f9", [
            "#21222c", "#ff5555", "#50fa7b", "#f1fa8c", "#bd93f9", "#ff79c6", "#8be9fd", "#f8f8f2",
            "#6272a4", "#ff6e6e", "#69ff94", "#ffffa5", "#d6acff", "#ff92df", "#a4ffff", "#ffffff",
        ]),
        scheme!("Nord", fg "#d8dee9", bg "#2e3440", cursor "#d8dee9", sel "#434c5e", accent "#88c0d0", [
            "#3b4252", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#88c0d0", "#e5e9f0",
            "#4c566a", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#8fbcbb", "#eceff4",
        ]),
        scheme!("Gruvbox Dark", fg "#ebdbb2", bg "#282828", cursor "#ebdbb2", sel "#504945", accent "#fabd2f", [
            "#282828", "#cc241d", "#98971a", "#d79921", "#458588", "#b16286", "#689d6a", "#a89984",
            "#928374", "#fb4934", "#b8bb26", "#fabd2f", "#83a598", "#d3869b", "#8ec07c", "#ebdbb2",
        ]),
    ]
}
