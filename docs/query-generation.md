# How Peruse builds the statements

One structure, the `View`, describes what the grid shows. Each statement that
Peruse runs comes from that one structure: the schema, a page of rows, the row
count, one cell, the statistics of a column, the histogram and the search. A
filter from the prompt and a sort from the key `s` therefore work together, and
the code needs no special case for them. The code is in
`crates/peruse-core/src/query.rs`.

## The view

The structure `View` has three fields:

| Field | Meaning |
|---|---|
| `base` | The relation to read from |
| `filter` | A `WHERE` expression from the user, with no `WHERE` word |
| `sort` | The columns to sort on, with a direction for each column |

The field `base` has two forms:

- `Base::Source` reads the open file. The engine gives the file the name `src`.
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
SELECT <projection> FROM src AS q WHERE (…) ORDER BY … LIMIT 250 OFFSET 1000
```

The function `page_sql` builds this statement. The projection comes from
`display_projection`. That function writes one item for each column:

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

- For a text column: `"c" ILIKE '%needle%' ESCAPE '\'`
- For a BLOB column: no item, because the grid shows the size only
- For each other column: `CAST("c" AS VARCHAR) ILIKE '%needle%' ESCAPE '\'`

A text column needs no cast, and a cast would copy each value. The function
first replaces `\`, `%` and `_` in the value of the user, so these characters
have no special meaning.

The statement is empty when the schema holds BLOB columns only.
