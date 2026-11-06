//! The index store: one SQLite database per project, in WAL mode so queries
//! read a consistent snapshot while one writer commits. Rows are keyed by file
//! so a file's rows are replaced in one transaction.

mod journal;
mod previews;
mod search;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

pub use journal::{EntryState, JournalEntry, JournalFile};
pub use previews::PreviewRow;
pub use search::{MatchMode, NameQuery};

use crate::Error;
use crate::scope::RefContext;

/// How long a connection waits for SQLite's own locks before failing.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Schema migrations, in order. Migration `n` (1-based) moves the database
/// from version `n - 1` to `n`; later phases append to this list.
const MIGRATIONS: &[&str] = &[
    MIGRATION_1,
    MIGRATION_2,
    MIGRATION_3,
    MIGRATION_4,
    MIGRATION_5,
    previews::MIGRATION,
    journal::MIGRATION,
];

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

/// Symbol lookup by qualified name, for commands naming a symbol by name.
const MIGRATION_2: &str = "CREATE INDEX symbols_by_qualified_name ON symbols(qualified_name);";

/// Modules and build constructs from build files. Lists are stored one item
/// per line.
const MIGRATION_3: &str = "
CREATE TABLE modules (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    build TEXT NOT NULL,
    root TEXT NOT NULL,
    source_roots TEXT NOT NULL,
    test_roots TEXT NOT NULL,
    parent TEXT,
    notes TEXT NOT NULL
);
CREATE TABLE build_issues (
    file TEXT NOT NULL,
    line INTEGER NOT NULL,
    construct TEXT NOT NULL
);
";

/// How many symbols hold each trigram, so fuzzy search can start from the
/// rarest ones; and names in case-blind order, for prefix search.
const MIGRATION_4: &str = "
CREATE VIRTUAL TABLE symbols_fts_vocab USING fts5vocab(symbols_fts, 'row');
CREATE INDEX symbols_name_nocase ON symbols (name COLLATE NOCASE);
";

/// Tier 2 references that can leave their file's local scopes: bound to a
/// symbol of the file, or not bound at all. `enclosing` and `target` are
/// `symbols.id` values of the same file, replaced with it. `context` and
/// `state` are the codes of [`RefContext`] and [`RefState`].
const MIGRATION_5: &str = "
CREATE TABLE refs (
    id INTEGER PRIMARY KEY,
    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    context INTEGER NOT NULL,
    line INTEGER NOT NULL,
    column INTEGER NOT NULL,
    start_byte INTEGER NOT NULL,
    end_byte INTEGER NOT NULL,
    enclosing INTEGER,
    target INTEGER,
    state INTEGER NOT NULL,
    low INTEGER NOT NULL
);
CREATE INDEX refs_by_name ON refs(name);
CREATE INDEX refs_by_file ON refs(file_id);
";

/// How a file is held in the index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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
    /// Binary: neither parsed nor searched, but kept in the manifest so an
    /// unchanged file is not read again.
    Skipped,
}

impl FileStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Indexed => "indexed",
            Self::SearchOnly => "search_only",
            Self::Pending => "pending",
            Self::Error => "error",
            Self::Skipped => "skipped",
        }
    }

    fn parse(text: &str) -> Self {
        match text {
            "indexed" => Self::Indexed,
            "search_only" => Self::SearchOnly,
            "pending" => Self::Pending,
            "skipped" => Self::Skipped,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Point {
    /// 1-based line.
    pub line: u32,
    /// 1-based column, in Unicode scalar values.
    pub column: u32,
    /// Byte offset.
    pub byte: u32,
}

/// A range between two points, end exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Span {
    /// First position.
    pub start: Point,
    /// Position after the last one.
    pub end: Point,
}

/// How much to trust a symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// Parsed cleanly.
    Normal,
    /// Inside or around a parse-error region.
    Low,
}

/// One `symbols` row: a definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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

/// What a stored reference binds to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RefState {
    /// A symbol of its file.
    Bound,
    /// Nothing in its file.
    Free,
    /// Unknown: an opaque scope may hide what it means.
    Uncertain,
}

impl RefState {
    fn code(self) -> i64 {
        match self {
            Self::Bound => 0,
            Self::Free => 1,
            Self::Uncertain => 2,
        }
    }

    fn from_code(code: i64) -> Self {
        match code {
            0 => Self::Bound,
            2 => Self::Uncertain,
            _ => Self::Free,
        }
    }
}

fn context_code(context: RefContext) -> i64 {
    match context {
        RefContext::Call => 0,
        RefContext::Type => 1,
        RefContext::Import => 2,
        RefContext::Read => 3,
        RefContext::Write => 4,
    }
}

fn context_from_code(code: i64) -> RefContext {
    match code {
        0 => RefContext::Call,
        1 => RefContext::Type,
        2 => RefContext::Import,
        4 => RefContext::Write,
        _ => RefContext::Read,
    }
}

/// A reference to store with its file: one that can leave the file's local
/// scopes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefRow {
    /// The name.
    pub name: String,
    /// Its context.
    pub context: RefContext,
    /// Where it is.
    pub range: Span,
    /// The innermost enclosing symbol, by index into the file's symbols.
    pub enclosing: Option<usize>,
    /// The symbol it binds to, by index into the file's symbols.
    pub target: Option<usize>,
    /// What it binds to.
    pub state: RefState,
    /// How much to trust it.
    pub confidence: Confidence,
}

/// A stored reference, with the symbols it names by ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefHit {
    /// Root-relative path of its file.
    pub path: String,
    /// The name.
    pub name: String,
    /// Its context.
    pub context: RefContext,
    /// Start of the name.
    pub start: Point,
    /// Byte after the name.
    pub end_byte: u32,
    /// ID of the innermost enclosing symbol.
    pub enclosing: Option<String>,
    /// ID of the symbol it binds to.
    pub target: Option<String>,
    /// What it binds to.
    pub state: RefState,
    /// How much to trust it.
    pub confidence: Confidence,
}

/// A symbol and the file it is defined in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolHit {
    /// Root-relative path of the defining file.
    pub path: String,
    /// Language of the defining file.
    pub language: String,
    /// The symbol.
    pub symbol: SymbolRow,
}

/// One file's new state for [`Store::commit`].
#[derive(Debug, Clone)]
pub struct FileUpdate {
    /// The manifest row.
    pub file: FileRow,
    /// Every symbol of the file, replacing those stored; `None` keeps the
    /// stored symbols and references and updates the manifest row only (a
    /// touch).
    pub symbols: Option<Vec<SymbolRow>>,
    /// The references to store, replacing those stored, when `symbols` is
    /// given.
    pub refs: Vec<RefRow>,
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
        self.commit_scan(updates, deleted, None)
    }

    /// Like [`Store::commit`], also replacing every directory hash when `dirs`
    /// is given; otherwise the hashes of the directories holding the changed
    /// files are dropped.
    pub fn commit_scan(
        &mut self,
        updates: &[FileUpdate],
        deleted: &[String],
        dirs: Option<&[(String, [u8; 32])]>,
    ) -> Result<(), Error> {
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
        if let Some(dirs) = dirs {
            write_dirs(&tx, dirs).map_err(|e| sql_error(&path, &e))?;
        } else {
            // The hashes of the directories above a changed file are stale
            // now; without them, the next full scan compares those
            // directories file by file instead of skipping them.
            let mut stale = HashSet::new();
            for changed in updates
                .iter()
                .map(|u| u.file.path.as_str())
                .chain(deleted.iter().map(String::as_str))
            {
                let mut dir = changed;
                while let Some(i) = dir.rfind('/') {
                    dir = &dir[..i];
                    stale.insert(dir);
                }
                stale.insert("");
            }
            let mut remove = tx
                .prepare_cached("DELETE FROM dirs WHERE path = ?1")
                .map_err(|e| sql_error(&path, &e))?;
            for dir in stale {
                remove.execute([dir]).map_err(|e| sql_error(&path, &e))?;
            }
        }
        tx.commit().map_err(|e| sql_error(&path, &e))
    }

    /// Replaces the stored modules and build issues.
    pub fn replace_build(&mut self, model: &codix_provider::BuildModel) -> Result<(), Error> {
        let path = self.path.clone();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| sql_error(&path, &e))?;
        write_build(&tx, model).map_err(|e| sql_error(&path, &e))?;
        tx.commit().map_err(|e| sql_error(&path, &e))
    }

    /// Marks files pending, so the next scan reparses them; returns how many
    /// were indexed.
    pub fn mark_pending(&mut self, paths: &[String]) -> Result<usize, Error> {
        let path = self.path.clone();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| sql_error(&path, &e))?;
        let mut marked = 0;
        for file in paths {
            marked += tx
                .execute(
                    "UPDATE files SET status = 'pending' WHERE path = ?1",
                    [file],
                )
                .map_err(|e| sql_error(&path, &e))?;
        }
        tx.commit().map_err(|e| sql_error(&path, &e))?;
        Ok(marked)
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
    let Some(symbols) = &update.symbols else {
        return Ok(());
    };
    tx.execute("DELETE FROM refs WHERE file_id = ?1", [file_id])?;
    tx.execute("DELETE FROM symbols WHERE file_id = ?1", [file_id])?;
    let mut rowids = Vec::with_capacity(symbols.len());
    let mut insert = tx.prepare_cached(
        "INSERT INTO symbols (symbol_id, file_id, name, kind, qualified_name, parent,
             start_line, start_column, start_byte, end_line, end_column, end_byte,
             name_line, name_column, name_start_byte, name_end_byte,
             signature, visibility, confidence)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
    )?;
    for s in symbols {
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
        rowids.push(tx.last_insert_rowid());
    }
    let mut insert = tx.prepare_cached(
        "INSERT INTO refs (file_id, name, context, line, column, start_byte, end_byte,
             enclosing, target, state, low)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
    )?;
    for r in &update.refs {
        insert.execute(params![
            file_id,
            r.name,
            context_code(r.context),
            r.range.start.line,
            r.range.start.column,
            r.range.start.byte,
            r.range.end.byte,
            r.enclosing.and_then(|i| rowids.get(i)),
            r.target.and_then(|i| rowids.get(i)),
            r.state.code(),
            i64::from(r.confidence == Confidence::Low),
        ])?;
    }
    Ok(())
}

const REF_SELECT: &str = "SELECT f.path, r.name, r.context, r.line, r.column, r.start_byte,
        r.end_byte, e.symbol_id, t.symbol_id, r.state, r.low
     FROM refs r JOIN files f ON f.id = r.file_id
     LEFT JOIN symbols e ON e.id = r.enclosing
     LEFT JOIN symbols t ON t.id = r.target";

fn ref_hit(row: &rusqlite::Row<'_>) -> rusqlite::Result<RefHit> {
    Ok(RefHit {
        path: row.get(0)?,
        name: row.get(1)?,
        context: context_from_code(row.get(2)?),
        start: Point {
            line: row.get(3)?,
            column: row.get(4)?,
            byte: row.get(5)?,
        },
        end_byte: row.get(6)?,
        enclosing: row.get(7)?,
        target: row.get(8)?,
        state: RefState::from_code(row.get(9)?),
        confidence: if row.get::<_, i64>(10)? == 0 {
            Confidence::Normal
        } else {
            Confidence::Low
        },
    })
}

fn write_dirs(tx: &Transaction<'_>, dirs: &[(String, [u8; 32])]) -> rusqlite::Result<()> {
    tx.execute("DELETE FROM dirs", [])?;
    let mut insert = tx.prepare_cached("INSERT INTO dirs (path, hash) VALUES (?1, ?2)")?;
    for (path, hash) in dirs {
        insert.execute(params![path, hash.as_slice()])?;
    }
    Ok(())
}

fn write_build(tx: &Transaction<'_>, model: &codix_provider::BuildModel) -> rusqlite::Result<()> {
    tx.execute("DELETE FROM modules", [])?;
    tx.execute("DELETE FROM build_issues", [])?;
    let mut insert = tx.prepare_cached(
        "INSERT INTO modules (name, build, root, source_roots, test_roots, parent, notes)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;
    for m in &model.modules {
        insert.execute(params![
            m.name,
            m.build,
            m.root,
            m.source_roots.join("\n"),
            m.test_roots.join("\n"),
            m.parent,
            m.notes.join("\n"),
        ])?;
    }
    let mut insert =
        tx.prepare_cached("INSERT INTO build_issues (file, line, construct) VALUES (?1, ?2, ?3)")?;
    for issue in &model.unsupported {
        insert.execute(params![issue.file, issue.line, issue.construct])?;
    }
    Ok(())
}

fn lines(text: &str) -> Vec<String> {
    text.lines().map(str::to_owned).collect()
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
    s.signature, s.visibility, s.confidence, f.language";

fn symbol_hit(row: &rusqlite::Row<'_>) -> rusqlite::Result<SymbolHit> {
    let name_line: u32 = row.get(12)?;
    let name_column: u32 = row.get(13)?;
    let name: String = row.get(2)?;
    let name_len = u32::try_from(name.chars().count()).unwrap_or(u32::MAX);
    Ok(SymbolHit {
        path: row.get(0)?,
        language: row.get(19)?,
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

    /// Every directory hash, by root-relative path.
    pub fn dirs(&self) -> Result<std::collections::HashMap<String, [u8; 32]>, Error> {
        let mut stmt = self
            .tx
            .prepare("SELECT path, hash FROM dirs")
            .map_err(|e| self.err(&e))?;
        stmt.query_map([], |r| {
            let hash: Vec<u8> = r.get(1)?;
            Ok((r.get(0)?, hash.try_into().unwrap_or([0; 32])))
        })
        .and_then(Iterator::collect)
        .map_err(|e| self.err(&e))
    }

    /// The stored modules and build issues.
    pub fn build(&self) -> Result<codix_provider::BuildModel, Error> {
        let mut stmt = self
            .tx
            .prepare("SELECT name, build, root, source_roots, test_roots, parent, notes FROM modules ORDER BY id")
            .map_err(|e| self.err(&e))?;
        let modules = stmt
            .query_map([], |r| {
                Ok(codix_provider::Module {
                    name: r.get(0)?,
                    build: r.get(1)?,
                    root: r.get(2)?,
                    source_roots: lines(&r.get::<_, String>(3)?),
                    test_roots: lines(&r.get::<_, String>(4)?),
                    parent: r.get(5)?,
                    notes: lines(&r.get::<_, String>(6)?),
                })
            })
            .and_then(Iterator::collect)
            .map_err(|e| self.err(&e))?;
        let mut stmt = self
            .tx
            .prepare("SELECT file, line, construct FROM build_issues ORDER BY file, line")
            .map_err(|e| self.err(&e))?;
        let unsupported = stmt
            .query_map([], |r| {
                Ok(codix_provider::Unsupported {
                    file: r.get(0)?,
                    line: r.get(1)?,
                    construct: r.get(2)?,
                })
            })
            .and_then(Iterator::collect)
            .map_err(|e| self.err(&e))?;
        Ok(codix_provider::BuildModel {
            modules,
            unsupported,
        })
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

    /// The stored references of a file, in file order.
    pub fn refs_in(&self, path: &str) -> Result<Vec<RefHit>, Error> {
        let sql = format!("{REF_SELECT} WHERE f.path = ?1 ORDER BY r.start_byte");
        let mut stmt = self.tx.prepare(&sql).map_err(|e| self.err(&e))?;
        stmt.query_map([path], ref_hit)
            .and_then(Iterator::collect)
            .map_err(|e| self.err(&e))
    }

    /// The stored references named `name`, by path and position.
    pub fn refs_named(&self, name: &str) -> Result<Vec<RefHit>, Error> {
        let sql = format!("{REF_SELECT} WHERE r.name = ?1 ORDER BY f.path, r.start_byte");
        let mut stmt = self.tx.prepare(&sql).map_err(|e| self.err(&e))?;
        stmt.query_map([name], ref_hit)
            .and_then(Iterator::collect)
            .map_err(|e| self.err(&e))
    }

    /// Symbols whose ID starts with `prefix`, sorted by ID.
    pub fn symbols_with_id_prefix(&self, prefix: &str) -> Result<Vec<SymbolHit>, Error> {
        let sql = format!(
            "SELECT {SYMBOL_COLUMNS} FROM symbols s JOIN files f ON f.id = s.file_id
             WHERE s.symbol_id >= ?1 AND s.symbol_id < ?2 ORDER BY s.symbol_id"
        );
        let mut stmt = self.tx.prepare(&sql).map_err(|e| self.err(&e))?;
        stmt.query_map([prefix, &prefix_end(prefix)], symbol_hit)
            .and_then(Iterator::collect)
            .map_err(|e| self.err(&e))
    }

    /// Symbols whose simple or qualified name is `name`, sorted by ID.
    pub fn symbols_named(&self, name: &str) -> Result<Vec<SymbolHit>, Error> {
        let sql = format!(
            "SELECT {SYMBOL_COLUMNS} FROM symbols s JOIN files f ON f.id = s.file_id
             WHERE s.qualified_name = ?1 OR s.name = ?1 ORDER BY s.symbol_id"
        );
        let mut stmt = self.tx.prepare(&sql).map_err(|e| self.err(&e))?;
        stmt.query_map([name], symbol_hit)
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

/// The smallest string greater than every string starting with `prefix`.
fn prefix_end(prefix: &str) -> String {
    let mut chars: Vec<char> = prefix.chars().collect();
    while let Some(last) = chars.pop() {
        if let Some(next) = char::from_u32(u32::from(last) + 1) {
            chars.push(next);
            return chars.into_iter().collect();
        }
    }
    String::from(char::MAX)
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
