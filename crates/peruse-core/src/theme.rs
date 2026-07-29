//! The color palettes.
//!
//! The themes are in this crate, and not in the terminal front-end. Each color
//! is a group of three numbers for red, green and blue. No type here comes
//! from a terminal library. A future front-end with a graphical user interface
//! can therefore use the same themes and the same theme files.
//!
//! An author writes a theme as a short [`Base`]: a background color, a text
//! color and some other colors. The function [`Theme::from_base`] then
//! calculates the 32 roles that the user interface paints with. Each built-in
//! theme is therefore consistent, and a new theme needs about fifteen lines.

use serde::Deserialize;
use std::path::{Path, PathBuf};

/// One color, as a quantity of red, of green and of blue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    /// The quantity of red.
    pub r: u8,
    /// The quantity of green.
    pub g: u8,
    /// The quantity of blue.
    pub b: u8,
}

/// Makes a color from one number, such as `0xff8000`.
pub const fn rgb(v: u32) -> Color {
    Color {
        r: (v >> 16) as u8,
        g: (v >> 8) as u8,
        b: v as u8,
    }
}

impl Color {
    /// Mixes two colors. The value `pct` is the percentage of `other` in the
    /// result. The value 0 gives `self`, and the value 100 gives `other`.
    pub fn mix(self, other: Color, pct: u32) -> Color {
        let f = |a: u8, b: u8| -> u8 {
            let a = a as u32;
            let b = b as u32;
            ((a * (100 - pct) + b * pct) / 100) as u8
        };
        Color {
            r: f(self.r, other.r),
            g: f(self.g, other.g),
            b: f(self.b, other.b),
        }
    }

    /// Gives the brightness of the color, from the formula of the standard
    /// Rec. 601. Peruse uses the brightness to select a text color that the
    /// user can read on a given background color.
    pub fn luma(self) -> u32 {
        (self.r as u32 * 299 + self.g as u32 * 587 + self.b as u32 * 114) / 1000
    }

    /// Writes the color in the form `#rrggbb`.
    pub fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

/// Reads a color from the form `#rrggbb` or the form `#rgb`. The number sign is
/// optional.
fn parse_hex(s: &str) -> Option<Color> {
    let s = s.trim().trim_start_matches('#');
    match s.len() {
        6 => u32::from_str_radix(s, 16).ok().map(rgb),
        3 => {
            let v = u32::from_str_radix(s, 16).ok()?;
            // Repeat each digit: #abc becomes #aabbcc.
            let (r, g, b) = ((v >> 8) & 0xf, (v >> 4) & 0xf, v & 0xf);
            Some(rgb((r * 0x11) << 16 | (g * 0x11) << 8 | (b * 0x11)))
        }
        _ => None,
    }
}

/// Declares the list of roles one time.
///
/// The macro then builds two structures from that list: the theme that the
/// program uses, and the theme that Peruse reads from a TOML file.
macro_rules! define_theme {
    ($($field:ident),* $(,)?) => {
        /// A complete set of colors. Each field is one role.
        #[derive(Clone, Debug)]
        pub struct Theme {
            /// The name of the theme.
            pub name: String,
            /// `true` when the background color is dark.
            pub dark: bool,
            $(pub $field: Color,)*
        }

        /// A theme file, before Peruse applies it to a base theme.
        #[derive(Debug, Default, Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawTheme {
            name: Option<String>,
            dark: Option<bool>,
            /// The name of the built-in theme to start from.
            extends: Option<String>,
            $($field: Option<String>,)*
        }

        impl Theme {
            /// Applies the fields of a theme file to this theme. A field that
            /// the file does not name keeps its value.
            fn apply(&mut self, raw: &RawTheme) -> Result<(), String> {
                if let Some(n) = &raw.name { self.name = n.clone(); }
                if let Some(d) = raw.dark { self.dark = d; }
                $(
                    if let Some(s) = &raw.$field {
                        self.$field = parse_hex(s)
                            .ok_or_else(|| format!("{}: not a hex colour: {s:?}", stringify!($field)))?;
                    }
                )*
                Ok(())
            }

            /// Gives the name and the color of each role. The theme picker
            /// uses this list to show the colors of a theme.
            pub fn roles(&self) -> Vec<(&'static str, Color)> {
                vec![$((stringify!($field), self.$field),)*]
            }
        }
    };
}

define_theme!(
    // The frame: the title bar, the borders and the background.
    bg,
    bg_alt,
    fg,
    dim,
    border,
    accent,
    // The grid.
    header_fg,
    header_bg,
    gutter_fg,
    gutter_bg,
    cursor_row,
    cursor_cell,
    sel_col,
    match_bg,
    match_fg,
    // The families of values.
    null,
    number,
    string,
    boolean,
    temporal,
    binary,
    nested,
    // The messages.
    error,
    warn,
    ok,
    // The status line and the footer.
    status_fg,
    status_bg,
    key_fg,
    // The SQL prompt.
    kw,
    lit,
    ident,
    comment,
);

/// The short form that an author writes a theme in.
#[derive(Clone, Copy, Debug)]
pub struct Base {
    /// The name of the theme.
    pub name: &'static str,
    /// `true` when the background color is dark.
    pub dark: bool,
    /// The background color of the grid.
    pub bg: Color,
    /// The background color of the title bar and the panels.
    pub bg_alt: Color,
    /// The color of the text.
    pub fg: Color,
    /// The color of the text that is less important.
    pub dim: Color,
    /// The color of the borders.
    pub border: Color,
    /// The background color of the row under the cursor.
    pub sel: Color,
    /// The color for an error.
    pub red: Color,
    /// The color for a value of text.
    pub green: Color,
    /// The color for a date or a time.
    pub yellow: Color,
    /// The color for a link and for the parts with the focus.
    pub blue: Color,
    /// The color for a value that is true or false.
    pub magenta: Color,
    /// The color for a number.
    pub cyan: Color,
    /// The color for a warning and for a value of bytes.
    pub orange: Color,
}

impl Theme {
    /// Calculates each role of a theme from a short [`Base`].
    pub fn from_base(b: &Base) -> Theme {
        Theme {
            name: b.name.to_string(),
            dark: b.dark,

            bg: b.bg,
            bg_alt: b.bg_alt,
            fg: b.fg,
            dim: b.dim,
            border: b.border,
            accent: b.blue,

            header_fg: b.fg,
            header_bg: b.bg_alt,
            gutter_fg: b.dim,
            gutter_bg: b.bg,
            cursor_row: b.sel,
            cursor_cell: b.sel.mix(b.blue, 35),
            sel_col: b.sel.mix(b.bg, 55),
            match_bg: b.yellow,
            match_fg: if b.yellow.luma() > 140 { rgb(0x101010) } else { rgb(0xf0f0f0) },

            null: b.dim,
            number: b.cyan,
            string: b.green,
            boolean: b.magenta,
            temporal: b.yellow,
            binary: b.orange,
            nested: b.magenta.mix(b.blue, 40),

            error: b.red,
            warn: b.orange,
            ok: b.green,

            status_fg: b.fg,
            status_bg: b.bg_alt,
            key_fg: b.blue,

            kw: b.magenta,
            lit: b.green,
            ident: b.fg,
            comment: b.dim,
        }
    }
}

/// The themes that Peruse holds inside the program.
pub static BUILTINS: &[Base] = &[
    Base {
        name: "peruse-dark",
        dark: true,
        bg: rgb(0x11141a),
        bg_alt: rgb(0x1a1f27),
        fg: rgb(0xd6dbe5),
        dim: rgb(0x6b7484),
        border: rgb(0x2b323d),
        sel: rgb(0x232b36),
        red: rgb(0xef6b73),
        green: rgb(0x8bd49c),
        yellow: rgb(0xe6c384),
        blue: rgb(0x7aa2f7),
        magenta: rgb(0xc490d8),
        cyan: rgb(0x6cc7d9),
        orange: rgb(0xe09a5c),
    },
    Base {
        name: "peruse-light",
        dark: false,
        bg: rgb(0xfcfcfd),
        bg_alt: rgb(0xeef1f5),
        fg: rgb(0x21262e),
        dim: rgb(0x77808f),
        border: rgb(0xd3d9e1),
        sel: rgb(0xe2e8f0),
        red: rgb(0xc0392b),
        green: rgb(0x2c7a4b),
        yellow: rgb(0x9a6b00),
        blue: rgb(0x2b62c4),
        magenta: rgb(0x8b3fa8),
        cyan: rgb(0x0f7b8a),
        orange: rgb(0xb35c1e),
    },
    Base {
        name: "nord",
        dark: true,
        bg: rgb(0x2e3440),
        bg_alt: rgb(0x3b4252),
        fg: rgb(0xd8dee9),
        dim: rgb(0x7b88a1),
        border: rgb(0x434c5e),
        sel: rgb(0x434c5e),
        red: rgb(0xbf616a),
        green: rgb(0xa3be8c),
        yellow: rgb(0xebcb8b),
        blue: rgb(0x81a1c1),
        magenta: rgb(0xb48ead),
        cyan: rgb(0x88c0d0),
        orange: rgb(0xd08770),
    },
    Base {
        name: "dracula",
        dark: true,
        bg: rgb(0x282a36),
        bg_alt: rgb(0x343746),
        fg: rgb(0xf8f8f2),
        dim: rgb(0x6272a4),
        border: rgb(0x44475a),
        sel: rgb(0x44475a),
        red: rgb(0xff5555),
        green: rgb(0x50fa7b),
        yellow: rgb(0xf1fa8c),
        blue: rgb(0x8be9fd),
        magenta: rgb(0xff79c6),
        cyan: rgb(0x8be9fd),
        orange: rgb(0xffb86c),
    },
    Base {
        name: "gruvbox-dark",
        dark: true,
        bg: rgb(0x282828),
        bg_alt: rgb(0x32302f),
        fg: rgb(0xebdbb2),
        dim: rgb(0x928374),
        border: rgb(0x3c3836),
        sel: rgb(0x3c3836),
        red: rgb(0xfb4934),
        green: rgb(0xb8bb26),
        yellow: rgb(0xfabd2f),
        blue: rgb(0x83a598),
        magenta: rgb(0xd3869b),
        cyan: rgb(0x8ec07c),
        orange: rgb(0xfe8019),
    },
    Base {
        name: "solarized-dark",
        dark: true,
        bg: rgb(0x002b36),
        bg_alt: rgb(0x073642),
        fg: rgb(0x93a1a1),
        dim: rgb(0x586e75),
        border: rgb(0x0c4956),
        sel: rgb(0x073642),
        red: rgb(0xdc322f),
        green: rgb(0x859900),
        yellow: rgb(0xb58900),
        blue: rgb(0x268bd2),
        magenta: rgb(0xd33682),
        cyan: rgb(0x2aa198),
        orange: rgb(0xcb4b16),
    },
    Base {
        name: "solarized-light",
        dark: false,
        bg: rgb(0xfdf6e3),
        bg_alt: rgb(0xeee8d5),
        fg: rgb(0x586e75),
        dim: rgb(0x93a1a1),
        border: rgb(0xded8c5),
        sel: rgb(0xeee8d5),
        red: rgb(0xdc322f),
        green: rgb(0x859900),
        yellow: rgb(0xb58900),
        blue: rgb(0x268bd2),
        magenta: rgb(0xd33682),
        cyan: rgb(0x2aa198),
        orange: rgb(0xcb4b16),
    },
    Base {
        name: "high-contrast",
        dark: true,
        bg: rgb(0x000000),
        bg_alt: rgb(0x141414),
        fg: rgb(0xffffff),
        dim: rgb(0x9a9a9a),
        border: rgb(0x555555),
        sel: rgb(0x2a2a2a),
        red: rgb(0xff5f5f),
        green: rgb(0x5fff87),
        yellow: rgb(0xffff5f),
        blue: rgb(0x5fafff),
        magenta: rgb(0xff5fff),
        cyan: rgb(0x5fffff),
        orange: rgb(0xffaf5f),
    },
    Base {
        name: "mono",
        dark: true,
        bg: rgb(0x0d0d0d),
        bg_alt: rgb(0x1a1a1a),
        fg: rgb(0xdadada),
        dim: rgb(0x6f6f6f),
        border: rgb(0x333333),
        sel: rgb(0x262626),
        red: rgb(0xbdbdbd),
        green: rgb(0xbdbdbd),
        yellow: rgb(0xdadada),
        blue: rgb(0xdadada),
        magenta: rgb(0xbdbdbd),
        cyan: rgb(0xbdbdbd),
        orange: rgb(0xdadada),
    },
];

/// Gives the name of each built-in theme.
pub fn builtin_names() -> Vec<&'static str> {
    BUILTINS.iter().map(|b| b.name).collect()
}

/// Finds a built-in theme by its name. The search ignores the case of the
/// letters.
pub fn builtin(name: &str) -> Option<Theme> {
    BUILTINS
        .iter()
        .find(|b| b.name.eq_ignore_ascii_case(name))
        .map(Theme::from_base)
}

impl Default for Theme {
    /// Gives the first built-in theme.
    fn default() -> Self {
        Theme::from_base(&BUILTINS[0])
    }
}

/// Gives the directory `<config>/peruse/themes`. Peruse reads the `.toml`
/// theme files of the user from this directory.
pub fn user_theme_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "peruse").map(|d| d.config_dir().join("themes"))
}

/// Reads a theme file.
///
/// The line `extends = "nord"` starts from a built-in theme. Without that
/// line, the file starts from the default theme. A file therefore names only
/// the roles that it changes.
pub fn load_theme_file(path: &Path) -> Result<Theme, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let raw: RawTheme = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut theme = match raw.extends.as_deref() {
        Some(base) => {
            builtin(base).ok_or_else(|| format!("{}: unknown base theme {base:?}", path.display()))?
        }
        None => Theme::default(),
    };
    if raw.name.is_none()
        && let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            theme.name = stem.to_string();
        }
    theme.apply(&raw).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(theme)
}

/// Gives each theme that Peruse can use.
///
/// The built-in themes come first, and then the themes of the user. A theme of
/// the user replaces a built-in theme with the same name.
pub fn available() -> Vec<Theme> {
    let mut out: Vec<Theme> = BUILTINS.iter().map(Theme::from_base).collect();
    let Some(dir) = user_theme_dir() else {
        return out;
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return out;
    };
    let mut user: Vec<Theme> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        .filter_map(|p| load_theme_file(&p).ok())
        .collect();
    user.sort_by(|a, b| a.name.cmp(&b.name));
    for t in user {
        match out.iter().position(|o| o.name == t.name) {
            Some(i) => out[i] = t,
            None => out.push(t),
        }
    }
    out
}

/// Finds the theme for the option `--theme`.
///
/// The argument is the name of a built-in theme, the name of a theme of the
/// user, or the path of a `.toml` file.
pub fn resolve(name: &str) -> Result<Theme, String> {
    let p = Path::new(name);
    if p.extension().is_some_and(|e| e == "toml") {
        return load_theme_file(p);
    }
    available()
        .into_iter()
        .find(|t| t.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| {
            format!(
                "unknown theme {name:?} (available: {})",
                available()
                    .iter()
                    .map(|t| t.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_forms_parse() {
        assert_eq!(parse_hex("#ff8000"), Some(rgb(0xff8000)));
        assert_eq!(parse_hex("ff8000"), Some(rgb(0xff8000)));
        assert_eq!(parse_hex("#f80"), Some(rgb(0xff8800)));
        assert_eq!(parse_hex("nope"), None);
        assert_eq!(parse_hex("#ff80"), None);
    }

    #[test]
    fn every_builtin_expands() {
        for b in BUILTINS {
            let t = Theme::from_base(b);
            assert_eq!(t.name, b.name);
            assert_eq!(t.roles().len(), 32);
        }
    }

    #[test]
    fn overrides_apply_on_top_of_extends() {
        let raw: RawTheme = toml::from_str("extends = \"nord\"\nnumber = \"#ff0000\"\n").unwrap();
        let mut t = builtin(raw.extends.as_deref().unwrap()).unwrap();
        t.apply(&raw).unwrap();
        assert_eq!(t.number, rgb(0xff0000));
        assert_eq!(t.bg, rgb(0x2e3440), "untouched roles keep the base value");
    }

    #[test]
    fn match_text_stays_readable() {
        for b in BUILTINS {
            let t = Theme::from_base(b);
            let contrast = t.match_bg.luma().abs_diff(t.match_fg.luma());
            assert!(contrast > 80, "{} match_bg/fg too close", t.name);
        }
    }
}
