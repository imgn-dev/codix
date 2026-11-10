//! Every command gives the same output, exit code and warnings through a
//! daemon as in-process.
//!
//! `live_*` tests bind real sockets, so they run on CI; locally inside the
//! Claude Code sandbox, the suite runs with `--skip live_`.

use std::fs::File;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, SystemTime};

use codix_testkit::Run;
use codix_testkit::fixture::{BILLING, indexed};

const CODIX: &str = env!("CARGO_BIN_EXE_codix");

fn codix(runtime: &Path, cwd: &Path, args: &[&str]) -> Run {
    let output = Command::new(CODIX)
        .args(args)
        .current_dir(cwd)
        .env("CODIX_RUNTIME_DIR", runtime)
        .env_remove("NO_COLOR")
        .output()
        .expect("run codix");
    Run {
        code: output.status.code(),
        stdout: String::from_utf8(output.stdout).expect("UTF-8"),
        stderr: String::from_utf8(output.stderr).expect("UTF-8"),
    }
}

/// `status` and `capabilities` name the daemon, the one place where the two
/// paths differ.
fn without_daemon_line(run: Run) -> Run {
    let mask = |text: &str| -> String {
        if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(text) {
            if value["command"] == "status" {
                value["result"]["daemon"] = serde_json::Value::Null;
            }
            if value["command"] == "capabilities" && value["result"]["project"].is_object() {
                value["result"]["project"]["daemon"] = serde_json::Value::Null;
            }
            return format!("{value}\n");
        }
        text.split_inclusive('\n')
            .filter(|line| !line.trim_start().starts_with("daemon: "))
            .collect()
    };
    Run {
        stdout: mask(&run.stdout),
        ..run
    }
}

/// Each case: the directory it runs from (relative to the root, or `..` for
/// the root's parent) and its arguments; `{ROOT}` stands for the root.
const CASES: &[(&str, &[&str])] = &[
    (".", &["status"]),
    (".", &["status", "--json"]),
    (".", &["capabilities"]),
    (".", &["capabilities", "--json"]),
    (".", &["config", "--list", "--show-origin"]),
    (
        ".",
        &["config", "--describe", "index.max_file_size", "--json"],
    ),
    (".", &["config", "index.max_file_size"]),
    (".", &["config", "index.max_file_sise", "1MB"]),
    // A local key: `.codix/` is not indexed, so both rounds see the same tree.
    (".", &["config", "index.scan_interval", "5m"]),
    (".", &["config", "--unset", "index.scan_interval"]),
    (".", &["find", "Invoice"]),
    (".", &["find", "Inv", "--prefix", "--json"]),
    (".", &["find", "Invocie", "--fuzzy"]),
    (".", &["find", "Invoice.*", "--glob"]),
    (".", &["find", "total", "--kind", "method", "--limit", "1"]),
    (".", &["find", "total", "--limit", "0"]),
    (".", &["find", "nothing"]),
    (".", &["def", "Invoice"]),
    (".", &["def", "total"]),
    (".", &["def", "total", "--json"]),
    (".", &["def", "nowhere"]),
    (".", &["def", "src/billing/Invoice.java:7:16"]),
    (
        ".",
        &[
            "refs",
            "total",
            "--kind",
            "method",
            "--path",
            "src/billing/Cart.java",
        ],
    ),
    (".", &["refs", "Invoice.total", "--kind", "field", "--json"]),
    (".", &["refs", "nowhere"]),
    (".", &["outline", "src"]),
    (".", &["outline", "src/billing/Invoice.java", "--json"]),
    (".", &["outline", "missing.java"]),
    (".", &["show", "Cart.total", "--context", "1"]),
    (
        ".",
        &[
            "show",
            "total",
            "--kind",
            "method",
            "--path",
            "src/billing/Cart.java",
        ],
    ),
    (".", &["grep", "total"]),
    (".", &["grep", "total(", "--json"]),
    (
        ".",
        &["grep", "total", "--lang", "java", "--kind", "method"],
    ),
    (".", &["index"]),
    (".", &["index", "--json"]),
    (".", &["init"]),
    ("src/billing", &["find", "Invoice"]),
    ("src/billing", &["outline", "."]),
    ("src/billing", &["grep", "total", "--here"]),
    ("..", &["--root", "{ROOT}", "find", "Cart"]),
];

fn run_all(runtime: &Path, root: &Path) -> Vec<Run> {
    let shown = root.display().to_string();
    CASES
        .iter()
        .map(|(dir, args)| {
            let cwd = if *dir == ".." {
                root.parent().expect("parent").to_path_buf()
            } else {
                root.join(dir)
            };
            let args: Vec<String> = args.iter().map(|a| a.replace("{ROOT}", &shown)).collect();
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            without_daemon_line(codix(runtime, &cwd, &args))
        })
        .collect()
}

#[test]
fn live_every_command_answers_the_same_through_a_daemon() {
    let runtime = tempfile::tempdir().expect("runtime dir");
    let (_guard, root) = indexed(CODIX, BILLING);
    // Files written less than a second before a scan are racy, and the next
    // scan rehashes them: the first round's `index` would touch them and the
    // second's would not. Old files and one more scan leave nothing racy.
    for (path, _) in BILLING {
        File::options()
            .write(true)
            .open(root.join(path))
            .and_then(|f| f.set_modified(SystemTime::now() - Duration::from_hours(1)))
            .expect("set mtime");
    }
    assert_eq!(codix(runtime.path(), &root, &["index"]).code, Some(0));

    let in_process = run_all(runtime.path(), &root);
    let started = codix(runtime.path(), &root, &["daemon", "start"]);
    assert_eq!(started.code, Some(0), "{}", started.stderr);
    let through_daemon = run_all(runtime.path(), &root);
    let status = codix(runtime.path(), &root, &["status"]);
    codix(runtime.path(), &root, &["daemon", "stop"]);

    assert!(status.stdout.contains("daemon: pid "), "{}", status.stdout);
    for (((dir, args), a), b) in CASES.iter().zip(&in_process).zip(&through_daemon) {
        assert_eq!(
            a.code, b.code,
            "{dir} {args:?}: exit code\n{}\n{}",
            a.stderr, b.stderr
        );
        assert_eq!(a.stdout, b.stdout, "{dir} {args:?}: stdout");
        assert_eq!(a.stderr, b.stderr, "{dir} {args:?}: stderr");
    }
    // The cases cover results, no results, usage, ambiguity and I/O errors;
    // without Java, nothing has symbols to be ambiguous about.
    let codes: std::collections::BTreeSet<i32> =
        in_process.iter().filter_map(|run| run.code).collect();
    let expected: &[i32] = if cfg!(with_lang_java) {
        &[0, 1, 2, 3, 5]
    } else {
        &[0, 1, 2, 5]
    };
    assert_eq!(codes, expected.iter().copied().collect());
}
