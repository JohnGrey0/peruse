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
| `g`, `Home` | `Top` | first row |
| `G`, `End` | `Bottom` | last row |
| `l`, `→`, `Tab` | `ColRight` | next column |
| `h`, `←`, `Shift-Tab` | `ColLeft` | previous column |
| `^` | `ColFirst` | first column |
| `$` | `ColLast` | last column |
| `#` | `GotoRow` | jump to row number |

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
| `R` | `ResetView` | reset to the whole file |
| `/` | `Search` | search all columns |
| `n` | `SearchNext` | next match |
| `N` | `SearchPrev` | previous match |

The key `f` starts the filter builder, and not the prompt. The builder is the
first thing that a new user finds, and it needs no knowledge of SQL. The prompt
that takes a `WHERE` expression moved to the key `E`. The three commands all
build the same list of conditions. The document [filter.md](filter.md)
describes that list.

### The group Inspect

| Keys | Command | Description |
|---|---|---|
| `m` | `ToggleMeta` | file metadata panel |
| `i` | `ToggleStats` | statistics for this column |
| `Enter` | `InspectCell` | show this cell in full |
| `r` | `Record` | show this row as a vertical record, one column per line |

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
| `?`, `F1` | `Help` | this help |
| `:`, `Ctrl-P` | `Palette` | run a command by name |
| `Esc` | `Cancel` | cancel the running query |
| `q`, `Ctrl-C` | `Quit` | quit |

## How Peruse finds the command for a key

The function `commands::resolve` reads the key and the modifier keys. It then
finds the first entry that holds that pair.

The function `normalise` removes the shift key from a character first. A
character that the terminal can print holds its own shift state: the character
`G` is the shift key and the key `g`. Some terminals send the shift key with
the capital letter, and some terminals do not. Peruse must accept the two
forms.

A modifier key of a key that is not a character stays. The pair `Ctrl-F` is
therefore different from the key `f`.

## The help overlay

The function `overlays::draw_help` builds the help from the same table. For
each group, it writes the name of the group and then each entry of that group.
The help and the keys can therefore never disagree.

The overlay then adds two more parts:

- the keys of the prompt: `Enter`, `Esc`, `↑` and `↓`, `Ctrl-W`, `Ctrl-U`,
  `Ctrl-K`, `Ctrl-A` and `Ctrl-E`. The key `Tab` completes a column name in the
  filter prompt and in the SQL prompt.
- two notes: Peruse rejects a statement that would write, and the clipboard
  uses OSC 52, so it works through SSH

The overlay is higher than a usual terminal. The keys `j` and `k` scroll it,
and the last row shows the position.

## The command palette

The key `:` opens the palette. The palette shows a prompt and the list of the
commands that match the text. The function `App::palette_items` keeps a command
when one of these three tests succeeds:

- The description matches the text.
- The group matches the text.
- The keys of the command hold the text.

The first two tests use `commands::fuzzy_match`. That function gives `true`
when each character of the text occurs in the other text, in the same order.
The characters do not need to follow each other, so the text `tp` finds "theme
picker". An empty text matches each command, so the palette shows the full list
at the start.

The keys `↑` and `↓` move through the list, the key `Enter` runs the command,
and the key `Esc` closes the palette.

## The footer

The footer shows a short form of the most important commands. The list
`FOOTER_HINTS` gives them in order:

`Help`, `Quit`, `Search`, `FilterBuild`, `Record`, `Sql`, `SortCycle`,
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
frequently the wrong one: "build a filter …" would give "build".

## The keys inside an overlay

The record view and the filter builder hold their own keys. These keys are not
in the table `BINDINGS`, because they work in one overlay only. Each of the two
overlays therefore writes its keys along its bottom edge, where the user
already looks. The document [user-interface.md](user-interface.md) lists them.
