# codix — Resolution Plan

The limits the second round of agent runs left open (see
`2026-11-12-agent_feedback_plan.md`), fixed: references codix drops, `def`
on a call, the missing type hierarchy, C function-pointer members, mentions
of a name outside the code, and binding references across files, which
needs a resolver for Java and one for C. One branch, one commit per step,
the full local suite on every build after each step, one CI run at the end.

## The limits

1. **`refs` drops uses of a private member in its own file.** A call through
   another instance (`dlbf.resolveBean(…)`) or from an anonymous class is
   unbound in the file's scopes; the symbol being private, codix drops it
   instead of listing it as textual. `refs resolveBean` reported 1 call of 6,
   with counts that look complete.
2. **`def` on a call ignores arguments and inheritance.** At
   `getBean(beanName, null, args)` it offered the one-argument overloads of
   the class instead of `AbstractBeanFactory.getBean(String, Class,
   Object...)`, inherited.
3. **No type hierarchy.** "Every class implementing `BeanPostProcessor`"
   took 9 to 10 calls of `refs` and globs, and missed classes not named
   `*PostProcessor`.
4. **C function-pointer members are not indexed.** `sched_class`'s
   `pick_next_task`, `file_operations`' `read`: nothing leads from a slot to
   the functions filling it.
5. **Mentions outside the code are invisible before a rename:** Javadoc
   `{@link Type#name}`, XML, properties, docs. `rename` lists mentions in
   the renamed file only.
6. **References across files are textual.** Neither Java nor C has a
   resolver: every use in another file matches by name only, so `refs`
   mixes symbols of the same name, and `rename` refuses any symbol used
   outside its file.

## What the index lacks

The resolvers and the hierarchy need facts the parse sees but the index does
not keep:

| Fact | For | From |
| --- | --- | --- |
| A reference's receiver, as written: `dlbf`, `BeanFactoryUtils`, `this`, `super` | Java calls | `@local.receiver` text, when a name or a qualified name |
| The receiver's declared type, when it is a local, a parameter or a field | Java calls | a new `@local.type` capture on declarations |
| A call's argument count | arity, overloads | the call's argument list |
| A symbol's parameter count | arity | its signature |
| Supertypes, as written: `extends`, `implements` | hierarchy, inheritance | a new `@supertype` capture |
| Imports: name, alias, static, wildcard; the package | Java names | a new `@import` capture |
| `#include`s, as written | C | `preproc_include` |

They go into the index (new columns on `refs` and `symbols`, new tables
`supertypes` and `imports`), and into the resolver views.

## Decisions to take

- **(a) Resolvers ship as plugins too.** The Java and C resolvers implement
  `codix_resolver::Resolver`: compiled in for the monolithic build, and as
  `bin/codix-resolve-<lang>` in their language plugin for the modular one,
  as P.7 made possible. Recommended. Alternative: compiled in only, the
  modular flavour staying at tier 2.
- **(b) The resolver views change in place**, as v1, like the JSON contract
  before a release. Alternative: views v2 beside v1.
- **(c) `rename` edits bound references in other files.** Once a resolver
  binds them, they are sites like any other: the preview lists them, the
  checks run on every file edited. Textual ones are still refused, as now.
- **(d) Mentions elsewhere are listed, not edited.** The preview lists every
  whole word of the old name in other files, code comments and text files
  alike, as "mentions elsewhere", unchanged. Alternative: `--edit-mentions`
  to rename those in text files and comments too.
- **(e) C include paths:** `compile_commands.json` when the project has one;
  otherwise the including file's directory, then every `include/` directory
  of the tree, `arch/<any>/include` included. Unresolved includes leave
  their names textual.

## Design

### 1. Completeness of `refs`

In a member's own file, a use of its name that the file's scopes leave
unbound, through another object or from a nested or anonymous class, is
listed as textual whatever the member's visibility. Private members still
skip other files.

### 2. Index facts

As in the table above. Each language pack gains the captures it can give:
Java, Kotlin, C# and TypeScript have receivers, types, supertypes and
imports; C has includes, receivers (`a->f`, `a.f`) and arity; others what
their grammar shows. A pack without a capture leaves the fact empty, and
what needs it stays textual. The index schema changes: indexes are built
again.

### 3. `codix types`

```
codix types BeanPostProcessor             # what extends or implements it, transitively
codix types AsyncAnnotationBeanPostProcessor --super   # its supertypes, up to the root
```

Rows are types with their kind, place and how each relation was found:
`bound` when a resolver bound the supertype's name, `textual` when only the
name matched. `--depth N`, the narrowing options and paging as `refs` has
them.

### 4. `def` on a call

When a call's name is not bound in its file: the methods of that name in the
enclosing type and its supertypes, or in the receiver's declared type and
its supertypes when codix knows it; then only those whose parameter count
fits the call (varargs fitting more). The other candidates follow, marked as
not fitting, as `def` already lists every match.

### 5. Java resolver

Binds, per changed file and its dependents (the files whose references name
something it defines):

- type names (declarations, `extends`, casts, `new T()`, `T.member`):
  nested types, the file, single-type imports, the package, on-demand
  imports, then `java.lang` and other unknown types as `external`;
- `T.m(…)` and `T.f`: members of `T` and its supertypes, by arity;
- `x.m(…)` where `x` is a field, a parameter or a local with a declared
  type: members of that type and its supertypes, by arity;
- unqualified calls in a type: its members, then inherited ones, then
  enclosing types' (inner and anonymous classes).

It leaves textual what needs inference: `var`, lambdas' parameters, generic
type arguments, chained calls. Bound references read `bound` in `refs`, with
their target; `rename` edits them (c).

### 6. C resolver

A use of `f` in file X binds to the function `f` (with its declarations, as
one target) that X defines, or that a header X includes, directly or
through other headers, declares. `static` functions bind within their file.
Several candidates (config variants, `tools/` copies) bind to the one X's
includes reach; when more than one remains, the reference stays textual.
Calls through function pointers stay textual.

### 7. Function-pointer members

`int (*pick_next_task)(struct rq *)` in a struct is a field; a designated
initializer `.pick_next_task = __pick_next_task_fair` is a write reference to
that field, and a read reference to the function. `refs
sched_class.pick_next_task --context write` then lists every ops table
filling the slot, `at` names the table around each. Initializers inside a
macro call, such as `DEFINE_SCHED_CLASS(fair) = {…}`, parse as far as the
grammar lets them.

### 8. Mentions elsewhere

`rename` lists, unchanged, the whole-word mentions of the old name in other
files: comments and strings in code files, and text files (XML, properties,
Markdown, YAML). The search reads files in parallel and is limited to the
indexed files.

## Steps

1. `refs` completeness for members in their own file.
2. Index facts: captures for Java, C and the other packs, schema, views.
3. `codix types`.
4. `def` on a call: arity and inheritance.
5. C function-pointer members and designated initializers.
6. Mentions elsewhere in `rename`.
7. The Java resolver, compiled in and as a plugin; `rename` across files.
8. The C resolver, with include paths (e).
9. The agents again, same scenarios.

Steps 1, 5 and 6 stand alone; 3 and 4 need 2; 7 and 8 need 2 and 4.

## Results (step 9)

The same two scenarios as the second round, run again by fresh agents with
codix as their only tool, on copies of Spring Framework v6.2.0 and Linux
v6.12.

| Run | Spring: calls, time | Linux: calls, time |
| --- | --- | --- |
| 1 | 68, 21 s | 56, 115 s |
| 2 | 55, 10 s | 51, 133 s |
| 3 | 56, 38 s | 87, 155 s |

Time in run 3 is mostly `apply`: 17 s of Spring's and 49 s of Linux's. Both
went down after the run (below). Linux's calls went up with the scenario's
depth: the agent followed `copy_from_user` to x86's `rep movs` and every
`sched_class` slot.

What the fixes changed for the agents:

- `types BeanPostProcessor` answered task 2 in one call, where run 2 took
  about ten `refs` and globs; 59 of its 60 relations are bound.
- `rename BeanFactoryUtils.transformedBeanName` edited 11 sites in 7 files,
  left `AbstractBeanFactory.transformedBeanName` alone, and needed no
  `--exclude-unresolved`.
- `refs` listed the calls to the private `resolveBean` from anonymous
  classes, bound.
- No ambiguity errors in either run.

Resolution, on a cold index:

| | Spring | Linux |
| --- | --- | --- |
| References its scopes leave unbound | 1.30 M | 7.98 M |
| Bound by the resolver | 0.41 M | 1.46 M, 4.58 M after the fixes below |
| Outside the project | 0.57 M | (C has none) |
| Cold index, resolver included | 27 s | 38 min, of which 14 resolving |

Fixed after the run:

- The C resolver bound kernel calls to a userspace stub in
  `tools/include`: macros now count, and of the include directories holding
  a header, the nearest to the including file wins.
- Java overrides compare parameter types, not names, so a call through an
  interface binds to the closest override.
- `instanceof` pattern variables have their type.
- After a change, a resolver resolves again only the uses nothing binds:
  Spring's `apply` went from 8.5 s to 4.7 s. The C resolver reads only the
  symbols of the names used: a change to one Linux file resolves in 1.5 s,
  against about 30 s.
- `def` on a call marks a binding `scoped` only when no inherited method fits
  as well, and lists at most five symbols that do not fit.

Still open:

- The C resolver's first pass over Linux is slow: about 30 minutes since it
  binds macros too, each file's include closure computed apart. The cold
  index before it takes 24 minutes.
- Java binds by argument count, not types: `getBean(beanName, null, args)`
  fits two overloads, and `def` lists both first.
- Objects defined through macros (`DEFINE_SCHED_CLASS(fair)`), assembly, and
  calls through function pointers stay out of reach.
- No call hierarchy: following a call chain is a `def` and a `show` per hop.
- `show` and `at` cannot print lines outside a symbol.
- `rename` on C still groups every same-named non-static function as one,
  `tools/` copies included.
