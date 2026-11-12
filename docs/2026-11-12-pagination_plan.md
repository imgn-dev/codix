# codix — Pagination Plan

Paging through results that change while you use them: `find`, `refs` and
`grep`, from a terminal, an agent over MCP, an agent or a script with `--json`,
and a refactoring loop that edits what it pages through. Each step lands as one
reviewed change, on its own branch, with its tests, `cargo clippy --all-targets
-- -D warnings`, `cargo fmt --check` and the full test suite green on the three
CI platforms.

## The problem

Pages are slices of a list sorted in memory: `Page { limit, offset }` applied
after the query (`crates/codix-core/src/page.rs`). Queries keep the index
fresh, so each page is cut from the files as they are now. That is right for
freshness and wrong for offsets:

- **Refactoring skips results.** `refs` gives 50 of 200; you fix those 50; the
  next call, `--offset 50`, is cut from the 150 left and skips 50 you never
  saw. Nothing says so.
- **Next pages are computed by hand.** The text footer says
  `next: --offset 50`; JSON has no next page at all, so an agent adds `offset`
  and the length of `items` itself. Over MCP, the footer names a CLI flag.
- **Paging is often the wrong tool.** `refs getBean` on spring-framework gives
  4228 results: 85 pages. What helps is narrowing (a directory, a certainty),
  counting, or a list of files to edit.
- **Counts mislead.** `total`, `scoped` and `textual` count the definitions as
  references: of `getBean`'s 4228, 5 are its overloads' definitions, and they
  are exactly the 5 `scoped`. `target` names the first overload only, though
  the results cover all of them (`overloads()` in `scope.rs`).

## Goal

- A page never skips a result because earlier results changed.
- The next page is given, never computed: a command to copy in a terminal,
  arguments to resend over MCP and in JSON.
- Ways not to page: a count, a breakdown by directory, a list of files, a
  certainty filter, and everything at once for scripts.
- Counts that say what they count.
- Stateless: nothing kept between calls, in-process or in the daemon.

## Decisions taken

On 2026-11-12:

- **(a) `--offset` is removed**, from the CLI, requests and responses.
- **(b) The JSON contract stays v1.** codix is still in design: its contract
  changes in place, `SCHEMA_VERSION` stays 1 and `schemas/v1/` is updated.
- **(c) Fuzzy `find` corrects the name.** A fuzzy match is never certain, so
  fuzzy `find` answers with something concrete: the symbols of the closest
  name, paged like any exact lookup, and the other close names, closest first,
  as `git stats` answers with `git status` and its alternatives (see
  Fuzzy find).
- **(d) Arguments are quoted only when necessary**, for the platform's shell:
  POSIX on macOS and Linux, PowerShell on Windows.
- **(e) Breaking changes are allowed until a first release.**
  `crates/codix-cli/tests/schemas.rs` refuses any breaking change to a
  snapshot and asks for a new `SCHEMA_VERSION`. Until codix is released, a
  constant lets `CODIX_BLESS=1` bless breaking changes in place; without
  blessing, the test still fails and lists them. codix is used only on test
  checkouts, so nothing needs to stay compatible: an index from before a
  change is deleted and built again.

## Design

### Cursors

`--after <position>` replaces `--offset`. A position is readable, not opaque:

- `<path>`: after every result in that file;
- `<path>:<line>:<col>`: after the result at that place.

`<path>` is root-relative, so a cursor works from any directory. The next page
holds the results that sort after the position, in the order each command
already uses (`sort_results`): definitions, then the results in the anchor
file (for `refs`, the definition's file), then by path, line and column. For
the comparison, a definition counts as part of its file, and a position no
longer in the results is placed where it would sort, so a result edited away
does not lose the cursor.

**Pages end at a file boundary** when that keeps at least half the page:
50 results in 7 files become 44 results in 6 when the seventh would be cut.
The cursor is then a path, and no edit to a file already seen, however it
moves lines, can shift the next page. A file that alone holds more than half
a page is cut inside, at a position; only edits to that file, above that
position, can then shift results across the cut. A page never exceeds
`--limit`.

**The next page keeps the query's meaning.** What the first page resolved is
written into the next call:

- `refs`: the target's symbol ID replaces the name, so a rename during the
  refactoring is followed (`renamed`) and the name never resolves to another
  symbol;
- `find`: the mode that matched (`--exact`, `--prefix` or `--glob`) is added,
  so the fallback cannot switch modes between pages; after a fuzzy
  correction, the corrected name replaces the query, with `--exact`;
- `grep`: nothing to pin.

A cursor is a position, so it is valid with any query: reusing it with other
arguments is not an error, it means "results after this place".

### Fuzzy find

Fuzzy matching runs when exact and prefix matching find nothing, or with
`--fuzzy`. Today it answers up to 500 symbols of many names, ranked by edit
distance, which no position cursor can page and no reader can act on. It
becomes a correction:

- Close names are ranked by edit distance, then by trigrams shared with the
  query, then by name, as today, but as distinct names.
- **The closest name is taken as the one meant**, and its symbols are the
  results: the same list as `find <name> --exact`, sorted and paged by
  position like it.
- **The other close names are offered**, closest first, at most 8, each with
  its distance and its number of symbols. When several names share the
  smallest distance, the first by the order above is taken and the others
  lead the list.

```
no symbol named getBaen; showing getBean
spring-beans/src/main/java/org/springframework/beans/factory/BeanFactory.java:146:9  method  …
…
5 results
other names: getBeans (1 edit, 3 symbols)  codix find getBeans
             getBeanName (4 edits, 12 symbols)  codix find getBeanName
```

JSON: `mode` is `fuzzy`, `corrected` holds the name shown, and
`alternatives: [{name, distance, symbols}]`. Over MCP, each alternative gives
its argument, `query=getBeans`. Next pages run `find getBean --exact`, so they
stay on the corrected name.

### The next page

Responses carry `next`, absent on the last page: the request fields to
override, everything else unchanged.

```json
"next": {"after": "spring-beans/src/main/java/org/springframework/beans/factory/support/AbstractBeanFactory.java",
         "symbol": "java:spring-beans/src/main/java/org/springframework/beans/factory/BeanFactory.java::BeanFactory.getBean#3677"}
```

Text output ends with the next page:

- **CLI**: the full command, ready to copy:
  ```
  44 of 4228 results
  next: codix refs 'java:spring-beans/…::BeanFactory.getBean#3677' --after spring-beans/src/main/java/org/springframework/beans/factory/support/AbstractBeanFactory.java
  ```
  It is the command line as invoked (`argv[0]` included, so
  `./target/release/codix` stays), with `--after` set and the pinned arguments
  replaced, every other argument in place. Arguments are quoted only when
  needed: POSIX single quotes on macOS and Linux (`#`, `*` and spaces are
  quoted, so zsh's globbing does not touch a symbol ID), PowerShell's on
  Windows. The line has no colour codes. The response is rendered by the CLI
  from its own `argv`, so it works the same through the daemon.
- **MCP**: `next: symbol=… after=…`, the arguments to change, since agents do
  not type commands.

No previous page: earlier pages are still in the reader's terminal or context,
and a backward cursor doubles the design.

### Not paging

- **`--count`**: the total and the breakdown, no results. `refs` adds its
  counts.
- **Breakdown**: whenever the results do not all fit (more than shown, or
  `--count`), the footer adds where they are: the results are grouped by the
  first path component below the deepest directory common to all of them,
  biggest first, at most 8 groups and a line for the rest. Each group comes
  with its command (CLI) or argument (MCP):
  ```
  1204  spring-context   codix refs getBean --path spring-context
   911  spring-beans     codix refs getBean --path spring-beans
   …
  ```
  Paths are shown from the cwd, as `--path` takes them; a group can be a file,
  which `--path` also accepts. JSON: `breakdown: [{path, count}]`.
- **`--files`**: one row per file, with its number of results, in the same
  order, paged by file (`--after <path>`): the worklist of a refactoring.
- **`--all`** (CLI only): no limit, for scripts. It is not a request field, so
  MCP does not offer it; an agent that wants more passes `limit`. JSON stays
  one document: codix sorts the whole list in memory anyway, so streaming one
  result per line would save nothing, and `jq -c '.result.references.items[]'`
  gives lines.
- **`refs --confidence scoped|textual`**: keeps one certainty. On `getBean`,
  `scoped` is the difference between 4228 results and the 0 references codix
  is sure of.

### Counts

- `refs` responses count definitions apart: `definitions`, and `scoped` and
  `textual` for references only. `total` stays the number of results before
  paging, definitions included, since it counts the list being paged.
- `targets` lists every overload's symbol ID; `target`, the first, goes.

### Contract

v1, changed in place (see (b) and (e)): `offset` removed from paged requests
and responses, `after` added to requests, `next` and `breakdown` to responses,
`refs` responses with `definitions` and `targets`, `find` responses with
`corrected` and `alternatives`; `--count`, `--files` and `--confidence` as
request fields. README (Queries), `docs/commands.md` and the MCP tool
descriptions document cursors, fuzzy correction and the ways not to page.

Unchanged: `outline`, `previews` and the other lists are not paged today and
stay so. `refs`, `def`, `show` and `rename` given an unknown name keep their
error; the same correction could serve them later.

## Steps

1. **Cursors.** `--after` with file boundaries and pinning; `--offset`
   removed; `next` in JSON and MCP text; `refs` counts and `targets`; the schema
   check allowing breaking changes before a release (e), and the v1 schemas
   blessed. Until step 3, fuzzy `find` answers one page
   with no `next`. Tests: the refactoring loop (edit the first page's
   results, the second page is the rest), an edit that moves lines in a file
   already seen, a file larger than a page, a cursor whose result was
   deleted, a rename between pages.
2. **The copyable command.** Rebuilt from `argv` with the pinned arguments;
   POSIX and PowerShell quoting when necessary, tested on symbol IDs, spaces,
   quotes and `--after=x` forms.
3. **Fuzzy correction.** Distinct names, the closest taken, alternatives with
   their commands, ties.
4. **`--count` and `--all`.**
5. **Breakdown.** Grouping below the common directory, files as groups, the
   8-group cap.
6. **`--files`.**
7. **`refs --confidence`.**

Steps 3 to 7 are independent of each other once 1 and 2 are in.
