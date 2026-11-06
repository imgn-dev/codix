//! Pending edit plans. Planning writes one row in its own short transaction,
//! without the project's write lock, so any number of agents can plan at
//! once.

use rusqlite::{OptionalExtension, TransactionBehavior, params};

use super::Store;
use crate::Error;
use crate::edit::{EditSet, PlannedFile, PreviewResponse, preview_id};

/// The `previews` table. Plans are stored as JSON: only codix reads them,
/// whole.
pub(super) const MIGRATION: &str = "
CREATE TABLE previews (
    id TEXT PRIMARY KEY,
    created INTEGER NOT NULL,
    operation TEXT NOT NULL,
    request TEXT NOT NULL,
    edits TEXT NOT NULL,
    files TEXT NOT NULL,
    response TEXT NOT NULL
) WITHOUT ROWID;
";

/// IDs tried before giving up on finding an unused one.
const ID_ATTEMPTS: usize = 8;

/// A pending plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewRow {
    /// Seconds since the Unix epoch when it was planned.
    pub created: u64,
    /// The operation that planned it, such as `rename`, which `apply` runs
    /// again to rebase it.
    pub operation: String,
    /// That operation's request, as JSON.
    pub request: String,
    /// The edits.
    pub edits: EditSet,
    /// Each edited file as planning found it.
    pub files: Vec<PlannedFile>,
    /// What planning returned; its `id` is the row's.
    pub response: PreviewResponse,
}

fn json<T: serde::Serialize>(value: &T) -> Result<String, Error> {
    serde_json::to_string(value).map_err(|e| Error::Internal {
        message: format!("cannot store a preview: {e}"),
    })
}

fn parse<T: serde::de::DeserializeOwned>(id: &str, text: &str) -> Result<T, Error> {
    serde_json::from_str(text).map_err(|e| Error::Internal {
        message: format!("preview {id} is unreadable: {e}"),
    })
}

impl Store {
    /// Stores a plan under a new ID, which it also sets in the response.
    pub fn insert_preview(&mut self, row: &mut PreviewRow) -> Result<(), Error> {
        let path = self.path.clone();
        let err = |e: &rusqlite::Error| super::sql_error(&path, e);
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| err(&e))?;
        for _ in 0..ID_ATTEMPTS {
            let id = preview_id();
            let taken: bool = tx
                .query_row("SELECT 1 FROM previews WHERE id = ?1", [&id], |_| Ok(()))
                .optional()
                .map_err(|e| err(&e))?
                .is_some();
            if taken {
                continue;
            }
            row.response.id.clone_from(&id);
            tx.execute(
                "INSERT INTO previews (id, created, operation, request, edits, files, response)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    id,
                    i64::try_from(row.created).unwrap_or(i64::MAX),
                    row.operation,
                    row.request,
                    json(&row.edits)?,
                    json(&row.files)?,
                    json(&row.response)?,
                ],
            )
            .map_err(|e| err(&e))?;
            return tx.commit().map_err(|e| err(&e));
        }
        Err(Error::Internal {
            message: "no unused preview ID found".to_owned(),
        })
    }

    /// The plan stored under `id`, if any.
    pub fn preview(&self, id: &str) -> Result<Option<PreviewRow>, Error> {
        let row = self
            .conn
            .query_row(
                "SELECT created, operation, request, edits, files, response
                 FROM previews WHERE id = ?1",
                [id],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, String>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| self.err(&e))?;
        let Some((created, operation, request, edits, files, response)) = row else {
            return Ok(None);
        };
        Ok(Some(PreviewRow {
            created: u64::try_from(created).unwrap_or(0),
            operation,
            request,
            edits: parse(id, &edits)?,
            files: parse(id, &files)?,
            response: parse(id, &response)?,
        }))
    }

    /// Every stored plan, oldest first.
    pub fn previews(&self) -> Result<Vec<PreviewRow>, Error> {
        let ids: Vec<String> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id FROM previews ORDER BY created, id")
                .map_err(|e| self.err(&e))?;
            stmt.query_map([], |r| r.get(0))
                .and_then(Iterator::collect)
                .map_err(|e| self.err(&e))?
        };
        let mut rows = Vec::with_capacity(ids.len());
        for id in ids {
            rows.extend(self.preview(&id)?);
        }
        Ok(rows)
    }

    /// Removes a plan; whether there was one.
    pub fn delete_preview(&mut self, id: &str) -> Result<bool, Error> {
        self.conn
            .execute("DELETE FROM previews WHERE id = ?1", [id])
            .map(|n| n > 0)
            .map_err(|e| self.err(&e))
    }
}
