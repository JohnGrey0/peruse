//! Builds a `CREATE TABLE` statement for another database from a data file.
//!
//! A frequent job with a data file is to put it in a warehouse. That job needs
//! a table, and a table needs a type for each column, a rule about NULL, a
//! size for each text column, a primary key and some indexes. A person writes
//! those by hand, and the file holds the answer to each of them already.
//!
//! This module does two things:
//!
//! * It changes a profile of the file into a statement for one dialect.
//! * It suggests a primary key and some indexes from the numbers in the
//!   profile.
//!
//! The engine measures the profile. This module is pure: it takes numbers and
//! gives text. Each rule is therefore easy to test and easy to read.
//!
//! **The result is a start, and not an answer.** The module writes what the
//! data shows. It cannot know that a column holds a foreign key, that a value
//! that is unique today stays unique tomorrow, or that your warehouse has a
//! rule about names. Read the statement before you run it.

use crate::model::CellKind;
use std::fmt;

/// The database that the statement is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialect {
    /// Oracle Database.
    Oracle,
    /// MySQL, and also MariaDB.
    MySql,
    /// PostgreSQL.
    Postgres,
    /// Snowflake.
    Snowflake,
    /// Google BigQuery.
    BigQuery,
    /// Microsoft SQL Server.
    SqlServer,
    /// DuckDB. This dialect is useful to test the result.
    DuckDb,
    /// Amazon DynamoDB. This one is not SQL. See [`Dialect::is_sql`].
    DynamoDb,
}

impl Dialect {
    /// The name of each dialect, for the command line and for a message.
    pub const ALL: &'static [(&'static str, Dialect)] = &[
        ("oracle", Dialect::Oracle),
        ("mysql", Dialect::MySql),
        ("mariadb", Dialect::MySql),
        ("postgres", Dialect::Postgres),
        ("postgresql", Dialect::Postgres),
        ("snowflake", Dialect::Snowflake),
        ("bigquery", Dialect::BigQuery),
        ("sqlserver", Dialect::SqlServer),
        ("mssql", Dialect::SqlServer),
        ("duckdb", Dialect::DuckDb),
        ("dynamodb", Dialect::DynamoDb),
    ];

    /// Reads a dialect from its name. The case of the letters does not matter.
    pub fn parse(s: &str) -> Option<Dialect> {
        let want = s.trim().to_ascii_lowercase();
        Dialect::ALL
            .iter()
            .find(|(n, _)| *n == want)
            .map(|(_, d)| *d)
    }

    /// Gives the names that [`Dialect::parse`] accepts, for a message.
    pub fn names() -> String {
        let mut v: Vec<&str> = Dialect::ALL.iter().map(|(n, _)| *n).collect();
        v.dedup();
        v.join(", ")
    }

    /// Gives `false` for a database with no `CREATE TABLE` statement.
    pub fn is_sql(self) -> bool {
        self != Dialect::DynamoDb
    }

    /// Gives `true` when the dialect has no index that a user makes.
    ///
    /// BigQuery and Snowflake organize the data themselves. An index in the
    /// sense of the other databases does not exist there.
    fn has_indexes(self) -> bool {
        !matches!(self, Dialect::BigQuery | Dialect::Snowflake)
    }

    /// Puts a name in the form that the dialect needs.
    pub fn quote(self, name: &str) -> String {
        match self {
            Dialect::MySql | Dialect::BigQuery => format!("`{}`", name.replace('`', "``")),
            Dialect::SqlServer => format!("[{}]", name.replace(']', "]]")),
            _ => format!("\"{}\"", name.replace('"', "\"\"")),
        }
    }

    /// The line that starts a comment.
    fn comment(self) -> &'static str {
        "--"
    }
}

impl fmt::Display for Dialect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Dialect::Oracle => "oracle",
            Dialect::MySql => "mysql",
            Dialect::Postgres => "postgres",
            Dialect::Snowflake => "snowflake",
            Dialect::BigQuery => "bigquery",
            Dialect::SqlServer => "sqlserver",
            Dialect::DuckDb => "duckdb",
            Dialect::DynamoDb => "dynamodb",
        })
    }
}

/// What the engine measured about one column.
#[derive(Clone, Debug)]
pub struct ColumnProfile {
    /// The name of the column, as the file holds it.
    pub name: String,
    /// The type of the column, as DuckDB writes it.
    pub sql_type: String,
    /// The family of values.
    pub kind: CellKind,
    /// The number of rows that hold no value.
    pub nulls: u64,
    /// The number of different values. The number is close, and not exact,
    /// because an exact count of a large file is slow.
    pub distinct: u64,
    /// The largest number of characters, for a column of text.
    pub max_len: Option<u64>,
}

impl ColumnProfile {
    /// Gives `true` when each row holds a value.
    pub fn is_not_null(&self) -> bool {
        self.nulls == 0
    }
}

/// What the engine measured about the file.
#[derive(Clone, Debug)]
pub struct TableProfile {
    /// The name for the table in the statement.
    pub table: String,
    /// The number of rows in the file.
    pub rows: u64,
    /// One entry for each column, in the order of the file.
    pub columns: Vec<ColumnProfile>,
    /// The positions of the columns of the primary key. The list is empty
    /// when no group of columns is unique.
    pub key: Vec<usize>,
    /// `true` when the engine counted the key exactly. `false` shows that the
    /// key comes from a count that is only close.
    pub key_is_exact: bool,
}

/// The reason that a column is worth an index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexReason {
    /// The column holds a date or a time. A query on such a column almost
    /// always asks for a period.
    Time,
    /// The name of the column ends with `_id` or a similar word, so the
    /// column probably points to another table.
    Reference,
    /// The column has few different values against many rows. A query that
    /// selects one of those values reads a small part of the table.
    Selective,
}

impl IndexReason {
    /// The text that the comment in front of the index holds.
    pub fn text(self) -> &'static str {
        match self {
            IndexReason::Time => "holds a time, so a query asks for a period",
            IndexReason::Reference => "the name shows a reference to another table",
            IndexReason::Selective => "few values against many rows",
        }
    }
}

/// The largest number of indexes that the module suggests.
///
/// Each index costs space, and it makes each write slower. A list of twelve
/// indexes is not advice, it is noise. A short list makes the reader judge
/// each entry.
const MAX_INDEXES: usize = 5;

/// The number of rows below which uniqueness proves very little.
///
/// In a file of ten rows, a column of ten different values is unique by
/// accident as often as by design.
pub const WEAK_KEY_ROWS: u64 = 1000;

/// Gives `true` when the name looks like the name of a key.
///
/// A key that a person made almost always says so in its name. The search for
/// a key tries such a column first, because a column of measures can be unique
/// by accident and is still the wrong key.
pub fn is_key_name(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    l == "id"
        || l == "key"
        || ["_id", "_key", "_code", "_no", "_uuid", "_guid"]
            .iter()
            .any(|s| l.ends_with(s))
}

/// Gives `true` when the type holds a measure, and not a category.
///
/// A value with a fraction is almost always a quantity: a price, a delay, a
/// distance. A query asks for a range of such a value, or it adds the values
/// up. An index by equality rarely helps, so the module does not suggest one.
pub fn is_measure(sql_type: &str) -> bool {
    let t = sql_type.trim().to_ascii_uppercase();
    let base = t.split('(').next().unwrap_or(&t).trim();
    matches!(base, "FLOAT" | "REAL" | "DOUBLE" | "DECIMAL" | "NUMERIC")
}

/// Gives the columns that are worth an index, with the reason for each one.
///
/// The rules are the three in [`IndexReason`]. The list is in order of
/// confidence, and it stops at [`MAX_INDEXES`].
///
/// These are candidates, and not answers. The module reads the shape of the
/// data. It cannot know which queries you will write, and the queries are what
/// decides an index.
pub fn index_candidates(p: &TableProfile) -> Vec<(usize, IndexReason)> {
    let mut out: Vec<(usize, IndexReason)> = Vec::new();
    for (i, c) in p.columns.iter().enumerate() {
        // A column of the key already has an index. A column of bytes or of
        // a nested value cannot have a useful one.
        if p.key.contains(&i) || matches!(c.kind, CellKind::Binary | CellKind::Nested) {
            continue;
        }
        let lower = c.name.to_ascii_lowercase();
        let reason = if c.kind == CellKind::Temporal {
            IndexReason::Time
        } else if ["_id", "_key", "_code", "_fk", "_no", "_ref"]
            .iter()
            .any(|s| lower.ends_with(s))
            || lower == "id"
        {
            IndexReason::Reference
        } else if p.rows >= 1000
            // Below about eight values, the database reads the table and
            // ignores the index.
            && c.distinct >= 8
            // One value must select a small part of the table.
            && c.distinct * 20 <= p.rows
            && !is_measure(&c.sql_type)
        {
            IndexReason::Selective
        } else {
            continue;
        };
        // One value in the whole column selects each row, or none of them.
        if c.distinct < 2 {
            continue;
        }
        out.push((i, reason));
    }

    // A time and a reference come first. They are the two that a query almost
    // always uses. The others follow, and the most selective comes first.
    out.sort_by_key(|(i, r)| {
        let rank = match r {
            IndexReason::Time => 0,
            IndexReason::Reference => 1,
            IndexReason::Selective => 2,
        };
        (rank, std::cmp::Reverse(p.columns[*i].distinct))
    });
    out.truncate(MAX_INDEXES);
    out
}

/// Rounds a length up, so that a small change to the data does not need a
/// change to the table.
///
/// A column that holds 37 characters today can hold 40 tomorrow. The function
/// therefore gives room, and it gives a usual number and not an odd one.
fn text_size(max_len: u64) -> u64 {
    const STEPS: [u64; 10] = [16, 32, 64, 128, 256, 512, 1024, 2048, 4000, 8000];
    let want = max_len.saturating_add(max_len / 4).max(1);
    for s in STEPS {
        if want <= s {
            return s;
        }
    }
    want.next_power_of_two()
}

/// Reads the precision and the scale from a type such as `DECIMAL(18,3)`.
fn decimal_parts(sql_type: &str) -> Option<(u32, u32)> {
    let open = sql_type.find('(')?;
    let close = sql_type.find(')')?;
    let inner = sql_type.get(open + 1..close)?;
    let (p, s) = inner.split_once(',')?;
    Some((p.trim().parse().ok()?, s.trim().parse().ok()?))
}

/// Gives the type of a column in the other database.
///
/// The function reads the DuckDB type and the measured length. A text column
/// therefore gets a size that fits the data, and not the largest size that the
/// database allows.
pub fn map_type(d: Dialect, c: &ColumnProfile) -> String {
    let t = c.sql_type.trim().to_ascii_uppercase();
    let base = t.split('(').next().unwrap_or(&t).trim().to_string();

    // A nested value has no type in most databases. The engine reads such a
    // value as text, so the table takes the text of the value.
    if c.kind == CellKind::Nested {
        return match d {
            Dialect::Oracle => "CLOB".into(),
            Dialect::MySql => "JSON".into(),
            Dialect::Postgres => "jsonb".into(),
            Dialect::Snowflake => "VARIANT".into(),
            Dialect::BigQuery => "JSON".into(),
            Dialect::SqlServer => "NVARCHAR(MAX)".into(),
            Dialect::DuckDb => c.sql_type.clone(),
            Dialect::DynamoDb => "S".into(),
        };
    }

    if let Some((p, s)) = decimal_parts(&t)
        && (base == "DECIMAL" || base == "NUMERIC")
    {
        return match d {
            Dialect::Oracle | Dialect::Snowflake => format!("NUMBER({p},{s})"),
            Dialect::MySql | Dialect::SqlServer => format!("DECIMAL({p},{s})"),
            Dialect::Postgres => format!("numeric({p},{s})"),
            Dialect::BigQuery if p <= 38 => "NUMERIC".into(),
            Dialect::BigQuery => "BIGNUMERIC".into(),
            Dialect::DuckDb => c.sql_type.clone(),
            Dialect::DynamoDb => "N".into(),
        };
    }

    // DynamoDB has three types for a value: a number, a text and bytes.
    if d == Dialect::DynamoDb {
        return match c.kind {
            CellKind::Number => "N",
            CellKind::Binary => "B",
            CellKind::Bool => "BOOL",
            _ => "S",
        }
        .into();
    }

    let text = |n: u64| -> String {
        let size = text_size(n);
        match d {
            // A VARCHAR2 in Oracle holds 4000 bytes at the most, unless the
            // database uses extended strings. A longer column becomes a CLOB.
            Dialect::Oracle if size > 4000 => "CLOB".into(),
            Dialect::Oracle => format!("VARCHAR2({size} CHAR)"),
            Dialect::MySql if size > 8000 => "TEXT".into(),
            Dialect::MySql => format!("VARCHAR({size})"),
            Dialect::Postgres => format!("varchar({size})"),
            Dialect::Snowflake => format!("VARCHAR({size})"),
            Dialect::BigQuery => "STRING".into(),
            Dialect::SqlServer if size > 4000 => "NVARCHAR(MAX)".into(),
            Dialect::SqlServer => format!("NVARCHAR({size})"),
            Dialect::DuckDb => "VARCHAR".into(),
            Dialect::DynamoDb => "S".into(),
        }
    };

    let pick = |oracle: &str, mysql: &str, pg: &str, snow: &str, bq: &str, ss: &str| -> String {
        match d {
            Dialect::Oracle => oracle.into(),
            Dialect::MySql => mysql.into(),
            Dialect::Postgres => pg.into(),
            Dialect::Snowflake => snow.into(),
            Dialect::BigQuery => bq.into(),
            Dialect::SqlServer => ss.into(),
            Dialect::DuckDb => c.sql_type.clone(),
            Dialect::DynamoDb => "S".into(),
        }
    };

    match base.as_str() {
        "BOOLEAN" | "BOOL" => pick("NUMBER(1)", "TINYINT(1)", "boolean", "BOOLEAN", "BOOL", "BIT"),
        "TINYINT" | "UTINYINT" => pick(
            "NUMBER(3)", "TINYINT", "smallint", "NUMBER(3,0)", "INT64", "TINYINT",
        ),
        "SMALLINT" | "USMALLINT" => pick(
            "NUMBER(5)", "SMALLINT", "smallint", "NUMBER(5,0)", "INT64", "SMALLINT",
        ),
        "INTEGER" | "INT" | "UINTEGER" => {
            pick("NUMBER(10)", "INT", "integer", "NUMBER(10,0)", "INT64", "INT")
        }
        "BIGINT" | "UBIGINT" => pick(
            "NUMBER(19)", "BIGINT", "bigint", "NUMBER(19,0)", "INT64", "BIGINT",
        ),
        "HUGEINT" | "UHUGEINT" => pick(
            "NUMBER(38)",
            "DECIMAL(38,0)",
            "numeric(38,0)",
            "NUMBER(38,0)",
            "BIGNUMERIC",
            "DECIMAL(38,0)",
        ),
        "FLOAT" | "REAL" => pick(
            "BINARY_FLOAT", "FLOAT", "real", "FLOAT", "FLOAT64", "REAL",
        ),
        "DOUBLE" => pick(
            "BINARY_DOUBLE",
            "DOUBLE",
            "double precision",
            "FLOAT",
            "FLOAT64",
            "FLOAT",
        ),
        "DATE" => pick("DATE", "DATE", "date", "DATE", "DATE", "DATE"),
        "TIME" => pick(
            "INTERVAL DAY(0) TO SECOND(6)",
            "TIME(6)",
            "time",
            "TIME",
            "TIME",
            "TIME",
        ),
        "INTERVAL" => pick(
            "INTERVAL DAY(9) TO SECOND(6)",
            "BIGINT",
            "interval",
            "VARCHAR(64)",
            "INTERVAL",
            "BIGINT",
        ),
        "UUID" => pick(
            "RAW(16)",
            "CHAR(36)",
            "uuid",
            "VARCHAR(36)",
            "STRING",
            "UNIQUEIDENTIFIER",
        ),
        "BLOB" | "BYTEA" | "BINARY" | "VARBINARY" | "BIT" => pick(
            "BLOB",
            "LONGBLOB",
            "bytea",
            "BINARY",
            "BYTES",
            "VARBINARY(MAX)",
        ),
        _ if base.starts_with("TIMESTAMP") => {
            // A timestamp with a time zone is a different type in each
            // database, and the difference matters.
            if t.contains("TIME ZONE") || base.ends_with("TZ") {
                pick(
                    "TIMESTAMP(6) WITH TIME ZONE",
                    "TIMESTAMP(6)",
                    "timestamptz",
                    "TIMESTAMP_TZ",
                    "TIMESTAMP",
                    "DATETIMEOFFSET(6)",
                )
            } else {
                pick(
                    "TIMESTAMP(6)",
                    "DATETIME(6)",
                    "timestamp",
                    "TIMESTAMP_NTZ",
                    "DATETIME",
                    "DATETIME2(6)",
                )
            }
        }
        // VARCHAR, and each type that the engine reads as text.
        _ => text(c.max_len.unwrap_or(64)),
    }
}

/// Writes the complete statement for a dialect.
pub fn render(p: &TableProfile, d: Dialect) -> String {
    if d == Dialect::DynamoDb {
        return render_dynamodb(p);
    }

    let mut out = String::new();
    let c = d.comment();
    out.push_str(&format!("{c} Generated by peruse from {} rows.\n", thousands(p.rows)));
    out.push_str(&format!(
        "{c} The types, the NULL rules and the sizes come from the data itself.\n"
    ));
    out.push_str(&format!("{c} Read this before you run it: the data of today cannot promise\n"));
    out.push_str(&format!("{c} the data of tomorrow.\n\n"));

    out.push_str(&format!("CREATE TABLE {} (\n", d.quote(&p.table)));

    // Put the types under each other. A person reads the result, so the
    // columns must line up.
    let name_w = p
        .columns
        .iter()
        .map(|c| d.quote(&c.name).chars().count())
        .max()
        .unwrap_or(0);
    let types: Vec<String> = p.columns.iter().map(|c| map_type(d, c)).collect();
    let type_w = types.iter().map(|t| t.chars().count()).max().unwrap_or(0);

    let mut lines: Vec<String> = Vec::new();
    for (i, col) in p.columns.iter().enumerate() {
        let name = d.quote(&col.name);
        // BigQuery writes NOT NULL, but it has no primary key that it
        // enforces. The other databases take both.
        let null = if col.is_not_null() { "NOT NULL" } else { "" };
        let mut line = format!(
            "  {:<name_w$}  {:<type_w$}  {}",
            name,
            types[i],
            null,
            name_w = name_w,
            type_w = type_w
        );
        while line.ends_with(' ') {
            line.pop();
        }
        // Say what the data showed, so the reader can judge the choice.
        // A file with no row gives no percentage.
        let pct = (col.nulls * 100).checked_div(p.rows).unwrap_or(0);
        let mut notes = vec![format!("{} distinct", thousands(col.distinct))];
        if col.nulls > 0 {
            notes.push(format!("{pct}% null"));
        }
        if let Some(n) = col.max_len {
            notes.push(format!("longest {n}"));
        }
        line.push_str(&format!("  {c} {}", notes.join(", ")));
        lines.push(line);
    }

    // The primary key goes at the end, as a constraint on the table. A
    // composite key needs that form, and one form for the two cases is
    // easier to read.
    let key_line = (!p.key.is_empty() && d != Dialect::BigQuery).then(|| {
        let cols: Vec<String> = p
            .key
            .iter()
            .map(|i| d.quote(&p.columns[*i].name))
            .collect();
        format!(
            "  CONSTRAINT {} PRIMARY KEY ({})",
            d.quote(&format!("pk_{}", p.table)),
            cols.join(", ")
        )
    });

    let n = lines.len();
    for (i, line) in lines.iter().enumerate() {
        let last = i + 1 == n && key_line.is_none();
        // The comma goes in front of the comment, and not after it.
        match line.split_once(&format!("  {c} ")) {
            Some((code, note)) if !last => out.push_str(&format!("{code},  {c} {note}\n")),
            _ => out.push_str(&format!("{line}\n")),
        }
    }
    if let Some(k) = key_line {
        out.push_str(&format!("{k}\n"));
    }
    out.push_str(");\n");

    // Say why there is a key, or why there is none.
    out.push('\n');
    if p.key.is_empty() {
        out.push_str(&format!(
            "{c} No column and no pair of columns is unique, so this table has no\n\
             {c} primary key. Add one of your own, or add a generated key.\n"
        ));
    } else {
        let names: Vec<&str> = p.key.iter().map(|i| p.columns[*i].name.as_str()).collect();
        let how = if p.key_is_exact {
            "an exact count"
        } else {
            "a count that is close, so test it"
        };
        out.push_str(&format!(
            "{c} The key ({}) is unique over the {} rows of the file, by {how}.\n",
            names.join(", "),
            thousands(p.rows)
        ));
        // Uniqueness in a small file proves very little. Say so, instead of
        // letting the reader believe a number that cannot carry that weight.
        if p.rows < WEAK_KEY_ROWS {
            out.push_str(&format!(
                "{c} The file holds few rows, so this column can be unique by accident.\n\
                 {c} Check it against the meaning of the data before you trust it.\n"
            ));
        }
    }

    let idx = index_candidates(p);
    if !idx.is_empty() && d.has_indexes() {
        out.push_str(&format!(
            "\n{c} Index candidates, from the shape of the data. Your queries decide\n\
             {c} which of these earn their cost. Each index makes a write slower.\n"
        ));
        for (i, reason) in idx {
            let col = &p.columns[i];
            out.push_str(&format!(
                "CREATE INDEX {} ON {} ({});  {c} {}\n",
                d.quote(&format!("ix_{}_{}", p.table, col.name)),
                d.quote(&p.table),
                d.quote(&col.name),
                reason.text()
            ));
        }
    } else if !idx.is_empty() {
        out.push_str(&format!(
            "\n{c} {d} organizes the data itself, so it has no index to create.\n\
             {c} These columns are still the ones that a query will select on:\n"
        ));
        for (i, reason) in idx {
            out.push_str(&format!(
                "{c}   {} — {}\n",
                p.columns[i].name,
                reason.text()
            ));
        }
    }
    out
}

/// Writes the request that makes a DynamoDB table.
///
/// DynamoDB has no `CREATE TABLE` statement. It takes a request in JSON, and
/// that request names the key attributes only. Each other attribute needs no
/// declaration, because the table has no fixed schema.
fn render_dynamodb(p: &TableProfile) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "// Generated by peruse from {} rows.\n\
         // DynamoDB takes no SQL statement for a new table. It takes this\n\
         // request, for the AWS command:\n\
         //   aws dynamodb create-table --cli-input-json file://this.json\n\
         // Only the key attributes need a declaration. A table with no fixed\n\
         // schema holds each other attribute with no declaration.\n",
        thousands(p.rows)
    ));

    // The partition key spreads the rows over the machines, so it must have
    // many different values. A sort key then orders the rows inside one
    // partition, and a time is the usual choice.
    let partition = p
        .key
        .first()
        .copied()
        .or_else(|| {
            (0..p.columns.len()).max_by_key(|i| p.columns[*i].distinct * u64::from(p.columns[*i].is_not_null()))
        });
    let sort = p
        .key
        .get(1)
        .copied()
        .or_else(|| {
            (0..p.columns.len()).find(|i| {
                Some(*i) != partition && p.columns[*i].kind == CellKind::Temporal
            })
        });

    let Some(pk) = partition else {
        out.push_str("// The file holds no column, so there is no key.\n");
        return out;
    };

    let attr = |i: usize| -> String {
        let c = &p.columns[i];
        format!(
            "    {{ \"AttributeName\": \"{}\", \"AttributeType\": \"{}\" }}",
            c.name,
            match c.kind {
                CellKind::Number => "N",
                CellKind::Binary => "B",
                _ => "S",
            }
        )
    };

    let mut attrs = vec![attr(pk)];
    let mut keys = vec![format!(
        "    {{ \"AttributeName\": \"{}\", \"KeyType\": \"HASH\" }}",
        p.columns[pk].name
    )];
    if let Some(s) = sort {
        attrs.push(attr(s));
        keys.push(format!(
            "    {{ \"AttributeName\": \"{}\", \"KeyType\": \"RANGE\" }}",
            p.columns[s].name
        ));
    }

    out.push_str(&format!(
        "{{\n  \"TableName\": \"{}\",\n  \"AttributeDefinitions\": [\n{}\n  ],\n  \
         \"KeySchema\": [\n{}\n  ],\n  \"BillingMode\": \"PAY_PER_REQUEST\"\n}}\n",
        p.table,
        attrs.join(",\n"),
        keys.join(",\n")
    ));

    out.push_str(&format!(
        "\n// The partition key is {}. It has {} different values over {} rows,\n\
         // so it spreads the rows well.\n",
        p.columns[pk].name,
        thousands(p.columns[pk].distinct),
        thousands(p.rows)
    ));
    if let Some(s) = sort {
        out.push_str(&format!(
            "// The sort key is {}. It orders the rows inside one partition.\n",
            p.columns[s].name
        ));
    } else {
        out.push_str("// No column fits a sort key. Add one if you query a range.\n");
    }

    // A column that is already a key needs no secondary index.
    let idx: Vec<(usize, IndexReason)> = index_candidates(p)
        .into_iter()
        .filter(|(i, _)| Some(*i) != partition && Some(*i) != sort)
        .collect();
    if !idx.is_empty() {
        out.push_str(
            "// DynamoDB reads by key only. A query on one of these therefore\n\
             // needs a global secondary index:\n",
        );
        for (i, reason) in idx {
            out.push_str(&format!(
                "//   {} — {}\n",
                p.columns[i].name,
                reason.text()
            ));
        }
    }
    out
}

/// Writes a number with a comma after each group of three digits.
fn thousands(n: u64) -> String {
    crate::source::human_count(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, ty: &str, nulls: u64, distinct: u64, max_len: Option<u64>) -> ColumnProfile {
        ColumnProfile {
            name: name.into(),
            sql_type: ty.into(),
            kind: CellKind::from_sql_type(ty),
            nulls,
            distinct,
            max_len,
        }
    }

    fn profile() -> TableProfile {
        TableProfile {
            table: "trips".into(),
            rows: 1000,
            columns: vec![
                col("id", "BIGINT", 0, 1000, None),
                col("vendor", "VARCHAR", 0, 3, Some(4)),
                col("pickup_at", "TIMESTAMP", 0, 900, None),
                col("fare", "DECIMAL(10,2)", 12, 400, None),
                col("notes", "VARCHAR", 800, 150, Some(300)),
            ],
            key: vec![0],
            key_is_exact: true,
        }
    }

    #[test]
    fn each_dialect_name_reads_back() {
        for (name, want) in Dialect::ALL {
            assert_eq!(Dialect::parse(name), Some(*want), "name {name}");
            assert_eq!(Dialect::parse(&name.to_uppercase()), Some(*want));
        }
        assert_eq!(Dialect::parse("nope"), None);
    }

    #[test]
    fn a_text_column_gets_a_size_that_fits_the_data() {
        // The size gives room, so a value that grows a little needs no change
        // to the table.
        assert_eq!(text_size(1), 16);
        assert_eq!(text_size(30), 64);
        assert_eq!(text_size(100), 128);
        assert!(text_size(300) >= 300);
        // The size never falls below the data.
        for n in [1u64, 7, 63, 64, 65, 999, 3999, 5000] {
            assert!(text_size(n) >= n, "size of {n} is too small");
        }
    }

    #[test]
    fn types_follow_the_dialect() {
        let c = col("v", "VARCHAR", 0, 5, Some(30));
        assert_eq!(map_type(Dialect::Oracle, &c), "VARCHAR2(64 CHAR)");
        assert_eq!(map_type(Dialect::MySql, &c), "VARCHAR(64)");
        assert_eq!(map_type(Dialect::Postgres, &c), "varchar(64)");
        assert_eq!(map_type(Dialect::BigQuery, &c), "STRING");
        assert_eq!(map_type(Dialect::SqlServer, &c), "NVARCHAR(64)");

        let n = col("n", "BIGINT", 0, 5, None);
        assert_eq!(map_type(Dialect::Oracle, &n), "NUMBER(19)");
        assert_eq!(map_type(Dialect::BigQuery, &n), "INT64");
        assert_eq!(map_type(Dialect::Snowflake, &n), "NUMBER(19,0)");
    }

    #[test]
    fn a_decimal_keeps_its_precision_and_its_scale() {
        let c = col("d", "DECIMAL(18,3)", 0, 5, None);
        assert_eq!(map_type(Dialect::Oracle, &c), "NUMBER(18,3)");
        assert_eq!(map_type(Dialect::MySql, &c), "DECIMAL(18,3)");
        assert_eq!(map_type(Dialect::Postgres, &c), "numeric(18,3)");
    }

    #[test]
    fn a_timestamp_keeps_its_time_zone() {
        let plain = col("t", "TIMESTAMP", 0, 5, None);
        let zoned = col("t", "TIMESTAMP WITH TIME ZONE", 0, 5, None);
        assert_eq!(map_type(Dialect::Postgres, &plain), "timestamp");
        assert_eq!(map_type(Dialect::Postgres, &zoned), "timestamptz");
        assert_eq!(map_type(Dialect::Snowflake, &plain), "TIMESTAMP_NTZ");
        assert_eq!(map_type(Dialect::Snowflake, &zoned), "TIMESTAMP_TZ");
    }

    #[test]
    fn a_very_long_text_becomes_a_large_type() {
        let c = col("body", "VARCHAR", 0, 5, Some(9000));
        assert_eq!(map_type(Dialect::Oracle, &c), "CLOB");
        assert_eq!(map_type(Dialect::MySql, &c), "TEXT");
        assert_eq!(map_type(Dialect::SqlServer, &c), "NVARCHAR(MAX)");
    }

    #[test]
    fn a_column_with_no_null_gets_not_null() {
        let sql = render(&profile(), Dialect::Postgres);
        let id_line = sql.lines().find(|l| l.contains("\"id\"")).unwrap();
        assert!(id_line.contains("NOT NULL"), "{id_line}");
        let notes_line = sql.lines().find(|l| l.contains("\"notes\"")).unwrap();
        assert!(!notes_line.contains("NOT NULL"), "{notes_line}");
        assert!(notes_line.contains("80% null"), "{notes_line}");
    }

    #[test]
    fn the_key_becomes_a_constraint() {
        let sql = render(&profile(), Dialect::Oracle);
        assert!(
            sql.contains("CONSTRAINT \"pk_trips\" PRIMARY KEY (\"id\")"),
            "{sql}"
        );
        assert!(sql.contains("unique over the 1,000 rows"), "{sql}");
    }

    #[test]
    fn a_composite_key_names_each_of_its_columns() {
        let mut p = profile();
        p.key = vec![1, 2];
        let sql = render(&p, Dialect::Postgres);
        assert!(
            sql.contains("PRIMARY KEY (\"vendor\", \"pickup_at\")"),
            "{sql}"
        );
    }

    #[test]
    fn a_table_with_no_unique_column_says_so() {
        let mut p = profile();
        p.key.clear();
        let sql = render(&p, Dialect::Postgres);
        assert!(!sql.contains("PRIMARY KEY"), "{sql}");
        assert!(sql.contains("no\n-- primary key"), "{sql}");
    }

    #[test]
    fn the_indexes_follow_the_shape_of_the_data() {
        let mut p = profile();
        // `vendor` has 3 values, which is below the floor. Give it enough
        // values to be worth an index.
        p.columns[1].distinct = 40;
        let idx = index_candidates(&p);
        let by_name: Vec<(&str, IndexReason)> = idx
            .iter()
            .map(|(i, r)| (p.columns[*i].name.as_str(), *r))
            .collect();
        // A time comes first, and it is always a candidate.
        assert_eq!(by_name.first(), Some(&("pickup_at", IndexReason::Time)));
        assert!(by_name.contains(&("vendor", IndexReason::Selective)), "{by_name:?}");
        // The key already has an index.
        assert!(!by_name.iter().any(|(n, _)| *n == "id"), "{by_name:?}");
        // `fare` is a decimal, so it is a quantity and not a category.
        assert!(!by_name.iter().any(|(n, _)| *n == "fare"), "{by_name:?}");
    }

    #[test]
    fn a_measure_is_not_worth_an_index() {
        // A price, a delay or a distance is a quantity. A query asks for a
        // range of it or adds it up, and an index by equality does not help.
        for ty in ["DOUBLE", "FLOAT", "REAL", "DECIMAL(10,2)"] {
            assert!(is_measure(ty), "{ty} must count as a measure");
        }
        for ty in ["BIGINT", "VARCHAR", "DATE", "BOOLEAN"] {
            assert!(!is_measure(ty), "{ty} must not count as a measure");
        }
    }

    #[test]
    fn a_column_with_very_few_values_is_worth_no_index() {
        let mut p = profile();
        // Below about eight values, the database reads the table instead.
        p.columns[1].distinct = 3;
        let idx = index_candidates(&p);
        assert!(!idx.iter().any(|(i, _)| p.columns[*i].name == "vendor"));
    }

    #[test]
    fn the_list_of_indexes_stays_short() {
        // A list of twelve indexes is noise, and not advice.
        let mut p = profile();
        p.key.clear();
        for n in 0..20 {
            p.columns
                .push(col(&format!("dim_{n}"), "VARCHAR", 0, 40, Some(8)));
        }
        assert!(index_candidates(&p).len() <= MAX_INDEXES);
    }

    #[test]
    fn a_name_that_ends_with_id_is_worth_an_index() {
        let mut p = profile();
        p.columns
            .push(col("customer_id", "BIGINT", 0, 999, None));
        let idx = index_candidates(&p);
        assert!(
            idx.iter()
                .any(|(i, r)| p.columns[*i].name == "customer_id" && *r == IndexReason::Reference),
            "a column named customer_id must be worth an index"
        );
    }

    #[test]
    fn a_column_with_one_value_is_worth_no_index() {
        let mut p = profile();
        p.columns.push(col("constant", "VARCHAR", 0, 1, Some(2)));
        let idx = index_candidates(&p);
        assert!(!idx.iter().any(|(i, _)| p.columns[*i].name == "constant"));
    }

    #[test]
    fn bigquery_and_snowflake_make_no_index() {
        for d in [Dialect::BigQuery, Dialect::Snowflake] {
            let sql = render(&profile(), d);
            assert!(!sql.contains("CREATE INDEX"), "{d} wrote an index:\n{sql}");
            assert!(
                sql.contains("organizes the data itself"),
                "{d} did not say why:\n{sql}"
            );
        }
        // BigQuery has no primary key that it enforces.
        assert!(!render(&profile(), Dialect::BigQuery).contains("PRIMARY KEY"));
    }

    #[test]
    fn each_dialect_quotes_a_name_in_its_own_form() {
        assert_eq!(Dialect::MySql.quote("a b"), "`a b`");
        assert_eq!(Dialect::SqlServer.quote("a b"), "[a b]");
        assert_eq!(Dialect::Postgres.quote("a b"), "\"a b\"");
        // A quotation mark in a name cannot end the name.
        assert_eq!(Dialect::Postgres.quote("we\"ird"), "\"we\"\"ird\"");
        assert_eq!(Dialect::MySql.quote("we`ird"), "`we``ird`");
    }

    #[test]
    fn dynamodb_gives_a_request_and_not_a_statement() {
        let out = render(&profile(), Dialect::DynamoDb);
        assert!(!out.contains("CREATE TABLE"), "{out}");
        assert!(out.contains("\"TableName\": \"trips\""), "{out}");
        assert!(
            out.contains("\"AttributeName\": \"id\", \"KeyType\": \"HASH\""),
            "{out}"
        );
        // The one temporal column becomes the sort key.
        assert!(
            out.contains("\"AttributeName\": \"pickup_at\", \"KeyType\": \"RANGE\""),
            "{out}"
        );
    }

    #[test]
    fn a_file_with_no_column_does_not_stop_the_program() {
        let p = TableProfile {
            table: "empty".into(),
            rows: 0,
            columns: Vec::new(),
            key: Vec::new(),
            key_is_exact: true,
        };
        for (_, d) in Dialect::ALL {
            let out = render(&p, *d);
            assert!(!out.is_empty(), "{d} gave nothing");
        }
    }

    #[test]
    fn the_statement_names_the_file_and_warns_the_reader() {
        let sql = render(&profile(), Dialect::Postgres);
        assert!(sql.contains("Generated by peruse from 1,000 rows"), "{sql}");
        assert!(sql.contains("Read this before you run it"), "{sql}");
    }
}
