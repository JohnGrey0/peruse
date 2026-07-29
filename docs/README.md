# The documents of Peruse

This directory holds one document for each major part of Peruse. Each document
starts with a summary of one paragraph, and then gives short sections. Read
`architecture.md` first. It shows how the other parts fit together.

## The list of documents

| Document | Subject |
|---|---|
| [architecture.md](architecture.md) | The two crates, the threads, and the path from a key to a frame |
| [engine.md](engine.md) | The set-up of DuckDB, the open operation, the pages, the counts and the CSV index |
| [query-generation.md](query-generation.md) | How the view becomes SQL, and each statement that Peruse builds |
| [filter.md](filter.md) | The list of conditions, the operators, and how the list becomes one expression |
| [ddl.md](ddl.md) | How Peruse measures a file and writes a `CREATE TABLE` statement for another database |
| [nested-values.md](nested-values.md) | A value that holds other values, and the tree that drills into it |
| [read-only-guard.md](read-only-guard.md) | The two layers that stop a write, and the statements that pass |
| [worker-and-concurrency.md](worker-and-concurrency.md) | The worker thread, the combination of requests, the epoch and the cancellation |
| [user-interface.md](user-interface.md) | The layout, the grid, the panels, the overlays and the prompt |
| [keys-and-commands.md](keys-and-commands.md) | The table of commands, and the help and the palette that come from it |
| [themes.md](themes.md) | The model of the colors, the built-in themes, and how to write a theme file |
| [performance.md](performance.md) | The measured times, and the decisions that give them |

## The rules for this documentation

These documents follow ASD-STE100, the ASD Simplified Technical English
Specification. The main rules are these:

- Use the approved words, and give each word one meaning.
- Write one topic in one sentence. An instruction has 20 words at the most, and
  a description has 25 words at the most.
- Write six sentences in a paragraph at the most.
- Use the active voice. Do not use the passive voice.
- Use the simple tenses: present, past and future.
- Write an instruction as a command: "Press `?`".
- Keep the articles "the" and "a".
- Use three words at the most in a group of nouns.
- Use the same word for the same thing each time.
- Do not use slang, an idiom, a metaphor or a joke.
- Give the full form of an abbreviation at its first use.
- Use a vertical list for complex information.

The word list below is American English, because the dictionary of the
specification uses American English. The words `color` and `gray` therefore
occur in the text. The names in the code do not change.

## The controlled term list

Each term below has one meaning in this documentation and in the comments of
the code. The right column gives the words that these documents do not use.

| Term | Meaning | Do not use |
|---|---|---|
| file | The data on the disk that Peruse shows | dataset, document, data source |
| file set | More than one file that Peruse shows as one table | dataset, partition |
| row | One record of the data | record, line, entry, tuple |
| column | One field of the data | field, attribute |
| cell | One value at one row and one column | box, item |
| value | The contents of one cell | data, datum |
| NULL | The SQL value that shows a missing value | null, nil, empty |
| grid | The table of rows and columns on the screen | table, sheet, viewer |
| page | A block of rows that Peruse reads in one operation | batch, chunk, window |
| view | The description of what the grid shows | query state, model |
| statement | A complete piece of SQL | SQL, code, command |
| query | A statement that only reads | read query, select |
| filter | The list of conditions that removes rows, and the expression that it compiles to | predicate |
| condition | One test in the filter | predicate, clause, rule |
| operator | The test that one condition makes, such as `=` or `contains` | comparison, test |
| sort | The order of the rows | ordering, sorting |
| schema | The name and the type of each column | header, columns |
| engine | The layer that uses DuckDB | backend, database layer |
| worker | The background thread that runs the engine | task, job, actor |
| epoch | The counter that Peruse increases at each change of the view | generation, version |
| request | A message to the worker | job, task, message |
| response | An answer from the worker | reply, result, event |
| panel | The metadata panel or the column statistics panel | sidebar, pane |
| overlay | A box that covers the grid: the help, the palette, the theme picker, the cell inspector, the record view or the filter builder | popup, dialog, modal |
| record view | The overlay that shows one row from the top to the bottom | transpose, detail view |
| field | One value in the record view, at any level | attribute, property |
| structure | A value that holds named values, from the SQL type `STRUCT` | object, record, map |
| list | A value that holds values in order, from the SQL type `LIST` | array, vector |
| tree | The lines of the record view, with their levels | hierarchy, outline |
| path | The way from the row to one value, such as `payload.commits[1].sha` | selector, key, pointer |
| open | To show the values that a line holds | expand, unfold, drill down |
| close | To hide the values that a line holds | collapse, fold |
| prompt | The editor of one line at the bottom of the screen | input bar, command line |
| key | One key of the keyboard | shortcut, binding, chord |
| command | One operation that a key or the palette starts | action, function |
| screen column | One character position across the terminal | column, cell, character |
| width | A measure in screen columns | size, length |
| draw | To write characters into the buffer of the frame | render, paint, blit |
| show | To put information in front of the user | display, present |
| give | To send a value back from a function | return, yield |
| reject | To refuse a statement that writes | block, deny, forbid |
| index | The table in memory that holds a file of text | cache, materialization |
| position | A number that selects an item in a list | index, offset |
| offset | The number of a row in the view, from 0 | index, position |
| sniffer | The part of DuckDB that finds the format of a CSV file | detector, parser |
| color | One color of a theme | colour, hue, shade |
| theme | A complete set of colors | palette, color scheme |
| role | One named color in a theme | slot, token, variable |
