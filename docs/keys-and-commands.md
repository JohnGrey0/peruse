# The keys and the commands

One table holds each command of Peruse, with its keys, its description and its
group. Three parts of the program read that table: the code that finds the
command for a key, the help overlay, and the command palette. Peruse can
therefore have many commands and stay easy to use. Each command that a key
starts is also in the help, and the user can also start it by name. The code is
in `crates/peruse-tui/src/commands.rs`.

## The table

The table `BINDINGS` holds one entry for each command. An entry has five
fields:

| Field | Contents |
|---|---|
| `cmd` | The command |
| `keys` | The keys that start the command |
| `label` | The keys, as the help and the footer write them |
| `desc` | The description of the command |
| `group` | The group of the command in the help |

The five groups come in this order: `Move`, `Query`, `Inspect`, `Columns` and
`Other`.

### The group Move

| Keys | Command | Description |
|---|---|---|
| `j`, `↓` | `RowDown` | next row |
| `k`, `↑` | `RowUp` | previous row |
| `PgDn`, `Ctrl-F` | `PageDown` | page down |
| `PgUp`, `Ctrl-B` | `PageUp` | page up |
| `Ctrl-D` | `HalfPageDown` | down half a page |
| `Ctrl-U` | `HalfPageUp` | up half a page |
| `J` | `StepDown` | down one step of rows |
| `K` | `StepUp` | up one step of rows |
| `L` | `StepRight` | right one step of columns |
| `H` | `StepLeft` | left one step of columns |
| `g` | `Top` | first row |
| `G` | `Bottom` | last row |
| `l`, `→`, `Tab` | `ColRight` | next column |
| `h`, `←`, `Shift-Tab` | `ColLeft` | previous column |
| `a`, `0`, `^`, `Home` | `ColFirst` | first column of this row |
| `z`, `$`, `End` | `ColLast` | last column of this row |
| `o`, `Ctrl-Home` | `Origin` | back to the start: first row and first column |
| `O`, `Ctrl-End` | `LastCell` | the far corner: last row and last column |
| `#` | `GotoRow` | jump to row number |

Many laptop keyboards have no Home key, no End key, no PgUp key and no PgDn
key. Peruse must stay complete on such a keyboard, so each command that moves
by more than one row or one column has a key outside that block. The test
`every_movement_works_on_a_keyboard_with_no_navigation_block` proves it.

Four rules give the letters:

- **`Home` and `End` reach the ends of a row**, and not the ends of the file. A
  grid puts the ends of the line on those two keys. The letters `a` and `z` do
  the same, because they are the first letter and the last letter of the
  alphabet and they need no shift key. The characters `^` and `$` of a text
  editor also work, for a user who knows them.
- **One key goes back to the start of the file.** The letter `o` is for the
  origin: it moves to the first row and the first column together. A user who
  is deep inside a large file needs one key to come back, and not two. The
  capital `O` goes to the other corner. A spreadsheet puts the same two
  commands on `Ctrl-Home` and `Ctrl-End`, so those two chords do the same here.
- **The capital letter moves by the step, and the small letter moves by one.**
  The pairs `j`/`J`, `k`/`K`, `h`/`H` and `l`/`L` therefore keep the same
  direction. The setting `step` gives the number, and the default is 10. See
  [settings.md](settings.md).
- **A chord moves by a page.** `Ctrl-F` and `Ctrl-B` move a full page, and
  `Ctrl-D` and `Ctrl-U` move half a page.

### The group Query

| Keys | Command | Description |
|---|---|---|
| `s` | `SortCycle` | sort by this column (asc → desc → off) |
| `S` | `SortClear` | clear all sorting |
| `f` | `FilterBuild` | build a filter from menus (no SQL needed) |
| `E` | `Filter` | filter rows with a WHERE expression |
| `=` | `FilterThisValue` | keep only the rows with the value in this cell |
| `!` | `FilterExcludeValue` | remove the rows with the value in this cell |
| `F` | `FilterClear` | clear the filter |
| `e` | `Sql` | edit the SQL query behind the grid |
| `u` | `Undo` | undo the last filter, sort or query |
| `U` | `Redo` | redo the change that u undid |
| `R` | `ResetView` | reset to the whole file |
| `/` | `Search` | search all columns |
| `n` | `SearchNext` | next match |
| `N` | `SearchPrev` | previous match |

The key `f` starts the filter builder, and not the prompt. The builder is the
first thing that a new user finds, and it needs no knowledge of SQL. The prompt
that takes a `WHERE` expression is on the key `E`. The three commands all build
the same list of conditions. The document [filter.md](filter.md) describes that
list.

### The group Inspect

| Keys | Command | Description |
|---|---|---|
| `m` | `ToggleMeta` | file metadata panel |
| `i` | `ToggleStats` | statistics for this column |
| `M` | `CyclePanels` | cycle the side panels: none, metadata, statistics, both |
| `d` | `CycleBand` | column details under the headers: off, compact, detailed |
| `Enter` | `InspectCell` | show this cell in full |
| `r` | `Record` | show this row as a vertical record, one column per line |

The letter `d` is for the details. The statistics panel describes one column at
the side of the grid, and the detail band describes each column that is on the
screen, under the names. See [user-interface.md](user-interface.md).

The band and the half page both live on the letter `d`. The chord `Ctrl-D`
moves the cursor, and the plain letter changes the band.

### The group Columns

| Keys | Command | Description |
|---|---|---|
| `>` | `Widen` | widen this column |
| `<` | `Narrow` | narrow this column |
| `w` | `FitWidths` | re-fit all column widths to what is on screen |
| `x` | `HideColumn` | hide this column |
| `X` | `ShowAllColumns` | show all hidden columns |

### The group Other

| Keys | Command | Description |
|---|---|---|
| `y` | `CopyCell` | copy this cell to the clipboard |
| `Y` | `CopyRow` | copy this row as TSV |
| `I` | `IndexCsv` | index this CSV now (makes jumping instant) |
| `t` | `ThemeNext` | next theme |
| `T` | `ThemePicker` | choose a theme |
| `,` | `Settings` | settings, and what this machine gives |
| `?`, `F1` | `Help` | this help |
| `:`, `p` | `Palette` | run a command by name |
| `Esc` | `Cancel` | cancel the running query |
| `q`, `Ctrl-C` | `Quit` | quit |

The palette has no chord. Visual Studio Code takes `Ctrl-P` for its own file
finder, and Peruse never sees that chord inside that terminal. The test
`the_palette_does_not_use_a_chord_that_an_editor_takes` keeps `Ctrl-P` out of
the table.

## How Peruse finds the command for a key

The function `commands::resolve` reads the key and the modifier keys. It then
finds the first entry that holds that pair.

The function `normalise` removes the shift key from a character first. A
character that the terminal can print holds its own shift state: the character
`G` is the shift key and the key `g`. Some terminals send the shift key with
the capital letter, and some terminals do not. Peruse must accept the two
forms.

A modifier key of a key that is not a character stays. The pair `Ctrl-F` is
therefore different from the key `f`, and `Ctrl-Home` is different from `Home`.

## The mouse

The mouse needs no entry in the table. It points at a place on the screen, and
the code that draws is the only code that knows what is at a place. The grid
writes its positions into `App::hit` after each frame, an overlay gives its box
back as an `OverlayHit`, and `App::on_mouse` reads the two.

| Event | Action | The key that it copies |
|---|---|---|
| The wheel | Move three rows up or down the view | `j` / `k` |
| The wheel with `Shift` or `Ctrl` | Move two columns to the side | `h` / `l` |
| The wheel to the side | The same as `Shift` and the wheel | `h` / `l` |
| A click | Put the cursor on that cell | The arrow keys |
| A click on the row of the names, or on the detail band | Move to that column. A click never sorts. | `H` / `L` |
| A click on the cell that the cursor is on | Open that row in the record view | `r` |
| A double click on a cell | The same | `r` |
| A click in a panel at the side, or under the last row | Nothing | none |
| The wheel in an overlay | Move the selection of the overlay | `j` / `k` |
| A click outside the box of an overlay | Close the overlay | `Esc` |
| A click on a line of the list of an overlay | Select that line | The arrow keys |
| A click on the line that is selected already | On a value, open or close it | `Space` |
| A double click on a line of the list | Open, run or apply that line | `Enter` |
| A movement of the pointer, a drag, a release, the right button | Nothing, and no frame | none |

Six rules control the mouse:

- **The plain wheel always moves the view up and down.** That is what the wheel
  does in each other program.
- **The shift key is the form that always works** for a movement to the side.
  Some terminals keep the control key with the wheel for the size of the text,
  and Peruse then never sees the event. A wheel that turns to the side does the
  same, but few mice have one.
- **A click on a column name moves to that column, and it does not sort.** A
  sort of a large file costs seconds, and a click that lands on the wrong
  column must not start that work. The key `s` sorts.
- **The wheel in an overlay becomes presses of an arrow key.** Each overlay has
  its own list, its own limits and its own preview, so the code that handles
  the keys stays the one place that moves a selection. A box that takes text is
  the one exception: an arrow key belongs to the text there, and it walks the
  history of the box. A turn of the wheel must never write in a box, so those
  states move the list themselves, or move nothing. Refer to
  `App::wheel_overlay`.
- **A click acts, and a double click opens.** A terminal reports a press and a
  release, and never a double click, so Peruse finds it itself: two presses of
  the left button, at the same row and the same column of the terminal, inside
  400 ms. A press that closes a pair ends it, so a third press starts a new
  pair and three quick presses never open the same thing two times. The second
  press acts on the line that the first press chose: a list moves under the
  pointer when the selection changes.
- **A click that lands on nothing changes nothing.** A click in a panel at the
  side of the grid, under the last row of the file, on the border of an
  overlay, on a prompt, on a heading or on the row of keys does nothing at all,
  and Peruse draws no frame for it.

The option `--no-mouse` and the setting `mouse = false` make Peruse ignore the
mouse. A terminal that gives the mouse to a program does not select text with
the mouse in the usual way, and a user who copies out of the grid with the
mouse each day needs that form back. The chooser reads the same option and the
same setting: it runs in front of the application, so the options of the
command line do not reach it, and `browser::mouse_wanted` reads them itself.

## The help overlay

The function `overlays::draw_help` builds the help from the same table. For
each group, it writes the name of the group and then each entry of that group.
The help and the keys can therefore never disagree.

The overlay then adds four more parts:

- the keys of the prompt: `Enter`, `Esc`, `↑` and `↓`, `Ctrl-W`, `Ctrl-U`,
  `Ctrl-K`, `Ctrl-A` and `Ctrl-E`. The word keys follow them: `Ctrl-←` and
  `Ctrl-→` move one word, and `Alt-←` and `Alt-→` do the same for the
  Option key of a Mac. `Ctrl-Backspace` deletes the word in front of the
  cursor, and `Ctrl-Delete` deletes the word after it. The key `Tab` takes the
  ghost completion, and the key `→` takes it at the end of a line.
- the mouse, with the option `--no-mouse` and the setting `mouse` beside it
- a legend for the dim mark after a column name. The mark gives the family of
  the values: `#` a number, `"` text, `?` a boolean, `@` a date or a time, `~`
  binary, `{` a structure, a list or a map. A user asked what the letter means,
  and the answer is beside the keys that the user already reads. The list
  `TYPE_MARKS` holds it, and the test
  `each_family_of_values_has_a_line_in_the_help` keeps a new family from
  arriving with no line.
- two notes: Peruse rejects a statement that would write, and the clipboard
  uses OSC 52, so it works through SSH

The overlay is higher than a usual terminal. The keys `j` and `k` scroll it,
and the last row shows the position.

## The command palette

The key `:` opens the palette, and the letter `p` also opens it. The palette
shows a prompt and the list of the commands that match the text. The function
`App::palette_items` keeps a command when one of these three tests succeeds:

- The description matches the text.
- The group matches the text.
- The keys of the command hold the text.

The first two tests use `commands::fuzzy_match`. That function gives `true`
when each character of the text occurs in the other text, in the same order.
The characters do not need to follow each other, so the text `tp` finds "theme
picker". An empty text matches each command, so the palette shows the full list
at the start.

The keys `↑` and `↓` move through the list, the key `Enter` runs the command,
and the key `Esc` closes the palette. The wheel also moves through the list.

The function `overlays::palette_rect` gives the box of the palette. The palette
holds its own prompt, so `ui::draw` asks that function where to put the cursor
of the terminal. One calculation therefore gives the box and the cursor.

## The footer

The footer shows a short form of the most important commands. The list
`FOOTER_HINTS` gives them in order:

`Help`, `Quit`, `Search`, `FilterBuild`, `Undo`, `Record`, `Sql`, `SortCycle`,
`ToggleMeta`, `ToggleStats`, `Palette`

A narrow terminal cuts the list at the end. The keys for "quit" and for "help"
must therefore never be the keys that Peruse removes. The function `short_desc`
gives a short form of each description, because the footer has little space.

## The tests

Two tests protect the table:

- The test `every_command_is_discoverable` makes sure that each entry has a
  description, a label, keys and a known group. A command with no entry would
  be available only to a user who knows its key.
- The test `no_two_commands_claim_the_same_chord` makes sure that no two
  commands hold the same key.

A third test, `every_footer_hint_has_a_short_form` in `ui.rs`, makes sure that
each command in `FOOTER_HINTS` has a short form. Without a short form, the
function `short_desc` gives the first word of the description, and that word is
frequently the wrong one: `build a filter from menus` would give "build".

## The keys inside an overlay

The record view, the filter builder, the settings page, the theme picker and
the chooser hold their own keys. These keys are not in the table `BINDINGS`,
because they work in one overlay only. Each of those overlays therefore writes
its keys along its bottom edge, where the user already looks. The document
[user-interface.md](user-interface.md) lists them, and
[chooser.md](chooser.md) lists the keys of the chooser.
