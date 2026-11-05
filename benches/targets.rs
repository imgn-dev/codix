//! The phase 1 performance targets, on pinned public repositories:
//! cold index of about 1M lines in 10k files under 30 s, no-change scan of a
//! very large tree under 1 s, and CLI lookup p95 under 150 ms.
//!
//! Run with `cargo bench -p codix-benches`. Builds the release binary with
//! every provider, downloads the repositories into `target/bench-repos` once,
//! and writes `benches/results/<version>.json`.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use codix_benches::{Pinned, checkout, files_and_lines, percentile, time};
use serde::Serialize;

const SPRING: Pinned = Pinned {
    name: "spring-framework",
    url: "https://github.com/spring-projects/spring-framework",
    tag: "v6.2.0",
    commit: "5024bb72279188f1d0dcfe19e8abdd4bdb9887c8",
};

const LINUX: Pinned = Pinned {
    name: "linux",
    url: "https://github.com/torvalds/linux",
    tag: "v6.12",
    commit: "adc218676eef25575469234709c2d87185ca223a",
};

/// Names looked up for the lookup percentile: common and rare, classes and
/// methods.
const LOOKUPS: &[&str] = &[
    "ApplicationContext",
    "BeanFactory",
    "getBean",
    "afterPropertiesSet",
    "DispatcherServlet",
    "JdbcTemplate",
    "toString",
    "resolveDependency",
    "ConfigurationClassPostProcessor",
    "nothingByThatName",
];

const LOOKUP_RUNS: usize = 10;
const SCAN_RUNS: usize = 5;

#[derive(Serialize)]
struct Target {
    name: &'static str,
    goal: &'static str,
    measured: String,
    met: bool,
}

#[derive(Serialize)]
struct Results {
    version: &'static str,
    cpus: usize,
    os: &'static str,
    cold_index: Measure,
    no_change_scan: Measure,
    lookup_p95_ms: f64,
    targets: Vec<Target>,
}

#[derive(Serialize)]
struct Measure {
    repository: String,
    files: usize,
    lines: usize,
    seconds: f64,
}

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn build_codix() -> PathBuf {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let status = Command::new(cargo)
        .args([
            "build",
            "--release",
            "--quiet",
            "-p",
            "codix-cli",
            "--features",
            "full",
        ])
        .current_dir(workspace())
        .status()
        .expect("run cargo build");
    assert!(status.success(), "building codix failed");
    workspace()
        .join("target/release")
        .join(format!("codix{}", std::env::consts::EXE_SUFFIX))
}

/// A cold index: no `.codix/`, then `codix init`.
fn cold_index(codix: &Path, repo: &Path) -> Duration {
    let data = repo.join(".codix");
    if data.exists() {
        std::fs::remove_dir_all(&data).expect("remove the previous index");
    }
    time(codix, repo, &["init"])
}

fn seconds(d: Duration) -> f64 {
    (d.as_secs_f64() * 1000.0).round() / 1000.0
}

fn main() {
    let codix = build_codix();
    let repos = workspace().join("target/bench-repos");
    let mut out = std::io::stdout().lock();

    let spring = checkout(&repos, SPRING);
    let (spring_files, spring_lines) = files_and_lines(&spring);
    let cold = cold_index(&codix, &spring);
    writeln!(
        out,
        "cold index {}: {spring_files} files, {spring_lines} lines, {:.2} s",
        SPRING.name,
        cold.as_secs_f64()
    )
    .expect("write");

    let mut lookups = Vec::new();
    for _ in 0..LOOKUP_RUNS {
        for name in LOOKUPS {
            lookups.push(time(&codix, &spring, &["find", name]));
        }
    }
    let p95 = percentile(&mut lookups, 95);
    writeln!(out, "lookup p95: {:.1} ms", p95.as_secs_f64() * 1000.0).expect("write");

    let linux = checkout(&repos, LINUX);
    let (linux_files, linux_lines) = files_and_lines(&linux);
    if !linux.join(".codix/index.db").exists() {
        let first = cold_index(&codix, &linux);
        writeln!(
            out,
            "first index {}: {:.1} s",
            LINUX.name,
            first.as_secs_f64()
        )
        .expect("write");
    }
    let mut scans: Vec<Duration> = (0..SCAN_RUNS)
        .map(|_| time(&codix, &linux, &["index"]))
        .collect();
    let scan = percentile(&mut scans, 50);
    writeln!(
        out,
        "no-change scan {}: {linux_files} files, median {:.3} s",
        LINUX.name,
        scan.as_secs_f64()
    )
    .expect("write");

    let results = Results {
        version: env!("CARGO_PKG_VERSION"),
        cpus: std::thread::available_parallelism().map_or(1, usize::from),
        os: std::env::consts::OS,
        targets: vec![
            Target {
                name: "cold index",
                goal: "1M lines across 10k files in under 30 s",
                measured: format!(
                    "{spring_lines} lines across {spring_files} files in {:.2} s",
                    cold.as_secs_f64()
                ),
                met: cold < Duration::from_secs(30),
            },
            Target {
                name: "no-change scan",
                goal: "100k files in under 1 s",
                measured: format!("{linux_files} files in {:.3} s", scan.as_secs_f64()),
                met: scan < Duration::from_secs(1),
            },
            Target {
                name: "CLI lookup p95",
                goal: "under 150 ms",
                measured: format!("{:.1} ms", p95.as_secs_f64() * 1000.0),
                met: p95 < Duration::from_millis(150),
            },
        ],
        cold_index: Measure {
            repository: format!("{} {}", SPRING.name, SPRING.tag),
            files: spring_files,
            lines: spring_lines,
            seconds: seconds(cold),
        },
        no_change_scan: Measure {
            repository: format!("{} {}", LINUX.name, LINUX.tag),
            files: linux_files,
            lines: linux_lines,
            seconds: seconds(scan),
        },
        lookup_p95_ms: (p95.as_secs_f64() * 10_000.0).round() / 10.0,
    };
    let path = workspace().join(format!(
        "benches/results/{}.json",
        env!("CARGO_PKG_VERSION")
    ));
    let json = serde_json::to_string_pretty(&results).expect("serialize results");
    std::fs::write(&path, format!("{json}\n")).expect("write results");
    for target in &results.targets {
        let mark = if target.met { "met" } else { "MISSED" };
        writeln!(
            out,
            "{mark:6}  {}: {} (goal: {})",
            target.name, target.measured, target.goal
        )
        .expect("write");
    }
}
