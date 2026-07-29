# The guard that keeps Peruse read-only

Peruse gives the user a promise: it does not change the data. Two layers keep
that promise. The first layer is the structure of the connection to DuckDB. The
second layer is the module `sql_guard`, which examines the words of a statement
from the user. The code is in `crates/peruse-core/src/sql_guard.rs`.

The two layers do not cover the same statements, and it is important to know
which layer stops what:

- Layer 1 covers each statement that **Peruse itself** builds. Peruse cannot
  write, because it holds no connection that can write.
- Layer 2 covers each statement that **the user** types. A statement such as
  `COPY (SELECT * FROM src) TO 'trips.parquet'` would write, and DuckDB would
  run it. Layer 2 is the layer that rejects it, and layer 2 works alone for
  this class of statement.

Layer 1 therefore makes the promise cheap to keep for the code of Peruse, and
layer 2 keeps the promise for the text of the user.

## Layer 1: the structure of the connection

Three facts stop a write, and none of them needs a check of the text:

- The database is always in memory. The engine calls
  `Connection::open_in_memory`, so the catalog holds no file on the disk.
- The engine reaches the files of the user only through `read_parquet` and
  `read_csv`. These two table functions can read, but they cannot write.
- DuckDB never opens the files of the user for write access.

The view `src` is a view in memory. A `CREATE OR REPLACE VIEW` statement and
the CSV index therefore write to memory only.

## Layer 2: the words of the statement

The function `ensure_read_only` examines a statement. It does these steps:

1. It removes the parts that can hold any text.
2. It rejects an empty text.
3. It rejects more than one statement.
4. It rejects a statement that does not start with a word that only reads.
5. It rejects a statement that holds a word that writes, at any position.

### Step 1: the clean-up

The function `scrub` replaces these parts:

| Part | Form | Replacement |
|---|---|---|
| A line comment | `-- …` to the end of the line | Nothing |
| A block comment | `/* … */` | One space |
| A value | `'…'`, with `''` for one quotation mark | `''` |
| An identifier | `"…"`, with `""` for one quotation mark | `id` |
| A value with dollar signs | `$$ … $$` | `''` |

This step must come first. Without it, a column with the true name `"update"`
or a value `'drop table'` would look like a statement that writes.

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

`INSERT`, `UPDATE`, `DELETE`, `DROP`, `CREATE`, `ALTER`, `REPLACE`,
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

The promise is about writing. A statement that the user types can still *read*
another file on the machine, for example with `read_csv('/etc/passwd')` inside
a subquery. Peruse runs the SQL of the user with the permissions of the user.
Peruse does block a read over the network: it never installs an extension and
never loads one, and the test
`the_engine_does_not_download_an_extension` in `engine.rs` proves it.

## The errors

The enumeration `GuardError` gives the reason for a refusal:

| Value | Meaning |
|---|---|
| `Empty` | The text holds no statement. |
| `MultipleStatements` | The text holds more than one statement. |
| `NotAQuery(word)` | The first word is not a word that only reads. |
| `Forbidden(word)` | A word that writes is in the statement. |
| `UnterminatedLiteral` | A quotation mark or a comment has no end. |
| `UnbalancedParens` | A filter expression has a different number of `(` and `)`. |

## Where the guard runs

The guard runs at four points:

1. In `main.rs`, on the option `--query`, before Peruse opens the file.
2. In `main.rs`, on the option `--filter`, before Peruse opens the file.
3. In `app.rs`, after each key in the prompt. The user therefore sees an error
   before the user presses Enter.
4. In `worker.rs`, on each request `SetView` with a statement from the user.

The check in the worker is the last check. It makes sure that no statement
reaches the engine without a check, whatever the caller does.
