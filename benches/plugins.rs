//! The plugin targets: a cold index as fast in the modular flavour as in the
//! monolithic one, the lookup p95 and the first command after a plugin
//! install in the modular flavour, and the size of the core and of each
//! plugin.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use codix_benches::{percentile, time};
use serde::Serialize;

/// Cold indexes per flavour, alternated so both see the same machine.
const COLD_RUNS: usize = 3;

/// What the modular flavour measured.
#[derive(Debug, Serialize)]
pub(crate) struct PluginMeasures {
    /// Median cold index of the monolithic `full` build, in seconds.
    pub(crate) cold_full_seconds: f64,
    /// Median cold index of the modular flavour, in seconds.
    pub(crate) cold_modular_seconds: f64,
    /// Lookup p95 of the modular flavour, in ms.
    pub(crate) lookup_p95_ms: f64,
    /// The first command after installing a plugin under a running daemon,
    /// in ms: the daemon is stopped, and the plugin's files parsed.
    pub(crate) first_command_after_install_ms: f64,
    /// The plugin installed, and its files in the checkout.
    pub(crate) installed: String,
    /// The modular core, in bytes.
    pub(crate) core_bytes: u64,
    /// Each plugin, as `<kind>/<name>`, in bytes.
    pub(crate) plugins_bytes: BTreeMap<String, u64>,
}

fn ms(d: Duration) -> f64 {
    (d.as_secs_f64() * 10_000.0).round() / 10.0
}

fn cold(codix: &Path, repo: &Path) -> Duration {
    let data = repo.join(".codix");
    if data.exists() {
        std::fs::remove_dir_all(&data).expect("remove the previous index");
    }
    time(codix, repo, &["init"])
}

/// Measures the modular binary `modular` against the monolithic `full`, on
/// the checkout `repo`; `lookups` are the names timed for the p95.
pub(crate) fn measure(
    full: &Path,
    modular: &Path,
    repo: &Path,
    lookups: &[&str],
    runs: usize,
) -> PluginMeasures {
    let mut full_runs = Vec::new();
    let mut modular_runs = Vec::new();
    for _ in 0..COLD_RUNS {
        full_runs.push(cold(full, repo));
        modular_runs.push(cold(modular, repo));
    }
    let cold_full = percentile(&mut full_runs, 50);
    let cold_modular = percentile(&mut modular_runs, 50);

    let mut samples = Vec::new();
    for _ in 0..runs {
        for name in lookups {
            samples.push(time(modular, repo, &["find", name]));
        }
    }
    let p95 = percentile(&mut samples, 95);

    let plugins = modular
        .parent()
        .and_then(Path::parent)
        .expect("the binary is in <prefix>/bin")
        .join("lib/codix/plugins");
    let (first, installed) = first_command_after_install(modular, repo, &plugins);

    PluginMeasures {
        cold_full_seconds: (cold_full.as_secs_f64() * 1000.0).round() / 1000.0,
        cold_modular_seconds: (cold_modular.as_secs_f64() * 1000.0).round() / 1000.0,
        lookup_p95_ms: ms(p95),
        first_command_after_install_ms: ms(first),
        installed,
        core_bytes: std::fs::metadata(modular).map_or(0, |m| m.len()),
        plugins_bytes: plugin_sizes(&plugins),
    }
}

/// Indexes `repo` without the Kotlin plugin, starts a daemon, puts the
/// plugin back, and times the next lookup.
fn first_command_after_install(modular: &Path, repo: &Path, plugins: &Path) -> (Duration, String) {
    let kotlin = plugins.join("language/kotlin");
    let aside = plugins.join("../kotlin-aside");
    std::fs::rename(&kotlin, &aside).expect("set the Kotlin plugin aside");
    cold(modular, repo);
    let status = Command::new(modular)
        .args(["daemon", "start"])
        .current_dir(repo)
        .status()
        .expect("start the daemon");
    assert!(status.success(), "the daemon did not start");
    std::fs::rename(&aside, &kotlin).expect("install the Kotlin plugin");
    let first = time(modular, repo, &["find", "getBean"]);
    let status = Command::new(modular)
        .args(["daemon", "stop"])
        .current_dir(repo)
        .status()
        .expect("stop the daemon");
    assert!(status.success(), "the daemon did not stop");
    let kotlin_files = kotlin_files(repo);
    (first, format!("kotlin, {kotlin_files} files"))
}

fn kotlin_files(dir: &Path) -> usize {
    let mut count = 0;
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() && entry.file_name() != ".codix" {
                pending.push(path);
            } else if path.extension().is_some_and(|e| e == "kt" || e == "kts") {
                count += 1;
            }
        }
    }
    count
}

fn plugin_sizes(plugins: &Path) -> BTreeMap<String, u64> {
    let mut sizes = BTreeMap::new();
    for kind in std::fs::read_dir(plugins).into_iter().flatten().flatten() {
        for plugin in std::fs::read_dir(kind.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            sizes.insert(
                format!(
                    "{}/{}",
                    kind.file_name().to_string_lossy(),
                    plugin.file_name().to_string_lossy()
                ),
                size(&plugin.path()),
            );
        }
    }
    sizes
}

fn size(path: &Path) -> u64 {
    if path.is_dir() {
        std::fs::read_dir(path)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| size(&e.path()))
            .sum()
    } else {
        std::fs::metadata(path).map_or(0, |m| m.len())
    }
}
