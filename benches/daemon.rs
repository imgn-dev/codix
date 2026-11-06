//! The phase 2 targets, with a daemon serving the checkout: query p95 through
//! the CLI and through MCP, the daemon's memory, and reindex after a save.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use codix_benches::{percentile, time};
use serde::Serialize;
use serde_json::{Value, json};

/// Queries timed through the daemon: one of each lookup command, then the
/// phase 1 names.
const QUERIES: &[&[&str]] = &[
    &["def", "DispatcherServlet.doDispatch"],
    &[
        "outline",
        "spring-core/src/main/java/org/springframework/util/StopWatch.java",
    ],
    &["show", "ClassUtils.getDefaultClassLoader"],
    &["find", "ApplicationContext"],
    &["find", "BeanFactory"],
    &["find", "getBean"],
    &["find", "afterPropertiesSet"],
    &["find", "JdbcTemplate"],
    &["find", "toString"],
    &["find", "nothingByThatName"],
];
const RUNS: usize = 10;
/// Queries before the memory is read.
const QUERIES_BEFORE_MEMORY: usize = 1000;
/// The file saved to measure a reindex.
const SAVED: &str = "spring-core/src/main/java/org/springframework/util/StopWatch.java";

/// What a daemon measured.
#[derive(Debug, Serialize)]
pub(crate) struct DaemonMeasures {
    /// Query p95 through the CLI, process start included.
    pub(crate) cli_p95_ms: f64,
    /// Query p95 through `codix mcp`, request to response.
    pub(crate) mcp_p95_ms: f64,
    /// Resident memory after the queries, in MB.
    pub(crate) memory_mb: u64,
    /// Indexing one saved file, from the end of the debounce to the commit.
    pub(crate) reindex_ms: u64,
}

fn ms(d: Duration) -> f64 {
    (d.as_secs_f64() * 10_000.0).round() / 10.0
}

fn status(codix: &Path, repo: &Path) -> Value {
    let out = Command::new(codix)
        .args(["status", "--json"])
        .current_dir(repo)
        .output()
        .expect("run codix status");
    serde_json::from_slice(&out.stdout).expect("status JSON")
}

fn wait_until(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_mins(5);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Starts a daemon on `repo`, measures, and stops it.
///
/// # Panics
///
/// When the daemon cannot start or a measure fails.
pub(crate) fn measure(codix: &Path, repo: &Path) -> DaemonMeasures {
    let started = Command::new(codix)
        .args(["daemon", "start"])
        .current_dir(repo)
        .output()
        .expect("run codix daemon start");
    assert!(
        started.status.success(),
        "daemon start failed: {}",
        String::from_utf8_lossy(&started.stderr)
    );
    // The watch status is set once the startup scan is done.
    wait_until("the daemon's startup scan is done", || {
        let watch = &status(codix, repo)["result"]["daemon"]["watch"];
        watch["recursive"] == true || watch["watched"].as_u64().unwrap_or(0) > 0
    });

    let mut cli = Vec::new();
    for _ in 0..RUNS {
        for args in QUERIES {
            cli.push(time(codix, repo, args));
        }
    }
    let mut mcp = mcp_samples(codix, repo);
    for i in 0..QUERIES_BEFORE_MEMORY {
        time(codix, repo, QUERIES[i % QUERIES.len()]);
    }
    let memory_mb = memory_mb(codix, repo);
    let reindex_ms = reindex_ms(codix, repo);

    Command::new(codix)
        .args(["daemon", "stop"])
        .current_dir(repo)
        .output()
        .expect("run codix daemon stop");
    DaemonMeasures {
        cli_p95_ms: ms(percentile(&mut cli, 95)),
        mcp_p95_ms: ms(percentile(&mut mcp, 95)),
        memory_mb,
        reindex_ms,
    }
}

/// A `codix mcp` session over its stdin and stdout.
struct Mcp {
    child: std::process::Child,
    input: std::process::ChildStdin,
    output: BufReader<std::process::ChildStdout>,
}

impl Mcp {
    fn start(codix: &Path, repo: &Path) -> Self {
        let mut child = Command::new(codix)
            .arg("mcp")
            .current_dir(repo)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("start codix mcp");
        let input = child.stdin.take().expect("stdin");
        let output = BufReader::new(child.stdout.take().expect("stdout"));
        let mut mcp = Self {
            child,
            input,
            output,
        };
        mcp.request(&json!({
            "jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "codix-benches", "version": "0" }
            }
        }));
        mcp.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        mcp
    }

    fn send(&mut self, message: &Value) {
        writeln!(self.input, "{message}").expect("write");
        self.input.flush().expect("flush");
    }

    /// Sends a request and waits for its response.
    fn request(&mut self, message: &Value) -> Value {
        self.send(message);
        loop {
            let mut line = String::new();
            assert!(
                self.output.read_line(&mut line).expect("read") > 0,
                "codix mcp exited"
            );
            let reply: Value = serde_json::from_str(&line).expect("JSON-RPC");
            if reply["id"] == message["id"] {
                return reply;
            }
        }
    }

    /// Ends the session: the server exits when its input closes.
    fn close(self) {
        let Self {
            mut child, input, ..
        } = self;
        drop(input);
        child.wait().ok();
    }
}

/// The same queries as MCP tool calls, timed from request to response.
fn mcp_samples(codix: &Path, repo: &Path) -> Vec<Duration> {
    let mut mcp = Mcp::start(codix, repo);
    let mut samples = Vec::new();
    let mut id = 1;
    for _ in 0..RUNS {
        for args in QUERIES {
            let arguments = match args {
                ["def" | "show", symbol] => json!({ "symbol": symbol }),
                ["outline", path] => json!({ "path": path }),
                [_, query] => json!({ "query": query }),
                _ => unreachable!("every query has one argument"),
            };
            let call = json!({
                "jsonrpc": "2.0", "id": id, "method": "tools/call",
                "params": { "name": args[0], "arguments": arguments }
            });
            id += 1;
            let start = Instant::now();
            let reply = mcp.request(&call);
            samples.push(start.elapsed());
            assert_eq!(reply["result"]["isError"], false, "{reply}");
        }
    }
    mcp.close();
    samples
}

/// The daemon's resident memory, from `daemon list`.
fn memory_mb(codix: &Path, repo: &Path) -> u64 {
    let out = Command::new(codix)
        .args(["daemon", "list", "--json"])
        .current_dir(repo)
        .output()
        .expect("run codix daemon list");
    let listed: Value = serde_json::from_slice(&out.stdout).expect("list JSON");
    let root = std::fs::canonicalize(repo).expect("canonical repository");
    listed["result"]["daemons"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|d| d["daemon"]["root"].as_str() == root.to_str())
        .and_then(|d| d["stats"]["memory_bytes"].as_u64())
        .expect("the daemon's memory")
        >> 20
}

/// Saves one file with a change, waits for the watcher to index it, and
/// restores the file.
fn reindex_ms(codix: &Path, repo: &Path) -> u64 {
    let path = repo.join(SAVED);
    let original = std::fs::read(&path).expect("read the saved file");
    let mut saved = original.clone();
    saved.extend_from_slice(b"\n// saved by the codix benchmark\n");
    let current = || {
        let diff = &status(codix, repo)["result"]["changed"];
        ["new", "modified", "deleted"]
            .iter()
            .all(|k| diff[k].as_array().is_some_and(Vec::is_empty))
    };
    std::fs::write(&path, &saved).expect("save the file");
    wait_until("the saved file is indexed", current);
    let measured = status(codix, repo)["result"]["daemon"]["watch"]["last_batch_ms"]
        .as_u64()
        .expect("the batch time");
    std::fs::write(&path, &original).expect("restore the file");
    wait_until("the restored file is indexed", current);
    measured
}
