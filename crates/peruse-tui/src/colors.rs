//! Changes the colors of a theme into the colors of a terminal.
//!
//! Some terminals cannot show 24-bit color. Examples are an old terminal, a
//! session with `TERM=xterm-256color`, and some shells of a build system. On
//! such a terminal, Peruse changes each color to the nearest color of the
//! xterm-256 set. Without that change, the colors look weak.

use peruse_core::theme::Color as CoreColor;
use ratatui::style::Color;

/// The number of colors that the terminal can show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Depth {
    /// The terminal can show 24-bit color.
    True,
    /// The terminal can show the 256 colors of the xterm set.
    Indexed256,
}

impl Depth {
    /// Finds the number of colors of the terminal from the environment
    /// variables.
    pub fn detect() -> Depth {
        let truecolor = std::env::var("COLORTERM")
            .map(|v| {
                let v = v.to_ascii_lowercase();
                v.contains("truecolor") || v.contains("24bit")
            })
            .unwrap_or(false);
        if truecolor {
            return Depth::True;
        }
        // Windows Terminal and the new console of Windows show 24-bit
        // color, but they do not set the variable COLORTERM.
        if std::env::var_os("WT_SESSION").is_some() {
            return Depth::True;
        }
        if std::env::var("TERM").is_ok_and(|t| t.contains("direct")) {
            return Depth::True;
        }
        #[cfg(windows)]
        {
            // The old console of Windows is not in a Windows version with
            // support today. Each console that runs this program can read
            // the escape sequences of a terminal.
            if std::env::var_os("TERM").is_none() {
                return Depth::True;
            }
        }
        Depth::Indexed256
    }
}

/// Gives the nearest color of the xterm-256 set.
///
/// A gray color uses the group of 24 grays. That group has more steps than the
/// cube of 6 by 6 by 6 colors, and it therefore gives a better gray.
///
/// The cube is coarse, so two colors that are near each other give one number.
/// A part of the program that must keep two colors apart on such a terminal
/// therefore needs this number. Refer to [`crate::grid::band_focus`].
pub fn to_256(c: CoreColor) -> u8 {
    let (r, g, b) = (c.r as i32, c.g as i32, c.b as i32);
    let spread = r.max(g).max(b) - r.min(g).min(b);
    if spread < 10 {
        let level = (r + g + b) / 3;
        if level < 8 {
            return 16;
        }
        if level > 248 {
            return 231;
        }
        return (232 + (level - 8) * 24 / 240).clamp(232, 255) as u8;
    }
    let q = |v: i32| -> i32 {
        // The steps of the cube are 0, 95, 135, 175, 215 and 255. The
        // distance between two steps is not always the same.
        const LEVELS: [i32; 6] = [0, 95, 135, 175, 215, 255];
        let mut best = 0;
        let mut best_d = i32::MAX;
        for (i, l) in LEVELS.iter().enumerate() {
            let d = (v - l).abs();
            if d < best_d {
                best_d = d;
                best = i as i32;
            }
        }
        best
    };
    (16 + 36 * q(r) + 6 * q(g) + q(b)) as u8
}

/// Changes one color of a theme into a color of the terminal.
pub fn conv(c: CoreColor, depth: Depth) -> Color {
    match depth {
        Depth::True => Color::Rgb(c.r, c.g, c.b),
        Depth::Indexed256 => Color::Indexed(to_256(c)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use peruse_core::theme::rgb;

    #[test]
    fn truecolor_passes_rgb_through() {
        assert_eq!(conv(rgb(0x123456), Depth::True), Color::Rgb(0x12, 0x34, 0x56));
    }

    #[test]
    fn greys_use_the_grey_ramp() {
        let c = to_256(rgb(0x808080));
        assert!((232..=255).contains(&c), "got {c}");
    }

    #[test]
    fn pure_black_and_white_clamp_to_the_cube_ends() {
        assert_eq!(to_256(rgb(0x000000)), 16);
        assert_eq!(to_256(rgb(0xffffff)), 231);
    }

    #[test]
    fn saturated_colours_use_the_colour_cube() {
        let red = to_256(rgb(0xff0000));
        assert_eq!(red, 196, "16 + 36*5");
        let green = to_256(rgb(0x00ff00));
        assert_eq!(green, 46, "16 + 6*5");
        for c in [red, green] {
            assert!((16..232).contains(&c));
        }
    }

    #[test]
    fn quantisation_picks_the_nearest_cube_level_not_the_floor() {
        // The value 0x5f is step 1 of the cube, which is 95. The value
        // 0x60 must also go to that step, and not down to step 0.
        assert_eq!(to_256(rgb(0x600000)), to_256(rgb(0x5f0000)));
    }
}
