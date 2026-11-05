//! The index store: one SQLite database per project, in WAL mode so queries
//! read a consistent snapshot while one writer commits. Rows are keyed by file
//! so a file's rows are replaced in one transaction.

mod search;

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

pub use search::{MatchMode, NameQuery};

use crate::Error;

/// How long a connection waits for SQLite's own locks before failing.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Schema migrations, in order. Migration `n` (1-based) moves the database
/// from version `n - 1` to `n`; later phases append to this list.
const MIGRATIONS: &[&str] = &[MIGRATION_1];

const MIGRATION_1: &str = "
CREATE TABLE files (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    size INTEGER NOT NULL,
    mtime_ns INTEGER NOT NULL,
    inode INTEGER NOT NULL,
    hash BLOB NOT NULL,
    language TEXT NOT NULL,
    parser_version TEXT NOT NULL,
    status TEXT NOT NULL
);

CREATE TABLE dirs (
    path TEXT PRIMARY KEY,
    hash BLOB NOT NULL
) WITHOUT ROWID;

CREATE TABLE symbols (
    id INTEGER PRIMARY KEY,
    symbol_id TEXT NOT NULL,
    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    qualified_name TEXT NOT NULL,
    parent TEXT,
    start_line INTEGER NOT NULL,
    start_column INTEGER NOT NULL,
    start_byte INTEGER NOT NULL,
    end_line INTEGER NOT NULL,
    end_column INTEGER NOT NULL,
    end_byte INTEGER NOT NULL,
    name_line INTEGER NOT NULL,
    name_column INTEGER NOT NULL,
    name_start_byte INTEGER NOT NULL,
    name_end_byte INTEGER NOT NULL,
    signature TEXT,
    visibility TEXT,
    confidence TEXT NOT NULL
);
CREATE INDEX symbols_by_name ON symbols(name);
CREATE INDEX symbols_by_file ON symbols(file_id);
CREATE INDEX symbols_by_symbol_id ON symbols(symbol_id);

CREATE VIRTUAL TABLE symbols_fts USING fts5(
    name, qualified_name,
    content = 'symbols', content_rowid = 'id',
    tokenize = 'trigram case_sensitive 0'
);
CREATE TRIGGER symbols_fts_insert AFTER INSERT ON symbols BEGIN
    INSERT INTO symbols_fts(rowid, name, qualified_name)
    VALUES (new.id, new.name, new.qualified_name);
END;
CREATE TRIGGER symbols_fts_delete AFTER DELETE ON symbols BEGIN
    INSERT INTO symbols_fts(symbols_fts, rowid, name, qualified_name)
    VALUES ('delete', old.id, old.name, old.qualified_name);
END;
CREATE TRIGGER symbols_fts_update AFTER UPDATE ON symbols BEGIN
    INSERT INTO symbols_fts(symbols_fts, rowid, name, qualified_name)
    VALUES ('delete', old.id, old.name, old.qualified_name);
    INSERT INTO symbols_fts(rowid, name, qualified_name)
    VALUES (new.id, new.name, new.qualified_name);
END;
";

/// How a file is held in the index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    /// Parsed; its symbols are in the index.
    Indexed,
    /// Searched as text only: plain text, not UTF-8, or minified.
    SearchOnly,
    /// Known to have changed and waiting to be reparsed.
    Pending,
    /// Reading or parsing failed.
    Error,
}

impl FileStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Indexed => "indexed",
            Self::SearchOnly => "search_only",
            Self::Pending => "pending",
            Self::Error => "error",
        }
    }

    fn parse(text: &str) -> Self {
        match text {
            "indexed" => Self::Indexed,
            "search_only" => Self::SearchOnly,
            "pending" => Self::Pending,
            _ => Self::Error,
        }
    }
}

/// One `files` row: the manifest entry change detection compares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRow {
    /// Root-relative path.
    pub path: String,
    /// Size in bytes.
    pub size: u64,
    /// Modification time, in nanoseconds since the Unix epoch.
    pub mtime_ns: i64,
    /// Inode number, or 0.
    pub inode: u64,
    /// BLAKE3 hash of the content.
    pub hash: [u8; 32],
    /// Detected language.
    pub language: String,
    /// Version of the pack, grammar and queries that produced the symbols.
    pub parser_version: String,
    /// Index status.
    pub status: FileStatus,
}

/// A position: 1-based line, 1-based column in Unicode scalar values, and
/// byte offset from the start of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Point {
    /// 1-based line.
    pub line: u32,
    /// 1-based column, in Unicode scalar values.
    pub column: u32,
    /// Byte offset.
    pub byte: u32,
}

/// A range between two points, end exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    /// First position.
    pub start: Point,
    /// Position after the last one.
    pub end: Point,
}

/// How much to trust a symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// Parsed cleanly.
    Normal,
    /// Inside or around a parse-error region.
    Low,
}

/// One `symbols` row: a definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolRow {
    /// Stable symbol ID.
    pub symbol_id: String,
    /// Simple name.
    pub name: String,
    /// Kind from the shared vocabulary.
    pub kind: String,
    /// Qualified name within the file, such as `Invoice.total`.
    pub qualified_name: String,
    /// Symbol ID of the enclosing symbol, if any.
    pub parent: Option<String>,
    /// Range of the whole definition.
    pub range: Span,
    /// Range of the name.
    pub name_range: Span,
    /// Declaration without its body, when captured.
    pub signature: Option<String>,
    /// Visibility, when the language has it.
    pub visibility: Option<String>,
    /// How much to trust the symbol.
    pub confidence: Confidence,
}

/// A symbol and the file it is defined in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolHit {
    /// Root-relative path of the defining file.
    pub path: String,
    /// The symbol.
    pub symbol: SymbolRow,
}

/// One file's new state for [`Store::commit`].
#[derive(Debug, Clone)]
pub struct FileUpdate {
    /// The manifest row.
    pub file: FileRow,
    /// Every symbol of the file, replacing those stored.
    pub symbols: Vec<SymbolRow>,
}

/// A connection to a project's index.
#[derive(Debug)]
pub struct Store {
    conn: Connection,
    path: PathBuf,
}

fn sql_error(path: &Path, e: &rusqlite::Error) -> Error {
    Error::Internal {
        message: format!("index {}: {e}", path.display()),
    }
}

impl Store {
    /// Opens the index at `path`, creating it and applying migrations as
    /// needed. Readers and the writer open it the same way.
    pub fn open(path: &Path) -> Result<Self, Error> {
        let conn = Connection::open(path).map_err(|e| sql_error(path, &e))?;
        let mut store = Self {
            conn,
            path: path.to_path_buf(),
        };
        store.configure()?;
        store.migrate()?;
        Ok(store)
    }

    fn err(&self, e: &rusqlite::Error) -> Error {
        sql_error(&self.path, e)
    }

    fn configure(&self) -> Result<(), Error> {
        self.conn
            .busy_timeout(BUSY_TIMEOUT)
            .and_then(|()| {
                self.conn
                    .execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;")
            })
            .map_err(|e| self.err(&e))
    }

    fn migrate(&mut self) -> Result<(), Error> {
        let path = self.path.clone();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| sql_error(&path, &e))?;
        tx.execute_batch("CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL)")
            .map_err(|e| sql_error(&path, &e))?;
        let current: i64 = tx
            .query_row(
                "SELECT coalesce(max(version), 0) FROM schema_version",
                [],
                |r| r.get(0),
            )
            .map_err(|e| sql_error(&path, &e))?;
        let current = usize::try_from(current).unwrap_or(0);
        if current > MIGRATIONS.len() {
            return Err(Error::Internal {
                message: format!(
                    "index {} has schema version {current}, newer than this codix ({}); run a newer codix or delete .codix/index.db",
                    path.display(),
                    MIGRATIONS.len()
                ),
            });
        }
        for (i, migration) in MIGRATIONS.iter().enumerate().skip(current) {
            tx.execute_batch(migration)
                .and_then(|()| tx.execute("DELETE FROM schema_version", []))
                .and_then(|_| {
                    tx.execute(
                        "INSERT INTO schema_version (version) VALUES (?1)",
                        [i64::try_from(i + 1).unwrap_or(i64::MAX)],
                    )
                })
                .map_err(|e| sql_error(&path, &e))?;
        }
        tx.commit().map_err(|e| sql_error(&path, &e))
    }

    /// The schema version.
    pub fn schema_version(&self) -> Result<u32, Error> {
        self.conn
            .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
            .map_err(|e| self.err(&e))
    }

    /// Replaces the rows of the updated files and removes the deleted ones, in
    /// one transaction.
    pub fn commit(&mut self, updates: &[FileUpdate], deleted: &[String]) -> Result<(), Error> {
        let path = self.path.clone();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| sql_error(&path, &e))?;
        for update in updates {
            write_file(&tx, update).map_err(|e| sql_error(&path, &e))?;
        }
        for removed in deleted {
            tx.execute("DELETE FROM files WHERE path = ?1", [removed])
                .map_err(|e| sql_error(&path, &e))?;
        }
        tx.commit().map_err(|e| sql_error(&path, &e))
    }

    /// Replaces one file's rows in one transaction.
    pub fn replace_file(&mut self, update: &FileUpdate) -> Result<(), Error> {
        self.commit(std::slice::from_ref(update), &[])
    }

    /// Starts a read snapshot: every query on it sees the database as of its
    /// first read, whatever writers commit meanwhile.
    pub fn snapshot(&mut self) -> Result<Snapshot<'_>, Error> {
        let path = self.path.clone();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(|e| sql_error(&path, &e))?;
        Ok(Snapshot { tx, path })
    }
}

fn write_file(tx: &Transaction<'_>, update: &FileUpdate) -> rusqlite::Result<()> {
    let f = &update.file;
    let file_id: i64 = tx.query_row(
        "INSERT INTO files (path, size, mtime_ns, inode, hash, language, parser_version, status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(path) DO UPDATE SET size = excluded.size, mtime_ns = excluded.mtime_ns,
             inode = excluded.inode, hash = excluded.hash, language = excluded.language,
             parser_version = excluded.parser_version, status = excluded.status
         RETURNING id",
        params![
            f.path,
            i64::try_from(f.size).unwrap_or(i64::MAX),
            f.mtime_ns,
            i64::try_from(f.inode).unwrap_or(i64::MAX),
            f.hash.as_slice(),
            f.language,
            f.parser_version,
            f.status.as_str()
        ],
        |r| r.get(0),
    )?;
    tx.execute("DELETE FROM symbols WHERE file_id = ?1", [file_id])?;
    let mut insert = tx.prepare_cached(
        "INSERT INTO symbols (symbol_id, file_id, name, kind, qualified_name, parent,
             start_line, start_column, start_byte, end_line, end_column, end_byte,
             name_line, name_column, name_start_byte, name_end_byte,
             signature, visibility, confidence)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
    )?;
    for s in &update.symbols {
        insert.execute(params![
            s.symbol_id,
            file_id,
            s.name,
            s.kind,
            s.qualified_name,
            s.parent,
            s.range.start.line,
            s.range.start.column,
            s.range.start.byte,
            s.range.end.line,
            s.range.end.column,
            s.range.end.byte,
            s.name_range.start.line,
            s.name_range.start.column,
            s.name_range.start.byte,
            s.name_range.end.byte,
            s.signature,
            s.visibility,
            match s.confidence {
                Confidence::Normal => "normal",
                Confidence::Low => "low",
            },
        ])?;
    }
    Ok(())
}

/// A consistent read view of the index.
#[derive(Debug)]
pub struct Snapshot<'a> {
    tx: Transaction<'a>,
    path: PathBuf,
}

const SYMBOL_COLUMNS: &str = "f.path, s.symbol_id, s.name, s.kind, s.qualified_name, s.parent,
    s.start_line, s.start_column, s.start_byte, s.end_line, s.end_column, s.end_byte,
    s.name_line, s.name_column, s.name_start_byte, s.name_end_byte,
    s.signature, s.visibility, s.confidence";

fn symbol_hit(row: &rusqlite::Row<'_>) -> rusqlite::Result<SymbolHit> {
    let name_line: u32 = row.get(12)?;
    let name_column: u32 = row.get(13)?;
    let name: String = row.get(2)?;
    let name_len = u32::try_from(name.chars().count()).unwrap_or(u32::MAX);
    Ok(SymbolHit {
        path: row.get(0)?,
        symbol: SymbolRow {
            symbol_id: row.get(1)?,
            name,
            kind: row.get(3)?,
            qualified_name: row.get(4)?,
            parent: row.get(5)?,
            range: Span {
                start: Point {
                    line: row.get(6)?,
                    column: row.get(7)?,
                    byte: row.get(8)?,
                },
                end: Point {
                    line: row.get(9)?,
                    column: row.get(10)?,
                    byte: row.get(11)?,
                },
            },
            name_range: Span {
                start: Point {
                    line: name_line,
                    column: name_column,
                    byte: row.get(14)?,
                },
                // Names never span lines, so the end column follows from the
                // name's length.
                end: Point {
                    line: name_line,
                    column: name_column + name_len,
                    byte: row.get(15)?,
                },
            },
            signature: row.get(16)?,
            visibility: row.get(17)?,
            confidence: if row.get::<_, String>(18)? == "low" {
                Confidence::Low
            } else {
                Confidence::Normal
            },
        },
    })
}

impl Snapshot<'_> {
    fn err(&self, e: &rusqlite::Error) -> Error {
        sql_error(&self.path, e)
    }

    /// The manifest row of `path`, if indexed.
    pub fn file(&self, path: &str) -> Result<Option<FileRow>, Error> {
        self.tx
            .query_row(
                "SELECT path, size, mtime_ns, inode, hash, language, parser_version, status
                 FROM files WHERE path = ?1",
                [path],
                file_row,
            )
            .optional()
            .map_err(|e| self.err(&e))
    }

    /// Every manifest row, sorted by path.
    pub fn files(&self) -> Result<Vec<FileRow>, Error> {
        let mut stmt = self
            .tx
            .prepare(
                "SELECT path, size, mtime_ns, inode, hash, language, parser_version, status
                 FROM files ORDER BY path",
            )
            .map_err(|e| self.err(&e))?;
        stmt.query_map([], file_row)
            .and_then(Iterator::collect)
            .map_err(|e| self.err(&e))
    }

    /// Every symbol of `path`, in source order.
    pub fn symbols_in(&self, path: &str) -> Result<Vec<SymbolHit>, Error> {
        let sql = format!(
            "SELECT {SYMBOL_COLUMNS} FROM symbols s JOIN files f ON f.id = s.file_id
             WHERE f.path = ?1 ORDER BY s.start_byte, s.id"
        );
        let mut stmt = self.tx.prepare(&sql).map_err(|e| self.err(&e))?;
        stmt.query_map([path], symbol_hit)
            .and_then(Iterator::collect)
            .map_err(|e| self.err(&e))
    }

    /// Symbols whose name matches `query`; see [`NameQuery`].
    pub fn find(&self, query: &NameQuery) -> Result<Vec<SymbolHit>, Error> {
        search::find(&self.tx, query).map_err(|e| self.err(&e))
    }

    /// Number of symbols in the index.
    pub fn symbol_count(&self) -> Result<u64, Error> {
        self.tx
            .query_row("SELECT count(*) FROM symbols", [], |r| r.get::<_, i64>(0))
            .map(|n| u64::try_from(n).unwrap_or(0))
            .map_err(|e| self.err(&e))
    }
}

fn file_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<FileRow> {
    let hash: Vec<u8> = row.get(4)?;
    Ok(FileRow {
        path: row.get(0)?,
        size: u64::try_from(row.get::<_, i64>(1)?).unwrap_or(0),
        mtime_ns: row.get(2)?,
        inode: u64::try_from(row.get::<_, i64>(3)?).unwrap_or(0),
        hash: hash.try_into().unwrap_or([0; 32]),
        language: row.get(5)?,
        parser_version: row.get(6)?,
        status: FileStatus::parse(&row.get::<_, String>(7)?),
    })
}
