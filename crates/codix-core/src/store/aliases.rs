//! Symbol aliases: the old IDs of symbols codix renamed, mapped to their new
//! ones, so an ID from before a rename still leads somewhere.

use rusqlite::{OptionalExtension, params};

use super::{Snapshot, Store};
use crate::Error;

/// The `aliases` table.
pub(super) const MIGRATION: &str = "
CREATE TABLE aliases (
    old TEXT PRIMARY KEY,
    new TEXT NOT NULL,
    created INTEGER NOT NULL,
    entry INTEGER NOT NULL
) WITHOUT ROWID;
CREATE INDEX aliases_by_new ON aliases(new);
CREATE INDEX aliases_by_entry ON aliases(entry);
";

/// Aliases older than this are dropped.
const MAX_AGE: u64 = 30 * 24 * 3600;

/// Aliases beyond this many, the oldest, are dropped.
const MAX_COUNT: i64 = 5_000;

impl Store {
    /// Records renames an apply made, collapsing chains: an alias that led
    /// to an old ID now leads to its new one. Drops expired aliases.
    pub fn add_aliases(
        &mut self,
        renames: &[(String, String)],
        entry: i64,
        now: u64,
    ) -> Result<(), Error> {
        let path = self.path.clone();
        let err = |e: &rusqlite::Error| super::sql_error(&path, e);
        let tx = self.conn.transaction().map_err(|e| err(&e))?;
        let created = i64::try_from(now).unwrap_or(i64::MAX);
        for (old, new) in renames {
            tx.execute(
                "UPDATE aliases SET new = ?2 WHERE new = ?1",
                params![old, new],
            )
            .map_err(|e| err(&e))?;
            // A rename back to an aliased ID makes that ID live again.
            tx.execute("DELETE FROM aliases WHERE old = ?1 OR old = new", [new])
                .map_err(|e| err(&e))?;
            tx.execute(
                "INSERT OR REPLACE INTO aliases (old, new, created, entry) VALUES (?1, ?2, ?3, ?4)",
                params![old, new, created, entry],
            )
            .map_err(|e| err(&e))?;
        }
        let oldest = created.saturating_sub(i64::try_from(MAX_AGE).unwrap_or(i64::MAX));
        tx.execute("DELETE FROM aliases WHERE created < ?1", [oldest])
            .map_err(|e| err(&e))?;
        tx.execute(
            "DELETE FROM aliases WHERE old NOT IN
                 (SELECT old FROM aliases ORDER BY created DESC, old LIMIT ?1)",
            [MAX_COUNT],
        )
        .map_err(|e| err(&e))?;
        tx.commit().map_err(|e| err(&e))
    }

    /// Drops the aliases an apply created, when it is undone.
    pub fn remove_aliases(&mut self, entry: i64) -> Result<(), Error> {
        self.conn
            .execute("DELETE FROM aliases WHERE entry = ?1", [entry])
            .map(|_| ())
            .map_err(|e| self.err(&e))
    }
}

impl Snapshot<'_> {
    /// The new ID of a renamed symbol: for a full ID, its alias; for an ID
    /// without its hash, the alias of the one old ID it starts.
    pub fn alias(&self, id: &str) -> Result<Option<String>, Error> {
        let exact: Option<String> = self
            .tx
            .query_row("SELECT new FROM aliases WHERE old = ?1", [id], |r| r.get(0))
            .optional()
            .map_err(|e| self.err(&e))?;
        if exact.is_some() || !id.ends_with('#') && id.contains('#') {
            return Ok(exact);
        }
        let prefix = if id.ends_with('#') {
            id.to_owned()
        } else {
            format!("{id}#")
        };
        let mut stmt = self
            .tx
            .prepare("SELECT new FROM aliases WHERE substr(old, 1, length(?1)) = ?1 LIMIT 2")
            .map_err(|e| self.err(&e))?;
        let found: Vec<String> = stmt
            .query_map([&prefix], |r| r.get(0))
            .and_then(Iterator::collect)
            .map_err(|e| self.err(&e))?;
        Ok(match found.as_slice() {
            [one] => Some(one.clone()),
            _ => None,
        })
    }
}
