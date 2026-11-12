//! `undo`: a preview reverting the latest apply not undone yet, applied
//! like any other with `apply`. An undo is not itself undone: running
//! `undo` again goes one apply further back, through the journal's last 20.

use std::fs;

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::edit::{Edit, EditSet, PreviewResponse, diff, hash, plan_file};
use crate::error::Site;
use crate::op::{Context, Operation};
use crate::project::FileId;
use crate::query::indexed;
use crate::render::since_epoch;
use crate::store::{EntryState, PreviewRow, Store};

/// `undo`: plans reverting the latest apply.
pub struct UndoOp;

/// Request for [`UndoOp`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct UndoRequest {}

/// The smallest edit turning `current` into `original`: what lies between
/// their common start and their common end.
fn minimal_edit(current: &str, original: &str) -> Edit {
    let prefix = current
        .char_indices()
        .zip(original.chars())
        .find(|((_, a), b)| a != b)
        .map_or(current.len().min(original.len()), |((i, _), _)| i);
    let room = current.len().min(original.len()) - prefix;
    let suffix = current[prefix..]
        .chars()
        .rev()
        .zip(original[prefix..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .scan(0, |total, len| {
            *total += len;
            Some(*total)
        })
        .take_while(|total| *total <= room)
        .last()
        .unwrap_or(0);
    Edit {
        start: prefix,
        end: current.len() - suffix,
        replacement: original[prefix..original.len() - suffix].to_owned(),
    }
}

/// The edits turning `current` back into `original`, one per changed line
/// when the apply kept the line count, as a rename does, so the preview
/// shows each site; otherwise a single edit around every change.
fn edits_back(current: &str, original: &str) -> Vec<Edit> {
    let now: Vec<&str> = current.split_inclusive('\n').collect();
    let before: Vec<&str> = original.split_inclusive('\n').collect();
    if now.len() != before.len() {
        return vec![minimal_edit(current, original)];
    }
    let mut edits = Vec::new();
    let mut offset = 0;
    for (line, was) in now.iter().zip(&before) {
        if line != was {
            let edit = minimal_edit(line, was);
            edits.push(Edit {
                start: offset + edit.start,
                end: offset + edit.end,
                replacement: edit.replacement,
            });
        }
        offset += line.len();
    }
    edits
}

impl Operation for UndoOp {
    const NAME: &'static str = "undo";
    type Request = UndoRequest;
    type Response = PreviewResponse;

    fn run(&self, ctx: &Context, _: UndoRequest) -> Result<PreviewResponse, Error> {
        let (project, _) = indexed(ctx)?;
        let mut store = Store::open(&project.index_path())?;
        let entry = store
            .entries(EntryState::Done)?
            .into_iter()
            .find(|e| e.operation != Self::NAME)
            .ok_or(Error::NothingToUndo)?;
        let mut edits = Vec::new();
        let mut files = Vec::new();
        let mut text = String::new();
        let mut changed = Vec::new();
        let mut sites = 0;
        for file in &entry.files {
            let id = FileId::from_canonical(file.path.as_str());
            let current = fs::read(project.path_of(&id)).unwrap_or_default();
            if hash(&current) != file.new_hash {
                changed.push(Site {
                    location: ctx.location(&id, 1, 1),
                    note: "changed since it was applied".to_owned(),
                });
                continue;
            }
            let (Ok(current), Ok(original)) = (
                String::from_utf8(current),
                std::str::from_utf8(&file.original),
            ) else {
                return Err(Error::Internal {
                    message: format!("{} is not UTF-8", file.path),
                });
            };
            let back = edits_back(&current, original);
            text.push_str(&diff(&file.path, &current, &back));
            files.push(plan_file(&file.path, &current, &back));
            sites += back.len();
            edits.extend(back.into_iter().map(|edit| (file.path.clone(), edit)));
        }
        if !changed.is_empty() {
            return Err(Error::Refused {
                code: "stale_preview",
                cause: format!(
                    "{} file(s) changed since `{}` was applied",
                    changed.len(),
                    entry.description
                ),
                hint: "undo applies only to files left as codix wrote them".to_owned(),
                sites: changed,
            });
        }
        let mut row = PreviewRow {
            created: since_epoch(),
            operation: Self::NAME.to_owned(),
            request: serde_json::json!({ "entry": entry.id }).to_string(),
            edits: EditSet::new(edits)?,
            files,
            response: PreviewResponse {
                id: String::new(),
                description: format!("undo {}", entry.description),
                files: entry.files.len(),
                sites,
                overrides: Vec::new(),
                excluded: Vec::new(),
                ambiguous: Vec::new(),
                diff: text,
            },
        };
        store.insert_preview(&mut row)?;
        Ok(row.response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::apply;

    #[test]
    fn a_rename_is_undone_site_by_site() {
        let original = "let total = 1;\nprint(x);\nreturn total;\n";
        let current = "let sum = 1;\nprint(x);\nreturn sum;\n";
        let back = edits_back(current, original);
        assert_eq!(back.len(), 2);
        assert_eq!(apply(current, &back), original);
        // Lines added or removed: one edit around the change.
        let grown = "let sum = 1;\nnew();\nprint(x);\nreturn sum;\n";
        let back = edits_back(grown, original);
        assert_eq!(back.len(), 1);
        assert_eq!(apply(grown, &back), original);
    }

    #[test]
    fn the_minimal_edit_turns_one_text_into_the_other() {
        for (current, original) in [
            (
                "long net = 1;\nreturn net;\n",
                "long base = 1;\nreturn base;\n",
            ),
            ("abc", "abc"),
            ("aXa", "aa"),
            ("aa", "aXa"),
            ("é net", "é base"),
            ("", "x"),
        ] {
            let edit = minimal_edit(current, original);
            assert_eq!(
                apply(current, &[edit]),
                original,
                "{current:?} → {original:?}"
            );
        }
    }
}
