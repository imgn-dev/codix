//! The plugins' exit check: on spring-framework and linux, the modular
//! flavour with every plugin and the monolithic `full` build give the same
//! `files`, `symbols`, `refs` and `modules` tables, apart from parser
//! versions, which name where each grammar comes from.
//!
//! Run with `cargo bench -p codix-benches --bench equivalence` from a
//! terminal. Indexes each repository from scratch with both flavours, so the
//! linux checkout takes several minutes, and writes
//! `benches/results/<version>-equivalence.json`.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;

use codix_benches::{Pinned, SPRING, build_codix, build_modular, checkout, time, workspace};
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;

const LINUX: Pinned = Pinned {
    name: "linux",
    url: "https://github.com/torvalds/linux",
    tag: "v6.12",
    commit: "adc218676eef25575469234709c2d87185ca223a",
};

/// Each table, as rows that do not depend on row IDs: references and
/// symbols by path and position, the symbols they name by ID.
const TABLES: &[(&str, &str)] = &[
    (
        "files",
        "SELECT path, size, hash, language, status FROM files ORDER BY path",
    ),
    (
        "symbols",
        "SELECT f.path, s.symbol_id, s.name, s.kind, s.qualified_name, s.parent,
                s.start_line, s.start_column, s.start_byte, s.end_line, s.end_column,
                s.end_byte, s.name_line, s.name_column, s.name_start_byte,
                s.name_end_byte, s.signature, s.visibility, s.confidence
         FROM symbols s JOIN files f ON f.id = s.file_id
         ORDER BY f.path, s.start_byte, s.symbol_id",
    ),
    (
        "refs",
        "SELECT f.path, r.name, r.context, r.line, r.column, r.start_byte, r.end_byte,
                e.symbol_id, t.symbol_id, r.state, r.low
         FROM refs r JOIN files f ON f.id = r.file_id
         LEFT JOIN symbols e ON e.id = r.enclosing
         LEFT JOIN symbols t ON t.id = r.target
         ORDER BY f.path, r.start_byte, r.name",
    ),
    (
        "modules",
        "SELECT name, build, root, source_roots, test_roots, parent, notes
         FROM modules ORDER BY root, name",
    ),
];

/// Differences shown per table.
const SHOWN: usize = 5;

#[derive(Serialize)]
struct Table {
    /// Rows in the monolithic index.
    full_rows: usize,
    /// Rows in the modular index.
    modular_rows: usize,
    /// Rows found in one index only.
    differing_rows: usize,
    /// The first of them, `-` for the monolithic one, `+` for the modular.
    first_differences: Vec<String>,
}

#[derive(Serialize)]
struct Compared {
    repository: String,
    full_seconds: f64,
    modular_seconds: f64,
    tables: BTreeMap<&'static str, Table>,
    equivalent: bool,
}

#[derive(Serialize)]
struct Results {
    version: &'static str,
    repositories: Vec<Compared>,
    equivalent: bool,
}

/// Every row of `sql`, each as one line.
fn rows(conn: &Connection, sql: &str) -> Vec<String> {
    let mut stmt = conn.prepare(sql).expect("prepare");
    let columns = stmt.column_count();
    stmt.query_map([], |row| {
        let mut line = String::new();
        for i in 0..columns {
            let value: rusqlite::types::Value = row.get(i)?;
            if i > 0 {
                line.push('\t');
            }
            line.push_str(&match value {
                rusqlite::types::Value::Null => "∅".to_owned(),
                rusqlite::types::Value::Integer(n) => n.to_string(),
                rusqlite::types::Value::Real(x) => x.to_string(),
                rusqlite::types::Value::Text(t) => t,
                rusqlite::types::Value::Blob(b) => b.iter().fold(String::new(), |mut hex, byte| {
                    write!(hex, "{byte:02x}").expect("write to a string");
                    hex
                }),
            });
        }
        Ok(line)
    })
    .and_then(Iterator::collect)
    .expect("rows")
}

/// The rows of two sorted lists found in one only.
fn compare(full: &[String], modular: &[String]) -> Table {
    let (mut i, mut j) = (0, 0);
    let mut differing = 0;
    let mut shown = Vec::new();
    let mut show = |line: String| {
        if shown.len() < SHOWN {
            shown.push(line);
        }
    };
    while i < full.len() || j < modular.len() {
        match (full.get(i), modular.get(j)) {
            (Some(a), Some(b)) if a == b => {
                i += 1;
                j += 1;
            }
            (Some(a), Some(b)) if a < b => {
                differing += 1;
                show(format!("- {a}"));
                i += 1;
            }
            (Some(a), None) => {
                differing += 1;
                show(format!("- {a}"));
                i += 1;
            }
            (_, Some(b)) => {
                differing += 1;
                show(format!("+ {b}"));
                j += 1;
            }
            (None, None) => break,
        }
    }
    Table {
        full_rows: full.len(),
        modular_rows: modular.len(),
        differing_rows: differing,
        first_differences: shown,
    }
}

/// Indexes `repo` from scratch with `codix`, and returns the seconds taken
/// and every table's rows.
fn index(codix: &Path, repo: &Path) -> (f64, Vec<Vec<String>>) {
    let data = repo.join(".codix");
    if data.exists() {
        std::fs::remove_dir_all(&data).expect("remove the previous index");
    }
    let seconds = time(codix, repo, &["init"]).as_secs_f64();
    let conn = Connection::open_with_flags(
        repo.join(".codix/index.db"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open the index");
    let mut tables: Vec<Vec<String>> = TABLES.iter().map(|(_, sql)| rows(&conn, sql)).collect();
    // Sorted as Rust compares, whatever SQLite's collation.
    for table in &mut tables {
        table.sort_unstable();
    }
    (seconds, tables)
}

fn main() {
    let full = build_codix();
    let modular = build_modular();
    let repos = workspace().join("target/bench-repos");
    let mut out = std::io::stdout().lock();
    let mut repositories = Vec::new();
    for pinned in [SPRING, LINUX] {
        let repo = checkout(&repos, pinned);
        let (full_seconds, full_tables) = index(&full, &repo);
        let (modular_seconds, modular_tables) = index(&modular, &repo);
        let mut tables = BTreeMap::new();
        for (((name, _), a), b) in TABLES.iter().zip(&full_tables).zip(&modular_tables) {
            let table = compare(a, b);
            writeln!(
                out,
                "{} {name}: {} rows, {} modular, {} differ",
                pinned.name, table.full_rows, table.modular_rows, table.differing_rows
            )
            .expect("write");
            tables.insert(*name, table);
        }
        let equivalent = tables.values().all(|t| t.differing_rows == 0);
        repositories.push(Compared {
            repository: format!("{} {}", pinned.name, pinned.tag),
            full_seconds: (full_seconds * 1000.0).round() / 1000.0,
            modular_seconds: (modular_seconds * 1000.0).round() / 1000.0,
            tables,
            equivalent,
        });
    }
    let results = Results {
        version: env!("CARGO_PKG_VERSION"),
        equivalent: repositories.iter().all(|r| r.equivalent),
        repositories,
    };
    writeln!(
        out,
        "{}",
        if results.equivalent {
            "equivalent"
        } else {
            "NOT equivalent"
        }
    )
    .expect("write");
    let path = workspace().join(format!(
        "benches/results/{}-equivalence.json",
        env!("CARGO_PKG_VERSION")
    ));
    let json = serde_json::to_string_pretty(&results).expect("serialize results");
    std::fs::write(&path, format!("{json}\n")).expect("write results");
}
