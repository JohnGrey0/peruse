# The layers that keep Peruse read-only

Peruse gives the user a promise: it does not change the data. Five layers keep
that promise. Each layer stops a write on its own, and the layers do not depend
on each other. The owner's words are the rule: Peruse must not change a file
that it reads, at all.

| Layer | What it does | Where |
|---|---|---|
| A | The guard on the words of a statement from the user | `sql_guard.rs` |
| B | The check on a stored view or macro of a database | `engine.rs`, `check_stored_objects` |
| C | The lock on the DuckDB connection | `engine.rs`, `lock_down` |
| D | The read-only handle on each local file, on Windows | `engine.rs`, `read_only_locks` |
| E | The guard on the writes of Peruse itself | `cache.rs`, and the paths below |

The letters are the order of the layers in the code, and not an order of
strength. Layer C is the strongest for an unknown statement, because it needs
no check of the words.

## Layer C: the lock on the connection

The database is always in memory. The engine calls `Connection::open_in_memory`,
so the catalog holds no file on the disk. The engine reaches a data file of the
user only through the table functions `read_parquet`, `read_csv`, `read_json`
and `read_json_auto`, which read but cannot write.

After the engine finds the files that the user opens, the function `lock_down`
locks the connection. It sets, in this order:

1. `allowed_paths`: the files that the user opened, and the write-ahead log of
   a database.
2. `allowed_directories`: the temporary directory of this engine, where DuckDB
   writes the pages of a large sort.
3. `allowed_configs`: `threads` and `memory_limit`, so the settings page still
   works.
4. `enable_external_access = false`. DuckDB then refuses each file outside the
   two lists, for a read and for a write, each function that writes a file of
   its own, such as a log or a profile, and each `ATTACH` of a second database.
5. `lock_configuration = true`. No statement can then change a setting, so no
   statement can undo the steps above.

The function `glob()` stops after the lock, so the engine finds the files
first. The engine never installs an extension and never loads one: the settings
`autoinstall_known_extensions` and `autoload_known_extensions` are both
`false`, so the engine cannot reach the network either. The test
`the_engine_does_not_download_an_extension` proves it.

The lock changes what a statement from `-q` can do. A statement can no longer
read a file that the user did not open. The owner accepted this. The function
`explain_lock` gives a message that says so and says how to read a second file.
The test `a_query_that_reads_a_file_the_user_did_not_open_gets_a_clear_message`
proves the message, and `the_lock_stops_a_write_to_a_file_outside_the_source`
proves that a `COPY … TO` past the guard still cannot write.

The view `src` is a view in memory. A `CREATE OR REPLACE VIEW` statement and
the index of a file of text therefore write to memory only.

### The scratch directory

One write to the disk does happen. A query can need more memory than its limit,
and a sort or a join over a file that is larger than the memory is the usual
reason. DuckDB then spills the rows to the disk. The function `configure` in
`engine.rs` gives it a directory of its own for that work: it sets
`temp_directory` to a directory below `peruse` inside the temporary directory
of the system, with a name for each engine. The setting keeps the pages away
from the directory that the user works in. Each engine has its own directory,
because DuckDB gives the same file names to each database: two windows of
Peruse that shared one directory would read the pages of each other.

The spill is not a hole in the promise. DuckDB writes its own pages there,
under names that it chooses, and it deletes them when the query ends. It never
writes to a data file of the user, and it never writes to a database file.

### A database file

A DuckDB database file is the one source that DuckDB itself opens. The engine
attaches it with the flag `READ_ONLY`:

```sql
ATTACH 'C:/data/shop.duckdb' AS "__peruse_db" (READ_ONLY);
```

The promise is stronger there than for a data file. The storage engine of DuckDB
refuses each write to that file, so the promise does not rest on the words of a
statement at all. The test `the_read_only_flag_stops_a_write_to_the_database`
writes through the connection, past the guard, and the database refuses it.

The guard still refuses a typed `ATTACH`, and the lock refuses it a second time,
so a user cannot attach a second database and write to that one.

## Layer A: the words of the statement

The function `ensure_read_only` examines a statement. It does these steps:

1. It removes the parts that can hold any text.
2. It rejects an empty text.
3. It rejects more than one statement.
4. It rejects a statement that does not start with a word that only reads.
5. It rejects a statement that holds a word that writes, at any position.
6. It rejects a statement that calls a function that writes, changes the state,
   or runs SQL from a text value.

### Step 1: the clean-up

The function `scrub` replaces these parts:

| Part | Form | Replacement |
|---|---|---|
| A line comment | `-- …` to a line feed or a carriage return | Nothing |
| A block comment | `/* … */`, which can nest | One space |
| A value | `'…'`, with `''` for one quotation mark | `''` |
| An escape string | `E'…'`, where a backslash hides the next character | `''` |
| An identifier | `"…"`, with `""` for one quotation mark | `id` |
| A dollar value | `$$ … $$`, or `$tag$ … $tag$` with a tag | `''` |

This step must come first. Without it, a column with the true name `"update"`
or a value `'drop table'` would look like a statement that writes.

The clean-up must agree with the scanner of DuckDB, or a `;` could hide from the
guard but not from DuckDB. Two rules make it agree:

- In an escape string `E'…'`, a backslash hides the next character. The value
  `E'\''` holds one quotation mark, and the `\'` does not end the value. A
  letter `E` or `e` is a string prefix only when no identifier character comes
  in front of it, so the identifier `codeE` is not a prefix.
- A `$` starts a dollar value only when no identifier character comes in front
  of it, and the tag follows the rules of an identifier. A `$` after an
  identifier, or a parameter such as `$1`, is an ordinary character. DuckDB
  reads it the same way.

The function also counts the statements. A semicolon separates two statements
only when text is on both sides of it. One semicolon at the end is therefore
correct. A semicolon inside a comment or a value is not a separator, because
step 1 removes it first.

### Step 4: the first word

A statement must start with one of these words:

`SELECT`, `WITH`, `FROM`, `DESCRIBE`, `DESC`, `SHOW`, `SUMMARIZE`, `EXPLAIN`,
`VALUES`, `TABLE`, `PIVOT`, `UNPIVOT`

### Step 5: the words that write

The statement must hold none of these words, at any position:

`INSERT`, `INTO`, `UPDATE`, `DELETE`, `DROP`, `CREATE`, `ALTER`,
`TRUNCATE`, `ATTACH`, `DETACH`, `COPY`, `EXPORT`, `IMPORT`, `INSTALL`, `LOAD`,
`VACUUM`, `CHECKPOINT`, `CALL`, `SET`, `RESET`, `PRAGMA`, `BEGIN`, `COMMIT`,
`ROLLBACK`, `TRANSACTION`, `PREPARE`, `EXECUTE`, `DEALLOCATE`, `GRANT`,
`REVOKE`, `USE`

A test of the first word alone is not sufficient. These two statements start
with an allowed word, but both of them write:

```sql
WITH a AS (SELECT 1) INSERT INTO t SELECT * FROM a
EXPLAIN COPY src TO 'out.parquet'
```

### Step 6: the functions that write

A keyword cannot hide inside a value, because step 1 blanks each value first. A
function is different: a user can call one in a `FROM` part or a `SELECT` part
with no keyword at all. Some functions of DuckDB write a file, change the state
of the database, or run SQL that comes from a text value. The guard refuses a
call to each of them: `enable_logging`, `disable_logging`, `write_log`,
`enable_profiling`, `disable_profiling`, `checkpoint`, `force_checkpoint`,
`truncate_duckdb_logs`, `query`, `query_table`, `json_execute_serialized_sql`,
`copy_database` and `import_database`.

The guard matches a name only when a `(` follows it, with spaces allowed
between. A column named `query` therefore still passes, and a call `query(…)`
fails. The function `query(…)` and the function `json_execute_serialized_sql(…)`
run SQL that comes from a text value. The clean-up blanks that value, so the
guard cannot see the SQL inside, and the only safe answer is to refuse the call.

The list comes from the catalogue of DuckDB. The test
`every_catalogue_function_is_classified` reads `duckdb_functions()` and the
pragma list. It fails when a later version of DuckDB adds a table function, a
table macro or a pragma that this module does not put in one of two lists: the
list that the guard refuses, or a list of functions that a review found safe. A
new function is then a review item, and not a silent hole. A pragma reaches the
engine only through `PRAGMA` or `CALL`, which step 5 refuses already.

## Layer B: a stored view or a stored macro

A DuckDB database file can hold a view whose query, or a macro whose body, calls
a function that writes a file. DuckDB runs that SQL when Peruse reads the
object, and the SQL does not come from the user, so step 4 and step 5 do not
see it.

Before Peruse reads any view or macro of a database, the function
`check_stored_objects` reads the stored SQL from `duckdb_views()` and from the
macro definitions, and it runs each through the guard. A view body is a
`CREATE VIEW … AS <query>`, so the guard drops the part up to the first `AS`
and checks the query. A database with a view or a macro that writes does not
open, and the message names the object. The test
`a_database_with_a_view_that_writes_does_not_open` proves it, and
`a_database_with_a_normal_view_still_opens` proves that a view that reads is
not refused.

## Layer D: the read-only handle on Windows

While Peruse shows a file from a local disk, the engine holds the file open
with a share mode that allows a read but denies a write and a delete
(`FILE_SHARE_READ`, with no `FILE_SHARE_WRITE` and no `FILE_SHARE_DELETE`). No
program can change the file while Peruse shows it. The handle covers each source
file, the write-ahead log of a database, and the local copy of a file from a
network drive. It releases when the engine closes, which is when the user opens
another file or quits. Refer to `read_only_locks`.

The engine never takes this handle on a file on a network drive. A coworker must
still write that file, so Peruse uses the same test as the local copies, through
`crate::remote`, and leaves a network file alone. The test
`a_local_file_cannot_be_written_while_peruse_reads_it` proves the handle, and
`a_file_on_a_network_drive_is_never_locked` proves the exception.

Linux and macOS have no share mode that stops another program from writing an
open file. On those systems this layer is empty, and the promise rests on the
other four layers. The field `locks` of the engine is then an empty list.

## Layer E: the writes of Peruse itself

Peruse writes only these paths, and none of them can be a source file:

- The settings file and the list of recent files, in the configuration
  directory.
- A theme file, which the user drops in the themes directory by hand. Peruse
  reads a theme file; it does not write one.
- The cache of the local copies, in the cache directory.
- The temporary directory of an engine, below the temporary directory of the
  system.
- The standard output, for `--ddl`. The clipboard, through the terminal.

The cache needs the most care, because it removes a directory tree to make
room. The functions `evict` and `clear` remove an entry only through
`remove_entry`, and `remove_entry` removes a directory only when the directory:

- holds the marker file `.peruse-cache`, which `plan` writes when it makes the
  entry;
- is a true directory and not a symbolic link or a junction;
- sits directly below the cache root.

The removal never follows a symbolic link or a junction: it removes the link
itself, so the target of the link stays. A directory that fails any of the
three tests stays. The test
`a_cache_entry_without_the_marker_is_never_removed` proves the marker rule, and
`a_junction_in_the_cache_is_removed_as_a_link_and_its_target_survives` proves
that a junction does not lead the delete out of the cache.

## What passes and what Peruse rejects

These statements pass:

```sql
SELECT * FROM src
WITH a AS (SELECT 1) SELECT * FROM a
FROM src SELECT count(*)
DESCRIBE src
SUMMARIZE src
EXPLAIN SELECT 1
SELECT * FROM src WHERE name = 'drop table x'
SELECT "update", "delete" FROM src
SELECT * FROM src -- copy this later
SELECT * /* insert */ FROM src
SELECT 1;
```

Peruse rejects these statements:

```sql
DROP TABLE src
SELECT * INTO other FROM src
INSERT INTO src VALUES (1)
UPDATE src SET a = 1
DELETE FROM src
CREATE TABLE t AS SELECT 1
ATTACH 'x.db'
INSTALL httpfs
COPY src TO 'out.csv'
EXPORT DATABASE 'dir'
PRAGMA database_list
SET threads TO 1
SELECT 1; DROP TABLE src
SELECT 'abc
```

## The filter expression

The function `ensure_safe_predicate` checks a `WHERE` expression. Peruse puts
that expression into a statement that it builds. The expression must therefore
not close the condition and start a new part.

The function puts `SELECT 1 WHERE` in front of the expression, and it then
calls `ensure_read_only`. The expression gets the same clean-up and the same
list of words that write. The text `1=1; DROP TABLE src` therefore fails,
because it holds two statements.

The function then makes one more test that a complete statement does not need:
the parentheses of the expression must balance. Peruse writes the expression
into `WHERE (<expr>)`, and it adds `ORDER BY` and `LIMIT` after it. An
expression with one more `)` than `(` closes the condition too early, and the
rest of the expression then becomes part of the statement. The expression
`1) ORDER BY 1--` would make the comment marker remove the `LIMIT` part, and
the engine would read the full file into one page. The clean-up removes the
comments and the text in quotation marks first, so a parenthesis inside a value
does not count.

## The filter builder writes safe SQL

The module `filter` builds a `WHERE` expression from a list of conditions. That
expression is safe because of the way that the module builds it, and not
because of a check after it:

- Each column name goes through `query::quote_ident`, so a double quotation
  mark in a name becomes two double quotation marks.
- Each value goes through `query::quote_str`, so a single quotation mark in a
  value becomes two single quotation marks. A value can therefore not close its
  own quotation marks.
- Each term goes in parentheses, and the compiler adds one pair for each step.
  The parentheses always balance.

The test `each_compiled_filter_passes_the_read_only_guard` in `filter.rs` puts
injection strings through each operator and each family of values, and it
asserts that `ensure_safe_predicate` accepts each result. The function
`App::apply_fset` also calls the guard on the compiled text. That call is not
necessary today, and it keeps the promise true if a later change to the
compiler makes a mistake.

## What the promise does not cover

The promise is about writing. Layer C keeps a statement to the files that the
user opened, so a statement can no longer read a file that the user did not
open, such as `read_csv('/etc/passwd')`. This is a change from earlier versions
of Peruse, and the owner accepted it. Peruse runs the SQL of the user with the
permissions of the user, but only on the files that the user chose. Peruse also
blocks a read over the network: it never installs an extension and never loads
one, the lock sets `enable_external_access = false`, and the test
`the_engine_does_not_download_an_extension` in `engine.rs` proves it.

## The errors

The enumeration `GuardError` gives the reason for a refusal:

| Value | Meaning |
|---|---|
| `Empty` | The text holds no statement. |
| `MultipleStatements` | The text holds more than one statement. |
| `NotAQuery(word)` | The first word is not a word that only reads. |
| `Forbidden(word)` | A word that writes, or a function that writes, is in the statement. |
| `UnterminatedLiteral` | A quotation mark or a comment has no end. |
| `UnbalancedParens` | A filter expression has a different number of `(` and `)`. |

## Where the guard runs

The guard runs at four points:

1. In `main.rs`, on the option `--query`, before Peruse opens the file.
2. In `main.rs`, on the option `--filter`, before Peruse opens the file.
3. In `app.rs`, after each key in the prompt. The user therefore sees an error
   before the user presses Enter.
4. In `worker.rs`, on each request `SetView`. The worker checks the statement
   with `ensure_read_only` and the filter with `ensure_safe_predicate`. A view
   holds text of the user in these two places, and each place gets the check
   that fits it.

The check in the worker is the last check. It makes sure that no text of the
user reaches the engine without a check, whatever the caller does. The test
`a_filter_that_writes_never_reaches_the_engine` sends a filter that writes
directly to the worker, past the front end, and the worker refuses it.
