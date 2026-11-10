//! A resolver for tests: binds each reference its file leaves unbound to the
//! only symbol of that name in the project, when there is exactly one. The
//! files that depend on a changed one are those with such a reference to a
//! name it defines.

use std::collections::BTreeSet;
use std::fs;
use std::io::Write as _;
use std::path::PathBuf;

use codix_resolver::{Resolver, Scratch};
use rusqlite::Connection;

/// The test resolver, recording each request in `log` when given: one line
/// `<files> -> <dependents>`, comma-separated.
#[derive(Debug, Default)]
pub struct Unique {
    /// Where requests are recorded.
    pub log: Option<PathBuf>,
}

impl Resolver for Unique {
    fn resolve(
        &mut self,
        index: &Connection,
        scratch: &mut Scratch,
        files: &[String],
    ) -> Result<Vec<String>, String> {
        let e = |e: rusqlite::Error| e.to_string();
        let given: BTreeSet<&str> = files.iter().map(String::as_str).collect();
        let mut dependents = BTreeSet::new();
        {
            let mut stmt = index
                .prepare(
                    "SELECT DISTINCT f.path FROM v1_refs r JOIN v1_files f ON f.id = r.file_id
                     WHERE r.state <> 'bound' AND r.name IN (
                         SELECT s.name FROM v1_symbols s JOIN v1_files sf ON sf.id = s.file_id
                         WHERE sf.path = ?1)",
                )
                .map_err(e)?;
            for file in files {
                for path in stmt
                    .query_map([file], |row| row.get::<_, String>(0))
                    .map_err(e)?
                {
                    let path = path.map_err(e)?;
                    if !given.contains(path.as_str()) {
                        dependents.insert(path);
                    }
                }
            }
        }
        let mut refs = index
            .prepare(
                "SELECT r.id, r.name FROM v1_refs r JOIN v1_files f ON f.id = r.file_id
                 WHERE f.path = ?1 AND r.state <> 'bound'",
            )
            .map_err(e)?;
        let mut symbols = index
            .prepare("SELECT id FROM v1_symbols WHERE name = ?1 LIMIT 2")
            .map_err(e)?;
        scratch.batch(|scratch| {
            for path in given
                .iter()
                .copied()
                .chain(dependents.iter().map(String::as_str))
            {
                let found: Vec<(i64, String)> = refs
                    .query_map([path], |row| Ok((row.get(0)?, row.get(1)?)))
                    .and_then(Iterator::collect)
                    .map_err(e)?;
                for (reference, name) in found {
                    let targets: Vec<i64> = symbols
                        .query_map([&name], |row| row.get(0))
                        .and_then(Iterator::collect)
                        .map_err(e)?;
                    if let [target] = targets.as_slice() {
                        scratch.bind(reference, *target)?;
                    }
                }
            }
            Ok(())
        })?;
        let dependents: Vec<String> = dependents.into_iter().collect();
        if let Some(log) = &self.log {
            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log)
                .map_err(|e| e.to_string())?;
            writeln!(file, "{} -> {}", files.join(","), dependents.join(","))
                .map_err(|e| e.to_string())?;
        }
        Ok(dependents)
    }
}
