# codix

A headless code index for LLM agents and the people working with them:
IDE-grade navigation from the command line, without an IDE. codix indexes any
directory, stays current incrementally as files change, and answers structural
questions directly: where a symbol is defined, what a file contains, where a
name appears and inside which symbol.

This is phase 1: tier 1 symbols (definitions, outlines, name search) for Java,
Go, TypeScript, Python, Kotlin, JavaScript, Rust, C, C++, C#, PHP and Ruby, plus
keys and headings of JSON, YAML, TOML, XML, Markdown and SQL for search.

## Install

codix is one binary. To build it you need the Rust toolchain (rustup reads the
version from `rust-toolchain.toml`) and a C compiler for the grammars:

```
cargo build --release -p codix-cli --features full
cp target/release/codix ~/.local/bin/
```

`--features full` includes every provider. The default build has Java, Go,
Maven, Gradle and git; `minimal` has none, for plain-text search only.
`codix capabilities` lists what a build supports.

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
  `--exact`, `--prefix`, `--fuzzy` and `--glob` (on qualified names, such as
  `'Invoice.*'`) choose one. `--kind`, `--lang`, `--path` and `--here` narrow.
- `def` gives one definition; `--kind` and `--path` narrow an ambiguous name.
  Results carry symbol IDs, such as
  `java:src/billing/Invoice.java::Invoice.total#381c`; every command accepts an
  ID, its path part without the hash, `file:line:col`, or a name.
- `grep <regex>` searches the indexed files and names the symbol around each
  match; `--kind method` keeps matches inside methods.
- `index` brings the index up to date; `status` shows what changed since, the
  build modules found in Maven and Gradle files, and anything codix could not
  read in them.

Queries keep the index fresh themselves: lookups recheck the files they return,
and `grep` scans for changes first. Results are capped at 50; `--limit` and
`--offset` page through the rest.

Every command takes `--json` for stable, versioned output (`schema_version`).
Errors have a stable code, a cause and a hint:

```
error[ambiguous_symbol]: "total" matches 3 symbols
  java:src/billing/Cart.java::Cart.total#bda5
  java:src/billing/Invoice.java::Invoice.total#381c
  java:src/billing/Invoice.java::Invoice.total#b8b8
hint: pass a full symbol ID, or narrow with --kind or --path
```

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
| 4 | Edit refused (from phase 3) |
| 5 | Internal or I/O error |
| 6 | No project found, or project not indexed |

## Development

```
cargo test                     # the default build
cargo test -p codix-cli --features full
cargo clippy --all-targets -- -D warnings
cargo bench -p codix-benches   # performance targets on pinned repositories
```

Language packs keep their expected symbols in `corpus/<language>`; run the
tests with `CODIX_BLESS=1` to rewrite expectations and golden outputs after an
intended change. The design is in `docs/`.

## License

Apache License 2.0.
