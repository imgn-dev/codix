//! Tier 3: once a scan commits, the resolver of each language whose files
//! changed binds their references across files. A compiled-in resolver runs
//! in process; a plugin's runs as its own process, kept for later scans and
//! restarted after a failure, at most [`RESTARTS`] times in a row. Either
//! way it reads the index through the views and writes a scratch database,
//! which the store checks and merges. When it fails, its files keep their
//! tier 2 results and the scan reports a `plugin_invalid` warning.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::Duration;

use codix_provider::{ProviderKind, Source, registry};
use codix_resolver::client::Process;

use crate::Error;
use crate::op::Warning;
use crate::project::Project;
use crate::store::Store;

/// How long a resolver may take for one scan.
pub const TIMEOUT: Duration = Duration::from_secs(600);

/// Failures in a row after which a resolver plugin is no longer started.
pub const RESTARTS: u32 = 3;

/// Resolves the files that changed, by language, and merges the results.
pub(crate) fn run(
    project: &Project,
    store: &mut Store,
    changed: &BTreeMap<String, Vec<String>>,
) -> Result<Vec<Warning>, Error> {
    let registry = registry();
    let mut warnings = Vec::new();
    for (language, files) in changed {
        let source = registry
            .sources
            .get(&(ProviderKind::Language, language.clone()))
            .map(|(source, _)| source);
        let outcome = match source {
            Some(Source::CompiledIn) => match codix_resolver::compiled(language) {
                Some(mut resolver) => {
                    resolve(project, store, language, files, |index, scratch| {
                        codix_resolver::run(resolver.as_mut(), index, scratch, files)
                    })?
                }
                None => continue,
            },
            Some(Source::Plugin(_)) => match registry.language_resolvers.get(language) {
                Some(executable) => in_plugin(project, store, language, executable, files)?,
                None => continue,
            },
            None => continue,
        };
        if let Err(cause) = outcome {
            warnings.push(Warning {
                code: "plugin_invalid".to_owned(),
                message: format!(
                    "the {language} resolver failed: {cause}; its files keep their tier 2 results"
                ),
            });
        }
    }
    Ok(warnings)
}

/// A plugin's resolver process and its failures in a row.
#[derive(Default)]
struct Slot {
    process: Option<Process>,
    failures: u32,
}

fn processes() -> &'static Mutex<HashMap<PathBuf, Slot>> {
    static PROCESSES: OnceLock<Mutex<HashMap<PathBuf, Slot>>> = OnceLock::new();
    PROCESSES.get_or_init(Mutex::default)
}

/// Runs the resolver plugin at `executable`, starting it if needed.
fn in_plugin(
    project: &Project,
    store: &mut Store,
    language: &str,
    executable: &Path,
    files: &[String],
) -> Result<Result<(), String>, Error> {
    let mut processes = processes().lock().unwrap_or_else(PoisonError::into_inner);
    let slot = processes.entry(executable.to_path_buf()).or_default();
    if slot.process.is_none() {
        if slot.failures >= RESTARTS {
            return Ok(Err(format!(
                "not restarted after {RESTARTS} failures in a row"
            )));
        }
        match Process::start(executable) {
            Ok(process) => slot.process = Some(process),
            Err(cause) => {
                slot.failures += 1;
                return Ok(Err(cause));
            }
        }
    }
    let Some(process) = slot.process.as_mut() else {
        return Ok(Err("no resolver process".to_owned()));
    };
    let outcome = resolve(project, store, language, files, |index, scratch| {
        process.resolve(index, scratch, files, TIMEOUT)
    })?;
    if outcome.is_ok() {
        slot.failures = 0;
    } else {
        // Stopped: the next scan starts a fresh one.
        slot.process = None;
        slot.failures += 1;
    }
    Ok(outcome)
}

/// Runs `resolver` with a fresh scratch database, then merges what it wrote
/// for `files` and the dependents it names. The outer error is the index's.
fn resolve(
    project: &Project,
    store: &mut Store,
    language: &str,
    files: &[String],
    resolver: impl FnOnce(&Path, &Path) -> Result<Vec<String>, String>,
) -> Result<Result<(), String>, Error> {
    let scratch = project.data_dir().join(format!("resolve-{language}.db"));
    let remove = |path: &Path| match fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(Error::Io {
            path: path.to_path_buf(),
            source: e,
        }),
        _ => Ok(()),
    };
    remove(&scratch)?;
    if let Err(cause) = codix_resolver::Scratch::create(&scratch) {
        return Err(Error::Internal { message: cause });
    }
    let outcome = match resolver(&project.index_path(), &scratch) {
        Ok(dependents) => {
            let covered: Vec<String> = files
                .iter()
                .cloned()
                .chain(dependents)
                .collect::<BTreeSet<String>>()
                .into_iter()
                .collect();
            store
                .merge_resolutions(&scratch, &covered)?
                .map(|_| ())
                .map_err(|reason| format!("its results were refused: {reason}"))
        }
        Err(cause) => Err(cause),
    };
    remove(&scratch)?;
    Ok(outcome)
}
