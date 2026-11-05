//! Watch mode: a file watcher feeding changed directories into partial
//! scans, debounced, with full scans at startup and periodically to catch
//! what the watcher missed.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use codix_core::config::Config;
use codix_core::daemon::{Watch, WatchStatus};
use codix_core::lock::{QUERY_WAIT, WriteGate};
use codix_core::parse::PackIndexer;
use codix_core::project::{DATA_DIR, Project};
use codix_core::scan::{Targets, scan, scan_dirs};
use codix_core::store::Store;
use codix_core::vcs::Vcs;
use codix_core::walk::DEFAULT_EXCLUDES;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};

/// Caps the watches added one directory at a time, and makes the watcher add
/// them that way on every platform, for tests only.
pub const MAX_WATCHES_VAR: &str = "CODIX_TEST_MAX_WATCHES";

/// A batch is indexed once no event came for this long…
const QUIET: Duration = Duration::from_millis(100);
/// …or this long after its first event, whichever comes first.
const MAX_BATCH: Duration = Duration::from_secs(1);
/// The full-scan interval while some directory is unwatched.
const DEGRADED_INTERVAL: Duration = Duration::from_mins(1);

enum Message {
    Event(notify::Result<notify::Event>),
    /// Index what is pending now, then answer.
    Flush(Sender<()>),
}

/// The daemon's watcher: owns the scan pipeline on a thread of its own.
#[derive(Debug)]
pub struct Watcher {
    messages: Sender<Message>,
    status: Arc<Mutex<WatchStatus>>,
}

impl Watch for Watcher {
    fn flush(&self) -> bool {
        let (reply, answer) = mpsc::channel();
        if self.messages.send(Message::Flush(reply)).is_err() {
            return false;
        }
        // Busy with a long scan, such as the startup one: the caller scans
        // the tree itself, waiting for the lock as it would without a daemon.
        answer.recv_timeout(QUERY_WAIT).is_ok() && self.status().unwatched == 0
    }

    fn status(&self) -> WatchStatus {
        self.status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// Starts watching `project`: the startup scan, then changes as they come.
pub fn start(project: Project, gate: Arc<dyn WriteGate>, log: fn(&str)) -> Watcher {
    let (messages, inbox) = mpsc::channel();
    let status = Arc::new(Mutex::new(WatchStatus {
        recursive: false,
        watched: 0,
        unwatched: 0,
        rescan_seconds: 0,
        last_batch_ms: None,
    }));
    let events = messages.clone();
    let shared = Arc::clone(&status);
    thread::spawn(move || {
        let mut pipeline = Pipeline {
            skip: skip_dirs(&project),
            project,
            gate,
            log,
            status: shared,
            watcher: None,
            pending: Targets::default(),
            first_event: None,
            last_event: None,
        };
        pipeline.run(&events, &inbox);
    });
    Watcher { messages, status }
}

struct Pipeline {
    project: Project,
    gate: Arc<dyn WriteGate>,
    log: fn(&str),
    status: Arc<Mutex<WatchStatus>>,
    /// Directory names never indexed, so events in them are dropped.
    skip: Vec<String>,
    watcher: Option<RecommendedWatcher>,
    pending: Targets,
    first_event: Option<Instant>,
    last_event: Option<Instant>,
}

impl Pipeline {
    fn run(&mut self, events: &Sender<Message>, inbox: &Receiver<Message>) {
        self.full_scan("startup");
        self.watch(events);
        let mut next_full = Instant::now() + self.interval();
        loop {
            let now = Instant::now();
            let wake = self.batch_due().map_or(next_full, |due| due.min(next_full));
            match inbox.recv_timeout(wake.saturating_duration_since(now)) {
                Ok(Message::Event(Ok(event))) => self.add(&event),
                Ok(Message::Event(Err(error))) => {
                    (self.log)(&format!(
                        "watcher error: {error}; scanning the whole project"
                    ));
                    self.full_scan("watcher error");
                }
                Ok(Message::Flush(reply)) => {
                    self.index_pending();
                    reply.send(()).ok();
                }
                Err(RecvTimeoutError::Timeout) => {
                    if self.batch_due().is_some_and(|due| due <= Instant::now()) {
                        self.index_pending();
                    }
                    if next_full <= Instant::now() {
                        self.full_scan("periodic");
                        next_full = Instant::now() + self.interval();
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    fn config(&self) -> Option<Config> {
        match Config::load(&self.project) {
            Ok(config) => Some(config),
            Err(error) => {
                (self.log)(&format!("cannot load the configuration: {}", error.cause()));
                None
            }
        }
    }

    fn interval(&self) -> Duration {
        let configured = self
            .config()
            .map_or(Duration::from_mins(10), |c| c.scan_interval());
        let interval = if self.status().unwatched > 0 {
            configured.min(DEGRADED_INTERVAL)
        } else {
            configured
        };
        self.update(|s| s.rescan_seconds = interval.as_secs());
        interval
    }

    fn status(&self) -> WatchStatus {
        self.status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn update(&self, change: impl FnOnce(&mut WatchStatus)) {
        change(&mut self.status.lock().unwrap_or_else(PoisonError::into_inner));
    }

    fn full_scan(&self, why: &str) {
        let Some(config) = self.config() else {
            return;
        };
        let start = Instant::now();
        match scan(
            &self.project,
            &config,
            &PackIndexer::new(&config),
            self.gate.as_ref(),
        ) {
            Ok(report) => (self.log)(&format!(
                "{why} scan: {} indexed, {} deleted, {} unchanged in {} ms",
                report.indexed,
                report.deleted,
                report.unchanged,
                start.elapsed().as_millis()
            )),
            Err(error) => (self.log)(&format!("{why} scan failed: {}", error.cause())),
        }
    }

    /// Starts the OS watcher: one recursive watch where the OS offers it,
    /// one watch per directory on Linux, where inotify counts each against
    /// a quota shared by every program of the user.
    fn watch(&mut self, events: &Sender<Message>) {
        let sender = events.clone();
        let mut os_watcher = match notify::recommended_watcher(move |event| {
            sender.send(Message::Event(event)).ok();
        }) {
            Ok(os_watcher) => os_watcher,
            Err(error) => {
                (self.log)(&format!(
                    "cannot watch files: {error}; relying on periodic scans"
                ));
                self.update(|s| s.unwatched = self.directories().len());
                return;
            }
        };
        let cap = std::env::var(MAX_WATCHES_VAR)
            .ok()
            .and_then(|v| v.parse::<usize>().ok());
        if cfg!(target_os = "linux") || cap.is_some() {
            let mut watched = 0;
            let mut unwatched = 0;
            for dir in self.directories() {
                let allowed = cap.is_none_or(|cap| watched < cap);
                if allowed
                    && os_watcher
                        .watch(&self.project.root().join(&dir), RecursiveMode::NonRecursive)
                        .is_ok()
                {
                    watched += 1;
                } else {
                    unwatched += 1;
                }
            }
            if unwatched > 0 {
                (self.log)(&format!(
                    "{unwatched} directories could not be watched; they are rescanned periodically"
                ));
            }
            self.update(|s| {
                s.watched = watched;
                s.unwatched = unwatched;
            });
        } else if let Err(error) = os_watcher.watch(self.project.root(), RecursiveMode::Recursive) {
            (self.log)(&format!(
                "cannot watch the project: {error}; relying on periodic scans"
            ));
            self.update(|s| s.unwatched = self.directories().len());
        } else {
            self.update(|s| s.recursive = true);
        }
        self.watcher = Some(os_watcher);
    }

    /// Every indexed directory, root-relative (`""` is the root).
    fn directories(&self) -> Vec<String> {
        let mut dirs: BTreeSet<String> = Store::open(&self.project.index_path())
            .and_then(|mut store| store.snapshot()?.dirs())
            .map(|dirs| dirs.into_keys().collect())
            .unwrap_or_default();
        dirs.insert(String::new());
        dirs.into_iter().collect()
    }

    /// When the pending batch is due.
    fn batch_due(&self) -> Option<Instant> {
        Some(batch_due(self.first_event?, self.last_event?))
    }

    fn add(&mut self, event: &notify::Event) {
        if matches!(event.kind, EventKind::Access(_)) {
            return;
        }
        let Some(new_dirs) = add_paths(
            self.project.root(),
            &self.skip,
            &event.paths,
            &mut self.pending,
        ) else {
            return;
        };
        for dir in new_dirs {
            self.watch_new(&dir);
        }
        let now = Instant::now();
        self.first_event.get_or_insert(now);
        self.last_event = Some(now);
    }

    /// A directory created after the start needs a watch of its own, where
    /// watches are added one by one.
    fn watch_new(&mut self, dir: &Path) {
        let status = self.status();
        if status.recursive {
            return;
        }
        let cap = std::env::var(MAX_WATCHES_VAR)
            .ok()
            .and_then(|v| v.parse::<usize>().ok());
        let Some(watcher) = self.watcher.as_mut() else {
            return;
        };
        if cap.is_none_or(|cap| status.watched < cap)
            && watcher.watch(dir, RecursiveMode::NonRecursive).is_ok()
        {
            self.update(|s| s.watched += 1);
        } else {
            self.update(|s| s.unwatched += 1);
        }
    }

    fn index_pending(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let targets = std::mem::take(&mut self.pending);
        self.first_event = None;
        self.last_event = None;
        let Some(config) = self.config() else {
            return;
        };
        let start = Instant::now();
        match scan_dirs(
            &self.project,
            &config,
            &PackIndexer::new(&config),
            self.gate.as_ref(),
            &targets,
            None,
        ) {
            Ok(Some(report)) => {
                let millis = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
                self.update(|s| s.last_batch_ms = Some(millis));
                if report.indexed + report.touched + report.deleted > 0 {
                    (self.log)(&format!(
                        "changes: {} indexed, {} touched, {} deleted in {millis} ms",
                        report.indexed, report.touched, report.deleted
                    ));
                }
            }
            Ok(None) => {}
            Err(error) => (self.log)(&format!("indexing changes failed: {}", error.cause())),
        }
    }
}

/// A batch is due after its quiet time, or at most [`MAX_BATCH`] after its
/// first event.
fn batch_due(first: Instant, last: Instant) -> Instant {
    (last + QUIET).min(first + MAX_BATCH)
}

/// Adds what changed `paths` mean for the index to `targets`: the directory
/// holding each, and the subtree of each that is, or may have been, a
/// directory. Paths outside the root or in a skipped directory count for
/// nothing. Returns the directories among them, which may need a watch, or
/// `None` when nothing was added.
fn add_paths(
    root: &Path,
    skip: &[String],
    paths: &[PathBuf],
    targets: &mut Targets,
) -> Option<Vec<PathBuf>> {
    let mut added = false;
    let mut dirs = Vec::new();
    for path in paths {
        let Some(rel) = relative(root, path) else {
            continue;
        };
        if rel.split('/').any(|part| skip.iter().any(|s| s == part)) {
            continue;
        }
        targets.add_dir(rel.rfind('/').map_or("", |i| &rel[..i]));
        if path.is_dir() {
            targets.add_tree(&rel);
            dirs.push(path.clone());
        } else if !path.exists() {
            targets.add_tree(&rel);
        }
        added = true;
    }
    added.then_some(dirs)
}

/// Directory names whose events are dropped: `.codix/` itself, the default
/// excludes, and the version control systems' own directories.
fn skip_dirs(project: &Project) -> Vec<String> {
    Vcs::detect(project.root())
        .skip_dirs()
        .into_iter()
        .chain(DEFAULT_EXCLUDES.iter().copied())
        .chain([DATA_DIR])
        .map(str::to_owned)
        .collect()
}

/// `path` relative to `root`, with `/` separators; `None` outside it or for
/// the root itself.
fn relative(root: &Path, path: &Path) -> Option<String> {
    let rel: PathBuf = path.strip_prefix(root).ok()?.to_path_buf();
    let parts: Option<Vec<&str>> = rel.iter().map(|part| part.to_str()).collect();
    let rel = parts?.join("/");
    (!rel.is_empty()).then_some(rel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_batch_waits_for_quiet_but_not_forever() {
        let start = Instant::now();
        assert_eq!(batch_due(start, start), start + QUIET);
        let busy = start + Duration::from_millis(950);
        assert_eq!(batch_due(start, busy), start + MAX_BATCH);
    }

    #[test]
    fn events_map_to_the_directories_to_rescan() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = codix_core::project::canonicalize(dir.path()).expect("canonical");
        std::fs::create_dir_all(root.join("src/new")).expect("mkdir");
        std::fs::write(root.join("src/a.rs"), "").expect("write");
        let skip = vec![".codix".to_owned(), "target".to_owned()];
        let mut targets = Targets::default();
        let dirs = add_paths(
            &root,
            &skip,
            &[
                root.join("src/a.rs"),
                root.join("src/new"),
                root.join("src/gone"),
                root.join("README"),
            ],
            &mut targets,
        );
        assert_eq!(dirs, Some(vec![root.join("src/new")]));
        let mut expected = Targets::default();
        expected.add_dir("src");
        expected.add_dir("");
        expected.add_tree("src/new");
        expected.add_tree("src/gone");
        expected.add_tree("README");
        assert_eq!(targets, expected);

        let mut untouched = Targets::default();
        let skipped = [
            root.join(".codix/index.db-wal"),
            root.join("target/debug/x"),
            PathBuf::from("/elsewhere/file"),
        ];
        assert_eq!(add_paths(&root, &skip, &skipped, &mut untouched), None);
        assert!(untouched.is_empty());
    }

    #[test]
    fn paths_are_made_relative_to_the_root() {
        let root = Path::new("/work/p");
        assert_eq!(
            relative(root, Path::new("/work/p/a/b.rs")).as_deref(),
            Some("a/b.rs")
        );
        assert_eq!(relative(root, Path::new("/work/p")), None);
        assert_eq!(relative(root, Path::new("/work/other/x")), None);
    }
}
