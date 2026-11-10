# Writing a codix plugin

A plugin adds a provider to the modular flavour of codix without changing or
rebuilding it: a language (a tree-sitter grammar, a descriptor and queries,
and later a resolver), a build system (a descriptor and an executable), or a
version control system (a descriptor). How codix finds plugins is in the
[README](../README.md#plugins). The first-party plugins are built from the
crates under `crates/` by `cargo xtask plugins <out>`; they are the reference
for everything below.

## Layout

```
<search dir>/
  language/java/
    plugin.toml
    descriptor.toml
    queries/tags.scm
    queries/locals.scm      # optional: tier 2 scopes
    java.dylib              # .so on Linux, .dll on Windows
    bin/codix-resolve-java  # optional: a resolver
  build/maven/
    plugin.toml
    descriptor.toml
    bin/codix-build-maven
  vcs/mercurial/
    plugin.toml
    descriptor.toml
```

The directory names the provider: `<kind>/<name>/`, with the kind one of
`language`, `build` and `vcs`. A plugin that fails a check is listed by
`codix languages` as invalid, with the reason, and otherwise ignored; a
compiled-in provider, or a plugin found earlier in the search path, wins over
one of the same name, which is listed as left out.

## `plugin.toml`

```toml
schema = 1            # the manifest schema; codix refuses any other
kind = "language"     # as the directory
name = "java"         # as the directory
version = "1.2.0"     # informative: shown by `codix languages`

# Language plugins:
library = "java.dylib"        # a file in the plugin's directory
symbol = "tree_sitter_java"   # the function the library exports

[resolver]                    # optional
executable = "bin/codix-resolve-java"
protocol = 1
views = 1

# Build plugins:
# executable = "bin/codix-build-maven"
```

Unknown fields are refused. Paths are relative to the plugin's directory and
stay inside it.

## The descriptor

`descriptor.toml` is the same file a compiled-in provider embeds, checked the
same way, but at run time. Its fields are documented on
`LanguageDescriptor`, `BuildDescriptor` and `VcsDescriptor` in
`crates/codix-provider/src/descriptor.rs`; the descriptors of the first-party
providers, under `crates/codix-*/descriptor.toml`, show each one in use. Its
`name` must be the plugin's. A language's `version` is part of the parser
version stored per file: raise it when the queries or the descriptor change
what is extracted, so files are parsed again.

## Grammars

The library is a tree-sitter grammar compiled into a shared library, as
`tree-sitter build` makes it or as `cargo xtask plugins` does from the
grammar crate's `parser.c` and `scanner.c`. It holds C code and parse tables,
no Rust. codix opens it on the first file of its language, looks up `symbol`,
and checks the grammar's ABI against its tree-sitter (13 to 15). A library
that does not load, a missing symbol, an ABI out of range, or a query that
does not compile makes the plugin invalid with the cause: its files are
indexed as text, and `index` and `status` say why.

Installing, updating or removing a plugin changes the parser version of its
files (it hashes the library and the queries), so the next scan parses them
again.

## Queries

`queries/tags.scm` finds definitions, with tree-sitter's tags names:

| Capture | Meaning |
| --- | --- |
| `@definition.<kind>` | The definition node; `<kind>` is mapped onto codix's kinds by `[symbols.kinds]` |
| `@name` | Its name |
| `@parent` | The enclosing type of a definition written outside it, such as a Go method's receiver type; several, outermost first, for `a::B::f` |

`queries/locals.scm`, when there is one, gives the language tier 2: scopes,
local definitions and references, bound within the file.

| Capture | Meaning |
| --- | --- |
| `@local.scope.<kind>` | A scope, such as `function`, `class` or `block`. `(#set! local.opaque)` where a name not found may be a member codix cannot see, such as an inherited one; `(#set! local.opaque "members")` where such a member can hide symbols but not locals, as with a Kotlin lambda's receiver; `(#set! local.transparent)` for a scope whose symbols belong to the scope around it, as a C++ template's do |
| `@local.definition.<kind>` | The name of a local definition, such as a `parameter` or `variable`. `(#set! local.reuse)` makes it a write to an earlier definition of the name in the same scope, as Go's `:=` does; `(#set! local.reuse "visible")`, to any definition visible from there. `@local.after` in the same match makes it visible only after that node. `(#set! local.scope "<kind>")` defines it in the nearest enclosing scope of that kind, as JavaScript's `var` |
| `@local.reference.<context>` | A reference, in `call`, `type`, `import`, `read` or `write` context; `@local.receiver` is the object of a member access in the same match |
| `@local.global`, `@local.nonlocal` | A name a scope declares as the file's or an enclosing function's, as Python's `global` and `nonlocal` do |
| `@local.skip` | A name that is no reference |

When several patterns capture the same node as a reference, the one written
last wins; a definition always wins over a reference. A name found nowhere in
the file after crossing an opaque scope is uncertain: codix never guesses.

Tier 1 symbols from `tags.scm` are definitions of their enclosing scope
without a capture. `[scopes]` in the descriptor sets how lookups go:
namespaces, which scopes hoist, the self receivers. The queries of the
first-party packs, in `crates/codix-lang-*/queries/`, cover every construct
above, and `corpus/<language>/` holds what codix extracts from them.

## Build plugins

codix runs the executable once per scan, and again when a build file
changes:

```
<executable> discover --protocol 1 <root>
```

in `<root>`, the canonical project root, with no stdin and at most 30
seconds. It answers on stdout:

```json
{"protocol": 1, "model": {"modules": [...], "unsupported": [...]}}
```

The schema of the answer is
[`schemas/plugins/build-1.json`](../schemas/plugins/build-1.json). A
non-zero exit, a timeout, or an answer that does not match makes codix report
the plugin invalid with the cause (its first line of stderr, for an exit),
and the scan goes on without that build system's modules. Read the build
files; never run the build. A crate implementing `BuildResolver` gets this
`main` from `codix_provider::serve`.

## Resolvers

A resolver binds the references of its language across files: tier 3. It is
an executable that codix starts after a scan that changed files of its
language, and keeps running in the daemon. It speaks JSON lines on stdin and
stdout:

| Request | Reply |
| --- | --- |
| `{"hello": {"protocol": 1, "views": 1}}` | `{"hello": {"protocol": 1, "views": 1}}`; codix refuses other versions |
| `{"resolve": {"index": "<path>", "scratch": "<path>", "files": ["src/A.java"]}}` | `{"resolved": {"dependents": ["src/B.java"]}}`, or `{"error": {"message": "..."}}` |
| `"shutdown"` | none: it exits, as it does at the end of stdin |

`files` are the root-relative paths that changed (new, modified or deleted).
The resolver opens the index read-only, reads it through the views, binds the
references of `files` and of the files that depend on them, writes the
results into the scratch database, and names those dependents. codix then
checks the scratch database and replaces what it held for `files` and the
dependents. It refuses the whole database when a row names a reference of
another file or a target that is not a symbol of the index. A refused
answer, a crash, or no answer within 10 minutes leaves those files at tier 2
with a `plugin_invalid` warning. The daemon restarts a resolver after a
failure, at most three times in a row.

The views and the scratch schema are in
[`schemas/plugins/resolver-1.txt`](../schemas/plugins/resolver-1.txt):

- `v1_files`, `v1_symbols`, `v1_refs`, `v1_modules`: the index as resolvers
  see it. Reference contexts and states are spelled out (`call`, `bound`,
  `free`, `uncertain`). A change to a view's columns is a new version of the
  views, never an edit.
- `resolved(ref, target, state)`: one row per reference bound, `bound` to the
  `v1_symbols.id` in `target`, or `external` (outside the project) with no
  target.

The crate `codix-resolver` has all of it: the protocol types, the views and
scratch schema, a `Resolver` trait, and `serve`, the `main` of a resolver
executable. The same `Resolver` runs in process when compiled into codix
(`codix_resolver::register!`), so both flavours give the same results.

## Version control plugins

A descriptor only: the markers that show the system is in use, the
directories to skip, the ignore files to honour, and how codix keeps its
`.codix/` directory out of commits. See `crates/codix-vcs-hg/descriptor.toml`.
