# A value that holds other values

A JSON file, and a Parquet file with a structure inside it, hold a value that
holds other values. A grid has no place for such a value. This document says
what Peruse does about it. The code is in `crates/peruse-tui/src/tree.rs`, with
the query in `query.rs` and the reader in `engine.rs`.

## The problem

A JSON file frequently holds a list of objects, and each object holds other
objects:

```json
{"id": "2489651045", "type": "CreateEvent",
 "actor": {"id": 665991, "login": "petroav", "url": "https://api.github.com/…"},
 "payload": {"ref": "master", "commits": [{"sha": "aa"}, {"sha": "bb"}]}}
```

DuckDB reads such a file and gives a column of the type `STRUCT`. The grid can
only show that column as one long text:

```text
{'id': 665991, 'login': petroav, 'gravatar_id': '', 'url': 'https://api.git…
```

That text says what the value holds. It does not let the user read one field,
copy one field, or filter on one field.

## The answer: a tree

The record view (the key `r`) draws the row as a tree. Each field is one line,
and a line that holds other values opens and closes. The user moves through the
levels with `l` and `h`, and the key `/` finds a field at any level.

## Why the row arrives as JSON

The engine sends the row as JSON, and not as the text that DuckDB writes for a
structure. The text of a structure looks like this:

```text
{'id': 665991, 'login': petroav, 'gravatar_id': ''}
```

It has no reliable rule for a value that holds a quotation mark, a comma or a
brace. Peruse would have to write a parser for it, and that parser would be
wrong for some value in some file.

JSON has one rule, the database writes it, and a library reads it. The
statement is in `View::row_json_sql`:

```sql
SELECT CAST(to_json({'id': id, 'actor': actor, …}) AS VARCHAR)
FROM (SELECT * FROM src AS q WHERE … ORDER BY …) AS t
LIMIT 1 OFFSET 12480
```

One row of the usual file is some hundred bytes, so one statement for the
complete row costs less than one statement for each field.

Two families of values do not go into the JSON as they are. A BLOB becomes its
size, and a long text stops at 4096 characters. The rules are the same as the
rules of the grid, and for the same reason: a value of 10 MB must not move to
the user interface.

## The cost, and the index

The statement reads one row. For a Parquet file that is immediate. For a CSV
file or a JSON file with no index, DuckDB reads the file again for each row,
because a file of text has no structure that lets the reader go to a row.

Peruse indexes a file of text below 256 MB when it opens the file, so the usual
case is fast. For a larger file, the footer shows the note, and the key `I`
builds the index. See [engine.md](engine.md).

## The rule about a field with no value

DuckDB gives one type to a column. For a JSON file, it therefore joins the
fields of each row into one structure. The `payload` of a file of GitHub events
holds 20 fields for that reason, and one row holds a value in five of them.

Those 15 NULLs are the absence of a field in that row. They are not 15 missing
values, and a screen of them hides the five values that the row does have.

The tree therefore hides a field with no value **inside a structure**, and it
says how many it hides:

```text
▸ payload    struct    {5 of 20 fields}
```

The key `z` shows them. The count is always on the screen, so the tree never
hides anything in silence.

**A column of the row is a different case.** The schema declares that column,
so a NULL there is a value that the row does not have, exactly as in the grid.
The record view therefore shows each column always. The rule is in the function
`walk`, as the test `tree.rs::a_field_with_no_value_inside_a_structure_is_hidden_at_the_start`
proves.

## The path

Each line of the tree holds the path from the row to its value. The path has
two forms, and `query.rs` builds both:

| Function | Form | Use |
|---|---|---|
| `path_text` | `payload.commits[1].sha` | The screen, and the message about a filter. |
| `quote_path` | `"payload"."commits"[2]."sha"` | A statement. |

The two forms count the items of a list differently. Peruse counts each
position from 0, as it does everywhere else. DuckDB counts the items of a list
from 1. The function `quote_path` adds the one.

## A filter on a value inside a structure

The key `=` in the record view adds a condition on the selected value. Two
cases are not the same:

- **A column of the row.** The condition holds the column, and the filter
  builder can show it and edit it later. See [filter.md](filter.md).
- **A value inside a structure.** There is no column, so the condition holds
  the path as an expression, such as `"actor"."login" = 'petroav'`. It goes
  into the filter as a `Raw` term.

The guard runs on the expression in each case, so a condition from the tree
cannot reach the engine without a check.

A value that holds other values cannot go into a condition. The view says so
instead of building a statement that the database would refuse.

## The limits

- The tree opens 64 levels. No file makes a tree that deep, and the limit stops
  a value that points to itself from filling the memory.
- The find box looks at the name and at the value of each line. It keeps a line
  that holds a match below it, and it opens that line, so the user sees the way
  to the match.
- The order of the fields is the order of the file. The library that reads the
  JSON needs the feature `preserve_order` for that. Without it, the record view
  would show the columns in the order of the alphabet, and it would not agree
  with the grid.
