# codix

A headless code index for LLM agents and the people working with them:
IDE-grade navigation and safe renames from the command line, without an IDE.
codix indexes any directory, stays current incrementally as files change, and
answers structural questions directly: where a symbol is defined, what a file
contains, what refers to a name and inside which symbol.

This is phase 3: scopes (tier 2) for Java, Go, TypeScript, Python, Kotlin,
JavaScript, Rust, C, C++, C#, PHP and Ruby, so references are bound to the
definition they mean within a file, plus keys and headings of JSON, YAML, TOML,
XML, Markdown and SQL for search; a resolver for Java that binds references
across files; renames, across files where they are bound, always applied from a
reviewed preview, with undo; a per-project daemon that answers in milliseconds
and watches files; and `codix mcp`, the same commands as tools for agents.

## Install

codix is one binary. To build it you need the Rust toolchain (rustup reads the
version from `rust-toolchain.toml`) and a C compiler for the grammars:

```
cargo build --release -p codix-cli --features full
cp target/release/codix ~/.local/bin/
```

`--features full` includes every provider. The default build has Java, Go,
Maven, Gradle and git; `minimal` has none, and keeps every file as text.
`codix capabilities` lists what a build supports. The modular flavour instead
loads providers as plugins (see Plugins).

## Plugins

codix comes in two flavours:

- **Monolithic**: every provider compiled in (`--features full`, the default
  build, or `minimal`), and no plugins. For static Linux binaries, CI and
  sandboxes.
- **Modular**: the core with git only, and every language, build system and
  other version control system as a plugin, installed by a package manager.
  codix never downloads anything.

```
cargo build --release -p codix-cli --no-default-features --features plugins,vcs-git
cargo xtask dist dist/      # dist/bin/codix and every plugin in dist/lib/codix/plugins
```

The modular flavour finds plugins in, in order:

1. each directory of `CODIX_PLUGIN_PATH`;
2. `$XDG_DATA_HOME/codix/plugins` (by default `~/.local/share/codix/plugins`;
   `%LOCALAPPDATA%\codix\plugins` on Windows);
3. `lib/codix/plugins` beside the binary's directory, as invoked and with
   symlinks resolved, where a package manager installs them.

Never in the project: a repository cannot bring native code. The modular
flavour needs a C library that can load plugins: on Linux, glibc, at least the
version of the system it was built on (2.39 for the release builds, made on
Ubuntu 24.04); static musl builds are monolithic. A plugin is a
directory `<kind>/<name>/`; `cargo xtask plugins <out>` builds them.

A file of a known language whose plugin is not installed is indexed as text,
and `index` and `status` say which plugin to install, with the command the
packager set at build time in `CODIX_INSTALL_HINT` (such as
`brew install codix-{kind}-{name}`, where `{kind}` is `lang`, `build` or
`vcs`, and `{name}` the package: TSX ships with TypeScript, as
`codix-lang-typescript`). `codix languages` lists every provider,
where it comes from, its version, and why a plugin is not used: invalid, or
left out because a compiled-in provider or an earlier plugin of the same name
wins. Writing a plugin is described in [docs/plugins.md](docs/plugins.md).

## Workflow

Create the index once, at the project root:

```
$ codix init
initialized codix in /work/shop
version control: git
indexed: 412 parsed, 0 touched, 0 deleted, 0 unchanged
```

`init` creates `.codix/` and keeps it out of version control without editing
your ignore files (for git, `.codix/.gitignore` ignores the directory itself).
Commands work from any directory inside the project and show paths relative to
where they run.

Find a symbol, then look at it:

```
$ codix find total --kind method
src/billing/Cart.java:4:10     method  Cart.total     long total()
src/billing/Invoice.java:6:17  method  Invoice.total  public long total()
2 results

$ codix outline src/billing/Invoice.java
src/billing/Invoice.java
  3:14  class  public class Invoice
    4:18  field  private long total
    6:17  method  public long total()

$ codix show Invoice.total --kind method --context 1
src/billing/Invoice.java:6:17  method  Invoice.total
5 |
6 |     public long total() {
7 |         return total;
8 |     }
9 | }
```

- `find` matches exactly, then by prefix, then fuzzily, and says which matched;
  a misspelled name gives the symbols of the closest name, and the other close
  names with the command for each. `--exact`, `--prefix`, `--fuzzy`, `--glob`
  and `-E`/`--regex` choose one; the last two match qualified names, such as
  `'Invoice.*'` and `'^Invoice\.get'`. Every mode ignores case;
  `-s`/`--case-sensitive` matches it. `--kind`, `--lang`, `--path` and `--here` narrow.
- `def` gives the definition, or every symbol of a name that has several.
  `show`, `refs` and `def` answer for every symbol a name matches, listing
  them first; `--kind` and `--path` narrow, and `rename`, which edits, asks
  which one.
  Results carry symbol IDs, such as
  `java:src/billing/Invoice.java::Invoice.total#381c`; every command accepts an
  ID, its path part without the hash, `file:line:col`, or a name.
- `refs` lists what refers to a symbol or a local variable, definition first:
  `scoped` when its file's scopes bind it there, `bound` when a resolver binds
  it across files, `textual` when it matches by name only, in another file or
  through another object; `--confidence scoped|bound|textual` keeps one of
  them. Java has a resolver: imports, the package, supertypes and the declared
  types of locals, parameters and fields bind names and calls across files;
  what needs type inference (`var`, lambda parameters, chained calls) stays
  textual. `def file:line:col` follows the
  name at that place to its definition. In C, a function pointer member is a
  field, and a designated initializer such as `.read = ext4_read` writes it:
  `refs file_operations.read --context write` lists the tables filling it.
- `types BeanPostProcessor` lists what extends or implements a type,
  transitively, and `--super` its supertypes up to the roots, naming those
  outside the project; `--depth N` stops after N levels. Each relation says
  how it was found: `scoped` or `bound` when the name written binds to the
  type, `textual` when only the name matches.
- `at <path:line>…` names the symbol around each place, as other tools print
  them: `rg -n 'mutex_lock\(' fs/ext4 | codix at -` annotates each match with
  the function holding it. codix does not search text; your own grep does it
  better.
- `index` brings the index up to date; `status` shows what changed since, how
  many build modules Maven and Gradle files declare (`--modules` lists them),
  and anything codix could not read in them.
- `outline` shows a file's or directory's symbols; `--depth 1` keeps the top
  level, `--kind` the kinds asked for and what holds them.

Queries keep the index fresh themselves: lookups recheck the files they
return, and `refs` scans for changes first. Results are capped at 50 (`--limit`); the
last line is the command for the next page, ready to copy, which starts after
the last result shown (`--after`), so results fixed meanwhile shift nothing.
When a first page does not hold every result, it ends with where they are,
by directory, each with the command that keeps only that one. `--count` gives
how many results there are and where, `--files` one line per file with its
number of results (the worklist of a refactoring, paged by file; `--sort
count` for the files with the most first), and `--all` every result, for
scripts. Every lookup narrows with `--path`, `--here`, `--exclude-path` and
`--sources main|test` (from the build systems' source roots); `refs` also with
`--context call|type|import|read|write`.

Every command takes `--json` for stable, versioned output (`schema_version`).
Errors have a stable code, a cause and a hint:

```
error[ambiguous_symbol]: "total" matches 3 symbols
  java:src/billing/Cart.java::Cart.total#bda5
  java:src/billing/Invoice.java::Invoice.total#381c
  java:src/billing/Invoice.java::Invoice.total#b8b8
hint: pass a full symbol ID, or narrow with --kind or --path
```

`codix help <command>` describes any command: what it does, when to prefer it
over grep, what it returns. `codix capabilities` lists what each language
supports here, and `codix languages` how each kind of file is recognized. The
same descriptions are in [docs/commands.md](docs/commands.md).

## Changing code

Edits are planned first and applied second. `rename` returns a preview with an
ID, a summary and a diff, and writes nothing:

```
$ codix rename src/Invoice.java:12:14 net
preview 3f9a2c41: rename variable base at src/Invoice.java:12:14 to net
  1 file, 2 sites
--- a/src/Invoice.java
+++ b/src/Invoice.java
@@ -9,8 +9,8 @@
     }
 
     long withTax(long tax) {
-        long base = total();
-        return base + tax;
+        long net = total();
+        return net + tax;
     }
 
     long total(long discount) {
apply with: codix apply 3f9a2c41

$ codix apply 3f9a2c41
applied 3f9a2c41: rename variable base at src/Invoice.java:12:14 to net
  1 file, 2 sites
    src/Invoice.java
undo with: codix undo
```

- A rename changes the definition, the references its file's scopes bind to
  it and those a resolver binds to it in other files, and lists the other mentions of the name unchanged: in its file's
  comments and strings, and in other files, their comments and strings
  (Javadoc's `{@link Type#name}`) and text files (XML, properties).
  A C or C++ function's prototypes, `declaration` symbols, are renamed with it,
  in its file and in headers.
- It is refused, with a typed error and the sites, when the new name is not a
  valid name or collides with another, when it would change what a reference
  means (shadowing), when a site is in a syntax error, when the file is
  generated or vendored, and when other files or other objects may refer to
  the symbol without codix binding them; every file edited is checked. One condition at a time
  can be allowed: `--allow shadowing`, `--allow overload`,
  `--allow visibility-change`, `--allow generated`, and `--exclude-unresolved`
  to edit only the sites codix binds. A public Java class is named after its
  file, so renaming one is refused.
- `apply` writes through a journal: a file changed since planning outside the
  edited lines is rebased, and one changed inside them is refused as stale. A
  crash mid-apply is rolled back by the next command that writes the index.
- `undo` returns a preview reverting the latest apply, `previews` lists the
  pending ones and `discard` drops them; they expire after `previews.expiry`.
- A symbol's old ID keeps working after codix renames it: queries follow it to
  the new one and say so, and edits refuse it.

## Daemon

Without a daemon, each command opens the index itself. A daemon keeps the index
of one project open and current, so queries answer in milliseconds:

```
codix daemon start        # this project's daemon, in the background
codix daemon list         # every running daemon: project, memory, idle time
codix daemon stop [--all] # this project's daemon, or every one
codix watch               # the daemon in the foreground, logging to stderr
```

Commands use the project's daemon when one runs, with the same output. The
daemon watches the project's files and indexes each change within a moment of
the save; a full rescan every `index.scan_interval` catches what the watcher
missed. It stops by itself after `daemon.idle_timeout` without a request.
`status` names the running daemon and what its watcher covers.

## Agents

`codix mcp` serves the commands as MCP tools on stdin and stdout, with the same
names, descriptions and output as the CLI. For Claude Code:

```
claude mcp add codix -- codix mcp
```

Each call can name its directory (`cwd`), so one server covers every project
an agent works in; the server starts each project's daemon on its first call.
`codix init --agent-docs` writes `CODIX.md`, a short workflow guide to add to an
agent's instructions. `eval/` holds the benchmark that checks agents use codix
well with nothing but these descriptions.

## Configuration

`codix config` reads and edits both configuration files, as `git config` does:
`codix.toml`, committed, holds facts about the code; `.codix/config.toml`, not
committed, holds personal preferences. Each key belongs to one file.

```
codix config <key>                  # the effective value
codix config <key> <value>          # set it in the key's own file
codix config --unset <key>          # back to the default
codix config --list [--show-origin] # every value and where it comes from
codix config --describe [<key>]     # meaning, type, default and file
```

| Key | File | Default | |
| --- | --- | --- | --- |
| `index.max_file_size` | `codix.toml` | `2MB` | Larger files are skipped |
| `languages.overrides."<glob>"` | `codix.toml` | | Language of matching files |
| `sources.roots` | `codix.toml` | | Source directories, overriding the build files |
| `index.scan_interval` | `.codix/config.toml` | `10m` | Full rescan interval of a daemon |
| `daemon.idle_timeout` | `.codix/config.toml` | `30m` | A daemon stops after this long without a request |

Edits keep comments and ordering. `.codixignore`, in gitignore syntax, excludes
files from the index; the ignore files of the version control systems in use
apply too.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | Success with results |
| 1 | Success, no results |
| 2 | Usage error: bad flag, bad value, unknown symbol syntax |
| 3 | Ambiguous input; candidates listed |
| 4 | Refused: a daemon already serves the project, a file codix did not write is in the way |
| 5 | Internal or I/O error |
| 6 | No project found, or project not indexed |

## Development

```
cargo test                     # the default build
cargo test -p codix-cli --features full
cargo clippy --all-targets -- -D warnings
cargo xtask plugins target/plugins --debug    # every plugin, quick to build
CODIX_PLUGIN_PATH=$PWD/target/plugins \
  cargo test -p codix-cli --no-default-features --features plugins,vcs-git
cargo bench -p codix-benches   # performance targets on pinned repositories
cargo run -p codix-eval --release  # the LLM benchmark, through Claude Code
```

Run the two benchmarks from a terminal: inside a Claude Code session, the
sandbox lets no daemon bind its socket and `claude` reach its login. Tests that
need a real socket or file watcher are named `live_*`; where those are not
available, run the suite with `-- --skip live_`.

Language packs keep their expected symbols in `corpus/<language>`; run the
tests with `CODIX_BLESS=1` to rewrite expectations and golden outputs after an
intended change. The design is in `docs/`.

## License

Apache License 2.0.
