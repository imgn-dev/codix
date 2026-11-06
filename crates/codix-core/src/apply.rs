//! `apply`: writes a preview's edits, and the journal that makes them safe.
//!
//! Under the write lock, each file is checked against the plan. Unchanged
//! files take the planned edits; if any changed outside its edited regions,
//! the preview's operation runs again on the current files and its new
//! edits are taken when every region still reads as planned. Otherwise the
//! apply is refused with `stale_preview`.
//!
//! The journal entry, with the original contents, is written durably before
//! any file. Each file is written to a temp file and renamed into place,
//! after a last check that it did not change. A failure restores the files
//! already replaced; a crash leaves an unfinished entry, which the next
//! writer rolls back.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::config::Config;
use crate::edit::{EditSet, PlannedFile, apply, hash, regions};
use crate::error::Site;
use crate::op::{Context, Operation, Outcome};
use crate::parse::PackIndexer;
use crate::previews::{expired, unknown_preview};
use crate::project::{FileId, Project};
use crate::query::{complete, indexed};
use crate::rename::{RenameRequest, plan};
use crate::render::since_epoch;
use crate::store::{EntryState, JournalEntry, JournalFile, PreviewRow, Store};
use crate::symbol_id::SymbolRef;

/// Aborts the process after this many files are replaced, for crash tests.
pub const CRASH_AFTER_VAR: &str = "CODIX_TEST_CRASH_AFTER";

/// `apply`: applies a preview.
pub struct ApplyOp;

/// Request for [`ApplyOp`].
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ApplyRequest {
    /// The preview ID, as `rename` or `undo` returned it.
    pub id: String,
}

/// What [`ApplyOp`] answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ApplyResponse {
    /// The preview applied.
    pub id: String,
    /// What it did.
    pub description: String,
    /// Files written, relative to the cwd.
    pub files: Vec<String>,
    /// Sites edited.
    pub sites: usize,
    /// Whether files had changed since planning, outside the edited
    /// regions, so the plan was made again on them.
    pub rebased: bool,
}

impl Outcome for ApplyResponse {
    fn has_results(&self) -> bool {
        true
    }
}

fn stale(cause: String, sites: Vec<Site>) -> Error {
    Error::Refused {
        code: "stale_preview",
        cause,
        hint: "plan the edit again".to_owned(),
        sites,
    }
}

/// The edits to write, with the content each file was read with.
struct Current {
    edits: EditSet,
    contents: Vec<(FileId, String)>,
    rebased: bool,
}

impl Operation for ApplyOp {
    const NAME: &'static str = "apply";
    type Request = ApplyRequest;
    type Response = ApplyResponse;

    fn run(&self, ctx: &Context, request: ApplyRequest) -> Result<ApplyResponse, Error> {
        let (project, config) = indexed(ctx)?;
        complete(ctx, project, &config)?;
        let mut store = Store::open(&project.index_path())?;
        let row = store
            .preview(&request.id)?
            .ok_or_else(|| unknown_preview(&request.id))?;
        if expired(&row, config.preview_expiry(), since_epoch()) {
            store.delete_preview(&request.id)?;
            return Err(Error::Refused {
                code: "preview_expired",
                cause: format!("preview {} expired", request.id),
                hint: "plan the edit again; `previews.expiry` sets how long previews last"
                    .to_owned(),
                sites: Vec::new(),
            });
        }
        let current = current(ctx, project, &config, &mut store, &row)?;
        let _access = ctx
            .gate()
            .enter(project, None)?
            .ok_or_else(|| Error::Internal {
                message: "the write lock was given up without a timeout".to_owned(),
            })?;
        recover(project, &mut store)?;
        write(ctx, project, &config, &mut store, &row, &current)?;
        store.delete_preview(&request.id)?;
        Ok(ApplyResponse {
            id: request.id,
            description: row.response.description.clone(),
            files: current
                .contents
                .iter()
                .map(|(id, _)| ctx.location(id, 1, 1).path)
                .collect(),
            sites: current.edits.sites(),
            rebased: current.rebased,
        })
    }
}

/// The edits that apply now: the planned ones when no file changed, else
/// the operation's new plan, when its regions read as planned.
fn current(
    ctx: &Context,
    project: &Project,
    config: &Config,
    store: &mut Store,
    row: &PreviewRow,
) -> Result<Current, Error> {
    let mut contents = Vec::new();
    let mut changed = Vec::new();
    for planned in &row.files {
        let id = FileId::from_canonical(planned.path.as_str());
        let Ok(content) = fs::read_to_string(project.path_of(&id)) else {
            return Err(stale(
                format!("{} is gone", ctx.location(&id, 1, 1).path),
                Vec::new(),
            ));
        };
        if hash(content.as_bytes()) != planned.hash {
            changed.push(id.clone());
        }
        contents.push((id, content));
    }
    if changed.is_empty() {
        return Ok(Current {
            edits: row.edits.clone(),
            contents,
            rebased: false,
        });
    }
    rebase(ctx, project, config, store, row, &contents, &changed)
}

/// The operation's new plan on the changed files, when every edited region
/// still reads as planned.
fn rebase(
    ctx: &Context,
    project: &Project,
    config: &Config,
    store: &mut Store,
    row: &PreviewRow,
    contents: &[(FileId, String)],
    changed: &[FileId],
) -> Result<Current, Error> {
    let sites = |ids: &[FileId]| {
        ids.iter()
            .map(|id| Site {
                location: ctx.location(id, 1, 1),
                note: "changed since planning".to_owned(),
            })
            .collect()
    };
    if row.operation != "rename" {
        return Err(stale(
            "files changed since the preview was planned".to_owned(),
            sites(changed),
        ));
    }
    let mut request: RenameRequest =
        serde_json::from_str(&row.request).map_err(|e| Error::Internal {
            message: format!("preview {} has an unreadable request: {e}", row.response.id),
        })?;
    // A local is named by its location, which lines added above move.
    if let SymbolRef::Location { file, line, column } = SymbolRef::parse(&request.symbol)? {
        let moved = row
            .files
            .iter()
            .find(|f| f.path == file)
            .and_then(|planned| {
                let content = &contents.iter().find(|(id, _)| id.as_str() == file)?.1;
                moved_line(planned, content, line)
            })
            .ok_or_else(|| stale("the renamed definition changed".to_owned(), sites(changed)))?;
        request.symbol = format!("{file}:{moved}:{column}");
    }
    let plan = {
        let snapshot = store.snapshot()?;
        plan(ctx, project, config, &snapshot, &request)?
    };
    let planned_paths: Vec<&str> = row.files.iter().map(|f| f.path.as_str()).collect();
    let new_paths: Vec<&str> = plan.edits.files.iter().map(|f| f.path.as_str()).collect();
    if planned_paths != new_paths {
        return Err(stale(
            "the edit now touches other files".to_owned(),
            sites(changed),
        ));
    }
    let mut differing = Vec::new();
    for (planned, new) in row.files.iter().zip(&plan.edits.files) {
        let now = regions(&plan.file.content, &new.edits);
        for (i, region) in now.iter().enumerate() {
            let same = planned
                .regions
                .get(i)
                .is_some_and(|p| p.text == region.text);
            if !same {
                let id = FileId::from_canonical(new.path.as_str());
                differing.push(Site {
                    location: ctx.location(&id, region.first_line, 1),
                    note: region
                        .text
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .trim()
                        .to_owned(),
                });
            }
        }
        if now.len() < planned.regions.len() {
            let id = FileId::from_canonical(new.path.as_str());
            differing.push(Site {
                location: ctx.location(&id, 1, 1),
                note: format!(
                    "{} site(s) planned are gone",
                    planned.regions.len() - now.len()
                ),
            });
        }
    }
    if !differing.is_empty() {
        return Err(stale(
            format!(
                "{} edited site(s) changed, or are new, since the preview was planned",
                differing.len()
            ),
            differing,
        ));
    }
    Ok(Current {
        contents: vec![(plan.file.id.clone(), plan.file.content.clone())],
        edits: plan.edits,
        rebased: true,
    })
}

/// Where line `line`, the first of one of the planned regions, is now: the
/// nearest place its region's lines are found.
fn moved_line(planned: &PlannedFile, content: &str, line: u32) -> Option<u32> {
    let region = planned.regions.iter().find(|r| r.first_line == line)?;
    let count = (region.last_line - region.first_line + 1) as usize;
    let lines: Vec<&str> = content.split_inclusive('\n').collect();
    lines
        .windows(count)
        .enumerate()
        .filter(|(_, window)| window.concat() == region.text)
        .map(|(i, _)| u32::try_from(i + 1).unwrap_or(u32::MAX))
        .min_by_key(|candidate| candidate.abs_diff(line))
}

/// The journal entry an `undo` preview reverts.
fn undoes(row: &PreviewRow) -> Option<i64> {
    if row.operation != "undo" {
        return None;
    }
    serde_json::from_str::<serde_json::Value>(&row.request)
        .ok()?
        .get("entry")?
        .as_i64()
}

/// The temp file a write goes through, next to the file.
fn temp_path(path: &Path, preview: &str) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!(".{name}.codix-{preview}.tmp"))
}

/// Writes the files under the write lock, journaled.
fn write(
    ctx: &Context,
    project: &Project,
    config: &Config,
    store: &mut Store,
    row: &PreviewRow,
    current: &Current,
) -> Result<(), Error> {
    let mut files = Vec::new();
    for ((id, original), planned) in current.contents.iter().zip(&current.edits.files) {
        // Read again under the lock: it must still be what was planned on.
        let now = fs::read(project.path_of(id)).unwrap_or_default();
        if now != original.as_bytes() {
            return Err(stale(
                format!("{} changed while applying", ctx.location(id, 1, 1).path),
                Vec::new(),
            ));
        }
        let new = apply(original, &planned.edits);
        files.push((id.clone(), original.clone(), new));
    }
    let entry = JournalEntry {
        id: 0,
        preview: row.response.id.clone(),
        operation: row.operation.clone(),
        description: row.response.description.clone(),
        applied: since_epoch(),
        state: EntryState::Writing,
        undoes: undoes(row),
        files: files
            .iter()
            .map(|(id, original, new)| JournalFile {
                path: id.as_str().to_owned(),
                original: original.clone().into_bytes(),
                original_hash: hash(original.as_bytes()),
                new_hash: hash(new.as_bytes()),
            })
            .collect(),
    };
    let entry_id = store.begin_apply(&entry)?;
    let crash_after: Option<usize> = std::env::var(CRASH_AFTER_VAR)
        .ok()
        .and_then(|n| n.parse().ok());
    let mut replaced: Vec<&(FileId, String, String)> = Vec::new();
    for file in &files {
        let (id, original, new) = file;
        let path = project.path_of(id);
        if let Err(error) = replace(&path, original.as_bytes(), new.as_bytes(), &row.response.id) {
            for (id, original, new) in replaced.iter().rev() {
                // Best effort: the journal keeps the originals either way.
                let _ = replace(
                    &project.path_of(id),
                    new.as_bytes(),
                    original.as_bytes(),
                    &row.response.id,
                );
            }
            store.set_entry_state(entry_id, EntryState::RolledBack)?;
            return Err(error);
        }
        replaced.push(file);
        if crash_after == Some(replaced.len()) {
            std::process::abort();
        }
    }
    let ids: Vec<FileId> = files.iter().map(|(id, _, _)| id.clone()).collect();
    crate::scan::record_written(project, &PackIndexer::new(config), store, &ids)?;
    if let Some(undone) = undoes(row) {
        store.set_entry_state(undone, EntryState::Undone)?;
    }
    store.set_entry_state(entry_id, EntryState::Done)
}

/// Replaces a file's content `expected` by `new`, through a temp file next
/// to it, checking it just before the rename.
fn replace(path: &Path, expected: &[u8], new: &[u8], preview: &str) -> Result<(), Error> {
    let io = |source| Error::Io {
        path: path.to_path_buf(),
        source,
    };
    let temp = temp_path(path, preview);
    let permissions = fs::metadata(path).map_err(io)?.permissions();
    {
        let mut file = fs::File::create(&temp).map_err(io)?;
        file.write_all(new).map_err(io)?;
        file.sync_all().map_err(io)?;
    }
    fs::set_permissions(&temp, permissions).map_err(io)?;
    if fs::read(path).map_err(io)? != expected {
        let _ = fs::remove_file(&temp);
        return Err(stale(
            format!("{} changed while applying", path.display()),
            Vec::new(),
        ));
    }
    rename(&temp, path).inspect_err(|_| {
        let _ = fs::remove_file(&temp);
    })
}

/// Renames into place. On Windows a file another process holds open is
/// retried for about a second before the apply fails with `file_locked`.
fn rename(from: &Path, to: &Path) -> Result<(), Error> {
    let attempts = if cfg!(windows) { 10 } else { 1 };
    let mut last = None;
    for attempt in 0..attempts {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) => last = Some(e),
        }
        if attempt + 1 < attempts {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    let source = last.unwrap_or_else(|| std::io::Error::other("rename failed"));
    if cfg!(windows) {
        return Err(Error::Refused {
            code: "file_locked",
            cause: format!("{} stays locked by another program: {source}", to.display()),
            hint: "close the program holding it, then apply again".to_owned(),
            sites: Vec::new(),
        });
    }
    Err(Error::Io {
        path: to.to_path_buf(),
        source,
    })
}

/// Rolls back applies a crash left unfinished: files at their new content
/// get their original back; files at neither are left as they are. Runs
/// under the write lock.
pub(crate) fn recover(project: &Project, store: &mut Store) -> Result<(), Error> {
    for entry in store.entries(EntryState::Writing)? {
        for file in &entry.files {
            let path = project.path_of(&FileId::from_canonical(file.path.as_str()));
            let _ = fs::remove_file(temp_path(&path, &entry.preview));
            let Ok(now) = fs::read(&path) else {
                continue;
            };
            if hash(&now) == file.new_hash {
                replace(&path, &now, &file.original, &entry.preview)?;
            }
        }
        store.set_entry_state(entry.id, EntryState::RolledBack)?;
    }
    Ok(())
}
