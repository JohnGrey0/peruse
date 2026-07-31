# The themes

A theme is a complete set of colors for the screen. The themes are in the crate
`peruse-core`, and not in the terminal front-end. Each color is a group of
three numbers for red, green and blue, and no type comes from a terminal
library. An author writes a theme in a short form with 13 colors, and Peruse
calculates the 32 roles of the user interface from that form. A new theme
therefore needs about fifteen lines. The code is in
`crates/peruse-core/src/theme.rs`.

## The color

The structure `Color` holds three numbers: `r`, `g` and `b`. The function `rgb`
makes a color from one number, such as `0xff8000`. The structure has three
methods:

| Method | Result |
|---|---|
| `mix(other, pct)` | A mix of two colors. The value `pct` is the percentage of `other`. |
| `luma()` | The brightness, from the formula of the standard Rec. 601. |
| `hex()` | The color in the form `#rrggbb`. |

Peruse uses the brightness to select a text color that the user can read on a
given background color.

## The short form

The structure `Base` is the short form. It holds these fields:

| Field | Meaning |
|---|---|
| `name` | The name of the theme |
| `dark` | `true` when the background color is dark |
| `bg` | The background color of the grid |
| `bg_alt` | The background color of the title bar and the panels |
| `fg` | The color of the text |
| `dim` | The color of the text that is less important |
| `border` | The color of the borders |
| `sel` | The background color of the row under the cursor |
| `red` | The color for an error |
| `green` | The color for a value of text |
| `yellow` | The color for a date or a time |
| `blue` | The color for a link and for the parts with the focus |
| `magenta` | The color for a value that is true or false |
| `cyan` | The color for a number |
| `orange` | The color for a warning and for a value of bytes |

## The 32 roles

The function `Theme::from_base` calculates each role. Most roles take a color
of the short form directly:

| Group | Roles |
|---|---|
| The frame | `bg`, `bg_alt`, `fg`, `dim`, `border`, `accent` |
| The grid | `header_fg`, `header_bg`, `gutter_fg`, `gutter_bg`, `cursor_row`, `cursor_cell`, `sel_col`, `match_bg`, `match_fg` |
| The values | `null`, `number`, `string`, `boolean`, `temporal`, `binary`, `nested` |
| The messages | `error`, `warn`, `ok` |
| The status line | `status_fg`, `status_bg`, `key_fg` |
| The SQL prompt | `kw`, `lit`, `ident`, `comment` |

Four roles need a calculation:

| Role | Calculation | Reason |
|---|---|---|
| `cursor_cell` | `sel` mixed with 35 % of `blue` | The cell under the cursor must look different from its row. |
| `sel_col` | `sel` mixed with 55 % of `bg` | The column under the cursor needs a weak background color. |
| `nested` | `magenta` mixed with 40 % of `blue` | A nested value needs its own color. |
| `match_fg` | A dark text on a bright `yellow`, or a light text on a dark `yellow` | The text of a match must always be readable. |

## The rules for a built-in theme

Tests in `theme.rs` check each built-in theme:

- The difference of the brightness between `match_bg` and `match_fg` is more
  than 80.
- The difference between `fg` and `bg` is more than 100. The smallest value in
  the list is 125, from `solarized-dark`.
- The difference between `fg` and `bg_alt` is more than 100. The header of the
  grid, the title bar and the status line put `fg` on `bg_alt`. The smallest
  value in the list is 115, from `solarized-dark`.
- The difference between `dim` and `bg` is more than 60. This value is smaller
  than the value for `fg`, because a dim text must look weak. The smallest
  value in the list is 70, from `tokyo-night`.
- A dark theme has a background that is darker than its text. A light theme has
  a background that is brighter than its text. This test finds a `dark` flag
  with the wrong value.
- Six themes or more are light.
- No two themes have the same name, and each name uses lower case letters,
  digits and hyphens only. The option `--theme catppuccin-mocha` therefore
  works.
- The function `builtin` finds each theme in the list, in any case of the
  letters.

The Zenburn palette is not in the list. Its gray for a comment, `#656555`, on
its background, `#3f3f3f`, gives a difference of 36 only, and the dim text is
therefore difficult to read.

## The built-in themes

Peruse holds 25 themes. Nine of them are light, because a user of a light
terminal also needs a choice.

| Group | Dark | Light |
|---|---|---|
| Peruse | `peruse-dark` (the default theme) | `peruse-light` |
| A dark form only | `nord`, `dracula`, `monokai`, `kanagawa` | - |
| Gruvbox | `gruvbox-dark` | `gruvbox-light` |
| Solarized | `solarized-dark` | `solarized-light` |
| Catppuccin | `catppuccin-mocha`, `catppuccin-frappe` | `catppuccin-latte` |
| Tokyo Night | `tokyo-night` | `tokyo-night-day` |
| One, from the Atom text editor | `one-dark` | `one-light` |
| Everforest | `everforest-dark` | `everforest-light` |
| Rose Pine | `rose-pine` | `rose-pine-dawn` |
| GitHub | `github-dark` | `github-light` |
| A special need | `high-contrast`, `mono` | - |

The list in the code keeps this order, and the key `t` therefore moves through
the groups in this order. The first theme must stay `peruse-dark`, because it is
the default theme.

The colors of a group come from the palette of that project. Five groups need a
decision, because a palette does not always give one color for each role:

- Monokai: the palette of the editor has no cyan. The yellow and the cyan
  therefore come from the Monokai palette for a terminal.
- Rose Pine: the palette gives six accent colors, but a theme needs seven. The
  color foam therefore does the work of the blue and of the cyan.
- Kanagawa: the red is peachRed, because the color samuraiRed is too dark on
  the background.
- Gruvbox and Everforest: each palette gives three levels of background, and
  Peruse uses the medium level.

The function `builtin_names` gives the names. The function `builtin` finds one
theme by its name, and it ignores the case of the letters.

## How to write a theme file

Peruse reads a theme file in the TOML format from the directory
`<config>/peruse/themes`. The function `user_theme_dir` gives that directory.
To see the directory on your system, run `peruse --list-themes`.

A file names only the roles that it changes. The line `extends` gives the theme
to start from. Without that line, the file starts from `peruse-dark`.

```toml
# <config>/peruse/themes/my-theme.toml
extends = "nord"

name = "my-theme"
dark  = true

number = "#ff8000"
string = "#a3be8c"
null   = "#4c566a"
```

The rules for a file are these:

- A color is a text in the form `#rrggbb` or `#rgb`. The number sign is
  optional. The form `#f80` becomes `#ff8800`.
- The file must hold no unknown field. An unknown field gives an error with the
  name of the file.
- Without a `name` field, the name of the theme comes from the name of the
  file.

A theme of the user replaces a built-in theme with the same name. The function
`available` gives the built-in themes first, and then the themes of the user in
the order of their names.

## How to select a theme

The user has three ways to select a theme:

- The option `--theme` accepts the name of a built-in theme, the name of a
  theme of the user, or the path of a `.toml` file. The function `resolve` does
  this work.
- The key `t` gives the next theme in the list.
- The key `T` opens the theme picker. The picker shows the name of each theme
  and a sample of its colors for the families of values. The keys `↑` and `↓`
  change the theme immediately, the key `Enter` keeps the theme, and the key
  `Esc` gives the previous theme back.

The option `--list-themes` prints each name and then stops the program.

## The colors of the terminal

The module `crates/peruse-tui/src/colors.rs` changes a theme color into a
terminal color. The enumeration `Depth` holds the number of colors of the
terminal.

The function `Depth::detect` gives `True` for 24-bit color in these four cases:

1. The variable `COLORTERM` holds `truecolor` or `24bit`.
2. The variable `WT_SESSION` exists. Windows Terminal shows 24-bit color, but
   it does not set `COLORTERM`.
3. The variable `TERM` holds `direct`.
4. The system is Windows and the variable `TERM` does not exist.

In each other case, the function gives `Indexed256`. Peruse then changes each
color to the nearest color of the xterm-256 set. Without that change, the
colors look weak on an old terminal.

The function `to_256` uses two methods:

- For a gray color, it uses the group of 24 grays. That group has more steps
  than the cube of colors, and it therefore gives a better gray.
- For each other color, it uses the cube of 6 by 6 by 6 colors. The steps of
  the cube are 0, 95, 135, 175, 215 and 255. The distance between two steps is
  not always the same, so the function selects the nearest step.
