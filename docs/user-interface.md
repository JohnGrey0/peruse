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

Peruse draws a frame only after something changed. With the mouse on, the
terminal reports each movement of the pointer, and Peruse does nothing with
those events. A frame for each of them would spend the processor of the user to
draw the same screen again.

## The title bar

The title bar shows these items from the left side:

1. The name `peruse`
2. The name of the file, or a name and the number of the other files. For a
   database, the name of the file and the table, such as `shop.duckdb ·
   main.sales`.
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

The width of each column comes from `App::fit_widths`, which measures the name
of the column and the widest value on the screen. The function `fitted_width`
then applies two limits:

- A column is always **five screen columns wider than its name**. The header
  and the band both write a fact after the name, and a column that stops at the
  name has no room for it. The constant is `NAME_HEADROOM` in `app.rs`, and it
  is the larger of the two demands: the header asks for three, and the compact
  band asks for five, because `100%` is four characters and one blank column
  stands in front of it.
- No column is wider than 60 screen columns. One column with a long name must
  not push the other columns off the right edge. This limit comes last, so it
  wins over the room after the name.

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
column of numbers is therefore one group at the right side.

A dim mark goes to the other side of the same screen column group. That mark is
the family of the values: `#` a number, `"` text, `?` a boolean, `@` a date or
a time, `~` binary, `{` a structure, a list or a map. It comes from
`CellKind::badge`, and the help overlay holds a legend for it.

A column shows the mark only when it is three screen columns wider than the
name. Two of those three stay blank between the name and the mark, because with
one blank column the mark reads as the last letter of the name. A column that is
too narrow for both keeps the name and drops the mark, so no cut letter of a
name stands where the mark belongs. A fitted column always has this room, and a
column that the user made narrow can lose it.

The header of the sort column shows an arrow, and the header of the cursor
column gets the accent color.

## The detail band

The key `d` puts a band of facts between the column names and the first row of
data. It answers the first question about a file: what is in each column? The
statistics panel answers that question for one column at a time, at the side.
The band answers it for each column that is on the screen, in the spirit of
`df.info()` and `df.describe()` of pandas.

The key moves through three modes, and the setting `band` keeps the mode
between sessions. The enumeration is `Band` in `app.rs`.

| Mode | Rows | Contents |
|---|---|---|
| `off` | 0 | The band is not on the screen. |
| `compact` | 1 | The type at the left, and the share of NULL values at the right. |
| `detailed` | 4 | The type, the share of NULL values, the count of the different values, and the range from the smallest value to the largest value. |

```text
    #    order_id customer_name    " #    amount_paid ordered_at        @ region    "
    BIGINT        VARCHAR            DOUBLE           TIMESTAMP           VARCHAR
    0% null       0% null            20% null         0% null             0% null
    ~5 distinct   ~5 distinct        ~3 distinct      ~5 distinct         ~3 distinct
    1001 → 1005   alice → erin       7.0 → 4000.75    2024-01… → 2024-01… APAC → US
  1          1001 alice                          10.5 2024-01-01 09:15:00 EU
  2          1002 bob                            NULL 2024-01-02 11:20:00 US
```

Six rules control the band:

- **The band takes the colors of the header, in three levels.** The eye then
  reads the names and the facts as one block that describes the columns, and it
  still finds the column that the cursor is on. The name of that column keeps
  the accent color and thick letters. Its facts take a mix of the accent color
  and the dim color, in thin letters, so they are quieter than the name above
  them. The facts of each other column stay dim. The function
  `grid::band_focus` gives the middle color, and a test holds the three apart in
  each of the 25 built-in themes. That middle color also gives some of its color
  away when it must. A terminal with 256 colors quantises each color to a cube
  with steps as large as 95 of the 255 levels of a channel, and three themes lost
  a level there: the band of the cursor column took the color of the other bands.
  The test therefore holds the three apart after that conversion as well.
- **Every column gives the same fact on the same row.** The facts therefore
  line up across the grid, and the eye can compare one fact over many columns.
  In the compact mode the share of NULL values goes to the right edge of the
  column for the same reason.
- **The detailed band has four rows, and not six.** The mean and the deviation
  are not in it. Only a column of numbers has them, so the row would be empty
  over each column of text, and a row of the grid is expensive. The statistics
  panel shows them. The reason is at `Band::DETAIL_ROWS`.
- **A narrow column keeps the number and drops the word.** The row of the
  different values writes `~3` and not `~3 d…`, because a cut word says less
  than the number by itself. The character `~` says that the count is an
  estimate, from `approx_count_distinct`.
- **A column with no facts yet shows a row of points.** A blank row would read
  as a zero, and a number from another view would be a lie. A share that is not
  zero never shows as `0%`, and a share below 100 never shows as `100%`.
- **A structure, a list and a BLOB have no range.** The row says `no order`,
  because `min()` over one of those gives an error and not a NULL.

### The band gives its rows back to the data

The function `grid::band_rows` decides how many rows the band takes. One row
goes to the column names, one row must stay for the data, and the data always
keeps one half of the rows. A summary of the columns must not become the main
part of the screen.

| Height of the grid | Rows of a detailed band |
|---|---|
| 0, 1, 2 | 0 |
| 3, 4 | 1 |
| 5, 6 | 2 |
| 7, 8 | 3 |
| 9 or more | 4 |

The band writes its rows from the top, so a band that gives rows back keeps the
facts that come first. The value `App::viewport_rows` falls by the same count,
so `App::ensure_rows` never asks for a row that the grid does not draw.

### The cost of the band

One request covers each column that the grid draws, so the band reads the view
one time and not one time for each column. The compact band over a plain
Parquet file needs no query at all: the footer holds the row count and the NULL
count of each column already. See [performance.md](performance.md).

## The panels

Peruse has two panels and four states. The key `m` adds the metadata or
removes it, the key `i` does the same for the column statistics, and the key
`M` moves through the four states in order.

| State | The side pane holds |
|---|---|
| `none` | nothing. The grid takes the full width. |
| `meta` | the metadata |
| `stats` | the statistics of the column under the cursor |
| `both` | the metadata above, the statistics below |

The setting `panels` keeps the choice between sessions, so a user who wants
the two panels always gets them at the next start. See
[settings.md](settings.md).

### The stacked view

In the state `both`, the metadata goes on top and the statistics below it. The
order never changes, so the eye finds each of them on the same side.

The line between the two moves. The function `ui::split_panels` asks
`panels::stats_content_height` how many rows the statistics of this column need,
and it gives them that height and no more. The statistics have an end: some rows
of numbers, a chart of one row, and the most frequent values. The metadata then
keeps each row that is left, because it holds the list of columns and that list
has no end.

One half of the pane is the wrong limit in the two directions. On a tall pane it
cuts the statistics while the metadata above them holds empty rows, and on a
short pane it gives the metadata too little to read. The limit is therefore the
room that the metadata must keep, `META_MIN_HEIGHT` = 8 rows. The statistics
also keep `STATS_MIN_HEIGHT` = 8 rows, so the border makes a small step and not
a large one while the numbers are on their way. A pane that cannot hold the two
minimum heights gives the metadata one half.

A side pane that is shorter than 14 rows holds one panel only. Two panels with
one row of text each are of no use. Peruse then draws the statistics and writes
the reason along the bottom edge, because a panel that goes away with no word
looks like a fault.

### The metadata panel

The panel shows the rows from `FileMeta::summary_rows`, then a list of the
columns, and then the read expression. The function `panels::plan_meta` divides
the rows of the panel between those three parts:

1. The list of columns keeps four rows before the summary takes any. The list is
   the part that the user reads at each move of the cursor.
2. The summary then takes the rows that are left, up to its own length.
3. The read expression comes last. It appears only when the whole list of
   columns is on the screen and rows stay free after it.

A short panel therefore writes fewer rows of the summary and no read
expression, and a tall panel writes each part in full. The panel needs no
`compact` flag from the caller.

The list of columns follows the cursor of the grid, so the keys `h` and `l`
scroll the panel and the grid together. The list holds the column under the
cursor in the middle, and its last row says how many columns are above and below
it. A file with 400 columns therefore needs no keys of its own. With room for
one row only, the name of the column takes that row, because the name is more
use than the count.

For a Parquet file, each column shows its percentage of NULL values. For a CSV
file and a JSON file, each column shows its type instead. The footer of a
Parquet file holds the exact count, and it costs almost no time. A count for a
file of text would need a scan, and the panel does not show an estimate.

**A column that holds a structure opens.** The column under the cursor also
shows its fields, one level deep and moved to the right:

```text
actor                     STRUCT(id BIGINT,…
  id                                  BIGINT
  login                              VARCHAR
  gravatar_id                        VARCHAR
repo                      STRUCT(id BIGINT,…
```

The fields come from the type of the column, through
`model::struct_fields`. That text is the one source that each format gives: a
Parquet footer names the leaves, and a CSV file and a JSON file name nothing.
For a Parquet file, each field also finds its own count of NULL values,
because the footer names a leaf by its path, such as `actor.login`.

Only the column under the cursor opens. A file can hold hundreds of columns,
and a list that opened each of them would be too long to read.

At the end, the panel shows the call that reads the same rows outside Peruse:
`read_parquet`, `read_csv` or `read_json_auto`. For a database it shows the two
statements `ATTACH … (READ_ONLY); FROM "__peruse_db"."main"."sales"`. The user
can look at the data and then copy that text into a script or into the DuckDB
command line. The panel shows the short form of the call, from
`Engine::read_expr`; the call that the engine runs holds the full list of
columns and is too long for a panel. See [engine.md](engine.md).

### The column statistics panel

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

### The cost of the statistics

The statistics of one column need a scan of the view: a count, a count of the
different values, the smallest value, the largest value and the most frequent
values. On a file of ten million rows that scan needs some tens of
milliseconds. See [performance.md](performance.md).

Two rules keep the panel quick, and they are the reason that the state `both`
is usable at all:

- **Peruse keeps each answer.** The value `stats_cache` holds the statistics of
  each column that the engine measured for this view. A move back to a column
  therefore needs no second scan. A change of the view empties the cache,
  because the numbers describe the rows of one view.
- **Peruse asks one time for each frame.** The function `App::ensure_stats`
  runs after each frame, and not at each press of a key. A user who holds the
  key `l` down moves across many columns between two frames, and each of those
  columns would otherwise start a scan that nobody reads.

The detail band uses the same two rules, in `App::band_cache` and
`App::ensure_band`.

## The overlays

| Overlay | Key | Contents |
|---|---|---|
| The help | `?` or `F1` | Each command, in groups, with the keys of the prompt, the mouse, the legend of the column marks and two notes |
| The command palette | `:` or `p` | A prompt and the list of the commands that match |
| The theme picker | `T` | The themes, with a sample of the colors of each one |
| The cell inspector | `Enter` | The complete value of one cell, on some lines. A cell that holds other values opens the record view instead. |
| The record view | `r` | One row of the grid, with one column on each line |
| The filter builder | `f` | The list of conditions, and the menus that make one |
| The settings page | `,` | The settings, and what the machine gives |

The keys `j` and `k` scroll an overlay, and the key `Esc` closes it. A turn of
the wheel moves the selection of the overlay: the code changes the event into
presses of an arrow key, so the code that handles the keys stays the one place
that moves a selection. A click outside the box closes the overlay, as `Esc`
does, and a click on a line of its list selects that line. In the cell
inspector, the key `y` copies the value.

The theme picker changes the theme at each move, so the user sees the theme
immediately. The key `Esc` gives the previous theme back.

An overlay that takes text gives the position of the terminal cursor back to
`ui::draw`. The caller therefore needs no second calculation of the layout. The
palette holds its own prompt, and `overlays::palette_rect` gives its box, so
`ui.rs` reads the cursor position from that one function.

The settings page holds its own keys. The document [settings.md](settings.md)
lists them.

### The record view

The grid reads a row from the left to the right, and it shows a value that
holds other values as one long text. A file with 300 columns therefore needs
300 presses of a key to read one row, and a JSON file gives a wall of text in
one cell:

```text
{'id': 665991, 'login': petroav, 'gravatar_id': '', 'url': 'https://api.git…
```

The record view puts the fields under each other instead, and a field that
holds other values opens:

```text
┌ record 1 of 11,351 ────────────────────────────────────────────────┐
│   id                VARCHAR   2489651045                           │
│   type              VARCHAR   CreateEvent                          │
│ ▾ actor             struct    {5 fields}                           │
│     id              number    665991                               │
│     login           text      petroav                              │
│     gravatar_id     text      (empty)                              │
│ ▸ repo              struct    {3 fields}                           │
│ ▸ payload           struct    {5 of 20 fields}                     │
│   org               null      NULL                                 │
└ 3/9 · l/h open · a open all · z hides empty · n/p row · / find · y ┘
```

The module `tree.rs` builds the tree, and the document
[nested-values.md](nested-values.md) describes it.

| Key | Action |
|---|---|
| `j` `k`, `↑` `↓` | Move to another line. |
| `PgUp` `PgDn`, `g` `G`, `Home` `End` | Move by ten lines, or to the ends. |
| `l` `→`, `h` `←` | Open a line, close a line. |
| `Space` | Open a line, or close it. |
| `Enter` | Open a line, or show one value in the cell inspector. |
| `a` `c` | Open each level, close each level. |
| `z` | Show the fields that hold no value, or hide them. |
| `n` `p` | The next row, the previous row. The lines that are open stay open. |
| `/` | Find a field by name or by value, at any level. |
| `y` `Y` | Copy the value, or the complete record as JSON. |
| `P` | Copy the path, such as `"payload"."commits"[1]."sha"`. |
| `=` `!` | Keep, or remove, the rows with this value. |
| `Esc` `q` `r` | Close. |

Five rules control what the view shows:

- A column that the grid hides is still in the view, in a dim color. The user
  opens this view to see the complete row, and a hidden column is exactly the
  column that the user cannot see in the grid.
- Four cases look the same in a plain grid, and they are not the same. The
  view writes `NULL` for a missing value, `(empty)` for a text with no
  character, `{n fields}` for a structure, and `…` for a row that the engine
  has not read yet.
- A column of the row always shows, even when it holds NULL. The schema
  declares that column, so the NULL is a value that the row does not have. A
  field inside a structure is a different case: see
  [nested-values.md](nested-values.md).
- The type column shows the type of the file for a plain column, and the
  family of the value for a field inside a structure. The type of a structure
  can be some thousand characters long, and a cut of it says nothing that the
  word `struct` does not say.
- The line that holds the cursor decides the keys along the bottom edge. A
  line that opens offers `l` and `h`, and a line that holds one value offers
  `Enter`.

When the view closes, the cursor of the grid moves to the column that holds
the selected line. A line three levels down still belongs to one column. The
user does not lose the place. A hidden column is the one exception: the cursor
must always be on a column that the user can see.

The cell inspector can come from the record view. The value `cell_from_record`
holds that fact, so the key `Esc` goes back to the record and not to the grid.
A value inside a structure has no column of its own, so the engine cannot read
it again. The tree holds the complete value already, and the inspector takes
it as it is.

### The filter builder

The builder has five steps, and it draws each step in the same box. The
document [filter.md](filter.md) describes the steps and the model behind them.

## The mouse

The mouse moves the grid, the cursor and the selection of an overlay. The
document [keys-and-commands.md](keys-and-commands.md) lists each event.

A mouse event arrives as a row and a column of the terminal. Only the code that
draws the grid knows which cell is at that place, so `grid::draw` writes the
positions into `App::hit` after each frame:

| Field | Contents |
|---|---|
| `header_y` | The row of the terminal that holds the column names |
| `band` | The number of rows of the detail band, under the names |
| `top` | The first row of the terminal that holds a row of data |
| `rows` | The number of rows of data on the screen |
| `cols` | Each column that the grid draws: its position in the schema, its column of the terminal, and its width |

The function `Hit::on_labels` gives `true` for the row of the names and for each
row of the band. A click there moves to the column and leaves the row where it
is, because the band describes a column and not a row.

A grid with no room for a row of data sets `rows` to zero. Without that, a click
would use the positions of an older frame. A grid that is higher than the file
keeps the rows under the last one empty, and a click there changes nothing:
`grid_mouse` compares the row against `App::max_row` first.

### The box of an overlay

An overlay covers the grid, so the mouse needs its box as well. Only the code
that draws an overlay knows the box that it chose, so each `overlays::draw_*`
gives an `app::OverlayHit` back and `ui::draw` writes it into `App::overlay`. A
mode with no overlay writes `None`.

| Field | Contents |
|---|---|
| `mode` | The mode that drew the box. A click acts only when it still agrees with `App::mode`, so a click can never use a box that is gone. |
| `area` | The box, with its border |
| `lines` | One pair for each line of the list on the screen: the row of the terminal, and the position of that line in the list |

The table `lines` exists because a list scrolls and because some lists hold a
heading that the selection goes past. The row on the screen is therefore not
the position in the list, and only the code that draws knows the two.

A click outside `area` calls `App::close_overlay`, which gives the grid back in
one press. It does not repeat `Esc`. In the record view `Esc` first clears the
text of the find line, and in the filter builder `Esc` goes back one step, so a
click on the grid behind such a box needed two or three presses.

A click on a line of `lines` selects that line. A click on the line that is
selected already acts on it, which for a value that holds other values means to
open it or to close it, as `Space` does. A double click opens it, runs it or
applies it, as `Enter` does. The grid follows the same rule: the first click
chooses, and the next one acts, so a user who reads another line never opens
something by accident. A click on the border, on a prompt, on a heading or on
the row of keys changes nothing, and Peruse draws no frame for it.

### The double click

A terminal reports a press and a release, and never a double click. The type
`app::Clicks` finds it: two presses of the left button, at the same row and the
same column of the terminal, inside `DOUBLE_CLICK` (400 ms). A press that
closes a pair carries a `paired` mark, so a third press starts a new pair and
three quick presses never open the same thing two times. `App::on_mouse` gives
each press of the left button to `Clicks` before it reads the mode, so a press
that lands on nothing still ends the pair.

The second press of a pair acts on the line that the first press chose, and
never on the line that the pointer covers by then. A list keeps the selected
line near the middle, so the first press moves the window, and the frame
between the two presses puts another line under the pointer. The two presses of
a pair are at one position of the terminal, so the line of the first press is
the line that the user means. The chooser follows the same rule.

### The mouse and a box that takes text

The wheel in an overlay becomes presses of an arrow key, so the code that
handles the keys stays the one place that moves a selection. A box that takes
text breaks that rule: an arrow key belongs to the text there, and it walks the
history of the box. A turn of the wheel must never write in a box, so
`App::wheel_overlay` treats three states of its own:

| State | What the wheel does |
|---|---|
| The find box of the record view | Moves the list of the fields, and leaves the text |
| The settings page while a value is being typed | Nothing. `Enter` writes the text into the setting under the selection, so the selection must not move. |
| The value steps and the SQL step of the filter builder | Nothing. Those steps hold a prompt and no list. |

The chooser follows the same rule: `Browser::on_mouse` moves the selection
itself and never sends an arrow key into the find box.

The option `--no-mouse` and the setting `mouse = false` turn the mouse off. A
terminal that gives the mouse to a program does not select text with the mouse
in the usual way. The chooser runs in front of the application, so the options
of the command line do not reach it: `browser::mouse_wanted` reads the option
from `std::env::args` and the setting from the settings file.

## The prompt

The module `input.rs` holds an editor of one line. The filter prompt, the SQL
prompt, the search prompt and the row-number prompt all use it. The prompt
takes the place of the status line.

Each position in the editor counts characters, and not bytes. A user can filter
on a value with characters that are not ASCII characters, and a viewer of data
sees such values frequently. A position in bytes would then be wrong.

### What each prompt opens with

A prompt opens with the text that the user usually starts from, and the cursor
comes after the last character.

| Prompt | Text |
|---|---|
| The filter (`f`) | The filter of the grid, or an empty line |
| The SQL prompt (`e`) over the file | `SELECT * FROM src WHERE ` |
| The SQL prompt (`e`) over a statement | The statement that the grid shows |
| The search (`/`) | The value that the last search looked for |
| The row number (`#`) | An empty line |

The constant `query::PROMPT_START` holds the text of the SQL prompt. It lives
beside the other generated SQL, because it holds the name `src` that the engine
gives to the open file, and that name must have one home. A user of a viewer
asks for a part of the file more frequently than for anything else, so the text
ends with the word `WHERE` and a space. The guard accepts that text, so the
prompt shows no error while the statement is still incomplete. `Ctrl-U` removes
the whole line for a user who wants another statement, and `Esc` leaves the grid
as it was.

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
| `Ctrl-←` `Ctrl-→` | Move one word |
| `Alt-←` `Alt-→` | Move one word |
| `Ctrl-Backspace` `Alt-Backspace` | Delete the word in front of the cursor |
| `Ctrl-Delete` `Alt-Delete` `Alt-D` | Delete the word after the cursor |

The Option key of a Mac sends `Alt`, so the `Alt` forms are the forms for a
Mac. A terminal that sends the character `0x08` for `Ctrl-Backspace` also
deletes the word in front of the cursor, because the editor accepts `Ctrl-H`
for it.

A terminal does not send a different code for each of these keys. Three forms
are outside the reach of the editor:

- A Mac terminal with the option "Option as Esc+" sends `Esc` in front of the
  arrow key, and Peruse reads that as `Esc` and closes the prompt. Select
  "Option as Meta", or the natural text editing keys, to get the word keys.
- A terminal that sends the same code for `Ctrl-Backspace` and for `Backspace`
  deletes one character. Use `Ctrl-W` there.
- A terminal that sends the same code for `Ctrl-Delete` and for `Delete`
  deletes one character. Use `Alt-D` there.

The history holds 200 lines at the most. It does not keep an empty line. A line
that the user runs again moves to the end of the history. When the user moves
through the history and then past the newest line, the editor gives the line of
the user back.

A prompt keeps the grid on the screen, and a user who writes a filter looks at
the data while doing it. The wheel therefore still moves the grid. The arrow
keys stay with the history, and a click does nothing.

The filter prompt and the SQL prompt get colors from the module `sqlhl.rs`.
That module finds the values, the comments, the numbers and the keywords. The
user therefore sees a quotation mark with no partner as soon as the user types
it. Peruse also checks the expression after each key, and it shows the error at
the right side of the row.

## The ghost completion

A prompt with a known list of answers writes the rest of the answer after the
cursor, in a dim color. A user who types `am` sees `amount` at once, and does
not have to remember the names or press a key to see them.

| Prompt | The list of answers |
|---|---|
| The filter and the SQL statement | the columns, and the fields inside them |
| The text step of the filter builder | the same |
| The find box of the record view | the fields of the row |
| The find box of the chooser | the names in the list |
| The value of a setting | the answers that the setting takes |

The list of a setting comes from `App::setting_choices`: the theme names for
`theme`, `none`/`meta`/`stats`/`both` for `panels`, `off`/`compact`/`detailed`
for the column details, and `yes`/`no` for the index.

The search prompt and the row-number prompt get no help. Peruse cannot know
what a user looks for. A setting that takes a number gets none for the same
reason: no part of a number says what the rest of it is.

Three rules control the text:

- **The shortest name wins.** A file with `amount` and `amount_tax` gives
  `amount` for the text `am`. The shortest name is the one that the user most
  probably wants, and one more character reaches the longer one.
- **The text appears at the end of a line only.** In the middle of a line there
  is no room for it: the text of the user is there.
- **The key `Tab` takes it, and the key `→` also takes it.** At the end of a
  line the key `→` has nothing else to do, and a user of a shell knows that
  form.

### A path into a structure

The completion follows a full stop into a structure. The text `actor.log`
gives the fields of `actor` that start with `log`:

```text
filter › actor.login
              ▲ the user typed "actor.log", and "in" is the ghost
```

The function `App::fields_at` walks the path. The first step names a column of
the file, and each step after it names a field of a structure, through
`model::struct_fields`. A list of structures gives the fields of the structure
inside it, so a path such as `payload.commits.sha` also completes.

The completion writes the whole path back into the line, and it puts each part
in quotation marks when a statement needs them.

## The status line and the footer

With no prompt and no message, the status line shows the name and the type of
the column under the cursor, the position of that column, and the position of
the cursor row. A message takes that place, with a sign for its kind.

The footer shows the keys for the current mode. In the normal mode, the keys
come from the list `FOOTER_HINTS`. A narrow terminal cuts the list at the end,
so the most important keys come first.

A file of text with no index has a real limit, and the user must know it. The
note about the key `I` therefore takes its space first, and the key hints fill
the space that stays. A Parquet file and a table of a database need no index, so
they never show that note.
