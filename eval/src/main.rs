//! The LLM benchmark: each model answers each task with only codix's MCP
//! tools, through Claude Code headless (`claude -p`), on the pinned
//! spring-framework checkout.
//!
//! Run with `cargo run -p codix-eval --release` from a terminal, not from a
//! Claude Code session, whose sandbox has no access to the Claude Code login.
//! Each run starts Claude Code with its own sandbox off. Options:
//! `--models haiku,sonnet,opus` (the default), `--tasks <id>,<id>` (default
//! all), `--claude <path>` (default `claude`). Writes
//! `eval/results/<version>.json` and the table in `eval/README.md`; raw
//! transcripts go to `target/eval/`.

mod report;
mod score;
mod task;
mod transcript;

use std::collections::BTreeSet;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use codix_benches::{SPRING, build_codix, checkout, workspace};
use serde::Serialize;

use crate::report::{Run, summarize, table};
use crate::score::{Arguments, defects};
use crate::task::{Category, Task, renamed_as_expected};

const MODELS: &[&str] = &["haiku", "sonnet", "opus"];
const TABLE_START: &str = "<!-- results -->";
const TABLE_END: &str = "<!-- /results -->";

struct Options {
    models: Vec<String>,
    tasks: Option<Vec<String>>,
    claude: String,
}

fn options() -> Options {
    let mut options = Options {
        models: MODELS.iter().map(|m| (*m).to_owned()).collect(),
        tasks: None,
        claude: "claude".to_owned(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let value = args.next().unwrap_or_else(|| panic!("{arg} needs a value"));
        let list = || value.split(',').map(str::to_owned).collect::<Vec<_>>();
        match arg.as_str() {
            "--models" => options.models = list(),
            "--tasks" => options.tasks = Some(list()),
            "--claude" => options.claude.clone_from(&value),
            other => {
                panic!("unknown option {other}; see the comment at the top of eval/src/main.rs")
            }
        }
    }
    options
}

#[derive(Serialize)]
struct Results {
    version: &'static str,
    repository: String,
    summary: std::collections::BTreeMap<String, report::Summary>,
    runs: Vec<Run>,
}

fn main() {
    let options = options();
    let root = workspace();
    let mut tasks = task::load(&root.join("eval/tasks"));
    if let Some(wanted) = &options.tasks {
        tasks.retain(|t| wanted.contains(&t.id));
    }
    let arguments = tool_arguments(&root.join(format!("schemas/v{}", schema_version(&root))));

    let codix = build_codix();
    let repo = checkout(&root.join("target/bench-repos"), SPRING);
    let out = root.join("target/eval");
    let mcp_config = prepare(&codix, &repo, &out);

    // Rename tasks write files: the checkout is restored after each one.
    let sources = tasks
        .iter()
        .any(|t| t.category == Category::Rename)
        .then(|| Sources::read(&repo));
    let mut runs = Vec::new();
    for model in &options.models {
        for task in &tasks {
            let stream = run_claude(&options.claude, model, &task.prompt, &mcp_config, &repo);
            let renamed = sources
                .as_ref()
                .map(|s| s.check_and_restore(&repo, task, &codix));
            fs::write(out.join(format!("{model}-{}.jsonl", task.id)), &stream)
                .expect("save the transcript");
            let transcript = transcript::parse(&stream);
            let leaks = transcript.setup.leaks();
            assert!(
                leaks.is_empty(),
                "the {model} run of {} is not a clean setup, so the benchmark stops: {}",
                task.id,
                leaks.join("; ")
            );
            let run = Run {
                model: model.clone(),
                task: task.id.clone(),
                category: task.category,
                success: transcript.error.is_none()
                    && task.answered_by(&transcript.answer)
                    && renamed.is_none_or(|ok| ok),
                turns: transcript.turns,
                tool_calls: transcript.calls.len(),
                input_tokens: transcript.input_tokens,
                output_tokens: transcript.output_tokens,
                cost_usd: transcript.cost_usd,
                defects: defects(task, &transcript, &arguments),
                answer: transcript.answer.clone(),
                error: transcript.error.clone(),
            };
            let mut stdout = std::io::stdout().lock();
            writeln!(
                stdout,
                "{model:6}  {:32}  {}  {} calls  {} defects",
                task.id,
                if run.success { "answered" } else { "missed  " },
                run.tool_calls,
                run.defects.len()
            )
            .ok();
            runs.push(run);
        }
    }
    // The servers' daemon for the checkout would otherwise run until idle.
    Command::new(&codix)
        .args(["daemon", "stop"])
        .current_dir(&repo)
        .stdout(Stdio::null())
        .status()
        .ok();

    let summary = summarize(&runs);
    let version = env!("CARGO_PKG_VERSION");
    let results = Results {
        version,
        repository: format!("{} {}", SPRING.name, SPRING.tag),
        summary: summary.clone(),
        runs,
    };
    let path = root.join(format!("eval/results/{version}.json"));
    fs::create_dir_all(root.join("eval/results")).expect("create eval/results");
    fs::write(
        &path,
        serde_json::to_string_pretty(&results).expect("serialize the results") + "\n",
    )
    .expect("write the results");
    update_readme(&root.join("eval/README.md"), &table(&summary), version);
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "\n{}\nresults: {}", table(&summary), path.display()).ok();
}

/// Indexes the checkout if needed and writes the MCP configuration the
/// runs use; its path.
fn prepare(codix: &Path, repo: &Path, out: &Path) -> PathBuf {
    if !repo.join(".codix/index.db").is_file() {
        let status = Command::new(codix)
            .arg("init")
            .current_dir(repo)
            .stdout(Stdio::null())
            .status()
            .expect("run codix init");
        assert!(status.success(), "codix init failed");
    }
    fs::create_dir_all(out).expect("create target/eval");
    let mcp_config = out.join("mcp.json");
    fs::write(
        &mcp_config,
        serde_json::json!({ "mcpServers": { "codix": { "command": codix, "args": ["mcp"] } } })
            .to_string(),
    )
    .expect("write the MCP configuration");
    mcp_config
}

/// The checkout's files as they were before any rename task.
struct Sources(std::collections::BTreeMap<PathBuf, Vec<u8>>);

impl Sources {
    fn read(repo: &Path) -> Self {
        fn walk(dir: &Path, files: &mut std::collections::BTreeMap<PathBuf, Vec<u8>>) {
            for entry in fs::read_dir(dir).expect("read a directory").flatten() {
                let path = entry.path();
                let name = entry.file_name();
                if name == ".codix" || name == ".claude" {
                    continue;
                }
                if path.is_dir() {
                    walk(&path, files);
                } else if let Ok(bytes) = fs::read(&path) {
                    files.insert(path, bytes);
                }
            }
        }
        let mut files = std::collections::BTreeMap::new();
        walk(repo, &mut files);
        Self(files)
    }

    /// For a rename task, whether the files changed exactly as expected;
    /// then puts every changed file back and drops pending previews.
    fn check_and_restore(&self, repo: &Path, task: &Task, codix: &Path) -> bool {
        let changed: Vec<(&PathBuf, &Vec<u8>, Vec<u8>)> = self
            .0
            .iter()
            .filter_map(|(path, before)| {
                let now = fs::read(path).unwrap_or_default();
                (now != *before).then_some((path, before, now))
            })
            .collect();
        let expected: BTreeSet<PathBuf> = task.renames.iter().map(|r| repo.join(&r.file)).collect();
        let actual: BTreeSet<PathBuf> = changed.iter().map(|(p, _, _)| (*p).clone()).collect();
        let ok = task.category != Category::Rename
            || (expected == actual
                && changed.iter().all(|(path, before, now)| {
                    let renames: Vec<&task::Rename> = task
                        .renames
                        .iter()
                        .filter(|r| repo.join(&r.file) == **path)
                        .collect();
                    renamed_as_expected(
                        &String::from_utf8_lossy(before),
                        &String::from_utf8_lossy(now),
                        &renames,
                    )
                }));
        for (path, before, _) in &changed {
            fs::write(path, before).expect("restore a file");
        }
        Command::new(codix)
            .args(["discard", "--all"])
            .current_dir(repo)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .ok();
        ok
    }
}

/// Runs one task, returning the stream-json transcript.
fn run_claude(claude: &str, model: &str, prompt: &str, mcp_config: &Path, repo: &Path) -> String {
    let output = Command::new(claude)
        .arg("-p")
        .arg(prompt)
        .args(["--model", model, "--mcp-config"])
        .arg(mcp_config)
        .args([
            "--strict-mcp-config",
            "--tools",
            "",
            "--allowedTools",
            "mcp__codix__*",
            "--setting-sources",
            "",
            // Built-in tools are off, so the sandbox guards nothing here;
            // left on, it would confine codix's MCP server and its daemon.
            // Hooks and auto memory have no place in a measurement either.
            "--settings",
            r#"{"sandbox":{"enabled":false},"disableAllHooks":true,"autoMemoryEnabled":false}"#,
            "--no-session-persistence",
            "--output-format",
            "stream-json",
            "--verbose",
        ])
        .current_dir(repo)
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|e| panic!("cannot run {claude}: {e}"));
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The arguments each tool takes: its request schema's properties, and `cwd`.
fn tool_arguments(schemas: &Path) -> Arguments {
    let mut arguments = Arguments::new();
    for entry in fs::read_dir(schemas)
        .expect("read the schema snapshots")
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(tool) = name.strip_suffix(".request.json") else {
            continue;
        };
        let schema: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(entry.path()).expect("read a schema"))
                .expect("parse a schema");
        let mut names: BTreeSet<String> = schema["properties"]
            .as_object()
            .map(|p| p.keys().cloned().collect())
            .unwrap_or_default();
        names.insert("cwd".to_owned());
        arguments.insert(tool.to_owned(), names);
    }
    arguments
}

/// The newest `schemas/v<N>` directory.
fn schema_version(root: &Path) -> u32 {
    fs::read_dir(root.join("schemas"))
        .expect("read schemas/")
        .flatten()
        .filter_map(|e| {
            e.file_name()
                .to_string_lossy()
                .strip_prefix('v')?
                .parse()
                .ok()
        })
        .max()
        .expect("a schemas/v<N> directory")
}

/// Replaces the table between the markers in the README.
fn update_readme(path: &PathBuf, table: &str, version: &str) {
    let readme = fs::read_to_string(path).expect("read eval/README.md");
    let (Some(start), Some(end)) = (readme.find(TABLE_START), readme.find(TABLE_END)) else {
        panic!("eval/README.md has no {TABLE_START} … {TABLE_END} section");
    };
    let updated = format!(
        "{}{TABLE_START}\nResults of codix {version} (`eval/results/{version}.json`):\n\n{table}{}",
        &readme[..start],
        &readme[end..]
    );
    fs::write(path, updated).expect("write eval/README.md");
}
