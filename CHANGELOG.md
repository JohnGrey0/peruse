# Changes

The versions follow [Semantic Versioning](https://semver.org). Peruse is
below version 1.0, so a minor version can still change the keys or the
settings. Each such change is in this file.

## 0.3.0

### Peruse only reads

Peruse promises that it never changes a file that it reads. Version 0.3.0
holds that promise in five layers, so a fault in one layer does not break it.
A test proves each layer alone.

- **The connection.** After Peruse finds the files, it locks DuckDB to those
  files, the write-ahead log of a database, and its own temporary directory.
  DuckDB then refuses each other file, and each change of a setting other than
  `threads` and `memory_limit`.
- **The guard.** The guard now rejects each function that writes a file,
  changes stored state or runs SQL from a text value: for example
  `enable_logging`, `enable_profiling`, `write_log`, `query` and
  `json_execute_serialized_sql`. A column with one of those names still works.
  A test reads the catalog of DuckDB and fails when a new version adds a
  function that the guard does not classify.
- **Views and macros in a database.** Peruse checks the SQL of each view and
  each macro of a `.duckdb` file with the guard before it reads the database.
  A view that can write stops the open, and the message names the view. Before
  this change, a database with one view could write a file when it opened.
- **A lock on Windows.** On a local disk, Peruse holds each source file open
  for the session with a share mode that lets another program read the file but
  not write it or delete it. Peruse never takes this lock on a network drive, so
  a program of a coworker can still write the file there. Linux and macOS have
  no equivalent of this share mode.
- **The writes of Peruse itself.** The local copies are removed only when the
  directory holds the mark that Peruse wrote, and the removal never follows a
  link.
- The guard now reads two forms of text as DuckDB reads them: a backslash in
  an `E'...'` text, and the `$` of a quoted text such as `$tag$...$tag$`.
  Before this change, either form could hide a second statement from the guard.

**Changed:** a query in the SQL prompt or in `--query` can read only the files
that you opened, as the view `src`, and the tables of an open database. It can
no longer read another file. The message tells you to open the second file
with Peruse, or to name it in the glob that you open.

### New

- **A local copy of a file on a network drive.** Peruse copies a file of 32 MB
  or more from a network drive to the local disk one time, with a progress
  screen, and then reads the copy. DuckDB reads a Parquet file in many small
  parts, and each part waits for the network. On a Parquet file of 1 GB, a sort
  took 1.1 s in place and 0.2 s from the copy.
- The copy stays for the next session, and the grid then opens at once. Peruse
  copies the file again only when the size or the modification time of the
  source changes. A glob copies each file and keeps the subdirectories, and a
  database brings its `.wal` file.
- You always know what you read. The title bar says `local copy`, the metadata
  panel shows the source and the copy, and each message names the source.
  Peruse opens the source for reading only.
- `Esc` on the progress screen stops the copy and reads the file in place. A
  copy that takes less than 400 ms shows no screen. `--ddl` uses a copy from an
  earlier session, but it makes no new copy.
- The copies take 20 GB at the most, and Peruse keeps 1 GB of the disk free.
  The copy that you used the longest time ago goes first. Peruse never removes
  a copy that another Peruse has open.
- `C` clears the local copies. It asks first, and a second `C` within 5 seconds
  clears them. The key `c` is near, and a copy can take minutes to make again.
  The settings page shows the number of copies, their size and their directory.
- `c` jumps to a column by its name. The prompt completes the name, and a part
  of the name is enough. A hidden column comes back when you ask for it.
- `v` shows each value of the column under the cursor with the number of rows
  that hold it, as a grid. The counts follow the filter, and `u` goes back.

### Faster

Each number below is a measurement from 0.2.0 to 0.3.0, and each one names the
file that it comes from. A number from one file is not a promise about another
file.

- **Far into a sorted view, each page is up to 60 times faster.** On 10 million
  rows of Parquet, the next page at the middle went from 613 ms to 17 ms, and a
  jump to the last screen went from 694 ms to 17 ms. On an indexed CSV file of
  1.27 GB, the next page at the middle went from 366 ms to 10 ms.
- The reason: a sorted page after row 100,000 sorted each column of each row,
  at each scroll. Peruse now sorts the sort columns and the position of each
  row one time, and keeps that order in memory. The order takes about 11 bytes
  for each row, and Peruse keeps it for one view at a time.
- That first deep page costs more on a DuckDB table that is already in the
  order of the sort, 85 ms and not 45 ms, and on a file set of 500 small
  Parquet files, 108 ms and not 92 ms. Each later page then costs less.
- **The first sorted page is up to three times faster:** from 77 ms to 29 ms
  on the same Parquet file, and from 120 ms to 38 ms with a sort on a column of
  text. The sorted rows of 0.2.0 were also wrong; see below.
- **A filtered page is up to 16 times faster.** At the middle of a DuckDB table
  of 10 million rows, it went from 332 ms to 21 ms, and on the Parquet file
  from 192 ms to 59 ms. DuckDB now reads the filter column first and the other
  columns for the rows of the page only.
- **A page of a file with more than 64 columns** holds the columns on the
  screen and 32 columns at each side of them. On a Parquet file of 1000
  columns, the first page went from 355 ms to 39 ms. A move to the side past
  those columns reads a new page, and `Y` asks for the complete row.
- **A search skips the columns that cannot hold the text.** The text of a
  number, a truth value, a date or a time uses few characters, so a search for
  a word does not read those columns. On 1000 columns, a search went from
  2024 ms to 120 ms. On a Parquet file, a search with no match over 250,000
  rows went from 99 ms to 7 ms. A column of intervals stays in the search.
- **A file set of 500 CSV files** opens to its first page in 34 ms and not 409
  ms, and its count takes 96 ms and not 476 ms. Peruse asks the sniffer about
  each file one time, on up to 8 threads. When the files agree, one read call
  reads the whole set.
- A file set of 100 Parquet files or more, whose files agree, costs about 20
  percent less in each statement. On 500 files, the open operation went from 92
  ms to 73 ms.
- Each new view reads its first page one time, and not two times. Over a sort
  of a large file, the second read was a second sort.
- A start with `--filter` or `--query` sends that view first. Before, the
  worker could count the whole file before it read the filtered view.
- On Windows, the chooser takes the facts of each file from the list of the
  directory. For 5000 files on a local disk, that went from 171 ms to 1.7 ms.
  On a network drive, each file cost one trip over the network.

### Fixed: wrong results

- **A sorted view showed some rows two times and other rows not at all.**
  DuckDB does not keep the order of rows with the same sort value from one page
  to the next. On 300,000 rows with three sort values, 0.2.0 put 117,481 rows
  on two pages. Now each row comes on one page only.
- Rows with the same sort value now keep the order of the file in a Parquet
  file, a table of a database and an indexed file of text. A file of text with
  no index, a view of a database and your own statement order them by each
  value.
- **`G`, `PgDn` and the wheel stopped at row 520** of a large file, on a
  terminal taller than 20 rows, until the count arrived. Peruse compared the
  first page with the size of a newer request, and took it for the last page.
- **A UUID column stopped the search and the record view for the whole file.**
  The filter operators `contains`, `starts with`, `ends with` and `does not
  contain` also failed on that column. A BIGNUM column stopped the search and
  the record view too. Peruse now changes each such value to text first, which
  costs nothing for a column of text.
- **The scroll bar covered the last character of a column** that ended at the
  right edge, so the number 900 read as 90. The grid now keeps its last screen
  column free.
- A `.csv` file with semicolons opened as one column, because Peruse gave the
  sniffer a comma. The sniffer now finds the delimiter of a `.csv` file. The
  extensions `.tsv`, `.tab` and `.psv` still give theirs, and `--delimiter`
  still wins.
- A JSON file that starts with a byte order mark and then a list opened as one
  row. PowerShell 5 writes that mark. A `.txt` or `.dat` file of JSON with that
  mark or with blank lines at the start went to the CSV reader.
- In a Parquet file set where one file lacks a column, the detail band showed
  too few NULL values for that column. No footer counts the rows of that file
  as NULL, so Peruse no longer takes that count from the footers.
- The statistics of a column failed when the column held NaN or infinity, which
  pandas writes for each missing float, or a large HUGEINT. The mean now shows
  `nan`, and the deviation leaves out the values that are not finite.
- A mean or a deviation below 0.001 showed as `0`, which reads as a column with
  one value.
- The histogram failed for a column with the name `b` or `n`.
- Your own statement failed everywhere when it ended with a line comment,
  because `) AS q` came after the comment on the same line.
- An enumeration with a bracket in a value, such as `ENUM('a[1]')`, read as a
  list. It lost its smallest and largest value, and it showed the wrong mark.
- `--ddl` gave an unsigned integer the signed type of the same width, so a
  large value overflowed on load. It also gave `TIME WITH TIME ZONE` a type of
  text. Each unsigned type now gets a type that holds its range, and the zoned
  time gets `timetz` in PostgreSQL and the plain time in the other dialects.
- Two Peruse windows that both wrote a large query to the disk read the blocks
  of each other, and that gave wrong rows.
- A statement that changed the columns, from `e` or from `--query`, left the
  message `rows: Binder Error: Referenced column … not found` on the status
  line over a grid that was correct.
- A search moves the cursor to the column that holds the value, and not only
  to the row. A match in a column outside the screen looked like no match.
- An old answer of the worker could put the value of another cell in the cell
  inspector, and an old part of a search could move the cursor of a new search.
- A metadata request that waited while the view changed was lost. The metadata
  panel and the detail band of a Parquet file then stayed empty for the
  session. A change of a setting was lost in the same way.

### Fixed: the screen and the keys

- `Shift-Tab` did nothing on any system.
- On Windows, the prompts refused each character that a keyboard types with
  the `AltGr` key, such as `@`, `[`, `|` and the Polish letters. The key `#` did
  nothing on a French keyboard.
- A count that `Esc` stopped left `G`, `O`, `#` and `/` saying `still counting`
  for the rest of the session. Those keys now start the count again.
- `x` moved the cursor to the last column of the file, so three presses hid
  the wrong columns. The cursor now goes to the column that takes the place of
  the hidden one.
- The cursor went off the screen when the grid lost rows: after `d`, a prompt,
  a panel or a smaller terminal.
- A statement longer than the prompt moves to the left so that the cursor
  stays on the screen, with a mark at the left edge. It went past the right
  edge before.
- `Esc` in a prompt keeps the line in the history, so an arrow key gives it
  back. `Esc` between two parts of a search did nothing before, and the search
  went on through the file.
- An index that failed after a change of the view left `indexing` on the
  screen, and `I` then said `already indexing`. Another failure cleared that
  state while the index ran, so `I` could start a second index.
- A page that failed was never asked for again. The next key or click now asks
  again, one time.
- `Esc` or `Enter` in the theme picker of the settings page closed the settings
  page too. They now go back to it.
- The help and the cell inspector counted each press of `j` after the end of
  the text, so `k` then did nothing for as many presses.
- On a view with no row, `y` copied the word NULL and `Enter` opened the cell
  inspector. The keys `r`, `y`, `Y`, `=`, `!` and `Enter` now say `the view
  holds no row`, and not `no row here yet`.
- The cell inspector and the statistics panel wrote a control character as it
  was, so a tab went away and an escape sequence read as `[31m`. Both now show
  the marks of the grid. The cell inspector writes `1 line`, not `1 lines`.
- On Windows, the title of the chooser showed `//?/C:/…` after a move. On each
  system, `~` replaced the home directory `/home/jo` at the start of
  `/home/john`.
- The settings page said that Peruse indexes a file of text below 256 MB. The
  limit is 64 MB, and the page now reads it from the program.
- A terminal of fewer than 120 columns gets a short form of the note `CSV:
  press I to index`, so the hints for the filter and the undo stay in the
  footer.
- The help names the second job of `Esc`: it closes the side panels when no
  query runs.

### Fixed: crashes, hangs and lost settings

- **A JSON file that nests very deep stopped the program.** Peruse checked the
  first 8 MB of the first file only. It now checks each file of a glob, and
  twice the number of rows that the reader examines in each file (the setting
  `sample_size`, and -1 for the whole file).
- A compressed JSON file (`.gz`, `.zst` or `.bz2`) of 80 bytes could hold 3000
  levels and stop the program. Peruse now reads it through DuckDB for the same
  check.
- **One typing mistake in the settings file cost you the whole file.** A file
  with a fault gives the built-in settings, and the next file that you opened
  wrote those settings over it. The list of recent files now changes only the
  list, and only in a file with no fault.
- Peruse writes the settings file to a temporary file first and then renames
  it. A crash during a write can no longer leave half a file.
- **A paste of more than one line ran the prompt at the first line break.**
  Each character after it reached the grid as a command, so `R` reset the view
  and `q` stopped the program. A paste now goes into the box as one line, and a
  paste over the grid does nothing. The console of Windows still sends a paste
  as keys.
- A quit during slow work waited until each request in the queue ended, which
  could take minutes. Peruse now stops each request and waits 2 seconds at the
  most.
- One `Esc` stopped only the statement that ran. Each other request of the
  batch, such as the statistics or the detail band, still read the whole file.
  One `Esc` now stops all of them.
- A kill or a crash during a large sort left the blocks of DuckDB in the
  temporary directory: 1.5 GB in one test. The next start of Peruse now removes
  the directory of each process that is not alive.
- A terminal with no row, and the chooser on a terminal of four to six rows,
  stopped the program.
- When the engine thread stopped, Peruse ended with the exit code 0. It now
  gives an error and the exit code 1.

### Changed: what you must know

- New settings: `local_copy` takes `auto`, `always` or `never`, with `auto` as
  the default. It is on the settings page and works at the next file.
- New settings in the file only: `local_copy_min_mb` is the size below which
  `auto` reads a file in place, 32 by default. `local_copy_max_gb` is the space
  of all the copies, 20 by default.
- The file also reads the names `remote_cache`, `remote_cache_min_mb` and
  `remote_cache_max_gb`. Peruse writes the names `local_copy`.
- New on the command line: `--local` copies the file from any drive and of any
  size, and `--no-local` reads it in place. Both win over the setting.
- `memory_limit` reads `GiB`, `MiB`, `KiB` and `TiB`, as the settings page
  shows them. An older Peruse gave an error for `8GiB`.
- A theme file now refuses a color with a sign in front, such as `+ff8000`.
  That color passed before, and a typing mistake went through in silence.
- A filter or a statement that DuckDB rejects gives the view before it back,
  with the message on the status line. The text stays in the history of the
  prompt for a correction, and `u` skips the view that failed. Before, the grid
  showed a screen of `·` under the bad view.
- The list of columns in the side pane no longer jumps at each turn of the
  wheel. It moves only when the cursor comes within three rows of one of its
  ends. With both panels open, the statistics keep the largest height that they
  needed, so the line between the two panels stays in its place.
- The grid moves its first column back to the left when the columns from there
  to the last one fit. A width that becomes smaller leaves no empty space.
- Each engine writes its temporary files to a directory of its own, below
  `peruse` in the temporary directory of the system. Two Peruse windows no
  longer share one.

### Security

- **The read-only guard missed a write after a carriage return.** DuckDB ends a
  line comment at a carriage return, and the guard ended it at a line feed
  only. A statement could hide a `COPY … TO` from the guard, and a test wrote a
  file to the disk that way. The guard now ends a line comment at either
  character.
- DuckDB accepts a block comment inside another one, and the guard stopped at
  the first `*/`. The guard now counts the levels, and it rejects a statement
  where an inner comment does not close.
- The guard no longer rejects the word `REPLACE`, so the function `replace()`
  and `SELECT * REPLACE (…)` work. DuckDB has no `REPLACE` statement, and each
  write with that word also holds `CREATE` or `INSERT`.
- An error message can quote a line of the file, and that line can hold an
  escape sequence. The message could then clear the screen, set the title or
  write to the clipboard. Peruse now shows each control character in it as a
  visible mark. The output of `--ddl` gets the same treatment on a terminal,
  and stays as it is in a file or a pipe.

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
