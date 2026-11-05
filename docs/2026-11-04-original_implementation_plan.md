# codix — Implementation Plan

Plan for `docs/2026-11-04-original_design.md` (the design). It follows the design's
roadmap: five phases, each ending in a releasable binary. Every phase lists its
tasks with scope, tests and exit criteria. Tasks are sized to land as one reviewed
change each; a phase gets a step-by-step plan when it starts.

## Assumptions

- **Stack as designed.** Rust, single static binary, Cargo workspace.
- **Location.** A new Cargo workspace at the root of this project.
- **License.** Apache License 2.0.
- **Scope.** The design's Future section is not part of this plan.
- **Versions.** No crate or toolchain version is pinned here. Each is chosen when
  its task starts, from the current release (`cargo search`, `cargo add`), and
  pinned in `Cargo.lock` and `rust-toolchain.toml`.
- **Development style.** Test first for every behavior. Each task lands with its
  tests, `cargo clippy -- -D warnings`, `cargo fmt --check` and the full test
  suite green.

## Workspace layout

```
Cargo.toml                  workspace, feature presets (default, full, minimal)
rust-toolchain.toml
crates/
  codix-core/               engine: operation layer, index store, scanner,
                            parser pool, resolver framework, edit engine
  codix-provider/           provider traits, descriptor schema, registration
  codix-cli/                clap frontend, text and JSON rendering → binary `codix`
  codix-mcp/                MCP frontend (phase 2)
  codix-daemon/             daemon, socket protocol, watcher (phase 2)
  codix-lang-<name>/        one per language pack
  codix-build-<name>/       maven, gradle
  codix-vcs-<name>/         git, hg, svn
  codix-testkit/            corpus runner, fixture projects, golden-output helpers
benches/                    scan, index and query benchmarks
corpus/<lang>/              per-language expected symbols, scopes, references
eval/                       LLM benchmark tasks and harness (phase 2)
```

The binary crate depends on providers only through Cargo features, so
`codix-core` never names a provider.

## Phase 1 — Core engine

**Goal.** A `codix` binary that indexes any directory incrementally and answers
tier 1 queries from the CLI.

**Deliverable.** `init`, `index`, `status`, `config`, `find`, `def`, `outline`,
`show`, `grep`, `capabilities` (static part) on the 12 launch languages plus the
search-only formats, with text and `--json` output.

### 1.1 Workspace, CI, release skeleton

- Workspace and crates above (empty ones only when first needed), `rust-toolchain.toml`,
  lints (`clippy` pedantic subset chosen once, `unsafe_code = "forbid"` outside
  grammar bindings), `rustfmt.toml`.
- CI on Linux, macOS and Windows: build, test, clippy, fmt; `cargo-hack` over the
  `default`, `full` and `minimal` presets and each provider feature alone.
- Release build: static Linux binary (musl), macOS and Windows binaries; size and
  startup time recorded per build.
- **Done when** an empty `codix --version` builds on all three platforms in CI,
  for every preset.

### 1.2 Operation layer, errors and output

- `Operation` trait: typed, serde-serializable request and response per command.
  The CLI parses arguments into requests and renders responses; it holds no
  logic. This is what keeps CLI and MCP identical later.
- Error type with stable codes (`no_project`, `not_indexed`, `ambiguous_symbol`,
  `unknown_symbol`, `invalid_config`, `not_supported_in_build`, …), one-line cause
  and hint, mapped to the exit codes table.
- Renderers: compact line-oriented text (`path:line:col`), and a versioned JSON
  envelope. Color and progress only on a terminal.
- Result bounding: `--limit` (default 50), `--offset`, total in the footer, the
  design's sort order.
- **Tests:** golden-output tests for text and JSON of a stub operation; exit code
  for each error class.

### 1.3 Project discovery and path identity

- Walk up to the nearest `.codix/` or `codix.toml`; `--root`; `no_project` and
  `not_indexed` with hints.
- Canonical file identity: root-relative path, on-disk case, symlinks resolved;
  case-insensitive filesystem detection; symlinks leaving the root skipped; loop
  detection.
- Path rendering relative to the current directory (`../` for outside), root-relative
  paths in JSON; input paths accepted in either form.
- `--here` and `--path` scope filters.
- **Tests:** nested projects, symlink loops, symlinks out of root, case-only
  differences (run on macOS and Windows CI), cwd in a subdirectory.

### 1.4 Configuration

- Settings schema as code: each key's type, default, allowed values, description
  and owning file (`codix.toml` or `.codix/config.toml`). Validation, `--describe`
  and documentation all read it.
- Loading and layering (local overrides shared).
- `codix config` in all five forms, `--json`, edits through `toml_edit`
  preserving comments and order, file created on first write.
- Change effects: the command reports what a change implies; a change to an
  indexing key marks affected files for reindex (wired in 1.10).
- Phase 1 keys: language overrides, source root overrides, max file size,
  periodic scan interval. Later phases add theirs to the same schema.
- **Tests:** round-trip preserving comments; unknown key and invalid value refused
  with nothing written; `--show-origin`.

### 1.5 Provider framework

- Descriptor schema (TOML) for the three provider kinds; descriptors choose among
  engine behaviors and carry no code.
- Provider traits for logic-bearing providers (resolvers, Gradle parsing).
- Link-time registration (`inventory` or `linkme`, chosen in this task), and
  `build.rs` embedding of enabled descriptors.
- Feature presets in the workspace `Cargo.toml`; features strictly additive.
- "Not supported in this build" path for files whose provider is compiled out.
- **Tests:** registry contents per preset; a file of a disabled language reports
  `not_supported_in_build`; a malformed descriptor fails the build, not the run.

### 1.6 Version control providers

- `codix-vcs-git` (`nested-file`), `codix-vcs-hg` and `codix-vcs-svn` (`hint`),
  all descriptor-only.
- Detection markers (including `.git` as a file for worktrees and submodules),
  skip directories, honored ignore files, multiple matching descriptors combined.
- `codix init` applies each strategy; codix never edits project ignore files.
- **Tests:** git worktree and submodule detection; git plus Jujutsu both applying;
  `.codix/.gitignore` content; `hint` printed once.

### 1.7 Walker and ignore rules

- Parallel walk with the `ignore` crate: `.codixignore`, provider-honored ignore
  files, defaults (`.codix/`, `node_modules`, `target`, `.venv`, `dist`), skip
  directories that always win.
- Size limit (default 2 MB), binary detection; minified and non-UTF-8 files
  marked search-only.
- **Tests:** precedence between `.codixignore`, `.gitignore` and skip
  directories; a non-git project honoring only `.codixignore`.

### 1.8 Index store

- `rusqlite` with bundled SQLite and FTS5; WAL; connection setup shared by
  readers and the writer.
- Migration framework with a schema version table; phase 1 creates `files`,
  `dirs`, `symbols` and the FTS table. Later phases add tables by migration.
- Store API keyed by file: replace all rows of one file in one transaction;
  batched commit of many files.
- Name search: exact, prefix, glob on qualified name, and fuzzy. FTS5 handles
  prefix; fuzzy needs its own ranking (trigram tokenizer candidates, then edit
  distance), decided in this task.
- **Tests:** concurrent reader during a large write sees a consistent snapshot;
  migration from an empty database; replace-file idempotency.

### 1.9 Write lock

- `.codix/write.lock` with OS advisory locks (std `File::lock` if available on
  the chosen toolchain, else a small crate), released on process exit.
- Wait with timeout (default 200 ms) for queries, blocking for `index`.
- Network filesystem detection for `.codix/` (NFS, SMB); when detected, every
  command warns, in text output and in JSON and MCP results.
- **Tests:** two processes contending; lock released after `kill -9`; warning
  present in every output format on a simulated network filesystem.

### 1.10 Manifest and change detection

- Scan algorithm steps 1–6 from the design: stat compare, BLAKE3 on stat change,
  touch vs change, deletions, parallel reparse, one commit.
- Merkle directory hashes to skip unchanged subtrees.
- Racy timestamp handling as in git.
- Under the lock: recheck each parsed file's hash before commit; re-queue
  changed files; idempotent commits.
- Parser version per file; a grammar or query change forces reparse.
- **Tests:** touch without change, change within mtime granularity, rename,
  delete, two files swapped between scans, concurrent `index` from two processes.

### 1.11 Language detection

- Chain: `codix.toml` override, modeline, shebang, filename, extension, content
  heuristics for `.h`, `.m`, `.pl`; unknown files indexed as plain text for search.
- Detection rules come from language pack descriptors.
- **Tests:** each step of the chain winning over the next one.

### 1.12 Parser pool and tier 1 extraction

- Tree-sitter parsing in a `rayon` pool, one parser per thread per language.
- `tags` query execution → symbols mapped to the shared kind vocabulary;
  qualified names and parent symbols from nesting; signatures and visibility
  where the query captures them.
- Parse-error regions: symbols inside get low confidence.
- Ranges as 1-based lines and Unicode-scalar columns, plus byte offsets.
- **Tests:** corpus runner in `codix-testkit` (source file + expected symbol
  list); error-region confidence.

### 1.13 Symbol IDs

- Format `lang:path::Qualified.name#hash`; hash from kind and signature.
- Parsing full IDs, unique path parts, and `file:line:col`.
- Ambiguity → `ambiguous_symbol` with candidates (exit 3).
- **Tests:** ID stable across body edits and unrelated edits; changed by rename,
  move and signature change; overloads distinct.

### 1.14 Language packs, tier 1

- Java and Go first, as the reference packs for descriptor layout and corpus
  format.
- Then TypeScript, Python, Kotlin, JavaScript, Rust, C, C++, C#, PHP, Ruby.
- Search-only formats: JSON, YAML, TOML keys; XML elements; Markdown headings;
  SQL statements' object names.
- Each pack: pinned grammar, `tags` query (from upstream where it exists, else
  written here), detection rules, corpus.
- **Done when** each language's corpus passes and `capabilities` lists it at
  tier 1.

### 1.15 Build system providers

- `codix-build-maven`: modules, parent POMs, property interpolation in source
  directories, `build-helper-maven-plugin` sources; fallback to conventional
  layout.
- `codix-build-gradle`: `settings.gradle(.kts)` includes and `sourceSets` blocks
  in both DSLs; anything else reported, never guessed.
- Results stored in a `modules` table (migration) and shown in `status`. The Java
  resolver consumes them in phase 4.
- **Tests:** fixture builds for each listed Maven feature; Gradle Groovy and
  Kotlin DSL fixtures; an unsupported Gradle construct reported in `status`.

### 1.16 Index commands

- `init`: creates `.codix/`, applies VCS strategies, first index.
- `index`: full or incremental scan; progress on a terminal only.
- `status`: indexed, pending, changed since last scan, errors, build modules.

### 1.17 Query commands

- `find` (match modes, `--kind`, `--lang`, `--path`), `def` by name, `outline`
  (file or directory, signatures only), `show` (range plus `--context N`).
- `grep`: regex over indexed files, scoped by language, path or kind, with the
  enclosing symbol of each match.
- Freshness: lookups stat-check returned files and reindex them; `grep` runs a
  no-change scan first; busy lock → results marked possibly stale.
- **Tests:** golden outputs per command; freshness after an edit made between
  two queries.

### 1.18 Phase exit

- Benchmarks in `benches/` on pinned public repositories of several sizes and
  languages: cold index (1M lines / 10k files < 30 s), no-change scan
  (100k files < 1 s), CLI lookup p95 < 150 ms. Results recorded per release.
- README with install and the query workflow.

## Phase 2 — Daemon and MCP

**Goal.** Millisecond queries through a per-project daemon, and the same
operations as MCP tools.

### 2.1 Daemon lifecycle

- `.codix/daemon.lock`, one daemon per canonical root; second start fails naming
  the running one.
- Socket: `.codix/daemon.sock`, moved to the user runtime directory under a root
  hash when the path exceeds the Unix limit; Windows named pipe
  `\\.\pipe\codix-<root hash>`.
- `.codix/daemon.json` handshake: root, socket, PID, version; stale or
  incompatible daemons replaced.
- Holds the write lock permanently; CLI writes are forwarded to it.
- Runtime registry; `daemon start`, `daemon stop`, `daemon list`, `daemon stop --all`.
- The CLI never starts a daemon. Idle stop after 30 minutes without requests,
  as a local config key; `status` names the running daemon.
- **Tests:** two projects side by side; nested project and git worktree each get
  their own daemon; upgrade replaces the stale daemon; long-path socket fallback.

### 2.2 Socket protocol

- Framed JSON messages carrying the operation-layer request and response types
  unchanged, so the daemon path and the in-process path share one code path.
- CLI: use the daemon when its handshake matches, otherwise run in-process.
- **Tests:** every command returns identical output through both paths.

### 2.3 Watch mode

- `notify` watcher feeding the scan pipeline, debounced (~100 ms).
- Full stat scan at startup and periodically (default 10 min, configurable).
- Watch quota exhaustion: fallback to periodic scans for unwatched directories,
  reported in `status`.
- Self-write registry so codix's own applies (phase 3) are not reparsed.
- **Tests:** dropped events recovered by the periodic scan; quota fallback
  simulated; reindex after one save < 50 ms.

### 2.4 Canonical descriptions and discovery

- One description per command (what, when to prefer it over grep, what it
  returns, minimum tier, one example), stored once and used by `--help`, MCP tool
  descriptions and generated docs.
- `help <command>`, `capabilities` (operations, per-language tiers for this
  project, index status, compiled-in providers), `languages`, `init --agent-docs`.
- **Tests:** a test fails when a command lacks a description or example, or when
  CLI and MCP descriptions differ.

### 2.5 `codix mcp`

- MCP server over stdio (official Rust SDK if suitable, checked in this task).
- Tools generated from the operation layer: same names, same semantics,
  input schemas derived from request types.
- Router: optional `cwd` per call, project discovery from it, call sent to the
  project's daemon (started on the first call for that project if none is
  running), paths relative to `cwd`.
- **Tests:** MCP calls against two projects from one server; tool list matches
  the CLI command list.

### 2.6 Output schema versioning

- Integer `schema_version` in every JSON response and MCP result; additive
  changes keep it, breaking changes increment it.
- Schema snapshots checked in CI, so an unintended change fails the build.

### 2.7 LLM benchmark, first run

- Harness in `eval/`: realistic tasks (locate, trace, read outline), several
  models, tool access only.
- Defects tracked: misused commands, guessed flags, grep fallbacks.

### 2.8 Phase exit

- Daemon query p95 < 10 ms; daemon memory on a 1M-line project < 500 MB.

## Phase 3 — Scopes and edits

**Goal.** Tier 2 references and safe, previewed, within-file renames with undo.

### 3.1 Tier 2 extraction

- `locals` queries → `scopes` and `refs` tables (migration), reference context
  kinds (call, type, import, read, write), enclosing symbol.
- Scope-aware binding of references within a file.
- Tier 2 for all 12 launch languages, each with scope corpus cases.

### 3.2 `refs` and `def` by location

- `refs` grouped by file with context kind and confidence (`textual` at tier 1,
  `scoped` at tier 2); `def` by location.
- Completeness freshness rule from the design.

### 3.3 Edit engine

- Edit set model (file, range, replacement), recorded file hashes and edited
  region contents; a region is the whole lines an edit touches.
- Line endings of each file preserved.
- `previews` table (migration), preview IDs, unified diff and summary (files,
  sites, overrides, excluded and ambiguous sites), the exact `apply` command in
  the output.

### 3.4 Pre-flight checks

- Name collision in target scope, shadowing in both directions, keyword and
  identifier validity (rules supplied by each language pack), parse-error
  regions, tier below requirement.
- Generated and vendored code: path patterns from VCS and build descriptors and
  a `codix.toml` key; edits refused unless planned with `--allow-generated`.
- Typed refusal errors (exit 4).
- Override flags, one condition each, listed in the preview; never applicable to
  parse-error regions. No `--force`.

### 3.5 `rename` within a file

- Tier 2: definition and scoped references in the file; cross-file sites listed as
  unresolved and excluded.
- Tier 1: refused unless `--textual`, which previews textual matches only.

### 3.6 `apply`

- Under the write lock: per-file check (unchanged / changed outside regions →
  rebase and rerun pre-flight / changed inside → `stale_preview` with sites).
- New references to a renamed symbol since planning → `stale_preview` listing
  them.
- Journal entry with plan and originals before writing, keeping the last 20
  applies; temp file plus rename per file, each file rechecked just before its
  rename; rollback from originals on failure.
- Windows: `ReplaceFileW` with brief retries; a file that stays locked fails the
  apply with a typed error and restores the originals.
- Index and manifest updated from the applied edits; self-writes registered with
  the watcher.
- Crash recovery: the next lock holder restores an unfinished journal entry.
- **Tests:** rebase onto a file changed outside regions; refusal when a region
  changed; kill mid-apply leaves every file either original or fully edited.

### 3.7 `previews`, `discard`, expiry

- List and discard pending previews; expiry from local config (default 1 hour);
  expiry on region change.

### 3.8 `undo`

- Returns a preview reverting the last apply, refused if its files changed since;
  applied with `apply`.

### 3.9 Phase exit

- Rename round-trips (rename then rename back reproduces original bytes) on the
  corpora and benchmark repositories; renamed projects still parse.
- LLM benchmark extended with rename tasks.

## Phase 4 — Resolution

**Goal.** Tier 3 for Java and Go: cross-file references, hierarchies, renames,
`add` and `safe-delete`.

### 4.1 Resolver framework

- Resolver provider trait: module and import resolution, reference resolution,
  supertype edges.
- `imports` and `relations` tables (migration); reference resolved symbol IDs.
- Resolution at index time; reverse dependency map marking dependents stale
  when an imported file changes, re-resolved in the same reindex.
- **Tests:** framework exercised by both resolvers; a change in an imported file
  re-resolves its dependents only.

### 4.2 Java resolver

- Source roots from the build providers; packages to directories.
- Single, wildcard and static imports; nested and inner types.
- Overload resolution by argument types; overrides across classes and interfaces;
  generics.
- Record component accessors as synthesized symbols.
- Build-generated symbols (Lombok, annotation processors) left unresolved.
- Corpus: one case per rule above.

### 4.3 Go resolver

- `go.mod` modules, packages, imports, method sets.
- Implicit interface satisfaction for `hierarchy` and for renames of interface
  methods.

### 4.4 Cross-file `rename`

- Resolved sites included; textual and unresolved sites listed separately,
  excluded unless included explicitly.
- Interface method renames propagate to every override and implementation.
- Ambiguous sites flagged in the preview.
- Java: Javadoc `{@link}` and `@see` references as a separate preview category,
  included with `--include-javadoc`.

### 4.5 Symbol aliases

- `aliases` table (migration); written by applied renames and safe deletes.
- Read queries follow aliases with a note; edit commands refuse with
  `symbol_renamed` or `symbol_deleted`.
- Live symbols win; chains collapse; expiry (30 days or 5,000 entries); undo
  removes the alias.

### 4.6 `add`

- Kinds: function, method, field, constant; body from stdin, `--body-file`, or
  the MCP string field.
- Placement within `--in` scope, indentation matched to the file, name checks via
  pre-flight. No formatter is run.

### 4.7 `safe-delete`

- Removes a definition with no remaining references; otherwise the preview lists
  the blocking ones. Tombstone alias on apply.

### 4.8 `callers`, `callees`, `hierarchy`

- Call hierarchy to a chosen depth; supertypes, subtypes, implementations.

### 4.9 Phase exit

- Cross-file rename round-trips on Java and Go repositories, which must still
  compile afterward (run in CI with their own toolchains).
- LLM benchmark with cross-file rename and add tasks.

## Phase 5 — Breadth

**Goal.** More tier 3 languages and embedded languages, driven by usage.

- **5.1 TypeScript tier 3:** `tsconfig` paths and aliases, re-exports, module
  resolution.
- **5.2 Python tier 3:** packages and imports; dynamic attribute access marked
  partly resolved, never guessed.
- **5.3 Kotlin tier 3:** shared resolution with Java for mixed codebases.
- **5.4 Embedded languages:** tree-sitter injections for HTML, Vue, Svelte and
  Markdown fenced code; symbols carry host file and region language.
- **5.5 Further packs and tier upgrades,** chosen from usage data.

Each language follows the 4.2 pattern: corpus per rule, round-trip renames,
LLM benchmark tasks.

## Known gaps

These are inconsistencies or omissions in the design. The plan follows the design
as written; each needs a decision.

1. **Aliases arrive after renames.** Phase 3 applies renames, which change symbol
   IDs, but aliases are in phase 4, and the design's `undo` removes an alias.
   Recommendation: move aliases (4.5) into phase 3.
2. **Commands the roadmap doesn't place.** `config`, `languages`, `help`,
   `previews`, `discard`, `daemon list`, `daemon stop --all` and
   `init --agent-docs` are not in the roadmap. This plan places them in 1.4, 2.4,
   3.7 and 2.1.
3. **Build providers in phase 1 have no consumer until phase 4.** Maven and Gradle
   source roots matter only to the Java resolver. They could move to phase 4
   without affecting phases 1–3.
4. **Fuzzy search.** The design relies on FTS5 for fuzzy search, but FTS5 offers
   prefix and token matching, not fuzzy matching. Task 1.8 adds a ranking step.
5. **Go interfaces are structural.** `hierarchy` and interface-method renames in
   Go need implicit satisfaction checks, which the design's Java-centered
   override rules don't cover.
