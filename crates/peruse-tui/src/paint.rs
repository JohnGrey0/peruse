//! Changes the colors of a theme into the styles of ratatui.
//!
//! Peruse makes one [`Paint`] value for each frame. That value holds the
//! number of colors of the terminal, so each function here needs one argument
//! less.

use peruse_core::theme::Color as CoreColor;
use ratatui::style::{Modifier, Style};

use crate::colors::{conv, Depth};

/// Makes a style of ratatui from a color of a theme.
#[derive(Clone, Copy)]
pub struct Paint {
    /// The number of colors that the terminal can show.
    pub depth: Depth,
}

impl Paint {
    /// Makes a `Paint` value for a terminal.
    pub fn new(depth: Depth) -> Paint {
        Paint { depth }
    }

    /// Gives a style with a color for the text.
    pub fn fg(&self, c: CoreColor) -> Style {
        Style::default().fg(conv(c, self.depth))
    }

    /// Gives a style with a color for the background.
    pub fn bg(&self, c: CoreColor) -> Style {
        Style::default().bg(conv(c, self.depth))
    }

    /// Gives a style with a color for the text and a color for the background.
    pub fn on(&self, fg: CoreColor, bg: CoreColor) -> Style {
        Style::default()
            .fg(conv(fg, self.depth))
            .bg(conv(bg, self.depth))
    }

    /// Adds thick letters to a style.
    pub fn bold(&self, s: Style) -> Style {
        s.add_modifier(Modifier::BOLD)
    }

}
