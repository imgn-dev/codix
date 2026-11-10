//! What resolvers found: references bound across files, kept apart from the
//! tier 2 results so a file's own scopes are never overwritten. A row goes
//! with its reference, and with its target: reparsing either file drops it.

use std::path::Path;

use rusqlite::params;

use super::{Snapshot, Store, sql_error};
use crate::Error;

/// The `resolutions` table. `state` is 0 for a reference bound to `target`,
/// 1 for one naming something outside the project.
pub(super) const MIGRATION: &str = "
CREATE TABLE resolutions (
    ref INTEGER PRIMARY KEY REFERENCES refs(id) ON DELETE CASCADE,
    target INTEGER REFERENCES symbols(id) ON DELETE CASCADE,
    state INTEGER NOT NULL
);
CREATE INDEX resolutions_by_target ON resolutions(target);
";

/// A reference a resolver bound, with the symbol it names by ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    /// Root-relative path of the reference's file.
    pub path: String,
    /// The reference's name.
    pub name: String,
    /// Its first byte.
    pub start_byte: u32,
    /// The symbol it names; `None` for one outside the project.
    pub target: Option<String>,
}

impl Store {
    /// Replaces what the index holds for the references of `files` with the
    /// rows of the scratch database at `scratch`, in one transaction. The
    /// outer error is the index's; the inner one rejects the scratch
    /// database, which then changes nothing: a row for a reference outside
    /// `files`, or a target that is not a symbol of the index.
    pub fn merge_resolutions(
        &mut self,
        scratch: &Path,
        files: &[String],
    ) -> Result<Result<usize, String>, Error> {
        let path = self.path.clone();
        let err = |e: &rusqlite::Error| sql_error(&path, e);
        let scratch = scratch.to_str().ok_or_else(|| Error::Internal {
            message: format!("{} is not UTF-8", scratch.display()),
        })?;
        self.conn
            .execute("ATTACH DATABASE ?1 AS scratch", [scratch])
            .map_err(|e| err(&e))?;
        let merged = merge(&mut self.conn, files);
        self.conn
            .execute_batch("DETACH DATABASE scratch")
            .map_err(|e| err(&e))?;
        match merged {
            Ok(result) => Ok(result),
            Err(e) => Err(err(&e)),
        }
    }
}

fn merge(
    conn: &mut rusqlite::Connection,
    files: &[String],
) -> rusqlite::Result<Result<usize, String>> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    tx.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS resolving (path TEXT PRIMARY KEY); DELETE FROM resolving;",
    )?;
    {
        let mut insert = tx.prepare("INSERT OR IGNORE INTO resolving (path) VALUES (?1)")?;
        for file in files {
            insert.execute([file])?;
        }
    }
    let outside: i64 = tx.query_row(
        "SELECT COUNT(*) FROM scratch.resolved s
         LEFT JOIN refs r ON r.id = s.ref
         LEFT JOIN files f ON f.id = r.file_id
         WHERE f.path IS NULL OR f.path NOT IN (SELECT path FROM resolving)",
        [],
        |row| row.get(0),
    )?;
    if outside > 0 {
        return Ok(Err(format!(
            "{outside} rows for references outside the files it was given"
        )));
    }
    let dangling: i64 = tx.query_row(
        "SELECT COUNT(*) FROM scratch.resolved s
         LEFT JOIN symbols t ON t.id = s.target
         WHERE s.target IS NOT NULL AND t.id IS NULL",
        [],
        |row| row.get(0),
    )?;
    if dangling > 0 {
        return Ok(Err(format!(
            "{dangling} rows bound to no symbol of the index"
        )));
    }
    tx.execute(
        "DELETE FROM resolutions WHERE ref IN (
             SELECT r.id FROM refs r JOIN files f ON f.id = r.file_id
             WHERE f.path IN (SELECT path FROM resolving))",
        [],
    )?;
    let merged = tx.execute(
        "INSERT INTO resolutions (ref, target, state)
         SELECT ref, target, CASE state WHEN 'bound' THEN 0 ELSE 1 END FROM scratch.resolved",
        [],
    )?;
    tx.commit()?;
    Ok(Ok(merged))
}

impl Snapshot<'_> {
    /// Every reference a resolver bound, by path and position.
    pub fn resolutions(&self) -> Result<Vec<Resolution>, Error> {
        let mut stmt = self
            .tx
            .prepare(
                "SELECT f.path, r.name, r.start_byte, t.symbol_id
                 FROM resolutions x
                 JOIN refs r ON r.id = x.ref
                 JOIN files f ON f.id = r.file_id
                 LEFT JOIN symbols t ON t.id = x.target
                 ORDER BY f.path, r.start_byte",
            )
            .map_err(|e| self.err(&e))?;
        stmt.query_map(params![], |row| {
            Ok(Resolution {
                path: row.get(0)?,
                name: row.get(1)?,
                start_byte: row.get(2)?,
                target: row.get(3)?,
            })
        })
        .and_then(Iterator::collect)
        .map_err(|e| self.err(&e))
    }
}
