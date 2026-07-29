# The table generator

The option `--ddl` writes a `CREATE TABLE` statement for another database from
a data file. The module `crates/peruse-core/src/ddl.rs` holds the rules, and
`Engine::profile` measures the file. The command line calls both and writes the
result to the standard output.

## Why it is in Peruse

A person who looks at a data file often has the next job in mind: to load the
file into a warehouse. That job needs a table, and a table needs a type for
each column, a rule about NULL, a size for each text column, a primary key and
some indexes.

The file holds the answer to most of those questions already. Peruse has the
file open, and it has an engine that can measure it. The work is therefore
small, and the result saves the user a page of typing.

The feature does not change the shape of the program. It reads, it writes to
the standard output, and it never starts the terminal.

## The two parts

The module divides in the usual way: one part measures, and one part decides.

| Part | Where | What it does |
|---|---|---|
| The profile | `Engine::profile` | Measures the file with SQL. |
| The rules | `ddl::render` | Changes the numbers into text. |

The second part is pure. It takes numbers and gives text, so each rule has a
test and no test needs a file.

## The profile

`Engine::profile` makes two queries at the most.

**One query measures each column.** It holds four aggregates for each column,
so DuckDB reads the file one time:

```sql
SELECT count(c1), approx_count_distinct(c1), max(length(CAST(c1 AS VARCHAR))),
       count(c2), approx_count_distinct(c2), NULL,
       …
FROM src
```

The count of the different values is close, and not exact.
`approx_count_distinct` reads the file one time and uses little memory. An
exact count of each column of a large file needs much more of both, and the
number only guides a choice.

The length has a meaning for a column of text. For each other column the
statement writes `NULL`, so a cast of a large column of numbers costs nothing.

**One query looks for a key of two columns.** The engine makes it only when no
single column is unique. See below.

## The search for a key

The search has three steps.

**Step 1: one column.** A column can be a key when it holds no NULL, when its
type is not bytes and not a nested value, and when it is not a measure. The
engine tries the candidates in this order:

1. the columns whose name looks like a key: `id`, `*_id`, `*_key`, `*_code`,
   `*_no`, `*_uuid`, `*_guid`
2. then the columns from the left

The count of the values cannot put one candidate in front of another, because
each true key holds one different value for each row. The name and the position
can.

The engine then counts the candidate exactly. It never writes a key that it did
not prove.

**Step 2: two columns.** A pair is unique only when its two columns hold many
values between them, so the six columns with the most values are the
candidates. One query measures each pair at the same time:

```sql
SELECT approx_count_distinct(concat_ws(chr(31), CAST(a AS VARCHAR), CAST(b AS VARCHAR))),
       …
FROM src
```

The character 31 joins the two values. Without it, the pair `("ab", "c")` and
the pair `("a", "bc")` would look the same. The engine then counts the best
pair exactly.

**Step 3: no key.** The statement then says that the file holds no unique
column and no unique pair, and it tells the reader to add a key.

A key of three columns is rare, and the search for one costs much more. The
search stops at two.

## The rules for a type

The function `ddl::map_type` reads the DuckDB type and gives the type of the
other database. Three rules need a note.

**A decimal keeps its precision and its scale.** `DECIMAL(18,3)` becomes
`NUMBER(18,3)` in Oracle and `numeric(18,3)` in PostgreSQL. A default type
would lose the digits after the point.

**A timestamp keeps its time zone.** A timestamp with a time zone and a
timestamp without one are different types in every database, and the difference
changes the values. `TIMESTAMP_NTZ` and `TIMESTAMP_TZ` in Snowflake, and
`timestamp` and `timestamptz` in PostgreSQL.

**A text column gets a size that fits the data.** The function
`ddl::text_size` measures the longest value, adds a quarter, and rounds up to a
usual number. A value that grows a little therefore needs no change to the
table. A column above the limit of the database gets the large type: `CLOB` in
Oracle, `TEXT` in MySQL, `NVARCHAR(MAX)` in SQL Server.

## The rules for an index

The function `ddl::index_candidates` gives at most five columns, with a reason
for each one.

| Reason | The rule |
|---|---|
| `Time` | The column holds a date or a time. |
| `Reference` | The name ends with `_id`, `_key`, `_code`, `_fk`, `_no` or `_ref`. |
| `Selective` | The file holds 1000 rows or more, the column holds 8 different values or more, one value selects a twentieth of the table or less, and the type is not a measure. |

Four rules keep the list short and useful:

- A column of the key gets no index. The key already has one.
- A column with fewer than two values gets no index. Such an index selects each
  row, or none of them.
- A measure gets no index. A price, a delay or a distance is a quantity. A
  query asks for a range of it, or it adds the values up. An index by equality
  rarely helps.
- The list stops at five. A list of twelve indexes is noise, and not advice. A
  short list makes the reader judge each entry.

BigQuery and Snowflake organize the data themselves and have no index that a
user makes. For those two, the statement names the columns and says why they
are of interest, but it writes no `CREATE INDEX`.

## The limits, and how the statement says them

The generator reads the data. The data does not hold the meaning of the data,
and the statement says so in its own text:

- The first four lines tell the reader to read the statement first.
- Below 1000 rows, the note about the key says that a column can be unique by
  accident.
- The line for each column holds the numbers that gave the choice: the number
  of different values, the percentage of NULL, and the longest value.

The reader can therefore judge each choice without a second look at the file.

## DynamoDB

DynamoDB takes no SQL statement for a new table. The function
`ddl::render_dynamodb` writes the JSON request for the AWS command
`aws dynamodb create-table` instead.

- The partition key is the key that the search found, or the column with the
  most different values. It spreads the rows over the machines, so it needs
  many values.
- The sort key is the second column of the key, or the first column that holds
  a time.
- Only the key attributes get a declaration. A table with no fixed schema holds
  each other attribute with no declaration.

The request then names the columns that would need a global secondary index,
because DynamoDB reads by key only.

## The view

The profile uses `View::scan_from`, so it measures the same rows that the grid
shows. The options `--query` and `--filter` therefore change the statement, and
a user can build a table for the result of a statement.
