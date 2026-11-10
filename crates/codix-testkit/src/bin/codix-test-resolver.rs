//! The test resolver as a plugin. A file `mode` next to the executable, read
//! on each request, makes it misbehave: `crash` exits while resolving,
//! `hang` never answers, `foreign` writes a row for a file it was not given,
//! `dangling` binds a reference to no symbol, and `future` (read at start)
//! speaks protocol 2. Requests are recorded in
//! `requests.log` next to it.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use codix_resolver::{Resolver, Scratch};
use codix_testkit::resolver::Unique;
use rusqlite::Connection;

fn beside(name: &str) -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
        .unwrap_or_else(|| PathBuf::from(name))
}

fn mode() -> String {
    std::fs::read_to_string(beside("mode"))
        .map(|m| m.trim().to_owned())
        .unwrap_or_default()
}

struct Modal(Unique);

impl Resolver for Modal {
    fn resolve(
        &mut self,
        index: &Connection,
        scratch: &mut Scratch,
        files: &[String],
    ) -> Result<Vec<String>, String> {
        match mode().as_str() {
            "crash" => std::process::exit(3),
            "hang" => loop {
                std::thread::sleep(std::time::Duration::from_secs(3600));
            },
            "foreign" => {
                let placeholders = vec!["?"; files.len()].join(",");
                let (reference, target): (i64, i64) = index
                    .query_row(
                        &format!(
                            "SELECT r.id, (SELECT MIN(id) FROM v1_symbols)
                             FROM v1_refs r JOIN v1_files f ON f.id = r.file_id
                             WHERE f.path NOT IN ({placeholders}) LIMIT 1"
                        ),
                        rusqlite::params_from_iter(files),
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .map_err(|e| e.to_string())?;
                scratch.bind(reference, target)?;
                Ok(Vec::new())
            }
            "dangling" => {
                let (reference, path): (i64, String) = index
                    .query_row(
                        "SELECT r.id, f.path FROM v1_refs r JOIN v1_files f ON f.id = r.file_id
                         LIMIT 1",
                        [],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .map_err(|e| e.to_string())?;
                scratch.bind(reference, i64::MAX)?;
                Ok(vec![path])
            }
            _ => self.0.resolve(index, scratch, files),
        }
    }
}

fn main() -> ExitCode {
    if mode() == "future" {
        return future();
    }
    codix_resolver::serve(&mut Modal(Unique {
        log: Some(beside("requests.log")),
    }))
}

/// Answers `hello` with protocol 2, then waits for the end of its input.
fn future() -> ExitCode {
    let mut stdout = std::io::stdout().lock();
    for _ in std::io::stdin().lock().lines() {
        if writeln!(stdout, r#"{{"hello":{{"protocol":2,"views":1}}}}"#)
            .and_then(|()| stdout.flush())
            .is_err()
        {
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}
