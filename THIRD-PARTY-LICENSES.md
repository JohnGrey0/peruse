# Third party licences

Peruse is Apache-2.0. See `LICENSE` for the text.

A build of Peruse contains other code. This file names that code and gives
the licence of each part. Read it when you must show where the program comes
from, or when a tool that examines licences asks you a question about it.

Two groups of code are in a build:

1. Rust crates, which Cargo fetches.
2. DuckDB, and 26 C and C++ libraries inside DuckDB. The build compiles
   these into the program, so the released binary holds them.

Every licence here permits you to give the program away, and none of them
asks you to release your own code. Read [Notes for a licence scan](#notes-for-a-licence-scan)
first if a tool reported a problem: three of the reports that such tools give
for this program are false, and that section says why.

---

## 1. Rust crates

The build uses **224** crates. Each one carries a permissive licence:

| Licence | Crates |
|---|---:|
| MIT OR Apache-2.0 (and the same in other spellings) | 138 |
| MIT | 53 |
| Apache-2.0 | 11 |
| Apache-2.0 OR MIT | 6 |
| Unlicense OR MIT | 2 |
| Zlib | 2 |
| BSD-2-Clause OR Apache-2.0 OR MIT | 2 |
| Other permissive: CC0-1.0, BSL-1.0, WTFPL, Unicode-DFS-2016, Unicode-3.0 | 10 |

No crate carries the GPL, the LGPL, or the AGPL. No crate is without a
licence.

Peruse chooses **MIT** for each crate that offers a choice.

One crate, `termina`, offers "MIT OR MPL-2.0". Two things make it safe.
Peruse chooses MIT, which the licence permits. And the build does not
compile it at all: it belongs to a backend of `ratatui` that Peruse does not
turn on. To confirm:

```sh
cargo tree -i termina -e normal    # says: nothing to print
```

To make this list again:

```sh
cargo metadata --format-version 1 --filter-platform <your-target>
```

### A note about `directories`

Peruse used the `directories` crate before. That crate reaches `option-ext`,
which carries the Mozilla Public Licence. The MPL is weak copyleft. It did
not reach the code of Peruse, but a licence tool reported it, and somebody
then had to answer for it.

Peruse now has `crates/peruse-core/src/dirs.rs`, which reads the same
environment variables and gives the same paths. `option-ext` is gone. To
confirm:

```sh
cargo tree -i option-ext    # says: did not match any packages
```

---

## 2. DuckDB

Peruse reads and queries files with DuckDB, version **v1.5.5**.

DuckDB carries the **MIT** licence.

    Copyright 2018-2025 Stichting DuckDB Foundation

The build compiles DuckDB from source, so the binary holds it.

---

## 3. The C and C++ libraries inside DuckDB

DuckDB carries these 26 libraries in its own source and compiles them into
itself. They are therefore in a build of Peruse.

The DuckDB source that Cargo fetches (through `libduckdb-sys`) does **not**
carry the licence files of these libraries. This file supplies them. For the
complete text of each one, read `third_party/<name>/LICENSE` in the DuckDB
source at tag `v1.5.5`:

    https://github.com/duckdb/duckdb/tree/v1.5.5/third_party

| Library | Licence | Copyright |
|---|---|---|
| brotli | MIT | Copyright 2016 Google Inc. |
| concurrentqueue | BSD-2-Clause, and Zlib for one part | (c) 2013-2016 Cameron Desrochers |
| fast_float | Apache-2.0 OR MIT OR BSL-1.0 | Daniel Lemire, Joao Paulo Magalhaes, and others |
| fastpforlib | Apache-2.0 | (c) Daniel Lemire |
| fmt | MIT | Copyright (c) 2012-2016 Victor Zverovich |
| fsst | MIT | Copyright 2018-2020, CWI, TU Munich, FSU Jena |
| httplib | MIT | Copyright (c) 2025 Yuji Hirose |
| hyperloglog | BSD-3-Clause | Copyright (c) 2014, Salvatore Sanfilippo |
| jaro_winkler | MIT | Copyright (c) 2022 Max Bachmann |
| libpg_query | PostgreSQL | Copyright (c) 1996-2017, PostgreSQL Global Development Group |
| lz4 | BSD-2-Clause | Copyright (c) 2011-2020, Yann Collet |
| mbedtls | Apache-2.0 OR GPL-2.0-or-later | Copyright The Mbed TLS Contributors |
| miniz | MIT | Copyright 2013-2014 RAD Game Tools and Valve Software; Copyright 2010-2014 Rich Geldreich and Tenacious Software LLC |
| parquet | Apache-2.0 | The Apache Software Foundation |
| pcg | Apache-2.0 OR MIT | Copyright 2014-2017 Melissa O'Neill |
| pdqsort | Zlib | Copyright (c) 2021 Orson Peters |
| re2 | BSD-3-Clause | Copyright 2016-2023 The RE2 Authors |
| ska_sort | BSL-1.0 | Copyright Malte Skarupke 2016 |
| skiplist | MIT | Copyright (c) 2015-2023 Paul Ross |
| snappy | BSD-3-Clause | Copyright 2008 Google Inc. |
| tdigest | Apache-2.0 | The Apache Software Foundation |
| thrift | Apache-2.0 | The Apache Software Foundation |
| utf8proc | MIT | Copyright (c) 2014-2021 Steven G. Johnson, Jiahao Chen, Peter Colberg, Tony Kelman, Scott P. Jones, and other contributors; Copyright (c) 2009 Public Software Group e. V., Berlin, Germany |
| vergesort | MIT | Copyright (c) 2015-2020 Morwenn |
| yyjson | MIT | Copyright (c) 2020 YaoYuan |
| zstd | BSD-3-Clause OR GPL-2.0 | Copyright (c) Meta Platforms, Inc. and affiliates |

### The choices that Peruse makes

Three libraries give a choice of licence. Peruse chooses the permissive one
in each case. This choice is a right that the licence itself gives:

| Library | The choice | Peruse chooses |
|---|---|---|
| zstd | BSD-3-Clause **or** GPL-2.0 | **BSD-3-Clause** |
| mbedtls | Apache-2.0 **or** GPL-2.0-or-later | **Apache-2.0** |
| fast_float | Apache-2.0 **or** MIT **or** BSL-1.0 | **MIT** |
| pcg | Apache-2.0 **or** MIT | **MIT** |

No GPL text applies to a build of Peruse.

### The notice that Apache-2.0 asks for

Section 4(d) of Apache-2.0 asks a program to carry the NOTICE of the work.
Apache Thrift gives this notice:

```
Apache Thrift
Copyright 2006-2010 The Apache Software Foundation.

This product includes software developed at
The Apache Software Foundation (http://www.apache.org/).
```

Apache Parquet gives the same form of notice.

The complete Apache-2.0 text is in `LICENSE`, because Peruse itself uses
that licence.

---

## Notes for a licence scan

A tool that examines licences reports three things about this program that
look like a problem but are not. Each one is explained below. Give this
section to the person who asks.

### 1. libpg_query appears to hold the GPL

**The report.** Two files hold the text of the GNU General Public License:

    third_party/libpg_query/src_backend_parser_gram.cpp
    third_party/libpg_query/include/parser/gram.hpp

**Why it is not a problem.** Bison, a program that writes parsers, made
these two files. Bison puts its own header at the top of what it writes.
That header holds the GPL, and then it holds this exception:

> As a special exception, you may create a larger work that contains part or
> all of the Bison parser skeleton and distribute that work under terms of
> your choice, so long as that work isn't itself a parser generator using the
> skeleton or a modified version thereof as a parser skeleton.

Peruse is a program that shows data files. It is not a parser generator. The
exception therefore applies, and the GPL does not reach Peruse.

The grammar itself comes from PostgreSQL and carries the PostgreSQL licence,
which is a permissive licence of the BSD form.

This is a known and usual result. Every program that uses PostgreSQL
grammar, or Bison, gets the same report.

### 2. The binary holds a TLS library and an HTTP library

**The report.** `mbedtls` and `httplib` are in the binary. A tool that looks
for network code reports them.

**Why it is not a problem.** DuckDB carries these for its `httpfs`
extension, which reads a file over a network. **Peruse does not build or
load that extension.** Peruse builds DuckDB with three features only:
`bundled`, `parquet`, and `json`.

Peruse opens a file on a disk and nothing else. It makes no network
connection. To confirm, watch the program with any tool that shows the
connections of a process: it opens none.

### 3. mbedtls and zstd name the GPL

**The report.** Files in `mbedtls` and `zstd` hold the letters "GPL".

**Why it is not a problem.** Both libraries offer a choice of two licences,
and the words appear because the file names the choice. Peruse takes the
permissive one in each case, as the table above records. A tool that reads
only for the letters "GPL", and not for the choice beside them, gives this
report.

---

## Licence texts

### MIT

Applies to: brotli, fmt, fsst, fast_float (chosen), httplib, jaro_winkler,
miniz, pcg (chosen), skiplist, utf8proc, vergesort, yyjson, and to most of
the Rust crates.

```
Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

### BSD-3-Clause

Applies to: hyperloglog, re2, snappy, zstd (chosen).

```
Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

 * Redistributions of source code must retain the above copyright notice,
   this list of conditions and the following disclaimer.
 * Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.
 * Neither the name of the copyright holder nor the names of its contributors
   may be used to endorse or promote products derived from this software
   without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE
LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR
CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF
SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS
INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE)
ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
POSSIBILITY OF SUCH DAMAGE.
```

### BSD-2-Clause

Applies to: lz4, concurrentqueue.

```
Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

 * Redistributions of source code must retain the above copyright notice,
   this list of conditions and the following disclaimer.
 * Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE
LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR
CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF
SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS
INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE)
ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
POSSIBILITY OF SUCH DAMAGE.
```

### Zlib

Applies to: pdqsort, one part of concurrentqueue, and two Rust crates.

```
This software is provided 'as-is', without any express or implied warranty.
In no event will the authors be held liable for any damages arising from the
use of this software.

Permission is granted to anyone to use this software for any purpose,
including commercial applications, and to alter it and redistribute it
freely, subject to the following restrictions:

1. The origin of this software must not be misrepresented; you must not claim
   that you wrote the original software. If you use this software in a
   product, an acknowledgment in the product documentation would be
   appreciated but is not required.
2. Altered source versions must be plainly marked as such, and must not be
   misrepresented as being the original software.
3. This notice may not be removed or altered from any source distribution.
```

### Boost Software License 1.0

Applies to: ska_sort.

```
Permission is hereby granted, free of charge, to any person or organization
obtaining a copy of the software and accompanying documentation covered by
this license (the "Software") to use, reproduce, display, distribute,
execute, and transmit the Software, and to prepare derivative works of the
Software, and to permit third-parties to whom the Software is furnished to
do so, all subject to the following:

The copyright notices in the Software and this entire statement, including
the above license grant, this restriction and the following disclaimer,
must be included in all copies of the Software, in whole or in part, and
all derivative works of the Software, unless such copies or derivative
works are solely in the form of machine-executable object code generated by
a source language processor.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE, TITLE AND NON-INFRINGEMENT. IN NO EVENT
SHALL THE COPYRIGHT HOLDERS OR ANYONE DISTRIBUTING THE SOFTWARE BE LIABLE
FOR ANY DAMAGES OR OTHER LIABILITY, WHETHER IN CONTRACT, TORT OR OTHERWISE,
ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
```

### PostgreSQL

Applies to: libpg_query.

```
Portions Copyright (c) 1996-2017, PostgreSQL Global Development Group
Portions Copyright (c) 1994, The Regents of the University of California

Permission to use, copy, modify, and distribute this software and its
documentation for any purpose, without fee, and without a written agreement
is hereby granted, provided that the above copyright notice and this
paragraph and the following two paragraphs appear in all copies.

IN NO EVENT SHALL THE UNIVERSITY OF CALIFORNIA BE LIABLE TO ANY PARTY FOR
DIRECT, INDIRECT, SPECIAL, INCIDENTAL, OR CONSEQUENTIAL DAMAGES, INCLUDING
LOST PROFITS, ARISING OUT OF THE USE OF THIS SOFTWARE AND ITS DOCUMENTATION,
EVEN IF THE UNIVERSITY OF CALIFORNIA HAS BEEN ADVISED OF THE POSSIBILITY OF
SUCH DAMAGE.

THE UNIVERSITY OF CALIFORNIA SPECIFICALLY DISCLAIMS ANY WARRANTIES,
INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS FOR A PARTICULAR PURPOSE. THE SOFTWARE PROVIDED HEREUNDER IS ON AN
"AS IS" BASIS, AND THE UNIVERSITY OF CALIFORNIA HAS NO OBLIGATIONS TO
PROVIDE MAINTENANCE, SUPPORT, UPDATES, ENHANCEMENTS, OR MODIFICATIONS.
```

### Apache-2.0

Applies to: fastpforlib, mbedtls (chosen), parquet, tdigest, thrift, and to
some of the Rust crates.

The complete text is in `LICENSE` at the root of this repository, because
Peruse itself uses this licence.

### Unicode

utf8proc holds data from the Unicode Character Database. Terms of use:

    https://www.unicode.org/copyright.html

---

## How to check this file again

After you change a dependency, confirm that nothing copyleft came in:

```sh
cargo tree -i option-ext        # must find nothing
cargo metadata --format-version 1 --filter-platform <your-target>
```

The DuckDB version is in `Cargo.lock`, under `libduckdb-sys`. If that
version changes, read the third_party directory of the matching DuckDB tag
and bring this file up to date.
