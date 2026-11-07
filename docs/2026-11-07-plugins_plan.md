# codix — Plugins Step-by-Step Plan

Step-by-step plan to split codix into a core application and plugins for
languages, their tier 3 resolvers, build systems and version control systems,
loaded at run time. It comes before phase 4 (see Order with phase 4). Each task
lands as one reviewed change, on its own branch, with its tests,
`cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` and the full
test suite green on the three CI platforms.

## Goal

- **One small core.** The `full` build is 38.8 MB, and compiled grammars are
  about 26 MB of it (C# 5.2, Kotlin 3.4, C++ 3.4, TypeScript 2.8, SQL 2.4,
  PHP 2.1, Ruby 2.1, Rust 1.1, the rest under 1 MB each). A user who works in
  Java should not carry C#.
- **codix does not download anything.** Homebrew, or whichever package manager
  ships codix, installs plugins. codix finds what is installed and loads it
  when a project needs it.
- **Every known language is still recognized without its plugin.** Its files
  are indexed as text, and codix says which plugin to install:
  `kotlin: 214 files indexed as text; plugin not installed (brew install codix-lang-kotlin)`.
- **Language logic lives in its plugin**, tier 3 resolution included: the
  core knows no language, build system or version control system but git.
- **Others can add providers** without changing or rebuilding codix: a
  tree-sitter grammar, a descriptor and queries make a language, an executable
  adds its resolver; a descriptor and an executable make a build system; a
  descriptor makes a version control system.
- **The monolithic build stays**, for static Linux binaries, CI and sandboxes.

## Decisions taken

On 2026-11-07:

- **(a) `unsafe` in one crate.** `codix-plugin` is exempt from
  `unsafe_code = "forbid"`, for its native grammar loader only (see Choices).
  Every other crate keeps `forbid`, and the monolithic build does not link
  `codix-plugin`.
- **(b) Two build flavours.**
  - *Modular*: the core, plus plugins. For Homebrew on macOS and Linux (glibc:
    a static musl binary cannot `dlopen`), and Windows.
  - *Monolithic*: today's `full`, `default` and `minimal` features, everything
    compiled in, no loader. For the static Linux binaries, CI and sandboxes.
  - CI tests both; the release workflow builds both.
- **(c) The modular core keeps git only.** Mercurial, Subversion, Maven and
  Gradle become plugins, like every language.
- **(d) Tier 3 resolvers are plugins**, shipped inside their language's
  plugin: Java's and Go's in phase 4, others later. The mechanism is below
  (Resolver plugins).

## What a provider needs

Checked on 2026-11-07 in the sources at `ab36048`, and for resolvers in the
phase 4 section of the implementation plan.

| Kind | Content | Code |
| --- | --- | --- |
| VCS (git, mercurial, subversion) | `descriptor.toml` | none: `register!(Vcs, "git", "descriptor.toml")` |
| Language (18 packs) | `descriptor.toml`, `queries/tags.scm`, `queries/locals.scm`, a grammar | the grammar only, C compiled by its crate; the pack's `lib.rs` is one `register!` call |
| Build (maven, gradle) | `descriptor.toml` | `BuildResolver::discover(&Path) -> BuildModel`, about 500 lines of Rust each; `BuildModel` is already `Serialize` and `JsonSchema` |
| Resolver (phase 4) | none | real logic, and lots of it: imports, packages and modules, overloads, overrides, generics, method sets. It reads the whole index and writes resolved targets, imports and relations |

The plan gives each kind the lightest mechanism that carries it:

| Kind | Plugin | Loaded by |
| --- | --- | --- |
| VCS | a directory with `plugin.toml` and `descriptor.toml` | reading the files |
| Language | the same, plus `queries/` and the grammar as a shared library | reading the files; `dlopen` of the library on the first file of the language |
| Build | the same, plus an executable | running it: `discover` writes the `BuildModel` as JSON on stdout |
| Resolver | an executable inside a language plugin | running it after a scan: it reads the index through read-only views and writes its results into a scratch database, which the core merges |

## Resolver plugins

A resolver holds the most code of any provider, needs the whole index, and
runs on every scan that changes a file of its language. Options considered:

| Option | Verdict |
| --- | --- |
| A Rust `cdylib` loaded in process, implementing a resolver trait | Rejected. Rust has no stable ABI: the plugin would have to be built by the same compiler against the same crate versions, which is checkable but brittle, and the process would hold two copies of every shared crate, with their own statics. A panic or a bug in the most complex code of codix would take down the daemon |
| A C ABI shared library (the interface written as `extern "C"`) | Rejected. Sound, but every query the resolver makes (symbols by name, members of a type, supertypes) becomes hand-written `unsafe` FFI in both directions, outside the one-module exception of Decision (a) |
| WebAssembly, with the index API as host functions | Rejected for now: the size and speed cost of Wasmtime (see Choices), for the same isolation a process gives |
| A process asking the core for each lookup over a pipe | Rejected. Resolving one Java file takes hundreds of lookups; spring-framework has thousands of Java files: millions of round trips per cold index |
| **A process reading the index itself, read-only, and writing results into a scratch database** | **Chosen.** Lookups are local SQLite reads, as fast as the core's own. The resolver never writes the index: the core merges its results in one transaction and keeps the write lock, the journal and the schema to itself. A crash or a hang only leaves that language at tier 2 for the scan |

How it works:

- **Contract.** The resolver sees the index through SQL views with a version
  in their name (`v1_files`, `v1_symbols`, `v1_refs`, `v1_modules`, and in
  phase 4 `v1_imports` and `v1_relations`), never the tables. The views are
  the stable interface: the core may change its tables and keep the views.
- **Results.** The scratch database has a fixed schema, versioned with the
  protocol: in this plan, a target and state for each reference; phase 4 adds
  imports, relations and synthesized symbols (record accessors). The core
  `ATTACH`es it, checks it, and merges it with `INSERT … SELECT`.
- **Protocol.** Requests and answers are JSON lines on stdin and stdout:
  `hello` (protocol and view versions, refused on mismatch), `resolve` (the
  index path, the scratch path, the changed files; the answer names the files
  that depend on them, for the reverse dependency map), and `shutdown`. Phase
  4 adds the edit-time requests its pre-flight checks need, such as the
  bindings visible at a position.
- **Parsing.** The resolver parses with its own copy of its grammar, linked
  in. It ships in the same plugin as the grammar library, built from the same
  grammar crate in the same `Cargo.lock`, and the fingerprint covers both, so
  their trees cannot disagree.
- **Lifetime.** The CLI starts a resolver for a scan and stops it after. The
  daemon keeps it running between scans, restarts it after a crash (at most
  three times in a row, then the language stays at tier 2 with a warning),
  and stops it with itself.
- **One implementation for both flavours.** A resolver is a library crate
  implementing a `Resolver` trait over the views, plus a small `[[bin]]` that
  speaks the protocol. The monolithic build calls the library in process,
  through the same views and scratch schema, so both flavours run the same
  code and the equivalence tests compare them.
- **For authors.** A new crate, `codix-resolver`, holds the protocol types,
  the view and scratch schemas, and the `Resolver` trait with the `[[bin]]`
  main loop. Resolvers depend on it, not on `codix-core`.

## Choices made before the plugin work

Checked on 2026-11-07 against the pinned crates and crates.io.

| Question | Choice | Evidence |
| --- | --- | --- |
| Language plugin format | A native shared library of the grammar's C code, exporting tree-sitter's own `tree_sitter_<name>` function: what `tree-sitter build` produces | No Rust in the library, so no Rust ABI to keep stable: the tree-sitter runtime stays in the core, and the library holds parse tables. tree-sitter checks the grammar's ABI itself: `Parser::set_language` fails outside `MIN_COMPATIBLE_LANGUAGE_VERSION..=LANGUAGE_VERSION` (13..=15 in tree-sitter 0.27.1). Any grammar built by the tree-sitter CLI works as a plugin |
| Loading it | `libloading` 0.9.0, in `codix-plugin`; the library is never unloaded | `Library::new`, `Library::get` and `LanguageFn::from_raw` (tree-sitter-language 0.1.9) are `unsafe`: Decision (a). A `Language` points into the library, so it must outlive every parser |
| The `unsafe` exception | `codix-plugin` repeats the workspace lint table without `unsafe_code`, and adds `clippy::undocumented_unsafe_blocks` and `unsafe_op_in_unsafe_fn` as errors | Cargo does not allow `[lints] workspace = true` with one lint changed. Each `unsafe` block then states why it is sound |
| Grammar plugins in WebAssembly instead | Rejected | tree-sitter 0.27.1 has a `wasm` feature with a safe `WasmStore::load_language(name, bytes)`: no `unsafe`, one artifact for every platform, and it works in static musl binaries. But it links Wasmtime 48 with Cranelift (`wasmtime-c-api-impl`), a runtime expected to weigh several MB in the core, and WebAssembly grammars parse slower |
| Build plugin format | An executable, run as `<exe> discover --protocol 1 <root>`, answering a `BuildModel` as JSON on stdout | No `unsafe`, no ABI, and a crash or a hang stays outside codix (timeout 30 s). Discovery runs once per scan and again when a build file changes (`scan.rs`), so a process start costs nothing that matters. A build plugin can be written in any language, Java included |
| Resolver plugin format | An executable reading the index through views and writing a scratch database | See Resolver plugins |
| Building language plugins | A workspace tool, `xtask`, compiles each grammar crate's `src/parser.c` and `src/scanner.c` into a shared library with the `cc` crate's compiler detection | No new tool on CI (the tree-sitter CLI would be one). The grammar sources come from the crates already pinned in `Cargo.lock`, so a plugin and the monolithic build parse with the same grammar |
| Plugin and core versions | `plugin.toml` carries `schema = 1`, and a resolver also its protocol and view versions. The core refuses a mismatch with `plugin_invalid`; the plugin's own version is informative | A few integers are the whole contract: each changes only when its part changes incompatibly |
| Fingerprint of a plugin | BLAKE3 of the library file, the resolver executable, the queries and the descriptor | `grammar_fingerprint` today asks the loaded `Language` for its ABI and counts, which would load every installed grammar at startup. Hashing the files needs no `dlopen`, and installing or updating a plugin still reparses and re-resolves its files on the next scan |
| Where plugins are found | In order: each directory of `CODIX_PLUGIN_PATH`; `$XDG_DATA_HOME/codix/plugins` (or `~/.local/share/…`, `%LOCALAPPDATA%\codix\plugins` on Windows); `<dir of the binary>/../lib/codix/plugins`, both as invoked and with symlinks resolved. Never the project, never `codix.toml` | A plugin is native code: a repository must not be able to plant one. Homebrew links each plugin formula's `lib/codix/plugins/<kind>/<name>/` into `$(brew --prefix)/lib/codix/plugins`, next to the linked `bin/codix`, so no configuration is needed |
| Conflicts | A compiled-in provider wins over a plugin of the same name; between plugins, the first in search order wins. `codix languages` lists the ones left out, and why | Predictable, and the monolithic build behaves as today whatever is installed |
| The install hint | A build-time template, `CODIX_INSTALL_HINT` (for example `brew install codix-{kind}-{name}`), read with `option_env!`; without it, a link to the README's plugin section | The core stays independent of the package manager; each packager sets its own command |
| Error codes | `not_supported_in_build` stays, with the install hint; new `plugin_invalid` (schema or protocol, missing symbol, grammar ABI, query that does not compile, build or resolver plugin that fails or answers badly) | Agents already handle `not_supported_in_build`; a missing plugin is the same situation for them |

### Crate versions

New: `libloading` 0.9.0 in `codix-plugin`, and `cc` (current release, checked
when P.8 starts) in `xtask`. `codix-resolver` uses `rusqlite` and `serde_json`,
already in the workspace. If a task turns out to need another crate, it is
added then, at its latest release, and this table gains a row.

### Sandbox

Loading a library, starting a process and opening the index read-only from a
second process are allowed in the sandbox; P.1 confirms all three. Tests
through a daemon keep the `live_` prefix and run on CI.

## New crates

- `codix-plugin`: plugin discovery, `plugin.toml`, the native grammar loader,
  and the clients for build and resolver plugins. Only the modular flavour
  links it, through the `plugins` feature of `codix-cli`.
- `codix-resolver`: what a resolver needs (Resolver plugins). Linked by the
  core, for the views and the merge, and by each resolver.
- `xtask` (not published, not linked): `cargo xtask plugins` builds the
  plugin directories, for packaging and for tests.

Existing crates change role but not shape. A language pack crate still
registers itself for the monolithic build, and its descriptor and queries are
also what `xtask` copies into the plugin. The Maven and Gradle crates gain a
small `[[bin]]` for the plugin form. Phase 4's resolvers are new crates in the
same shape: a library and a `[[bin]]`.

## Plugin layout

```
lib/codix/plugins/
  language/java/
    plugin.toml          # schema = 1, kind, name, version, library, symbol,
                         # resolver (protocol, views)
    descriptor.toml      # as in crates/codix-lang-java
    queries/tags.scm
    queries/locals.scm
    java.dylib           # .so on Linux, .dll on Windows
    bin/codix-resolve-java
  build/maven/
    plugin.toml          # schema = 1, kind, name, version, executable
    descriptor.toml
    bin/codix-build-maven
  vcs/mercurial/
    plugin.toml
    descriptor.toml
```

A language plugin may hold several grammars (TypeScript ships `typescript`
and `tsx`); `plugin.toml` then lists one library and symbol per language. The
symbol is named, not derived: `tree-sitter-kotlin-ng` and
`tree-sitter-sequel` export names that differ from codix's language names.

## Order of work

```
P.1 spike and measurements
 └─ P.2 lazy, fallible grammar access     (no behaviour change)
     └─ P.3 detection and install hints for every known language
         └─ P.4 plugin discovery, data-only plugins (VCS)
             ├─ P.5 language plugins (native grammars)
             ├─ P.6 build plugins (subprocess)
             └─ P.7 resolver plugins (views, scratch database, protocol)
                 └─ P.8 xtask, modular flavour, packaging layout
                     └─ P.9 daemon and plugin changes
                         └─ P.10 CI, release workflow, docs
                             └─ P.11 exit
```

P.2 and P.3 are useful on their own and change nothing for users but the
hint. P.5, P.6 and P.7 are independent once P.4 is merged; P.8 needs all
three.

## Tasks

### P.1 Spike and measurements

On a throwaway branch, not merged. Its findings go into this plan, which is
then amended and submitted again if any choice changes.

1. **Native grammar.** Compile `tree-sitter-java`'s C sources into a shared
   library with `cc`, load it with `libloading`, and parse the Java corpus on
   macOS, Linux (glibc) and Windows CI runners. Check that the symbol is
   exported on each, and that the output equals the compiled-in grammar's.
2. **Resolver path.** A process opens spring-framework's index read-only
   through views, reads every `refs` row with the symbols it would look up,
   writes one target per reference into a scratch database, and the core
   merges it. Measure each part. The cold index target (30 s, phase 3) must
   leave room for phase 4's real resolution on top.
3. **Sizes**, with the new release profile: the modular core with
   git only, and each plugin.
4. **Speed.** Cold index of spring-framework with the Java grammar compiled in,
   then as a plugin. Expected equal: the same machine code, called through
   the same function pointers.
5. **Sandbox.** Load a library, run a build plugin, and read the index from a
   second process inside the sandbox.

**Done when** the three platforms load the plugin and the figures are in
this plan.

### P.2 Lazy, fallible grammar access

1. `Grammar` holds a source instead of a `LanguageFn`: compiled in (today), or
   a plugin library to load. `Grammar::language()` returns
   `Result<&Language, PluginError>`, loading once per process.
2. Every caller moves to it: `parse.rs`, `refs.rs`, `rename.rs`, `apply.rs`,
   `preflight.rs` and the CLI's checks. A failure becomes `plugin_invalid`
   for that file, which is then indexed as text, like a language without a
   grammar.
3. Fingerprints are computed per language on first use, not for every
   language in `PackIndexer::new`.

**Done when** the suite passes unchanged, with no plugin code yet.

### P.3 Detection and install hints for every known language

1. `known.rs` gains, for every first-party language, the shebangs and
   modelines that today live only in pack descriptors, next to the existing
   extensions and file names; and the marker files of every first-party build
   system and VCS. A test checks that each compiled-in descriptor agrees with
   the known tables.
2. `not_supported_in_build` gains the install hint (Choices). Its message
   says "not installed" in the modular flavour and "not in this build" in the
   monolithic one.
3. `index` and `status` report, per provider without its pack, what it
   misses and the hint, once per provider: files indexed as text for a
   language, a `pom.xml` with no Maven plugin, a `.hg` directory with no
   Mercurial plugin.

**Done when** a project with Kotlin files and no Kotlin pack reports them
with the hint, in the index summary, in `status` and in the error.

### P.4 Plugin discovery and data-only plugins

1. `codix-plugin`: `plugin.toml` parsing and validation, the search path, and
   the conflict rules (Choices). Descriptors are validated as the build
   scripts validate them today, but at run time, into `plugin_invalid`.
2. The registry is built once from the compiled-in providers and the
   discovered plugins. Reading the manifests and descriptors is the only
   startup cost: no library is opened, no process started.
3. VCS plugins work end to end: Mercurial as a plugin is detected and its
   ignore rules apply.
4. `codix languages` (and the matching MCP output) gains, per provider, its
   source (compiled in, or the plugin's directory), its version and its
   status (available, not installed, invalid: reason, left out: reason).

**Done when** a VCS plugin in `CODIX_PLUGIN_PATH` behaves as a compiled-in
one, and a broken manifest is listed with its reason without failing other
commands.

### P.5 Language plugins

1. The native loader, behind the `plugins` feature: open the library, look up
   the symbol, check the ABI through `Parser::set_language`, compile the
   queries. Each failure is a `plugin_invalid` with its cause.
2. Loading happens on the first file of the language, in whichever thread
   indexes it, once per process.
3. Tests, with plugins built by the test kit from the packs in a temporary
   directory:
   - the Java corpus through the plugin gives the same symbols and refs as
     the compiled-in pack;
   - a missing symbol, a schema mismatch and a query that does not compile
     each give `plugin_invalid` and text indexing;
   - installing a plugin reparses its files on the next scan, and removing it
     returns them to text.

**Done when** those tests pass on the three platforms.

### P.6 Build plugins

1. The protocol, version 1:
   - `<exe> discover --protocol 1 <root>`, run in `<root>` with no stdin and
     a 30 s timeout;
   - stdout: `{"protocol": 1, "model": <BuildModel>}`, the `BuildModel`
     schema published in `docs/plugins.md` from its `JsonSchema`;
   - a non-zero exit, a timeout or bad JSON gives a `plugin_invalid` warning,
     and the scan goes on without that build system's modules.
2. `codix-build-maven` and `codix-build-gradle` gain a `[[bin]]` that calls
   their resolver and prints the answer.
3. Tests: the Maven and Gradle fixtures give the same model through the
   plugin as compiled in; a plugin that exits 1, hangs, or prints garbage.

**Done when** those tests pass on the three platforms.

### P.7 Resolver plugins

The mechanism only. Phase 4 writes the Java and Go resolvers on it, and adds
the `imports` and `relations` tables, their views and their scratch tables.

1. **Views.** Migration adding `v1_files`, `v1_symbols`, `v1_refs` and
   `v1_modules`. A test fails when a view's columns change, so a change is a
   new version, not an edit.
2. **`codix-resolver`.** Protocol types (`hello`, `resolve`, `shutdown`), the
   scratch schema (reference targets and states), the `Resolver` trait, and
   the `[[bin]]` main loop that serves a `Resolver` over stdin and stdout.
3. **Core side.** After a scan's parse transaction commits, the core runs the
   resolver of each language whose files changed: in process in the
   monolithic flavour, as the plugin's process in the modular one. It
   attaches the scratch database, checks it (every row names a reference of a
   changed file, every target an existing symbol), and merges it in one
   transaction. A rejected scratch database is a `plugin_invalid` warning,
   and those files keep their tier 2 results.
4. **Lifetime.** One process per scan in the CLI; kept by the daemon, with
   the restart limit (Resolver plugins).
5. **Tests**, with a test resolver in the test kit that binds each unbound
   reference to the only symbol of that name in the project, when there is
   exactly one:
   - in process and as a plugin, the same `refs` rows;
   - a resolver that crashes, hangs, answers another protocol version, or
     writes rows for files it was not given: each refused with
     `plugin_invalid`, the index unchanged;
   - a scan that changes one file resolves that file and the dependents the
     resolver names, no other.

**Done when** those tests pass on the three platforms.

### P.8 Modular flavour and packaging layout

1. `cargo xtask plugins <out>`: for each language pack, compile the grammar
   into a shared library, copy its resolver's `[[bin]]` if it has one, and
   write the plugin directory; for each build system, copy its `[[bin]]`; for
   Mercurial and Subversion, their descriptors. `--only <name>` for tests.
2. The modular flavour: `codix-cli` with `--no-default-features --features
   plugins,vcs-git`, and a release layout of `bin/codix` and
   `lib/codix/plugins/`.
3. The test kit builds plugins once per test run with the same code as
   `xtask`, so language, build and resolver tests can run against either
   flavour.

**Done when** the modular flavour passes the corpus and CLI tests with the
plugins built by `xtask`.

### P.9 Daemon and plugin changes

1. The daemon record gains a fingerprint of the plugin set (manifests,
   versions, library and executable sizes and modification times), next to
   the protocol and codix version it already carries (`control.rs`).
2. A client that finds another fingerprint restarts the daemon, as it does
   today for another version, and with it the resolver processes. A running
   watcher does not watch the plugin directories.

**Done when** a `live_` test installs a plugin under a running daemon, and
the next command uses it.

### P.10 CI, release workflow, docs

1. CI builds and tests both flavours on Linux, macOS and Windows.
2. `release.yml` adds a modular job for `aarch64-apple-darwin`,
   `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`,
   `aarch64-unknown-linux-gnu` and `x86_64-pc-windows-msvc`: one archive for
   the core, one per plugin, sizes recorded by `measure.py`. Still nothing
   published.
3. Docs:
   - README: flavours, how plugins are found, the install hint;
   - `docs/plugins.md` for plugin authors: layout, `plugin.toml`, descriptor
     reference, captures the queries must use, the build protocol and its
     JSON schema, the resolver protocol, views and scratch schema;
   - a Homebrew formula for the core and one plugin, in `packaging/`, as an
     example only.

**Done when** both flavours are green on the three platforms, and a release
run produces every archive.

### P.11 Exit

1. **Equivalence.** On spring-framework and linux, the modular flavour with
   every plugin and the monolithic `full` build give identical `files`,
   `symbols`, `refs` and `modules` tables, apart from parser versions.
2. **Benchmarks** (`benches/results/0.4.0.json`): cold index in both flavours
   (target: within 5 %), lookup p95, first command after a plugin install,
   sizes of the core and of each plugin.
3. **Release.** Workspace version 0.4.0.

**Done when** the equivalence holds, and the benchmark targets are met or
each miss is reported with its figure.

## Order with phase 4

This work first. P.2 changes how every caller reaches a grammar, and P.7 is
what phase 4's resolvers plug into. Phase 4's task 4.1 (resolver framework)
then shrinks to the `imports` and `relations` tables, their views and scratch
tables, the reverse dependency map, and the edit-time requests; 4.2 and 4.3
write the Java and Go resolvers as `codix-resolver` crates, shipped in the
Java and Go plugins.

## Open points

- **Edit-time resolution.** Phase 4's pre-flight checks for cross-file
  renames ask questions the index may not answer, such as which bindings a
  new name would capture in another file. They become protocol requests,
  answered by the running resolver; their list is phase 4's to set.
- **Linux glibc version.** A modular binary built on Ubuntu 24.04 needs its
  glibc. Homebrew on Linux builds bottles on its own images, so this matters
  only for archives used outside it; P.10 records the minimum.
- **Publishing.** A tap, formulae and their bottles are a release step,
  outside this plan, and need your go-ahead.
- **Local runs.** The benchmarks and the equivalence runs on the benchmark
  repositories run from your terminal, as in phase 3.
