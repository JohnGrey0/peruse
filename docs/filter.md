# The filter

Peruse holds the filter as a list of conditions, and not as one text. The list
compiles to a `WHERE` expression, and that expression goes into
`View::filter`. Each part of the code after that point sees one expression, so
the sort, the count, the statistics and the search all need no change. The
model is in `crates/peruse-core/src/filter.rs`, and the user interface for it
is in `app.rs` and `overlays.rs`.

## Why a list, and not a text

The filter prompt takes a `WHERE` expression as text. That prompt is quick for
a user who knows SQL. It is also the one thing that a user who does not know
SQL cannot use, and a viewer of data has many such users.

A list of conditions gives three things that a text cannot give:

- A user can build a condition from menus, with no knowledge of SQL.
- A key can add one condition to the filter, and keep the conditions that are
  in it already.
- The user interface can show the conditions, and the user can edit one of
  them or delete one of them.

The text stays available. An expression that the user types becomes one
condition of the same list, so the two forms stand together.

## The model

```
FilterSet
  conditions: Vec<Condition>

Condition
  join: Join          AND or OR. The first condition does not use it.
  term: Term

Term
  Cmp { column, kind, op, value, value2 }   a condition from the menus
  Raw(String)                               an expression that the user typed
```

The value `kind` is the family of values of the column, from
[`CellKind`](../crates/peruse-core/src/model.rs). It decides the form of the
value in the statement, and it decides which operators the builder offers.

## The operators

The function `Op::for_kind` gives the operators that have a meaning for a
family of values. The list starts with the operator that a user needs most.

| Family | Operators |
|---|---|
| Number, Temporal | `=` `<>` `>` `>=` `<` `<=` `between` `is one of` `is null` `is not null` |
| Text | `contains` `=` `<>` `starts with` `ends with` `does not contain` `is one of` `>` `<` `is null` `is not null` |
| Bool | `=` `<>` `is null` `is not null` |
| Binary | `is null` `is not null` |
| Nested | `contains` `does not contain` `is null` `is not null` |

A BLOB column gets the two tests for NULL only. The grid shows the size of such
a value and not its bytes, so a comparison against the text in the grid would
find no row and confuse the user.

## How a term becomes SQL

Each term gives an expression in parentheses.

| Operator | SQL |
|---|---|
| `is null` | `("c" IS NULL)` |
| `=` | `("c" = <value>)` |
| `between` | `("c" BETWEEN <value> AND <value2>)` |
| `is one of` | `("c" IN (<value>, <value>, …))` |
| `contains` | `(<c as text> ILIKE '%value%' ESCAPE '\')` |
| `does not contain` | `(NOT coalesce(<c as text> ILIKE '%value%' ESCAPE '\', false))` |

Four rules control the form of the SQL:

- **A number stays a number.** A value that holds digits only goes into the
  statement without quotation marks, so the comparison uses the order of the
  numbers. Each other value goes in quotation marks, and DuckDB casts it. A
  date therefore works as `'2024-01-01'`, and a bad value gives a message from
  the database and not a broken statement. The test of the characters comes
  before the test of the number, because `f64` reads the words `inf` and `NaN`,
  and those two words are names in a statement.
- **A text column needs no cast.** Each other family needs `CAST(… AS VARCHAR)`
  before `ILIKE`.
- **The characters `%` and `_` lose their meaning.** The user types a value,
  and not a pattern.
- **`does not contain` keeps the rows that hold NULL.** `NOT (NULL ILIKE …)`
  gives NULL, and the row would go away. A row with no text does not hold the
  value, so it must stay. The call to `coalesce` keeps it.

An `is one of` condition breaks its text at each comma. A comma inside
quotation marks belongs to the value, so `'a,b', c` gives the two values `a,b`
and `c`. An empty list gives `(false)`, because an empty `IN ()` is not legal
SQL.

## How the list becomes one expression

The compiler works from the left to the right, and it adds one pair of
parentheses at each step:

```
a OR b AND c   →   (((a) OR (b)) AND (c))
```

This order is not the SQL order, in which `AND` binds before `OR`. The user
reads the list from the top to the bottom, so the result must follow that same
order. What the user reads is what runs.

A condition that makes no test does not reach the statement. A condition is
blank when its operator needs a value and the value is empty, or when the
operator is `between` and the second value is empty.

## The three ways to filter

Each of the three builds the same list.

| Key | Command | What it does |
|---|---|---|
| `f` | `FilterBuild` | Opens the builder. It asks for a column, an operator and a value. |
| `E` | `Filter` | Opens the prompt that takes a `WHERE` expression. The text becomes the complete filter, as one `Raw` term. |
| `=` and `!` | `FilterThisValue`, `FilterExcludeValue` | Add one condition on the cell under the cursor, and apply it. |

The quick filters make two tests before they add a condition:

- A BLOB column has no value to filter on.
- A value that the page cut short would give a test for equality against text
  that no row holds. Peruse refuses, and it names the key that can filter on
  part of the value.

A missing value gives `IS NULL` or `IS NOT NULL`, and not a comparison against
the word "NULL".

## The steps of the builder

The builder is a small machine. The value `App::build` holds the step.

```
List ──a──> Column ──Enter──> Op ──Enter──> Value ──Enter──> List
  │                                          │
  │                                          └──Enter (between)──> Value2 ──> List
  │
  ├──r──> Raw ──Enter──> List
  └──Enter──> apply and close
```

- `Esc` goes back one step. From the list, it puts back the filter from the
  time before the builder opened.
- The builder opens on the step `Column` when the list is empty. An empty box
  is no use to the user, and the column is the first question in each case.
- The step `Column` starts on the column under the cursor. That is the column
  that the user looks at.
- The step `Op` starts on the operator of the draft condition. A new condition
  starts on `=`.
- The list shows the compiled expression, so the user can see what goes to the
  database.

## The safety of the compiled text

See [read-only-guard.md](read-only-guard.md). In short: the module writes the
statement itself, each name and each value goes through a quoting function, and
the parentheses always balance. A test feeds injection strings through each
operator and each family of values.
