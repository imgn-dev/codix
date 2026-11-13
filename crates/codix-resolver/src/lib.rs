//! What a resolver needs, and what the core needs to run one.
//!
//! A resolver binds references across files, for one language. It reads the
//! index through versioned views ([`VIEWS_SQL`]), never the tables, and
//! writes what it finds into a scratch database ([`Scratch`]), which the core
//! checks and merges. It runs in process when compiled in ([`register!`]), or
//! as a plugin's executable speaking the protocol ([`protocol`]) on stdin and
//! stdout ([`serve`], [`client`]): the same [`Resolver`] either way.

pub mod client;
pub mod protocol;
mod serve;

use std::path::Path;

use rusqlite::{Connection, OpenFlags};

pub use serve::serve;

#[doc(hidden)]
pub use inventory;

/// The protocol version.
pub const PROTOCOL: u32 = 1;

/// The version of the views, of their names and columns.
pub const VIEWS: u32 = 1;

/// The views of version 1, over the core's tables. Context and state are
/// spelled out, so the core may renumber its codes.
pub const VIEWS_SQL: &str = "
CREATE VIEW v1_files AS
SELECT id, path, language, status FROM files;

CREATE VIEW v1_symbols AS
SELECT id, symbol_id, file_id, name, kind, qualified_name, parent,
       start_byte, end_byte, name_start_byte, name_end_byte, visibility,
       signature, declared_type
FROM symbols;

CREATE VIEW v1_refs AS
SELECT id, file_id, name,
       CASE context WHEN 0 THEN 'call' WHEN 1 THEN 'type' WHEN 2 THEN 'import'
                    WHEN 4 THEN 'write' ELSE 'read' END AS context,
       start_byte, end_byte, enclosing, target,
       CASE state WHEN 0 THEN 'bound' WHEN 2 THEN 'uncertain' ELSE 'free' END AS state,
       receiver, receiver_type, arity
FROM refs;

CREATE VIEW v1_modules AS
SELECT name, build, root, source_roots, test_roots, parent FROM modules;

CREATE VIEW v1_supertypes AS
SELECT symbol, name FROM supertypes;

CREATE VIEW v1_imports AS
SELECT file_id, kind, name, alias, is_static, wildcard, system, line FROM imports;
";

/// The columns of each view of [`VIEWS_SQL`]: the contract. Once released,
/// changing one is a new version of the views, never an edit.
pub const VIEW_COLUMNS: &[(&str, &[&str])] = &[
    ("v1_files", &["id", "path", "language", "status"]),
    (
        "v1_symbols",
        &[
            "id",
            "symbol_id",
            "file_id",
            "name",
            "kind",
            "qualified_name",
            "parent",
            "start_byte",
            "end_byte",
            "name_start_byte",
            "name_end_byte",
            "visibility",
            "signature",
            "declared_type",
        ],
    ),
    (
        "v1_refs",
        &[
            "id",
            "file_id",
            "name",
            "context",
            "start_byte",
            "end_byte",
            "enclosing",
            "target",
            "state",
            "receiver",
            "receiver_type",
            "arity",
        ],
    ),
    (
        "v1_modules",
        &[
            "name",
            "build",
            "root",
            "source_roots",
            "test_roots",
            "parent",
        ],
    ),
    ("v1_supertypes", &["symbol", "name"]),
    (
        "v1_imports",
        &[
            "file_id",
            "kind",
            "name",
            "alias",
            "is_static",
            "wildcard",
            "system",
            "line",
        ],
    ),
];

/// The scratch database: one row per reference the resolver binds, to a
/// symbol of the project (`bound`, with its `v1_symbols.id`) or to something
/// outside it (`external`, no target).
pub const SCRATCH_SQL: &str = "
CREATE TABLE resolved (
    ref INTEGER PRIMARY KEY,
    target INTEGER,
    state TEXT NOT NULL CHECK (
        (state = 'bound' AND target IS NOT NULL)
        OR (state = 'external' AND target IS NULL)
    )
);
";

/// Binds the references of one language across files.
pub trait Resolver: Send {
    /// Resolves the references of `files` (root-relative paths, changed
    /// since the last call: new, modified or deleted) and of the files that
    /// depend on them, reading `index` and writing into `scratch`. Returns
    /// the dependents it resolved too; the core then replaces what it held
    /// for all of those files.
    fn resolve(
        &mut self,
        index: &Connection,
        scratch: &mut Scratch,
        files: &[String],
    ) -> Result<Vec<String>, String>;
}

/// A resolver compiled into this binary, for one language: submitted with
/// [`register!`].
pub struct CompiledResolver {
    /// The language it resolves.
    pub language: &'static str,
    /// Makes one.
    pub new: fn() -> Box<dyn Resolver>,
}

inventory::collect!(CompiledResolver);

/// Registers a compiled-in resolver: `register!("java", JavaResolver::new)`.
#[macro_export]
macro_rules! register {
    ($language:literal, $new:expr) => {
        $crate::inventory::submit! {
            $crate::CompiledResolver {
                language: $language,
                new: || Box::new(($new)()),
            }
        }
    };
}

/// The compiled-in resolver of `language`, if any.
pub fn compiled(language: &str) -> Option<Box<dyn Resolver>> {
    inventory::iter::<CompiledResolver>
        .into_iter()
        .find(|r| r.language == language)
        .map(|r| (r.new)())
}

/// Where a resolver writes what it finds.
pub struct Scratch {
    conn: Connection,
}

impl Scratch {
    /// Creates the scratch database at `path`, which must not exist.
    pub fn create(path: &Path) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        conn.execute_batch(SCRATCH_SQL)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Self { conn })
    }

    /// Opens a scratch database the core created.
    pub fn open(path: &Path) -> Result<Self, String> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Self { conn })
    }

    /// Binds the reference `reference` (`v1_refs.id`) to the symbol `target`
    /// (`v1_symbols.id`).
    pub fn bind(&mut self, reference: i64, target: i64) -> Result<(), String> {
        self.insert(reference, Some(target), "bound")
    }

    /// Records that the reference `reference` names something outside the
    /// project, such as a library's.
    pub fn external(&mut self, reference: i64) -> Result<(), String> {
        self.insert(reference, None, "external")
    }

    fn insert(&mut self, reference: i64, target: Option<i64>, state: &str) -> Result<(), String> {
        self.conn
            .prepare_cached(
                "INSERT OR REPLACE INTO resolved (ref, target, state) VALUES (?1, ?2, ?3)",
            )
            .and_then(|mut insert| insert.execute(rusqlite::params![reference, target, state]))
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Runs `write` in one transaction: much faster for many rows.
    pub fn batch<T>(
        &mut self,
        write: impl FnOnce(&mut Self) -> Result<T, String>,
    ) -> Result<T, String> {
        self.conn
            .execute_batch("BEGIN")
            .map_err(|e| e.to_string())?;
        match write(self) {
            Ok(value) => {
                self.conn
                    .execute_batch("COMMIT")
                    .map_err(|e| e.to_string())?;
                Ok(value)
            }
            Err(e) => {
                // The error that matters is the resolver's.
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }
}

/// Opens the index at `path` read-only, as a resolver sees it.
pub fn open_index(path: &Path) -> Result<Connection, String> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("{}: {e}", path.display()))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    Ok(conn)
}

/// Runs `resolver` on the index at `index`, writing into the scratch
/// database at `scratch`: what both the in-process path and [`serve`] do.
pub fn run(
    resolver: &mut dyn Resolver,
    index: &Path,
    scratch: &Path,
    files: &[String],
) -> Result<Vec<String>, String> {
    let index = open_index(index)?;
    let mut scratch = Scratch::open(scratch)?;
    resolver.resolve(&index, &mut scratch, files)
}
