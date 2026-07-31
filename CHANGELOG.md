# Changes

The versions follow [Semantic Versioning](https://semver.org). Peruse is
below version 1.0, so a minor version can still change the keys or the
settings. Each such change is in this file.

## 0.2.0

### The keys that moved

Peruse is below version 1.0, and these keys are not what they were. Press `?`
for the list that the program itself holds.

- `Home` and `End` now go to the first and the last **column** of the row, and
  not to the first and the last row. A grid puts the ends of the line on those
  two keys.
- `g` and `G` are the first and the last row, as before.
- New: `a` and `0` go to the first column, and `z` goes to the last. Those
  letters need no shift key, and `^` and `$` still work.
- New: `o` goes back to the start of the file, the first row and the first
  column together. `O` goes to the far corner. `^Home` and `^End` do the same
  two, as in a spreadsheet.
- New: `^D` and `^U` move half a page. `^F` and `^B` move a full page, as
  before.
- New: `J`, `K`, `H` and `L` move by one step, and the setting `step` gives the
  number. The default is 10.
- The reason for all of this: many laptop keyboards have no Home, End, PgUp or
  PgDn key, and Peruse must be complete without them. A test proves that each
  movement has a key outside that block.
- The command palette is now `:` or `p`. The chord `^P` is **gone**, because
  Visual Studio Code takes it for its own file finder and Peruse never sees it
  in that terminal.
- New: each prompt moves and deletes by one word. `^←` and `^→` move one word,
  `^Backspace` deletes the word in front of the cursor, and `^Delete` deletes
  the word after it. The `Alt` form of each of those keys does the same, for
  the Option key of a Mac. `^W`, `Alt+B` and `Alt+F` stay.

### The detail band (`d`)

- One key puts a band of facts between the column names and the first row of
  data. It moves through three modes: off, compact and detailed.
- Compact is one row: the type at the left and the share of NULL values at the
  right, so the shares line up down the grid.
- Detailed is four rows: the type, the share of NULL values, the count of the
  different values, and the range from the smallest value to the largest. Every
  column gives the same fact on the same row.
- A compact band over a plain Parquet file runs **no query at all**. The footer
  holds the row count and the NULL count of each column already.
- On a short terminal the band gives its rows back to the data, and the grid
  always keeps half its rows.
- The header now reads in three levels: the name of the column under the cursor
  keeps the accent color and thick letters, its facts take a mix of the accent
  color and the dim color in thin letters, and the facts of each other column
  stay dim. Without the middle level the name and its facts read as one block.
  A test holds the three apart in each of the 25 themes.
- The setting `band` keeps the mode for the next session, and the key writes it
  at once, as the theme key does.

### The mouse

- The wheel moves the rows. The wheel with the control key or the shift key,
  and a wheel that turns to the side, move across the columns.
- A click moves the cursor to that cell. A click on the row of the names moves
  to that column and never sorts: a sort of a large file costs seconds.
- A click on the cell that the cursor is on opens that row in the record view,
  as the key `r` does, and so does a double click. The first click on a cell
  only chooses it, so a user who wants to read another cell never gets a box on
  top of the data. A terminal reports no double click, so Peruse finds it
  itself: two presses at the same place inside 400 ms. The second press acts on
  the line that the first press chose: a list moves under the pointer when the
  selection changes, and a double click must never run a command that the user
  did not point at.
- In an overlay the wheel moves the selection, and a click selects the line
  under the pointer. A click on the line that is selected already opens or
  closes a value that holds other values, as `Space` does, and a double click
  opens it, runs it or applies it, as `Enter` does. The same rule as the grid:
  the first click chooses, and the next one acts.
- A click outside the box closes the overlay in one press, whatever state the
  overlay is in. It does not repeat `Esc`, which first clears the find text of
  the record view and first goes back one step in the filter builder; a click on
  the grid behind the box therefore needed two or three presses.
- A click that lands on nothing changes nothing: a panel at the side of the
  grid, the rows under the last row of the file, the border of an overlay, a
  prompt, a heading and the row of keys are all quiet.
- The wheel never writes in a box that takes text. An arrow key there walks the
  history of the box, so the find box of the record view moves its list
  instead, and the settings page and the value steps of the filter builder
  leave the wheel alone.
- The chooser takes the mouse too: the wheel moves the list, a click selects an
  entry, and a double click goes into a directory or chooses a file or a table.
- `--no-mouse` and the setting `mouse = false` turn the mouse off, for a user
  who wants the terminal to select text with the mouse as usual. The chooser
  reads the same option and the same setting.

### Formats

- **A DuckDB database file.** Peruse attaches it with `READ_ONLY` and shows one
  table of it. A database with more than one table opens a picker, which lists
  the tables and the views with the row count from the catalog and costs no
  scan. `--table orders` skips the question.
- From there everything works as it does for a file: the filter, the sort, the
  search, the statistics, the band, the record view and `--ddl`. Jumps are
  instant, and there is nothing to index.
- The metadata panel shows the two statements that opened the table, so your own
  SQL can join a second table of the same database.
- The first bytes of a file now decide a database before the extension does. A
  DuckDB database therefore opens whatever its name is, and a SQLite file gives
  a message that says how to write a table out, instead of a parse failure over
  binary data.

### The grid and the panels

- The dim character after a column name is the family of the values, and the
  help overlay now has a legend for it.
- A column that is too narrow for both the name and that character keeps the
  name. It no longer shows a cut letter where the character belongs.
- A fitted column is now five screen columns wider than its name. A column whose
  values are as narrow as its name stopped at the name, and it therefore lost
  the character of the family in the header and the type in the band. The limit
  of 60 screen columns still comes last.
- The SQL prompt (`e`) over a file now opens with `SELECT * FROM src WHERE `,
  and the cursor comes after the space. A user who wants a part of the file
  types the condition and nothing else. `^U` removes the whole line, and over a
  statement the prompt still opens with that statement.
- With both panels open, the line between them moves. The statistics take the
  rows that their own content needs, and the metadata keeps the rest, because
  its list of columns has no end. The metadata panel no longer loses its list on
  a tall side pane.
- The metadata panel asks for the footer again after a read that failed, so a
  file that changed on the disk needs no restart.

### Settings

- New: `band`, the rows of facts under the column names: `off`, `compact` or
  `detailed`. The default is `off`.
- New: `step`, the rows or columns that `J`, `K`, `H` and `L` move, from 1 to
  1000. The default is 10.
- New: `mouse`, `false` to make Peruse ignore the mouse. It is in the file and
  on the command line, and not on the page.
- Peruse indexes a file of text at the start below **64 MB** and with 256
  columns or fewer, and not below 256 MB. The index takes about one and a third
  times the size of the file, and a file of 170 MB with 10,000 columns costs 21
  seconds and 2.7 GB to index.

### Speed

Each number below is a measurement, and each one names the file that it comes
from. A number from one file is not a promise about another file.

- **A CSV file opens to its first screen 8.9 times faster**, on a file of 258
  MB. The interactive start on that same file went from 376 ms to 146 ms. The
  reason: DuckDB examined the file to find the delimiter and the types, and it
  did that again for every statement. Peruse now asks once, at the open
  operation, and writes the answer into the read call. One page of 50 rows of
  that file cost 100 ms, and 92 of those were the examination; the same page now
  costs 8 ms.
- **The metadata of a CSV file costs about 500 times less**, on the same file of
  258 MB. The panel took the dialect from a second call to the sniffer, which
  cost 50 ms. It now takes the answer of the open operation.
- **The search is 18 to 45 times faster.** It reads a window of 8192 rows in
  front of the cursor before the rest of its part, and a match there answers the
  usual search: over 250,000 rows that scan cost 90 ms, and over the window it
  costs 2 ms. The test on each column is also `contains(lower(value), lower(needle))` and no
  longer `ILIKE`, which needed 265 ms for the answer that `contains` gives in
  91, over 250,000 rows and nine columns.
- The parts of a search now double in size, up to 4 million rows. A search of
  ten million rows reads the file about six times and not forty.
- A page takes its rows first and changes the values after, so the database
  shares that work across its threads. One page of 50 rows costs 5.3 ms and not
  8.3, and a sorted page costs 8.5 and not 12.4.
- The metadata panel makes two statements fewer over a Parquet footer. The sizes
  of the file and the list of the encodings are the sums of the facts of the
  columns.
- The statistics of a column leave out the most frequent values above 100,000
  rows when almost every value is different. That query groups every row, and a
  list of values that each occur one time says nothing.
- DuckDB now keeps the footer of a Parquet file in its own cache.
- Peruse draws a frame only when something changed. With the mouse on, the
  terminal reports each movement of the pointer, and a frame for each of them
  would draw the same screen again.

### Themes

- 25 themes, 16 dark and 9 light. Gruvbox, Solarized, Catppuccin, Tokyo Night,
  One, Everforest, Rose Pine and GitHub each come in the two forms, and Monokai
  and Kanagawa come in a dark form.

### Installing

- Manifests for Scoop, Homebrew, WinGet and Chocolatey, so a user can install
  Peruse without Rust and without a build of DuckDB. `packaging/render.sh`
  fills them from a release, and `packaging/README.md` says where each one
  goes.

### Licences

- `THIRD-PARTY-LICENSES.md` names DuckDB, the 26 C and C++ libraries inside
  it, and the licence and copyright of each. `libduckdb-sys` ships none of
  those files, so this one supplies them. It goes in every release archive
  and in both crates.
- It also answers the three reports that a licence tool gives for Peruse and
  that are all false: the GPL header in libpg_query, which Bison wrote and
  which carries an exception; mbedtls and httplib, which belong to an
  extension that Peruse does not build; and the word "GPL" in two libraries
  that offer a choice of licence, where Peruse takes the other side.
- Dropped `directories`, which reached `option-ext` under the Mozilla Public
  Licence. `dirs.rs` reads the same environment variables and gives the same
  paths, so the settings of a user do not move. No copyleft crate is left.

### Building

- The oldest Rust is now 1.88, and was 1.95. One crate asked for 1.95, and
  Peruse used it for a memory total and a processor name. A job in CI builds
  on exactly the version in the manifest, so a dependency cannot raise it in
  silence.

## 0.1.0

The first release.

### The grid

- Reads one page of rows at a time, so a file larger than the memory opens
  in the time that a key press takes.
- A colour for each family of values, and a NULL that does not look like an
  empty text.
- Sort on a column, search each column, hide a column, fit the widths.
- Copy a cell or a row, through OSC 52, so it works over SSH.

### The record view (`r`)

- One row from the top to the bottom, one column on each line. A file with
  300 columns therefore needs no 300 presses of a key.
- A value that holds other values opens: a structure and a list both drill
  down, at any depth.
- A find box that reaches a field three levels down and opens the way to it.
- Filter on a value inside a structure, from its path.

### The filter

- A builder (`f`) that asks for a column, an operator and a value. No SQL.
- A `WHERE` prompt (`E`) for a user who prefers to type.
- `=` and `!` filter on the value under the cursor.
- The three build one list of conditions, so they work together.
- `u` and `U` go back and forward through the filters, the sorts and the
  statements.

### Reading only

- Two layers stop a write. The engine holds no connection that can write,
  and a guard refuses each statement from a user that could change anything.
- The guard also refuses a statement that would install an extension or
  reach the network.

### Formats

- Parquet, CSV, TSV, PSV, JSON, NDJSON, and the compressed forms of the text
  ones.
- An Arrow IPC file gives a message that says what to do, because the build
  of DuckDB inside Peruse holds no reader for it.

### The other parts

- A chooser (`peruse` with no file) that lists the data files that are near,
  and the files that the user opened before.
- A settings page (`,`) that keeps the theme, the threads, the memory limit,
  the sample size, the index and the panels. Each change keeps itself.
- The metadata panel and the column statistics, one above the other, and a
  metadata panel that opens the fields of a structure.
- `--ddl` writes a `CREATE TABLE` statement for Oracle, MySQL, PostgreSQL,
  Snowflake, BigQuery, SQL Server, DuckDB or DynamoDB, with a primary key and
  index candidates from the data itself.
- Nine themes, and themes of your own from TOML files.
- The rest of a name after the cursor, in each prompt that has a known list
  of answers.
