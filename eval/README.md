# codix LLM benchmark

No model knows codix from training, so the benchmark checks that names,
descriptions and outputs are enough: each model gets realistic tasks on a
real codebase and only codix's MCP tools, with no instructions about codix.

- **Repository:** spring-framework v6.2.0, the same pinned checkout as the
  performance benchmarks.
- **Tasks:** `tasks/<id>.toml`, in three categories: *locate* a definition,
  *trace* a fact through a definition's source, read an *outline*. An answer
  counts when it contains every expected string.
- **Models:** Claude Haiku, Sonnet and Opus, through Claude Code headless
  (`claude -p`) with every built-in tool off and codix as the only MCP server.
- **Defects,** each a sign of a name or description to improve:
  - *misused command:* a `usage` error, or an ambiguous symbol never resolved;
  - *guessed flag:* an argument the tool does not take;
  - *grep fallback:* `grep` called for a *locate* task.

Run it from a terminal, not from a Claude Code session, whose sandbox has no
access to the Claude Code login. Each run starts Claude Code without the user's
settings, `CLAUDE.md` or plugins, and with its sandbox, hooks and auto memory
off; with every built-in tool off, the sandbox has nothing to guard. The
benchmark stops if a run's setup shows anything but codix's tools:

```
cargo run -p codix-eval --release
cargo run -p codix-eval --release -- --models haiku --tasks trace-assert-not-null
```

It writes `results/<version>.json`, raw transcripts to `target/eval/`, and the
table below.

<!-- results -->
Results of codix 0.1.0 (`eval/results/0.1.0.json`):

| Model | Answered | Tool calls | Misused commands | Guessed flags | Grep fallbacks | Cost |
| --- | --- | --- | --- | --- | --- | --- |
| haiku | 20/20 | 42 | 1 | 0 | 0 | $0.03 |
| opus | 20/20 | 27 | 1 | 0 | 0 | $0.84 |
| sonnet | 20/20 | 27 | 0 | 1 | 0 | $0.42 |
<!-- /results -->
