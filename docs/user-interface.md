# The user interface

The screen has four rows of parts: the title bar, the body, the status line and
the footer. The body holds the grid, and it also holds a panel when the user
opens one. An overlay covers the middle of the screen. Peruse draws each frame
from the start, and the cost of one frame follows the number of cells on the
screen. The code is in `ui.rs`, `grid.rs`, `panels.rs`, `overlays.rs` and
`input.rs`.

## The layout of the frame

The function `ui::draw` divides the screen into four parts:

| Part | Height | Contents |
|---|---|---|
| The title bar | 1 row | The name of the file, the size of the view and the state |
| The body | The rest | The grid, and a panel when one is open |
| The status line | 1 row | A message, the prompt, or the position of the cursor |
| The footer | 1 row | The keys for the current mode |

When a panel is open, the body divides again. A terminal of 100 screen columns
or more puts the panel at the right side, with a width of 46 screen columns. A
narrow terminal puts the panel below the grid, with a height of 12 rows.

The layout can give an area of zero height when the terminal is very small.
Each draw function tests for that case first, because Peruse cannot draw in
such an area.

## The title bar

The title bar shows these items from the left side:

1. The name `peruse`
2. The name of the file, or a name and the number of the other files
3. The size of the view, as the number of rows and the number of columns
4. The size of the file on the disk, and the format
5. A word for each part of the view that removes rows or columns: `filtered`,
   `query`, the sort column with an arrow, and the number of hidden columns

At the right side, the title bar shows the work of the worker or the name of
the theme. The user must always know why the grid shows fewer rows than the
file holds.

## The grid

The module `grid.rs` writes each character into the buffer of the frame. It
does not use the widget `Table` of ratatui, because the grid needs control of
each cell. The widget does not give these four things:

- a scroll to the left and to the right, one column at a time
- a column of row numbers that always stays on the screen
- a color for each family of values
- a different color for the part of a cell that a search matches

### The layout of the columns

The function `grid::layout` runs before the draw operation, because the header
and the body both need the result. The function does these steps:

1. It calculates the width of the column of row numbers, from the largest row
   number on the screen.
2. It moves the first column to the right until the cursor column fits on the
   screen.
3. It gives the columns that fit, with a position and a width for each one.

The header shows a sign at the left edge or at the right edge when more columns
are outside the screen.

### The rows

For each row on the screen, the grid does these steps:

- It skips a row after the last row of the view. Such a row stays empty, and it
  must not show an old value from a previous view.
- It draws the row number at the right side of the gutter. The first row of the
  view has the number 1.
- It draws a row of points when the row is not in the current page. The grid
  therefore stays steady during a fast scroll.
- It draws each cell with the color of its family of values. A NULL value gets
  the word `NULL` and its own color.
- It paints the part of a cell that the search matches. The match keeps its own
  colors on each background color.

A scroll bar comes at the right edge when the terminal is wider than 40 screen
columns and the view has more rows than the screen.

### The header

The name of a column goes on the same side as the values of that column. A
column of numbers is therefore one group at the right side. The type character
goes to the other side, but only when the name does not fill the column.

The header of the sort column shows an arrow, and the header of the cursor
column gets the accent color.

## The panels

The key `m` opens the metadata panel. The panel shows the rows from
`FileMeta::summary_rows`, and then a list of the columns. The list follows the
cursor of the grid, so the keys `h` and `l` scroll the panel and the grid
together. For a Parquet file, each column shows its percentage of NULL values.
For a CSV file, each column shows its type instead.

At the end, the panel shows the `read_parquet` call or the `read_csv` call that
reads the same rows outside Peruse. The user can look at the data and then copy
that call into a script.

The key `i` opens the column statistics panel. The panel shows these parts:

- the rows from `ColumnStats::rows`: the type, the count, the NULL values, the
  number of different values, the minimum, the maximum, the mean and the
  deviation
- a small chart of the distribution, for a column of numbers, with the two
  edges below it
- the most frequent values, with a bar for each count
- a note when a filter is active, because the statistics then describe the rows
  that the filter keeps

In a column of keys, each frequent value has the count 1. The panel then shows
a short text instead of the bars, because a row of full bars would show a
frequency that the data does not have.

## The overlays

| Overlay | Key | Contents |
|---|---|---|
| The help | `?` or `F1` | Each command, in groups, with the keys of the prompt and two notes |
| The command palette | `:` or `Ctrl-P` | A prompt and the list of the commands that match |
| The theme picker | `T` | The themes, with a sample of the colors of each one |
| The cell inspector | `Enter` | The complete value of one cell, on some lines |
| The record view | `r` | One row of the grid, with one column on each line |
| The filter builder | `f` | The list of conditions, and the menus that make one |

The keys `j` and `k` scroll an overlay, and the key `Esc` closes it. In the
cell inspector, the key `y` copies the value.

The theme picker changes the theme at each move, so the user sees the theme
immediately. The key `Esc` gives the previous theme back.

An overlay that takes text gives the position of the terminal cursor back to
`ui::draw`. The caller therefore needs no second calculation of the layout. The
palette is older than that rule, and `ui.rs` repeats its calculation.

### The record view

The grid reads a row from the left to the right. A file with 300 columns
therefore needs 300 presses of a key to read one row. The record view puts the
columns under each other instead.

The values come from the page that the grid holds already, through
`App::record_value`. The view therefore costs no query and opens immediately.

| Key | Action |
|---|---|
| `j` `k`, `↑` `↓` | Move to another field. |
| `PgUp` `PgDn`, `g` `G` | Move by ten fields, or to the ends. |
| `n` `p`, `→` `←` | The next row, the previous row. The selected field stays. |
| `/` | Find a field by name. |
| `Enter` | Show the complete value in the cell inspector. |
| `y` `Y` | Copy the value, or the complete record. |
| `=` `!` | Keep, or remove, the rows with this value. |
| `Esc` `q` `r` | Close. |

Three rules control what the view shows:

- A column that the grid hides is still in the view, in a dim color. The user
  opens this view to see the complete row, and a hidden column is exactly the
  column that the user cannot see in the grid.
- Three cases look the same in a plain grid, and they are not the same. The
  view writes `NULL` for a missing value, `(empty)` for a text with no
  character, and `…` for a value that the engine has not read yet.
- The width of the name column comes from each column of the file, and not
  from the columns that the find box keeps. The list therefore does not move
  sideways while the user types.

When the view closes, the cursor of the grid moves to the field that the view
was on. The user does not lose the place. A hidden column is the one exception:
the cursor must always be on a column that the user can see.

The cell inspector can come from the record view. The value `cell_from_record`
holds that fact, so the key `Esc` goes back to the record and not to the grid.

### The filter builder

The builder has five steps, and it draws each step in the same box. The
document [filter.md](filter.md) describes the steps and the model behind them.

## The prompt

The module `input.rs` holds an editor of one line. The filter prompt, the SQL
prompt, the search prompt and the row-number prompt all use it. The prompt
takes the place of the status line.

Each position in the editor counts characters, and not bytes. A user can filter
on a value with characters that are not ASCII characters, and a viewer of data
sees such values frequently. A position in bytes would then be wrong.

The editor accepts these keys:

| Key | Operation |
|---|---|
| `Enter` | Apply the text |
| `Esc` or `Ctrl-C` | Close the prompt |
| `←` `→` `Home` `End` | Move the cursor |
| `↑` `↓` | Move through the history |
| `Backspace` `Delete` | Delete one character |
| `Ctrl-A` `Ctrl-E` | Move to the start or to the end |
| `Ctrl-U` `Ctrl-K` | Delete to the start or to the end |
| `Ctrl-W` | Delete the word in front of the cursor |
| `Alt-B` `Alt-F` | Move one word |

The history holds 200 lines at the most. It does not keep an empty line. A line
that the user runs again moves to the end of the history. When the user moves
through the history and then past the newest line, the editor gives the line of
the user back.

The filter prompt and the SQL prompt get colors from the module `sqlhl.rs`.
That module finds the values, the comments, the numbers and the keywords. The
user therefore sees a quotation mark with no partner as soon as the user types
it. Peruse also checks the expression after each key, and it shows the error at
the right side of the row.

## The status line and the footer

With no prompt and no message, the status line shows the name and the type of
the column under the cursor, the position of that column, and the position of
the cursor row. A message takes that place, with a sign for its kind.

The footer shows the keys for the current mode. In the normal mode, the keys
come from the list `FOOTER_HINTS`. A narrow terminal cuts the list at the end,
so the most important keys come first.

A CSV file with no index has a real limit, and the user must know it. The note
about the key `I` therefore takes its space first, and the key hints fill the
space that stays.
