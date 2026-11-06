//! The apply journal: each apply's plan and the files' original contents,
//! written durably before any file changes, so that a crash mid-apply can
//! be rolled back by the next writer, and an apply can be undone.

use rusqlite::{OptionalExtension, TransactionBehavior, params};

use super::Store;
use crate::Error;

/// The `journal` and `journal_files` tables.
pub(super) const MIGRATION: &str = "
CREATE TABLE journal (
    id INTEGER PRIMARY KEY,
    preview TEXT NOT NULL,
    operation TEXT NOT NULL,
    description TEXT NOT NULL,
    applied INTEGER NOT NULL,
    state TEXT NOT NULL,
    undoes INTEGER
);
CREATE TABLE journal_files (
    entry INTEGER NOT NULL REFERENCES journal(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    original BLOB NOT NULL,
    original_hash TEXT NOT NULL,
    new_hash TEXT NOT NULL
);
CREATE INDEX journal_files_by_entry ON journal_files(entry);
";

/// Entries kept; older ones are dropped.
const KEPT: i64 = 20;

/// Where an apply stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryState {
    /// Files are being written; left so by a crash.
    Writing,
    /// Every file was written.
    Done,
    /// A crash left it unfinished, and the originals were restored.
    RolledBack,
    /// Its changes were undone by a later apply.
    Undone,
}

impl EntryState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Writing => "writing",
            Self::Done => "done",
            Self::RolledBack => "rolled_back",
            Self::Undone => "undone",
        }
    }

    fn parse(text: &str) -> Self {
        match text {
            "writing" => Self::Writing,
            "rolled_back" => Self::RolledBack,
            "undone" => Self::Undone,
            _ => Self::Done,
        }
    }
}

/// One file of an apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalFile {
    /// Root-relative path.
    pub path: String,
    /// Its content before the apply.
    pub original: Vec<u8>,
    /// Hash of `original`.
    pub original_hash: String,
    /// Hash of the content the apply writes.
    pub new_hash: String,
}

/// One apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntry {
    /// Its ID.
    pub id: i64,
    /// The preview applied.
    pub preview: String,
    /// The operation that planned it, such as `rename` or `undo`.
    pub operation: String,
    /// What it did.
    pub description: String,
    /// Seconds since the Unix epoch when it was applied.
    pub applied: u64,
    /// Where it stands.
    pub state: EntryState,
    /// The entry an undo reverted.
    pub undoes: Option<i64>,
    /// Its files.
    pub files: Vec<JournalFile>,
}

impl Store {
    /// Records an apply about to write its files, durably, and drops the
    /// entries beyond the newest kept; its ID.
    pub fn begin_apply(&mut self, entry: &JournalEntry) -> Result<i64, Error> {
        let path = self.path.clone();
        let err = |e: &rusqlite::Error| super::sql_error(&path, e);
        self.conn
            .execute_batch("PRAGMA synchronous = FULL")
            .map_err(|e| err(&e))?;
        let result = (|| {
            let tx = self
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute(
                "INSERT INTO journal (preview, operation, description, applied, state, undoes)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    entry.preview,
                    entry.operation,
                    entry.description,
                    i64::try_from(entry.applied).unwrap_or(i64::MAX),
                    EntryState::Writing.as_str(),
                    entry.undoes,
                ],
            )?;
            let id = tx.last_insert_rowid();
            for file in &entry.files {
                tx.execute(
                    "INSERT INTO journal_files (entry, path, original, original_hash, new_hash)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        id,
                        file.path,
                        file.original,
                        file.original_hash,
                        file.new_hash
                    ],
                )?;
            }
            tx.execute(
                "DELETE FROM journal WHERE id NOT IN
                     (SELECT id FROM journal ORDER BY id DESC LIMIT ?1)",
                [KEPT],
            )?;
            tx.commit()?;
            Ok(id)
        })();
        self.conn
            .execute_batch("PRAGMA synchronous = NORMAL")
            .map_err(|e| err(&e))?;
        result.map_err(|e| err(&e))
    }

    /// Sets an entry's state.
    pub fn set_entry_state(&mut self, id: i64, state: EntryState) -> Result<(), Error> {
        self.conn
            .execute(
                "UPDATE journal SET state = ?1 WHERE id = ?2",
                params![state.as_str(), id],
            )
            .map(|_| ())
            .map_err(|e| self.err(&e))
    }

    /// Entries in a state, newest first.
    pub fn entries(&self, state: EntryState) -> Result<Vec<JournalEntry>, Error> {
        let ids: Vec<i64> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id FROM journal WHERE state = ?1 ORDER BY id DESC")
                .map_err(|e| self.err(&e))?;
            stmt.query_map([state.as_str()], |r| r.get(0))
                .and_then(Iterator::collect)
                .map_err(|e| self.err(&e))?
        };
        let mut entries = Vec::with_capacity(ids.len());
        for id in ids {
            entries.extend(self.entry(id)?);
        }
        Ok(entries)
    }

    /// The entry with this ID, if kept.
    pub fn entry(&self, id: i64) -> Result<Option<JournalEntry>, Error> {
        let head = self
            .conn
            .query_row(
                "SELECT preview, operation, description, applied, state, undoes
                 FROM journal WHERE id = ?1",
                [id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, Option<i64>>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| self.err(&e))?;
        let Some((preview, operation, description, applied, state, undoes)) = head else {
            return Ok(None);
        };
        let mut stmt = self
            .conn
            .prepare(
                "SELECT path, original, original_hash, new_hash FROM journal_files
                 WHERE entry = ?1 ORDER BY path",
            )
            .map_err(|e| self.err(&e))?;
        let files = stmt
            .query_map([id], |r| {
                Ok(JournalFile {
                    path: r.get(0)?,
                    original: r.get(1)?,
                    original_hash: r.get(2)?,
                    new_hash: r.get(3)?,
                })
            })
            .and_then(Iterator::collect)
            .map_err(|e| self.err(&e))?;
        Ok(Some(JournalEntry {
            id,
            preview,
            operation,
            description,
            applied: u64::try_from(applied).unwrap_or(0),
            state: EntryState::parse(&state),
            undoes,
            files,
        }))
    }
}
