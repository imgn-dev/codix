# codix — Agent Feedback Plan

Fixes from two agents that used codix as their only way to explore code: one
on spring-framework 6.2.0, one on Linux 6.12, each with 5 or 6 tasks
(tracing a call, finding implementations and call sites, a rename applied and
undone, a misspelled name). Their 124 calls were logged. The plan lands on one
branch, one commit per step, the full local suite after each step, and one CI
run at the end.

## What the agents ran into

| | Spring | Linux |
| --- | --- | --- |
| codix calls | 68 | 56 |
| Failed | 7, 4 of them ambiguous names | 8, 6 of them ambiguous names |
| Wasted, by their own count | about 9 | about 8 |
| Time in codix | 21 s | 115 s, about 80 s of it `grep` |
| `grep` calls | 8 | 13 |

- **Ambiguous names: 10 of the 124 calls.** The error lists bare IDs such as
  `…getBean#7197` and `…#d92b`, with no kind, line or signature, so each one
  cost a `find`. Causes: overloads in one file (`show` refuses them, `def`
  takes them), a C declaration and its definition (both indexed as
  `function`), copies under `tools/`, a struct field of the same name.
- **A definition missing from the index.** `kernel/sched/core.c` defines
  `pick_next_task` twice, in `#ifdef CONFIG_SCHED_CORE` and its `#else`, with
  the same signature. Only the first is indexed, and `def core.c:6491:1`
  answers with the other one.
- **Headers in the wrong language.** `*.h` is C unless the file contains a
  C++ marker anywhere, so `class ` in a comment makes `mm_types.h` C++
  (`cpp:include/linux/mm_types.h::page`). `languages` lists no `*.h` rule, nor
  the Perl and MATLAB files `status` reports.
- **No way to narrow some questions.** "Outside spring-beans", "main sources
  only", "calls only" (`refs mutex_lock` counted reads of the field
  `pwrctrl_priv.mutex_lock`), and "which files call it most" (`--files` is in
  path order).
- **A rename that breaks the build, silently.** With `--exclude-unresolved`,
  only the declaration was renamed; 10 calls in other files were left
  dangling, and nothing said so after `apply`. The preview lists those sites
  twice.
- **Undo is hard to read.** Applying an undo says "undo with: codix undo",
  which both agents read as a second undo; it is a redo. The undo preview of a
  4-site rename was one 114-line hunk.
- **`grep` adds little, slowly.** On Linux it takes 10–12 s a call: it reads
  86,000 files one after the other (ripgrep takes 1.75 s on all cores, 11.4 s
  on one). The agents used it to double-check `refs`, to count by place (a
  paging feature, not grep's), and to search text ripgrep finds as well. Its
  own contribution, the enclosing symbol of each match, needs no search of
  its own.
- **Output that costs a call or misleads:** `status` lists every module;
  `outline` has no depth (343 lines for `mm_types.h`); `capabilities` says
  "10816 files indexed" where `status` counts 9930 indexed; "no results" looks
  like a failure; the `--glob` example is unquoted, which zsh rejects;
  suggested commands repeat the full path of the binary.

Not in this plan, each a project of its own: a Java resolver, which binds
references across files (today every one is `textual`, and a cross-file
rename is refused); a type hierarchy command (implementations of an
interface); C parsing through attribute macros such as `__fastpath_inline`,
which make rename refuse a site (223 of 1000 Linux renames in the benchmark).

## Decisions taken

On 2026-11-12:

- **(c) `grep` is removed.** Agents with a shell have their own, faster and
  richer. What it added, the enclosing symbol of a place, becomes its own
  command, `codix at`, which also annotates another tool's results. Text-only
  files lose their only use: they stay in the index and `status`, as known
  files without symbols.

## Decisions to take

- **(a) `def` with several matches lists them all** (exit 0), definitions
  with a body first, instead of failing. A lookup has no reason to refuse:
  the agent wanted the list anyway. `show` and `refs` still need one symbol
  and keep the error, with the richer candidates. Alternative: keep the error
  everywhere.
- **(b) Declarations step aside.** When a name matches C or C++ definitions
  with a body and declarations without one, the declarations are dropped
  from resolution. `--kind declaration` still finds them. Alternative: rank
  them last but keep them in the ambiguity.

## Design

### Ambiguity

- Candidates carry their kind, place and signature, in text and JSON:
  ```
  error[ambiguous_symbol]: "DefaultListableBeanFactory.getBean" matches 2 symbols
    method  …/DefaultListableBeanFactory.java:363:18  public <T> T getBean(Class<T> requiredType)
            java:…::DefaultListableBeanFactory.getBean#7197
  ```
  JSON: `candidates: [{symbol_id, kind, path, line, column, signature}]`.
- `show` takes the overloads of a name in one file, as `def` does, and prints
  each.
- C and C++ declarations without a body are indexed as such: the parse marks
  them, and a symbol gets `declaration: true`; `find` and `outline` show the
  kind as `declaration`. Then (a) and (b).
- **The lost `#else` definition:** find whether the parse or the store drops
  the second symbol of an identical ID, keep both, and give the second an
  occurrence suffix (`…#14e7~2`). `def file:line:col` resolves the place, not
  the first symbol of that ID.

### Languages

- `*.h` is C++ only on C++ code: a marker at the start of a line, outside
  comments and strings (`class X {`, `namespace`, `template <`, an access
  specifier), or a C++-only token in code (`std::`, `nullptr`, `constexpr`).
- `languages` lists each ambiguous extension with its rule, and the languages
  detected without a provider (Perl, MATLAB, …) as search only.

### Narrowing

- `--exclude-path DIR`, repeatable, on every command that takes `--path`.
- `--sources main` or `--sources test`, from the build modules' source and
  test roots; a usage error naming the reason when no build system declares
  them.
- `refs --context call|type|import|read|write`, repeatable.
- `--files --sort count`: the files with the most results first. Not a
  stable order while editing, so such a page has no next page; the footer
  says how many files are left out.

### Rename, apply, undo

- The preview lists unresolved sites once.
- Applying a preview that leaves references unchanged ends with a warning,
  `references_left`: how many, and the command to see them.
- An undo preview inverts the edits the journal recorded, one hunk per site,
  and falls back to today's single edit when the file changed since.
- Applying an undo says what it undid, and that `codix undo` would redo it.

### `codix at`

What encloses a place: for each `path:line[:col]`, the innermost symbol
holding it, with its kind, qualified name (which names its containers),
signature and ID.

```
codix at mm/slub.c:4142 kernel/sched/core.c:6050:9
rg -n 'mutex_lock\(' fs/ext4 | codix at -
```

- **Places** are arguments, or lines on standard input with `-`, in the forms
  `rg -n`, `rg --vimgrep`, `grep -n` and compilers print: `path:line:`,
  `path:line:col:`, followed by any text. The text is kept, so the output is
  the input annotated:
  ```
  fs/ext4/super.c:1234   function  ext4_put_super     mutex_lock(&sbi->s_lock);
  fs/ext4/orphan.c:88    -                            mutex_lock(&orphan_lock);
  ```
- A place outside any symbol shows `-`; a file codix does not know, or a line
  past its end, gets an error on its own line, not a failed command.
- The files named are rechecked first, as lookups do, so the answer matches
  the file as it is.
- JSON and MCP: `places` in, one item per place out: the place, the text if
  any, and the enclosing symbol or null.

### Removing `grep`

- The command, its MCP tool, its operation, its schemas, `docs/commands.md`
  and the README section go. The paging tests that used `grep` move to `refs`
  and `find`, and the tests of `watch`, daemon parity, errors and MCP to
  other commands.
- The capability tier `search only` becomes `text`: files codix keeps track
  of without symbols.
- The schema test lets a command's snapshots go before a release, as it lets
  them change.

### Output

- `status` summarizes modules (`29 modules, 2 build systems`); `--modules`
  lists them.
- `outline --depth N`, and `--kind`, repeatable.
- `capabilities` counts files as `status` does: indexed, search only,
  skipped.
- "no results" names what found nothing and suggests the next step:
  `no symbol named "Foo" in spring-context (--path); find Foo looks
  everywhere`.
- Help examples quote patterns a shell would expand (`'Invoice.*'`).
- Suggested commands start with `codix` when the `codix` on `PATH` is the
  running binary, and with the path as invoked otherwise.

### Contract

v1 changes in place (pre-release): `grep` goes and `at` comes; ambiguity
candidates become objects, symbols gain `declaration`, apply responses can
carry `references_left`, requests gain `exclude_path`, `sources`, `context`,
`sort`, `depth`. The index
schema changes too: indexes are rebuilt.

## Steps

One branch, one commit each, the full local suite after each:

1. **Ambiguity**: richer candidates; `show` takes overloads; `def` lists every
   match (a).
2. **Declarations and duplicate definitions**: `declaration` in the index,
   declarations step aside (b); the `#else` definition kept with its own ID;
   `def file:line:col` resolves the place.
3. **Languages**: the `*.h` heuristic; `languages` shows rules and
   search-only languages.
4. **Narrowing**: `--exclude-path`, `--sources`, `refs --context`,
   `--files --sort count`.
5. **Rename, apply, undo**: single listing, `references_left`, per-site undo
   hunks, undo and redo messages.
6. **`codix at` in, `grep` out**: the command and its MCP tool, standard
   input in the forms tools print, then the removal and the tests moved.
7. **Output**: `status`, `outline --depth`/`--kind`, `capabilities` counts,
   "no results" messages, quoted examples, `codix` in suggested commands.
8. **The agents again**: the same two scenarios, logged the same way, to
   compare calls, failures and time.

README, `docs/commands.md` and the MCP descriptions follow each step.
