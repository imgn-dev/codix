//! The performance targets, on pinned public repositories: cold index of
//! about 1M lines in 10k files under 30 s, no-change scan of a very large
//! tree under 1 s, CLI lookup p95 under 150 ms; and with a daemon, query p95
//! under 10 ms, memory under 500 MB, reindex after a save under 50 ms; and
//! renames that, applied and renamed back, leave every file as it was; and
//! the modular flavour, with every plugin, as fast to index as the
//! monolithic build, within 5 %.
//!
//! Run with `cargo bench -p codix-benches` from a terminal, not from a Claude
//! Code session, whose sandbox lets no daemon bind its socket. Builds the
//! release binary with every provider and the modular flavour with every
//! plugin (`cargo xtask dist`), downloads the repositories into
//! `target/bench-repos` once, and writes `benches/results/<version>.json`.

use std::io::Write as _;
use std::path::Path;
use std::time::Duration;

mod daemon;
mod edits;
mod plugins;

use codix_benches::{
    Pinned, SPRING, build_codix, build_modular, checkout, files_and_lines, percentile, time,
    workspace,
};
use serde::Serialize;

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
    daemon: daemon::DaemonMeasures,
    /// The index of the cold-indexed repository, in MB.
    index_mb: u64,
    edits: edits::EditMeasures,
    linux_round_trips: edits::RoundTrips,
    /// The modular flavour, with every plugin.
    plugins: plugins::PluginMeasures,
    targets: Vec<Target>,
}

#[derive(Serialize)]
struct Measure {
    repository: String,
    files: usize,
    lines: usize,
    seconds: f64,
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

/// Everything the targets are checked against.
struct Measured {
    spring_files: usize,
    spring_lines: usize,
    cold: Duration,
    linux_files: usize,
    scan: Duration,
    p95: Duration,
    served: daemon::DaemonMeasures,
    edited: edits::EditMeasures,
    linux_trips: edits::RoundTrips,
    modular: plugins::PluginMeasures,
}

fn targets(m: &Measured) -> Vec<Target> {
    let &Measured {
        spring_files,
        spring_lines,
        cold,
        linux_files,
        scan,
        p95,
        ref served,
        ref edited,
        ref linux_trips,
        ref modular,
    } = m;
    let trips = [&edited.round_trips, linux_trips];
    vec![
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
        Target {
            name: "query via daemon p95",
            goal: "under 10 ms, through the CLI",
            measured: format!(
                "{} ms through the CLI, {} ms through MCP",
                served.cli_p95_ms, served.mcp_p95_ms
            ),
            met: served.cli_p95_ms < 10.0,
        },
        Target {
            name: "daemon memory",
            goal: "under 500 MB on a 1M-line project",
            measured: format!(
                "{} MB on {spring_lines} lines, after 1,000 queries",
                served.memory_mb
            ),
            met: served.memory_mb < 500,
        },
        Target {
            name: "reindex after one save",
            goal: "under 50 ms, from the end of the debounce",
            measured: format!("{} ms", served.reindex_ms),
            met: served.reindex_ms < 50,
        },
        Target {
            name: "rename round trips",
            goal: "renamed and back, every file as it was",
            measured: trips
                .iter()
                .map(|t| {
                    format!(
                        "{}: {} of {} restored, {} refused, {} failed",
                        t.repository,
                        t.restored,
                        t.attempted,
                        t.refused.values().sum::<usize>(),
                        t.failed.len()
                    )
                })
                .collect::<Vec<_>>()
                .join("; "),
            met: trips.iter().all(|t| t.failed.is_empty()),
        },
        Target {
            name: "modular cold index",
            goal: "within 5 % of the monolithic build",
            measured: format!(
                "{:.2} s modular, {:.2} s monolithic (medians of 3)",
                modular.cold_modular_seconds, modular.cold_full_seconds
            ),
            met: modular.cold_modular_seconds <= modular.cold_full_seconds * 1.05,
        },
        Target {
            name: "modular CLI lookup p95",
            goal: "under 150 ms",
            measured: format!("{:.1} ms", modular.lookup_p95_ms),
            met: modular.lookup_p95_ms < 150.0,
        },
    ]
}

/// The linux checkout's measures: its no-change scan and round trips.
struct Linux {
    files: usize,
    lines: usize,
    scan: Duration,
    trips: edits::RoundTrips,
}

fn linux(codix: &Path, repos: &Path, out: &mut impl std::io::Write) -> Linux {
    let linux = checkout(repos, LINUX);
    let (linux_files, linux_lines) = files_and_lines(&linux);
    if !linux.join(".codix/index.db").exists() {
        let first = cold_index(codix, &linux);
        writeln!(
            out,
            "first index {}: {:.1} s",
            LINUX.name,
            first.as_secs_f64()
        )
        .expect("write");
    }
    let mut scans: Vec<Duration> = (0..SCAN_RUNS)
        .map(|_| time(codix, &linux, &["index"]))
        .collect();
    let scan = percentile(&mut scans, 50);
    writeln!(
        out,
        "no-change scan {}: {linux_files} files, median {:.3} s",
        LINUX.name,
        scan.as_secs_f64()
    )
    .expect("write");

    let linux_trips =
        edits::measure(codix, &linux, LINUX.name, &["kernel", "mm", "lib"]).round_trips;
    writeln!(
        out,
        "round trips {}: {} of {} restored",
        LINUX.name, linux_trips.restored, linux_trips.attempted
    )
    .expect("write");
    Linux {
        files: linux_files,
        lines: linux_lines,
        scan,
        trips: linux_trips,
    }
}

/// The modular flavour, with every plugin, against the monolithic `codix`.
fn modular(codix: &Path, spring: &Path, out: &mut impl std::io::Write) -> plugins::PluginMeasures {
    let modular_codix = build_modular();
    let modular = plugins::measure(codix, &modular_codix, spring, LOOKUPS, LOOKUP_RUNS);
    writeln!(
        out,
        "modular: cold index {:.2} s (monolithic {:.2} s), lookup p95 {} ms, first command after installing {}: {} ms",
        modular.cold_modular_seconds,
        modular.cold_full_seconds,
        modular.lookup_p95_ms,
        modular.installed,
        modular.first_command_after_install_ms
    )
    .expect("write");
    modular
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

    let index_mb = std::fs::metadata(spring.join(".codix/index.db")).map_or(0, |m| m.len() >> 20);
    let served = daemon::measure(&codix, &spring);
    writeln!(
        out,
        "daemon: query p95 {} ms (CLI), {} ms (MCP); {} MB; reindex {} ms",
        served.cli_p95_ms, served.mcp_p95_ms, served.memory_mb, served.reindex_ms
    )
    .expect("write");

    let edited = edits::measure(&codix, &spring, SPRING.name, &["."]);
    writeln!(
        out,
        "edits: refs p95 {} ms, rename p95 {} ms, apply p95 {} ms; round trips {} of {} restored",
        edited.refs_p95_ms,
        edited.rename_p95_ms,
        edited.apply_p95_ms,
        edited.round_trips.restored,
        edited.round_trips.attempted
    )
    .expect("write");

    let Linux {
        files: linux_files,
        lines: linux_lines,
        scan,
        trips: linux_trips,
    } = linux(&codix, &repos, &mut out);

    let modular = modular(&codix, &spring, &mut out);

    let measured = Measured {
        spring_files,
        spring_lines,
        cold,
        linux_files,
        scan,
        p95,
        served,
        edited,
        linux_trips,
        modular,
    };
    let targets = targets(&measured);
    let results = Results {
        version: env!("CARGO_PKG_VERSION"),
        cpus: std::thread::available_parallelism().map_or(1, usize::from),
        os: std::env::consts::OS,
        targets,
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
        daemon: measured.served,
        index_mb,
        edits: measured.edited,
        linux_round_trips: measured.linux_trips,
        plugins: measured.modular,
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
