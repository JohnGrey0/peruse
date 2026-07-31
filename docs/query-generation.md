# How Peruse builds the statements

One structure, the `View`, describes what the grid shows. Each statement that
Peruse runs comes from that one structure: the schema, a page of rows, the row
count, one cell, one row as JSON, the statistics of a column, the facts of the
detail band, the histogram and the search. A filter from the prompt and a sort
from the key `s` therefore work together, and the code needs no special case for
them. The code is in `crates/peruse-core/src/query.rs`.

## The view

The structure `View` has three fields:

| Field | Meaning |
|---|---|
| `base` | The relation to read from |
| `filter` | A `WHERE` expression from the user, with no `WHERE` word |
| `sort` | The columns to sort on, with a direction for each column |

The field `base` has two forms:

- `Base::Source` reads the open file. The engine gives the file the name `src`.
  For a database, `src` is a view over one table of that database.
- `Base::Sql(text)` reads a statement from the user. The module `sql_guard`
  checks that statement first.

## The three parts of each statement

Three private functions build the parts that each statement needs:

| Function | Result for a file | Result for a statement from the user |
|---|---|---|
| `relation()` | `src AS q` | `(<statement>) AS q` |
| `where_clause()` | `` (empty) `` or ` WHERE (<filter>)` | The same |
| `order_clause()` | `` (empty) `` or ` ORDER BY "a" ASC, "b" DESC` | The same |

The function `relation()` removes a semicolon at the end of a statement from
the user. Each statement uses the same name `q` for its one relation.

Two public functions join those parts:

| Function | Result |
|---|---|
| `select(projection)` | `SELECT <projection> FROM … WHERE … ORDER BY …`, with no `LIMIT` part |
| `scan_from()` | `FROM … WHERE …`, with no `ORDER BY` part |

A caller that builds its own list of aggregates uses `scan_from()`. It needs the
same rows that the grid shows, and an aggregate needs no sort. The profile of a
file for a `CREATE TABLE` statement and the facts of the detail band are two such
callers.

The function `quote_ident` puts an identifier in double quotation marks, and it
writes a quotation mark in the name two times. The function `quote_str` does
the same for a value with single quotation marks. A column with a strange name
therefore works.

## The statements

### The schema

```sql
DESCRIBE SELECT * FROM src AS q
```

The function `describe_sql` builds this statement. It reads no rows. The result
gives the name, the type and the NULL property of each column. The engine reads
`"NO"` in the column `null` as a column that cannot hold NULL.

### A page of rows

```sql
SELECT <projection> FROM (SELECT * FROM src AS q WHERE (…) ORDER BY …
LIMIT 250 OFFSET 1000) AS page
```

The function `page_sql` builds this statement. The `LIMIT` part comes inside,
and the projection comes outside.

That order is worth the extra pair of parentheses. The database takes the page
first, and it then changes the values of those rows only. With the projection
beside the `LIMIT` part, the database changes the values of a whole block of
rows and shares that work across each of its threads. One page of 50 rows then
costs 8.3 milliseconds, and with the projection outside the same page costs 5.3.
A sorted page costs 12.4 and 8.5.

The order of the rows comes from the `LIMIT` part inside. The setting
`preserve_insertion_order` is `true`, so the projection outside keeps that
order.

The `LIMIT` part starts on a new line. A filter that ends with the comment
marker `--` then hides only the remainder of its own line, so the `LIMIT` part
stays active and the engine reads one page and not the full file. The function
`sql_guard::ensure_safe_predicate` refuses such a filter, and this new line is
the second protection.

The projection comes from `display_projection`. That function writes one item
for each column:

- For a BLOB column: `('blob ' || octet_length("c") || ' B')`
- For each other column: `substr(CAST("c" AS VARCHAR), 1, 4096)`

The cast happens in the database, so each type looks the same as it looks in
DuckDB. The function `substr` limits the quantity of data that moves to the
grid, so a wide column of text costs the same as a narrow one.

### The row count

```sql
SELECT count(*) FROM src AS q WHERE (…)
```

The function `count_sql` builds this statement. The statement has no `ORDER BY`
part. A sort cannot change a count, but it makes the database sort all of the
rows.

### One cell

```sql
SELECT CAST("c" AS VARCHAR) FROM src AS q WHERE (…) ORDER BY … LIMIT 1 OFFSET 42
```

The function `cell_sql` builds this statement for the cell inspector. The
statement has no `substr` part, so it gives the complete value.

### One row as JSON

```sql
SELECT CAST(to_json({'id': "id", 'actor': "actor", …}) AS VARCHAR)
FROM (SELECT * FROM src AS q WHERE … ORDER BY …) AS t
LIMIT 1 OFFSET 12480
```

The function `row_json_sql` builds this statement for the record view. JSON has
one rule for a value that holds a quotation mark, and the text that DuckDB
writes for a structure has none. The document
[nested-values.md](nested-values.md) gives the reason in full.

Two families of values do not go into the JSON as they are, and the rules are
the rules of the grid: a BLOB becomes its size, and a long text stops at 4096
characters.

### The statistics of a column

The function `stats_sql` builds one statement with these values:

- `count(*)` as the number of rows
- `count("c")` as the number of values that are not NULL
- `approx_count_distinct("c")` as the number of different values
- the minimum, the maximum, the mean and the standard deviation

The last four values follow the family of the column:

| Family | Minimum and maximum | Mean and deviation |
|---|---|---|
| Number | `min()` and `max()` | `avg()` and `stddev_samp()` |
| Text, Bool, Temporal | `min()` and `max()` | `NULL` |
| Binary, Nested | `NULL` | `NULL` |

The minimum and the maximum have a meaning for each type that has an order. The
mean and the deviation do not. A call to `avg()` on a VARCHAR column gives an
error, and not a NULL.

A column of numbers gets two more values at the end: the smallest value and the
largest value as a DOUBLE. The histogram needs those two edges, and this
statement reads the column one time already.

### The facts of the detail band

```sql
SELECT count(*) AS n_total,
  count("id") AS n_present_0, approx_count_distinct("id") AS n_distinct_0,
  substr(CAST(min("id") AS VARCHAR), 1, 64) AS v_min_0,
  substr(CAST(max("id") AS VARCHAR), 1, 64) AS v_max_0,
  count("name") AS n_present_1, approx_count_distinct("name") AS n_distinct_1,
  substr(CAST(min("name") AS VARCHAR), 1, 64) AS v_min_1,
  substr(CAST(max("name") AS VARCHAR), 1, 64) AS v_max_1
FROM src AS q WHERE (region = 'EU')
```

The function `band_sql` builds this statement. One statement measures every
column that the grid draws, so the database reads the view one time for all of
them. The statement gives the count of the rows one time, and then four values
for each column, in this order: the count of the values that are not NULL, the
count of the different values, the smallest value and the largest value. The
function `Engine::column_band` reads them by position.

Three rules control the statement:

- A structure, a list and a BLOB get `NULL AS v_min_i` and `NULL AS v_max_i`.
  Those three have no order, and `min()` over one of them gives an error and not
  a NULL.
- The two edges go through `substr` with `BAND_VALUE_CHARS` = 64 characters, so
  one huge value cannot arrive in full.
- The statement has no `ORDER BY` part. An aggregate needs none, and a sort of
  every row is slow.

The count of the different values is an estimate. `approx_count_distinct` reads
the column one time and holds little memory, and the band has room for a number
of two or three digits only.

The statement runs for a source that is not a plain Parquet file, such as a file
of text or a database, for a filtered view, for a view that holds a statement of
the user, for a column that the footer cannot name, and for the detailed mode. A
compact band over a plain Parquet file reads the footer instead and runs no
statement at all. See [performance.md](performance.md).

### The most frequent values

```sql
SELECT CAST("c" AS VARCHAR) AS v, count(*) AS n FROM src AS q
GROUP BY 1 ORDER BY n DESC, v LIMIT 8
```

The function `top_values_sql` builds this statement. The second sort key `v`
makes the order the same for two values with the same count.

### The histogram

The histogram needs two statements. The function `bounds_sql` gives the two
edges:

```sql
SELECT min("c")::DOUBLE, max("c")::DOUBLE FROM src AS q
```

The statistics panel does not call it: it takes the two edges from `stats_sql`,
which reads the column one time for the statistics and the edges together. The
function stays for a caller that wants the two edges alone.

The function `histogram_sql` then counts the values in 24 buckets of equal
width:

```sql
SELECT least(24 - 1, floor(("c"::DOUBLE - lo) / width))::BIGINT AS b,
       count(*)::BIGINT AS n
FROM src AS q WHERE "c" IS NOT NULL AND isfinite("c"::DOUBLE)
GROUP BY b ORDER BY b
```

The caller gives the two edges as numbers. A common table expression could
calculate them, but then the statement would read the column two times and join
it to itself. With the edges as numbers, one pass is enough. The function
`least` puts the largest value in the last bucket.

### The search

```sql
SELECT 5000 + off AS off FROM (
  SELECT row_number() OVER () - 1 AS off, *
  FROM (SELECT * FROM src AS q ORDER BY … LIMIT 250000 OFFSET 5000)
) WHERE <conditions> ORDER BY off LIMIT 500
```

The function `search_sql` builds this statement. The design has a reason:

- The caller needs a row offset that it can move the viewport to. The row
  numbers must therefore agree with the numbers that `page_sql` gives. Only the
  SQL function `row_number()` can do this.
- A window over the full table would number each row before the database could
  report the first match. On ten million rows, the user would wait some seconds
  and see nothing.
- The statement numbers the rows of one part of the view instead. The cost of
  each call is proportional to the size of that part.

The conditions use one item for each column:

- For a text column: `contains(lower("c"), lower('needle'))`
- For a BLOB column: no item, because the grid shows the size only
- For each other column: `contains(lower(CAST("c" AS VARCHAR)), lower('needle'))`

The function `contains` looks for a plain part of a text. `ILIKE` reads a
pattern first, and it costs about three times as much for the same answer. Over
a scan of 250,000 rows and nine columns, `ILIKE` needs 265 milliseconds and
`contains` needs 91.

The database puts the needle in lower case one time, because the needle is a
constant. Both sides of the test therefore follow the same rule for a letter,
also for a letter that English does not use.

The needle needs no escape character. A search for `50%` looks for the three
characters, and `%` is not a wildcard here. A text column needs no cast, because
a cast would copy each value for no gain.

The statement is empty when the schema holds BLOB columns only.
