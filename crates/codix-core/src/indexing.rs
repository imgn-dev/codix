//! The index commands: `init`, `index` and `status`.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use codix_provider::{Module, Unsupported};
use serde::{Deserialize, Serialize};

use crate::Error;
use crate::config::Config;
use crate::op::{Context, Operation, Outcome};
use crate::parse::PackIndexer;
use crate::project::{DATA_DIR, Project};
use crate::scan::{RACY_MTIME, ScanReport, scan};
use crate::store::{FileStatus, Store};
use crate::vcs::Vcs;
use crate::walk::walk;

/// The `init` operation: creates `.codix/`, keeps it out of version control,
/// and indexes the project.
pub struct InitOp;

/// Request for [`InitOp`]: nothing to choose.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InitRequest {}

/// What `init` did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitResponse {
    /// The project root.
    pub root: PathBuf,
    /// Whether `.codix/` was created, rather than found.
    pub created: bool,
    /// Version control systems detected.
    pub vcs: Vec<String>,
    /// Suggestions for systems whose ignores live in tracked files; given only
    /// when `.codix/` is created.
    pub hints: Vec<String>,
    /// The first index.
    pub scan: ScanReport,
}

impl Outcome for InitResponse {
    fn has_results(&self) -> bool {
        true
    }
}

impl Operation for InitOp {
    const NAME: &'static str = "init";
    type Request = InitRequest;
    type Response = InitResponse;

    fn run(&self, ctx: &Context, _: InitRequest) -> Result<InitResponse, Error> {
        let root = ctx.init_root();
        let data = root.join(DATA_DIR);
        let created = !data.is_dir();
        fs::create_dir_all(&data).map_err(|source| Error::Io {
            path: data.clone(),
            source,
        })?;
        let project = Project::at(&root)?;
        let vcs = Vcs::detect(project.root());
        let hints = vcs.apply_ignore_strategies(&project)?;
        let config = Config::load(&project)?;
        let report = scan(&project, &config, &PackIndexer::new(&config))?;
        Ok(InitResponse {
            root: project.root().to_path_buf(),
            created,
            vcs: vcs.names().into_iter().map(str::to_owned).collect(),
            hints: if created { hints } else { Vec::new() },
            scan: report,
        })
    }
}

/// The `index` operation: brings the index up to date.
pub struct IndexOp;

/// Request for [`IndexOp`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IndexRequest {
    /// Rebuild from nothing instead of scanning for changes.
    #[serde(default)]
    pub full: bool,
}

impl Outcome for ScanReport {
    fn has_results(&self) -> bool {
        true
    }
}

impl Operation for IndexOp {
    const NAME: &'static str = "index";
    type Request = IndexRequest;
    type Response = ScanReport;

    fn run(&self, ctx: &Context, request: IndexRequest) -> Result<ScanReport, Error> {
        let project = ctx.project()?;
        if !project.data_dir().is_dir() {
            return Err(Error::NotIndexed {
                root: project.root().to_path_buf(),
            });
        }
        if request.full {
            remove_index(project)?;
        }
        let config = Config::load(project)?;
        scan(project, &config, &PackIndexer::new(&config))
    }
}

/// Deletes the index database and its WAL files.
fn remove_index(project: &Project) -> Result<(), Error> {
    let index = project.index_path();
    for suffix in ["", "-wal", "-shm"] {
        let path = PathBuf::from(format!("{}{suffix}", index.display()));
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => return Err(Error::Io { path, source }),
        }
    }
    Ok(())
}

/// The `status` operation: the state of the index, like `git status`.
pub struct StatusOp;

/// Request for [`StatusOp`]: nothing to choose.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StatusRequest {}

/// Files by how the index holds them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileCounts {
    /// Parsed for symbols.
    pub indexed: usize,
    /// Searched as text only.
    pub search_only: usize,
    /// Waiting to be reparsed.
    pub pending: usize,
    /// Could not be read or parsed.
    pub errors: usize,
    /// Binary, kept out of search.
    pub skipped: usize,
}

/// Files whose stat differs from the index, found without reading them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Changes {
    /// Not in the index.
    pub new: Vec<String>,
    /// Size, mtime or inode changed.
    pub modified: Vec<String>,
    /// In the index, gone from disk.
    pub deleted: Vec<String>,
}

/// What `status` reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusResponse {
    /// The project root.
    pub root: PathBuf,
    /// Files in the index.
    pub files: FileCounts,
    /// Changes since the last scan.
    pub changed: Changes,
    /// Files whose last scan failed, with the reason.
    pub errors: Vec<String>,
    /// Modules found in build files.
    pub modules: Vec<Module>,
    /// Build constructs codix could not interpret.
    pub build_issues: Vec<Unsupported>,
}

impl Outcome for StatusResponse {
    fn has_results(&self) -> bool {
        true
    }
}

impl Operation for StatusOp {
    const NAME: &'static str = "status";
    type Request = StatusRequest;
    type Response = StatusResponse;

    fn run(&self, ctx: &Context, _: StatusRequest) -> Result<StatusResponse, Error> {
        let project = ctx.project()?;
        project.require_indexed()?;
        let config = Config::load(project)?;
        let mut store = Store::open(&project.index_path())?;
        let snapshot = store.snapshot()?;
        let rows = snapshot.files()?;
        let build = snapshot.build()?;

        let mut files = FileCounts::default();
        let mut errors = Vec::new();
        for row in &rows {
            match row.status {
                FileStatus::Indexed => files.indexed += 1,
                FileStatus::SearchOnly => files.search_only += 1,
                FileStatus::Pending => files.pending += 1,
                FileStatus::Skipped => files.skipped += 1,
                FileStatus::Error => {
                    files.errors += 1;
                    errors.push(row.path.clone());
                }
            }
        }

        let walked = walk(
            project,
            &Vcs::detect(project.root()),
            config.max_file_size(),
        )?;
        let manifest: HashMap<&str, _> = rows.iter().map(|row| (row.path.as_str(), row)).collect();
        let mut changed = Changes::default();
        for file in &walked.files {
            match manifest.get(file.id.as_str()) {
                None => changed.new.push(file.id.as_str().to_owned()),
                Some(row) => {
                    // A racily clean file's mtime is not stored; its size and
                    // inode are all a stat check can compare.
                    let mtime_differs = row.mtime_ns != RACY_MTIME && row.mtime_ns != file.mtime_ns;
                    if row.size != file.size || row.inode != file.inode || mtime_differs {
                        changed.modified.push(file.id.as_str().to_owned());
                    }
                }
            }
        }
        let on_disk: std::collections::HashSet<&str> =
            walked.files.iter().map(|f| f.id.as_str()).collect();
        changed.deleted = rows
            .iter()
            .filter(|row| !on_disk.contains(row.path.as_str()))
            .map(|row| row.path.clone())
            .collect();

        Ok(StatusResponse {
            root: project.root().to_path_buf(),
            files,
            changed,
            errors,
            modules: build.modules,
            build_issues: build.unsupported,
        })
    }
}
