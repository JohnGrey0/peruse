# The local copies of files from a network drive

Peruse copies a large file from a network drive to the local disk one time, and
it then reads the copy. DuckDB reads a Parquet file with many small reads at
different positions, and on a network drive each read waits for the network.
One long sequential read is much faster than those many small reads. The copy
stays for the next session, and Peruse makes a new copy when the source
changes. The code is in `crates/peruse-core/src/remote.rs`,
`crates/peruse-core/src/cache.rs` and `crates/peruse-tui/src/copying.rs`.

## What the user sees

1. The user opens a file on a network drive, for example
   `peruse \\server\team\sales.parquet` or `peruse A:\sales.parquet`.
2. A screen shows the copy: the source, the directory of the copy, a bar, the
   bytes, the rate and the time that is left.
3. The grid opens. The title bar shows `local copy`, and the status line says
   how long the copy took.
4. At the next open of the same file, the grid opens at once from the copy.

A copy that ends in less than 400 milliseconds shows no screen.

| Key on the copy screen | Action |
|---|---|
| `Esc`, `q` | Stop the copy, and read the file on the network instead |
| `Ctrl-C` | Stop the copy, and quit |

The metadata panel shows two more rows at the top:

| Row | Contents |
|---|---|
| `source` | The path that the user gave |
| `local copy` | The path of the copy that the engine reads |

Each other text names the source. The read expression of the panel, the list
of files, an error message and the title bar all show the path that the user
gave. The function `Rewrite::apply` changes each path of the copy back to the
path of the source.

## Which file gets a copy

The function `cache::plan` makes the decision. It reads the sizes and the times
of the files, and it copies nothing.

| Setting `local_copy` | A file on a network drive | A file on a local disk |
|---|---|---|
| `auto` (the default) | A copy, when the file is 32 MB or more | No copy |
| `always` | A copy, of any size | A copy, of any size |
| `never` | No copy | No copy |

The option `--local` gives `always` for one call, and the option `--no-local`
gives `never`. An option on the command line wins over the setting.

Peruse reads the file in place in these cases:

- The file is larger than the limit of the copies (`local_copy_max_gb`, 20 GB).
  The status line says so.
- The local disk would keep less than 1 GB free after the copy. The status line
  says so.
- The directory of the copies is on a network drive itself.
- The file is in the directory of the copies already.
- The input is not a file or a glob that Peruse can find. The engine then gives
  its own message.

The option `--ddl` uses a copy from an earlier session when one exists. It
makes no new copy: the measurement reads the whole file one time, and a copy
would read it one time more.

## How Peruse finds a network drive

The module `remote.rs` holds the one test for each system:

| System | A network drive is |
|---|---|
| Windows | A path `\\server\share\...`, a path `\\?\UNC\...`, or a drive letter that `GetDriveTypeW` reports as `DRIVE_REMOTE` |
| Linux | A file system with a network type in `/proc/self/mountinfo`, such as `nfs4`, `cifs`, `smb3`, `9p`, `fuse.sshfs` or `fuse.rclone` |
| macOS | A file system that `statfs` does not mark with `MNT_LOCAL`, or with a network name such as `smbfs` or `afpfs` |

On Windows, the complete path of a mapped drive letter starts with
`\\?\UNC\server\share`, so a link to a share counts too. On Linux, a FUSE file
system is local unless its program is a network program in the list.
The function `statfs` of Linux gives one number for each FUSE file system, so
it cannot tell `sshfs` from a FUSE program for a local disk. The file
`/proc/self/mountinfo` gives the name of the program, and Peruse reads that. The
types `9p` and `drvfs` are the drives of Windows inside WSL, and a read through
them is slow in the same way.

A failed test gives the answer "local". Peruse then reads the file in place, as
it did before this feature.

The trait `Probe` holds the two questions: "is this path on a network drive?"
and "how much space is free on this disk?". The tests give a fake probe that
treats a local directory as a network drive.

## Where the copies are

| System | The directory |
|---|---|
| Windows | `%LOCALAPPDATA%\peruse\cache\copies` |
| macOS | `~/Library/Caches/peruse/copies` |
| Each other | `$XDG_CACHE_HOME/peruse/copies`, or `~/.cache/peruse/copies` |

On Windows the copies are in `LOCALAPPDATA` and not in `APPDATA`. A company can
copy `APPDATA` to a server at each sign-in, and gigabytes of data there would
go back to the network.

One entry holds the copy of one file, or the copies of the files of one glob:

```text
copies/<key>/manifest.toml   the source, the last use, and the size and time of each file
copies/<key>/data/...        the copies, with the names of the source
copies/<key>/tmp/...         a copy that is not complete yet
copies/<key>/lease-<pid>     one for each Peruse that reads the entry now
```

The key is a hash of 64 bits (FNV-1a) of the complete path of the source. On
Windows the path is in small letters, because Windows sees no difference
between capital and small letters. The drive letter `A:` and the share behind
it give the same complete path, so they share one copy.

## When a copy is correct

The manifest holds the size and the time of the last change of each file of the
source, in nanoseconds. A copy is correct when each of these holds:

- The manifest names the same source.
- Each file of the source has a record with the same size and time.
- Each copy has the size of its record.
- The manifest names no file that the source no longer holds.

A source that changed gets a new copy of the files that changed. The files that
did not change stay, so a glob over a directory of 100 files copies one file
when one file changes.

## How a copy runs

The function `CopyJob::run` does these steps:

1. It removes old entries when the copies need the room. See
   [The limit of the copies](#the-limit-of-the-copies).
2. It refuses to start when less than 1 GB would stay free on the local disk.
3. It reads each file with reads of 8 MB, from the start to the end. On
   Windows it opens the file with `FILE_FLAG_SEQUENTIAL_SCAN`. It writes to a
   part file in `tmp`.
4. It examines the flag of the user before each read, and it stops when the
   flag is set. The part file then goes.
5. After each file, it compares the size and the time of the source with the
   values from before the copy. A file that changed during the copy gives a
   message, and Peruse reads it in place.
6. It gives the part file the time of the last change of the source, and
   then its name in `data`.
7. It removes each file in `data` that the source no longer holds.
8. It writes the manifest last, to a temporary name first.

Another Peruse can read an older copy in the same entry. On Windows, that copy
is open, and the system refuses to put the new copy in its place in step 6 or
to remove the old file in step 7. The old copy is then correct only when it has
the size and the time of the source now. A size alone is not enough, because a
source that changed can keep its size. In each other case the copy stops with
a message, the entry gets no new manifest, and Peruse reads the file in place.
The next session after the other Peruse closes makes the new copy.

A copy that is not complete never appears in `data`, and a manifest never names
a copy that is not complete. A Peruse that stops during a copy leaves a part in
`tmp`, and the next copy into that entry removes each part that is older than
10 minutes.

A read that waits for a server that went away does not end. The copy screen
waits 2 seconds after `Esc`, and then it continues without the copy. The thread
of the copy removes its part when the read returns.

Peruse never writes to the source. It opens each file of the source for reading
only, and each write goes into the directory of the copies.

## The limit of the copies

The copies take 20 GB at the most together, and the setting `local_copy_max_gb`
changes that number. Before a copy, the function `evict` removes the entries
that were used the least recently, until the new copy fits under the limit and
on the disk. Each open of a copy writes the time of the use into its manifest.

The function never removes the entry that it copies into, and never an entry
that another Peruse reads now. Each Peruse holds a lease on the entry that it
reads: a file `lease-<pid>` that stays open until the session ends. On Windows
the file is open with no permission for a delete. On Linux and macOS it holds a
shared `flock`. The test for a lease tries to remove the file, or to take the
lock for itself, so a lease of a process that stopped never protects an entry.

The command `C` (clear the local copies) removes each entry that no Peruse reads
now, and it says how much space it gave back. The command asks first: the first
`C` says how many copies there are and how large they are, and a second `C`
in 5 seconds removes them. The key `c` goes to a column, and a copy of a large
file can take minutes over the network, so one slip of the Shift key must not
remove it. The palette runs the same command, and it also asks first.

The settings page shows the number of copies, their size and their directory.
The page reads the directory one time when it opens, and not at each frame. An
entry that a stopped copy left holds no copy, and the page does not count it.

## A glob and a database

A glob gets one entry for all its files. The function `glob()` of DuckDB finds
the files, so the set agrees with the set that the engine reads. Each copy keeps
its path below the directory in front of the first wildcard, so the directories
of a Hive layout, such as `year=2024/`, stay. The engine then opens the same
glob over the directory of the copies.

A DuckDB database keeps the changes that it did not write into the file yet in
a second file, `<name>.wal`. The entry copies that file too, so the copy shows
the same rows as the source. The table picker reads the tables of the copy.

## Why a copy on the disk, and not a table in memory

Peruse can also make a file fast with a table in memory, and it does that for a
file of text already. See [engine.md](engine.md#the-index-of-a-file-of-text).
The measurements below decided the design.

**How the numbers were measured.** The share is `\\server\share`, on a
network of 10 gigabits, with a round trip of less than 1 millisecond. The
example `netcache-bench` made the files. Before each run, a copy on the server
side made a new file that this machine did not hold in its memory. The bytes
that the network adapter received show that: each run in place received the
bytes that it read. Each number is the median of three runs, unless the row
gives two numbers. A row with two numbers had two runs. The build was a debug
build, in which DuckDB itself is optimized. The local disk is an NVMe disk.

The steps are the steps that a user waits for: the first frame (the open, the
columns and 50 rows), a move to the last row, a sort on a column of text, the
statistics of the first column of numbers, and the metadata panel.

### Parquet

A file of 1.03 GB, with 20 million rows and 7 columns:

| Step | In place | First session, with the copy | A copy of an earlier session |
|---|---|---|---|
| the copy | - | 1075 ms (955 MB/s) | - |
| the first frame | 100 ms | 1146 ms | 62 and 64 ms |
| move to the last row | 123 ms | 32 ms | 31 ms |
| sort on a column of text | 1091 ms | 208 ms | 204 and 193 ms |
| statistics of a column | 228 ms | 77 ms | 96 and 81 ms |
| the metadata panel | 105 ms | 37 ms | 58 and 38 ms |
| all of the above | 1670 ms | 1519 ms | 472 and 425 ms |

The file of 64 MB in `test_data` (`large.parquet`, 19 columns):

| Step | In place | First session, with the copy | A copy of an earlier session |
|---|---|---|---|
| the copy | - | 83 ms | - |
| the first frame | 154 ms | 161 ms | 55 ms |
| move to the last row | 90 ms | 31 ms | 31 ms |
| sort on a column of text | 107 ms | 34 ms | 28 ms |
| statistics of a column | 114 ms | 79 ms | 78 ms |
| all of the above | 529 ms | 341 ms | 219 ms |

Windows keeps the pages of a file from a share in memory after a read. A second
run in place on the same file received 1 MB from the network, and it was still
slow: 218 and 230 ms to the first frame, 279 and 337 ms for the sort, and 1017
and 1136 ms for all the steps. Each small read still goes through the client of
the network file system. The copy on the local disk was 2.3 times faster than
that.

### Files of text

A CSV file of 494 MB (4 million rows) and a file of JSON lines of 255 MB (1.5
million rows). Peruse makes the index of a file of text when it opens it, up to
64 MB. A larger file gets the index when the user presses `I`.

| CSV, with the index | In place | First session, with the copy | A copy of an earlier session |
|---|---|---|---|
| the copy | - | 527 ms (938 MB/s) | - |
| the first frame | 135 ms | 623 ms | 80 and 79 ms |
| `count(*)` | 696 ms | 168 ms | 167 and 138 ms |
| the index | 846 ms | 691 ms | 646 and 625 ms |
| each step after the index | 22 ms or less | the same | the same |
| all of the above | 1765 ms | 1523 ms | 948 and 899 ms |

| CSV, with no index | In place | First session, with the copy |
|---|---|---|
| the first frame | 119 ms | 615 ms |
| `count(*)` | 586 ms | 163 ms |
| move to the last row | 800 ms | 802 ms |
| sort on a column of text | 187 ms | 193 ms |
| statistics of a column | 375 ms | 376 ms |
| all of the above | 2118 ms | 2177 ms |

| JSON, with the index | In place | First session, with the copy | A copy of an earlier session |
|---|---|---|---|
| the copy | - | 279 ms (915 MB/s) | - |
| the first frame | 228 ms | 432 ms | 150 and 157 ms |
| `count(*)` | 300 ms | 64 ms | 77 and 79 ms |
| the index | 260 ms | 241 ms | 283 and 275 ms |
| all of the above | 830 ms | 763 ms | 546 and 546 ms |

### A table in memory

The example loaded each file with `CREATE TABLE t AS SELECT * FROM ...`:

| File | The load from the share | The load from the local disk | The memory that DuckDB held | Sort, after the load |
|---|---|---|---|---|
| Parquet, 1.03 GB | 1557 and 1644 ms | 550 ms | 2.64 GB | 20 and 17 ms |
| CSV, 494 MB | 1175 ms | - | 523 MB | 14 ms |
| JSON, 255 MB | 495 ms | - | 204 MB | 5 ms |

### What the numbers show

- **A Parquet file gains the most.** Each step that reads the data is 1.5 to 5
  times faster from the copy. On this fast network, the first session with
  the copy costs about the same as a session in place, and each later session
  is 3.7 times faster. The file of 64 MB is faster from the first session. A
  slower network makes each small read wait longer, so the gain is larger
  there.
- **A file of text gains less, but it gains.** The reader of DuckDB reads a
  file of text in large blocks, from the start to the end, and its speed is
  near the speed of the network. In the first session, the copy costs as much
  as it saves. In each later session the first frame is faster, and a session
  is 1.5 to 1.9 times faster. Peruse therefore copies a file of text too.
- **A file of text with no index gains only in the first read.** Windows keeps
  the pages of the file in memory after the first read, and the later steps of
  the session then read memory, both in place and from the copy. A network or
  a machine that keeps fewer pages in memory makes each step read the network
  again, and the copy then helps each step.
- **A table in memory is the index of a file of text.** For CSV and JSON, the
  index of Peruse already holds the rows in memory, and the table took about
  the size of the file. A second way to load a file of text into memory would
  add nothing.
- **A table in memory costs too much for Parquet.** The table took 2.6 times
  the size of the file. A file of 10 GB would need 26 GB, which is more than
  the memory limit of most machines, and DuckDB would then write the table to
  its temporary directory. A table in memory also goes away at the end of the
  session. The copy on the disk has none of these costs, and the system keeps
  its pages in memory in any case.
- **A small file gains nothing that a user can feel.** A file below 32 MB opens
  in place in some tens of milliseconds, and a copy of it would only add an
  entry to the directory of the copies.

The example `netcache-bench` measures a file in place, from a copy and in
memory:

```sh
cargo run -p peruse-core --example netcache-bench -- gen <dir>
cargo run -p peruse-core --example netcache-bench -- run <file on a share>
cargo run -p peruse-core --example netcache-bench -- run <file on a share> --copy <dir>
cargo run -p peruse-core --example netcache-bench -- run <file on a share> --no-index
cargo run -p peruse-core --example netcache-bench -- mem <file on a share>
```

A second `run` with the same `--copy` directory reads the copy of the first.

## What Peruse does not do yet

- The copy comes before the first frame. On a slow network, a file of some
  gigabytes keeps the user at the copy screen for some time, and `Esc` is the
  only way to the grid before the copy ends. A copy behind the grid, with a
  change of the view to the copy at the end, would show the first frame at
  once.
- The chooser of files does not mark a file that has a copy.
- The test of a network drive on Linux and macOS has no test on a true network
  drive. The tests cover the text of `/proc/self/mountinfo` and the names of
  the file systems.
- The measurements come from one fast network. A network of 1 gigabit, or a
  VPN, has a lower rate and a longer round trip. The copy then takes longer,
  and each small read in place waits longer too. No measurement shows the
  balance there yet.
- Peruse does not load a Parquet file into memory. The table took 2.6 times
  the size of the file. A small Parquet file could get a table in memory, as a
  file of text gets its index, when the memory limit allows it.
