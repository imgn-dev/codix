//! The type hierarchy: each type's supertypes as written, and what the name
//! written binds to, by its file's scopes or by a resolver.

use rusqlite::params;

use super::{SYMBOL_COLUMNS, Snapshot, SymbolHit, symbol_hit};
use crate::Error;

/// A supertype as one type names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Supertype {
    /// The type that names it.
    pub subtype: SymbolHit,
    /// The name, as written without type arguments.
    pub name: String,
    /// What the name binds to.
    pub binding: SupertypeBinding,
}

/// What a supertype's name binds to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupertypeBinding {
    /// A symbol of its file, by its scopes: its ID.
    Scoped(String),
    /// A symbol of another file, by a resolver: its ID.
    Bound(String),
    /// Something outside the project, by a resolver.
    External,
    /// Nothing known: only the name connects it.
    Unbound,
}

/// The last name of a name written with qualifiers: `Serializable` for
/// `java.io.Serializable`, `Shape` for `geo::Shape`, `J` for `\N\J`.
pub fn simple_name(name: &str) -> &str {
    let after_dot = name.rsplit('.').next().unwrap_or(name);
    let after_colons = after_dot.rsplit("::").next().unwrap_or(after_dot);
    after_colons.rsplit('\\').next().unwrap_or(after_colons)
}

/// The supertypes of `s` (an alias of `symbols`) by `t` (of `supertypes`),
/// with what each binds to: the first type reference of that name in the
/// type's header, after its name, as its scopes or a resolver bound it.
const SUPERTYPE_SELECT: &str = "SELECT {SYMBOL_COLUMNS}, t.name,
    (SELECT CASE
         WHEN r.state = 0 THEN 's' || bt.symbol_id
         WHEN x.state = 0 THEN 'b' || xt.symbol_id
         WHEN x.state = 1 THEN 'x'
     END
     FROM refs r
     LEFT JOIN symbols bt ON bt.id = r.target
     LEFT JOIN resolutions x ON x.ref = r.id
     LEFT JOIN symbols xt ON xt.id = x.target
     WHERE r.file_id = s.file_id AND r.name = t.simple AND r.context = 1
       AND r.start_byte >= s.name_end_byte AND r.start_byte < s.end_byte
     ORDER BY r.start_byte LIMIT 1)
 FROM supertypes t
 JOIN symbols s ON s.id = t.symbol
 JOIN files f ON f.id = s.file_id";

fn supertype(row: &rusqlite::Row<'_>) -> rusqlite::Result<Supertype> {
    let binding: Option<String> = row.get(22)?;
    Ok(Supertype {
        subtype: symbol_hit(row)?,
        name: row.get(21)?,
        binding: match binding.as_deref() {
            Some(b) if b.starts_with('s') => SupertypeBinding::Scoped(b[1..].to_owned()),
            Some(b) if b.starts_with('b') => SupertypeBinding::Bound(b[1..].to_owned()),
            Some("x") => SupertypeBinding::External,
            _ => SupertypeBinding::Unbound,
        },
    })
}

impl Snapshot<'_> {
    /// Every supertype written with `name` as its last name, with the type
    /// that writes it.
    pub fn supertypes_named(&self, name: &str) -> Result<Vec<Supertype>, Error> {
        self.supertype_rows("t.simple = ?1", name)
    }

    /// The supertypes the symbol `symbol_id` writes.
    pub fn supertypes_of(&self, symbol_id: &str) -> Result<Vec<Supertype>, Error> {
        self.supertype_rows("s.symbol_id = ?1", symbol_id)
    }

    fn supertype_rows(&self, filter: &str, value: &str) -> Result<Vec<Supertype>, Error> {
        let sql = format!(
            "{} WHERE {filter} ORDER BY f.path, s.start_byte, t.rowid",
            SUPERTYPE_SELECT.replace("{SYMBOL_COLUMNS}", SYMBOL_COLUMNS)
        );
        let mut stmt = self.tx.prepare(&sql).map_err(|e| self.err(&e))?;
        stmt.query_map(params![value], supertype)
            .and_then(Iterator::collect)
            .map_err(|e| self.err(&e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_simple_name_is_the_last_one() {
        assert_eq!(simple_name("java.io.Serializable"), "Serializable");
        assert_eq!(simple_name("geo::Shape"), "Shape");
        assert_eq!(simple_name("Base"), "Base");
        assert_eq!(simple_name("\\N\\J"), "J");
    }
}
