# codix commands

Generated from `crates/codix-core/src/describe.rs`, the descriptions `--help` and MCP show; `CODIX_BLESS=1 cargo test -p codix-cli --test describe` rewrites it.

## `codix init`

Create a codix project in this directory and index it.

- **When:** Once per project, before any other command. Running it again rescans.
- **Returns:** The project root, the version control systems found, and what the first index found.

```
codix init
```

## `codix index`

Bring the index up to date with the files on disk.

- **When:** Rarely needed: queries recheck the files they return, and a daemon watches files. --full rebuilds from nothing.
- **Returns:** How many files were parsed, touched, deleted and unchanged.

```
codix index
```

## `codix status`

Show the state of the index, like `git status`.

- **When:** To check that the index is current, which build modules were found, and whether a daemon runs.
- **Returns:** File counts, changes since the last scan, build modules, and the daemon.

```
codix status
```

## `codix capabilities`

Show what codix supports here: commands, languages and their tiers, providers.

- **When:** Before relying on a command for a language: tier 1 means definitions, outlines and name search.
- **Returns:** Each command with the tier it needs, each language with its tier and, in a project, its file count, and the index state.

```
codix capabilities --json
```

## `codix languages`

List every language codix knows, with its tier and file names, and every provider.

- **When:** To check how a kind of file is handled, or whether a language, build system or version control system is available.
- **Returns:** Each language with its tier, extensions, file names, and whether it is in this build; each provider with its version, where it comes from, and whether it is used.

```
codix languages
```

## `codix config`

Read or change settings in codix.toml and .codix/config.toml, like `git config`.

- **When:** Instead of editing the files: values are checked, and each key goes to its own file.
- **Returns:** The value; for a change, what it affects; with --list, every value.

```
codix config index.max_file_size 4MB
```

## `codix find`

Find symbols by name: exact match, then prefix, then fuzzy.

- **When:** Instead of grep to locate a class, function or method: it matches names, not text, and returns IDs the other commands accept.
- **Returns:** Matching symbols with kind, location and ID; for a misspelled name, the symbols of the closest name and the other close names.
- **Needs:** tier 1 (symbols)

```
codix find Invoice --kind class
```

## `codix def`

Show where a symbol is defined.

- **When:** To go from a name, a symbol ID or a `file:line:col` to its definition.
- **Returns:** The definition's location, signature and ID; for a name with several symbols, every one of them.
- **Needs:** tier 1 (symbols)

```
codix def Invoice.total
```

## `codix refs`

List every reference to a symbol or local variable, definition first.

- **When:** Instead of grep to find who uses a name: references are bound by scope, so a different variable with the same name is not one of them.
- **Returns:** Each site with its context (call, type, read, write), enclosing symbol and target; `scoped` when bound by scope, `textual` when matched by name only. A name with several symbols covers them all.
- **Needs:** tier 1 (symbols)

```
codix refs Invoice.total
```

## `codix outline`

Show the symbols of a file or directory as a tree, signatures only.

- **When:** Instead of reading a whole file to learn its structure.
- **Returns:** Nested symbols with kind, signature and line.
- **Needs:** tier 1 (symbols)

```
codix outline src/billing/Invoice.java
```

## `codix show`

Print the source of a symbol.

- **When:** Instead of reading a file to see one class, method or function.
- **Returns:** The symbol's lines, with --context lines around them; for a name with several symbols, the list, then each one's lines, at most --limit.
- **Needs:** tier 1 (symbols)

```
codix show Invoice.total --context 2
```

## `codix at`

Name the symbol around each place: a file and line, as other tools print them.

- **When:** To know which function or class holds a line, such as each match of your own grep: `rg -n pattern | codix at -`.
- **Returns:** For each place, the innermost enclosing symbol, its kind and qualified name, then any text that followed the place.
- **Needs:** tier 1 (symbols)

```
codix at src/billing/Invoice.java:12
```

## `codix rename`

Plan renaming a symbol or local variable within its file; returns a preview to apply.

- **When:** Instead of editing by hand or with sed: only the definition and the references bound to it change, and the plan is checked for collisions, shadowing and unresolved references first.
- **Returns:** A preview: its ID, the sites, any override used, the sites left unchanged, the diff, and the `apply` command to run. Nothing is written before `apply`.
- **Needs:** tier 2 (scopes)

```
codix rename Invoice.total sum
```

## `codix apply`

Apply a preview: write its edits to the files.

- **When:** After reviewing a preview from `rename`. Files changed since planning are handled: edits are rebased when their regions are intact, refused with stale_preview when not.
- **Returns:** The files written and the sites edited; `undo` reverts it.

```
codix apply 3f9a2c41
```

## `codix undo`

Plan reverting the latest apply not undone yet; returns a preview to apply.

- **When:** To take back an apply. Running it again goes one apply further back, up to the last 20.
- **Returns:** A preview restoring the files as they were, refused if they changed since; apply it with `apply`.

```
codix undo
```

## `codix previews`

List the pending previews that `apply` can apply.

- **When:** To find a preview ID again, or check whether a preview still applies after files changed.
- **Returns:** Each preview with its ID, what it does, its age and its state: ready, rebase (files changed elsewhere; apply adapts it), stale or expired (removed now; plan again).

```
codix previews
```

## `codix discard`

Drop pending previews without applying them.

- **When:** When a preview will not be applied, so it no longer shows in `previews`.
- **Returns:** The IDs dropped.

```
codix discard 3f9a2c41
```

## `codix watch`

Run this project's daemon in the foreground, logging to stderr, until interrupted.

- **When:** To see what the daemon indexes as files change; `daemon start` runs it in the background.
- **Returns:** Log lines on stderr.

```
codix watch
```

## `codix mcp`

Serve codix's commands as MCP tools on stdin and stdout.

- **When:** To give an agent codix as tools: add `codix mcp` as an MCP server. Each call can name its directory, so one server covers several projects.
- **Returns:** MCP tool results with the text the CLI prints; `--json` on the CLI gives the JSON.

```
codix mcp
```

## `codix daemon`

Start, stop or list per-project daemons, which keep the index open and current.

- **When:** For a long session in a project: queries through a daemon answer in milliseconds.
- **Returns:** The daemons started, stopped or running.

```
codix daemon start
```

## `codix daemon start`

Start this project's daemon in the background.

- **When:** Once per session; it stops by itself after 30 minutes without requests.
- **Returns:** The daemon's process ID and socket.

```
codix daemon start
```

## `codix daemon stop`

Stop this project's daemon, or every daemon with --all.

- **When:** When done with a project, or before upgrading codix.
- **Returns:** The daemons stopped.

```
codix daemon stop --all
```

## `codix daemon list`

List every running daemon of this user.

- **When:** To see which projects have a daemon, and their memory and idle time.
- **Returns:** Each daemon's project, process ID, memory, idle time and version.

```
codix daemon list
```
