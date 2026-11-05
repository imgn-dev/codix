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

List every language codix knows, with its tier and file names.

- **When:** To check how a kind of file is handled, or whether this build includes a language.
- **Returns:** Each language with its tier, extensions, file names, and whether it is in this build.

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
- **Returns:** Matching symbols with kind, location and ID, best match first.
- **Needs:** tier 1 (symbols)

```
codix find Invoice --kind class
```

## `codix def`

Show where a symbol is defined.

- **When:** To go from a name, a symbol ID or a `file:line:col` to its definition.
- **Returns:** The definition's location, signature and ID; the candidates when the name is ambiguous.
- **Needs:** tier 1 (symbols)

```
codix def Invoice.total
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
- **Returns:** The symbol's lines, with --context lines around them.
- **Needs:** tier 1 (symbols)

```
codix show Invoice.total --context 2
```

## `codix grep`

Search file contents with a regular expression; each match names its enclosing symbol.

- **When:** For text that is not a symbol name, such as strings, comments or call sites. For names, prefer find.
- **Returns:** Matching lines with location and enclosing symbol.
- **Needs:** any indexed text file

```
codix grep "TODO|FIXME" --lang java
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
- **Returns:** MCP tool results: the text the CLI prints, and its JSON as structured content.

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
