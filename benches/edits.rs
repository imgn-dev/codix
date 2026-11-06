//! The phase 3 measures, with a daemon serving the checkout: `refs` p95,
//! `rename` planning and `apply` p95, and rename round trips: a sample of
//! symbols renamed and renamed back, which must leave every file as it was.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use codix_benches::percentile;
use serde::Serialize;
use serde_json::Value;

/// Symbols renamed and back, per repository.
pub(crate) const SAMPLE: usize = 1000;

/// Symbols whose `refs` are timed.
const REFS_SAMPLE: usize = 100;

/// Seed of the sample, so every run picks the same symbols.
const SEED: u64 = 0x5eed_c0de;

/// Kinds sampled: what a rename names.
const KINDS: &[&str] = &[
    "class",
    "struct",
    "interface",
    "trait",
    "enum",
    "function",
    "method",
    "field",
    "property",
    "constant",
    "variable",
    "type_alias",
];

/// What the rename round trips found.
#[derive(Debug, Serialize)]
pub(crate) struct RoundTrips {
    /// Repository and directories sampled.
    pub(crate) repository: String,
    /// Symbols tried.
    pub(crate) attempted: usize,
    /// Renamed and back, every file as it was.
    pub(crate) restored: usize,
    /// Plans refused, by error code.
    pub(crate) refused: BTreeMap<String, usize>,
    /// Symbols whose round trip changed a file, restored after.
    pub(crate) failed: Vec<String>,
}

/// What the edit commands measured.
#[derive(Debug, Serialize)]
pub(crate) struct EditMeasures {
    /// `refs` p95 through the CLI and the daemon.
    pub(crate) refs_p95_ms: f64,
    /// `rename` planning p95.
    pub(crate) rename_p95_ms: f64,
    /// `apply` p95.
    pub(crate) apply_p95_ms: f64,
    /// The round trips.
    pub(crate) round_trips: RoundTrips,
}

fn ms(d: Duration) -> f64 {
    (d.as_secs_f64() * 10_000.0).round() / 10.0
}

/// Runs codix; its exit code, its JSON output, and how long it took.
fn codix_json(codix: &Path, repo: &Path, args: &[&str]) -> (i32, Value, Duration) {
    let start = Instant::now();
    let out = Command::new(codix)
        .args(args)
        .arg("--json")
        .current_dir(repo)
        .output()
        .expect("run codix");
    let elapsed = start.elapsed();
    let json = serde_json::from_slice(&out.stdout)
        .or_else(|_| serde_json::from_slice(&out.stderr))
        .unwrap_or(Value::Null);
    (out.status.code().unwrap_or(-1), json, elapsed)
}

/// Symbols of `dirs`, from their outlines, as (ID, name, file).
fn symbols(codix: &Path, repo: &Path, dirs: &[&str]) -> Vec<(String, String, String)> {
    fn walk(nodes: &[Value], path: &str, found: &mut Vec<(String, String, String)>) {
        for node in nodes {
            let kind = node["kind"].as_str().unwrap_or_default();
            if KINDS.contains(&kind)
                && let (Some(id), Some(name)) = (node["symbol_id"].as_str(), node["name"].as_str())
            {
                found.push((id.to_owned(), name.to_owned(), path.to_owned()));
            }
            walk(
                node["children"].as_array().map_or(&[], Vec::as_slice),
                path,
                found,
            );
        }
    }
    let mut found = Vec::new();
    for dir in dirs {
        let (_, json, _) = codix_json(codix, repo, &["outline", dir]);
        for file in json["result"]["files"]
            .as_array()
            .map_or(&[][..], Vec::as_slice)
        {
            let path = file["root_path"].as_str().unwrap_or_default();
            walk(
                file["symbols"].as_array().map_or(&[], Vec::as_slice),
                path,
                &mut found,
            );
        }
    }
    found
}

/// A fixed pseudo-random sample, by a linear congruential generator.
fn sample<T: Clone>(items: &[T], count: usize) -> Vec<T> {
    let mut state = SEED;
    let mut picked = Vec::new();
    let mut indices: Vec<usize> = (0..items.len()).collect();
    while picked.len() < count.min(items.len()) {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let remaining = indices.len();
        let i = usize::try_from(state >> 33).unwrap_or(0) % remaining;
        picked.push(items[indices.swap_remove(i)].clone());
    }
    picked
}

/// Plans a rename and applies it; the error code if refused, with the
/// planning and apply times.
fn rename(
    codix: &Path,
    repo: &Path,
    symbol: &str,
    new_name: &str,
) -> Result<(Duration, Duration), String> {
    let args = [
        "rename",
        symbol,
        new_name,
        "--exclude-unresolved",
        "--all-overloads",
    ];
    let (code, json, planned) = codix_json(codix, repo, &args);
    if code != 0 {
        return Err(json["error"]["code"]
            .as_str()
            .unwrap_or("unknown")
            .to_owned());
    }
    let id = json["result"]["id"].as_str().unwrap_or_default().to_owned();
    let (code, json, applied) = codix_json(codix, repo, &["apply", &id]);
    if code != 0 {
        return Err(format!(
            "apply:{}",
            json["error"]["code"].as_str().unwrap_or("unknown")
        ));
    }
    Ok((planned, applied))
}

/// Starts a daemon on `repo`, measures, and stops it.
///
/// # Panics
///
/// When the daemon cannot start.
pub(crate) fn measure(codix: &Path, repo: &Path, label: &str, dirs: &[&str]) -> EditMeasures {
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
    let targets = sample(&symbols(codix, repo, dirs), SAMPLE);

    let mut refs = Vec::new();
    for (id, _, _) in targets.iter().take(REFS_SAMPLE) {
        refs.push(codix_json(codix, repo, &["refs", id]).2);
    }

    let mut planning = Vec::new();
    let mut applying = Vec::new();
    let mut trips = RoundTrips {
        repository: format!("{label} {}", dirs.join(", ")),
        attempted: targets.len(),
        restored: 0,
        refused: BTreeMap::new(),
        failed: Vec::new(),
    };
    for (id, name, file) in &targets {
        let before = std::fs::read(repo.join(file)).unwrap_or_default();
        let fresh = format!("{name}_rt");
        match rename(codix, repo, id, &fresh) {
            Err(code) => {
                *trips.refused.entry(code).or_default() += 1;
                continue;
            }
            Ok((planned, applied)) => {
                planning.push(planned);
                applying.push(applied);
            }
        }
        let (_, json, _) = codix_json(codix, repo, &["def", id]);
        let renamed = json["result"]["renamed"]["to"]
            .as_str()
            .or_else(|| json["result"]["definition"]["symbol_id"].as_str())
            .unwrap_or_default()
            .to_owned();
        let back = rename(codix, repo, &renamed, name);
        if back.is_ok() && std::fs::read(repo.join(file)).unwrap_or_default() == before {
            trips.restored += 1;
        } else {
            trips.failed.push(format!("{id}: {back:?}"));
            // The checkouts are tarballs, not clones: restore the bytes.
            std::fs::write(repo.join(file), &before).expect("restore the file");
        }
    }

    Command::new(codix)
        .args(["daemon", "stop"])
        .current_dir(repo)
        .output()
        .expect("run codix daemon stop");
    let p95 = |samples: &mut Vec<Duration>| {
        if samples.is_empty() {
            0.0
        } else {
            ms(percentile(samples, 95))
        }
    };
    EditMeasures {
        refs_p95_ms: p95(&mut refs),
        rename_p95_ms: p95(&mut planning),
        apply_p95_ms: p95(&mut applying),
        round_trips: trips,
    }
}
