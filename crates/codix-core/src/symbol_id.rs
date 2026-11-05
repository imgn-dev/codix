//! Symbol IDs: a readable path plus a short disambiguating hash, such as
//! `py:src/billing/invoice.py::Invoice.total#a3f9`, and the ways commands
//! name a symbol: a full ID, the path part alone, `file:line:col`, or a name.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::Error;
use crate::project::Project;
use crate::store::{Snapshot, SymbolHit, SymbolRow};

/// Hex digits of the hash when they suffice to tell symbols apart.
const HASH_DIGITS: usize = 4;

/// Candidates listed with an `ambiguous_symbol` error.
const MAX_CANDIDATES: usize = 20;

/// The ID of a symbol. The hash comes from the kind and the signature, so
/// overloads differ while edits to the body or elsewhere leave the ID alone.
pub fn format(
    prefix: &str,
    path: &str,
    qualified_name: &str,
    kind: &str,
    signature: Option<&str>,
) -> String {
    format!(
        "{}{}",
        path_part(prefix, path, qualified_name),
        &full_hash(kind, signature)[..HASH_DIGITS]
    )
}

/// The ID without its hash: `prefix:path::qualified#`.
fn path_part(prefix: &str, path: &str, qualified_name: &str) -> String {
    format!("{prefix}:{path}::{qualified_name}#")
}

fn full_hash(kind: &str, signature: Option<&str>) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(kind.as_bytes());
    hasher.update(&[0]);
    hasher.update(signature.unwrap_or_default().as_bytes());
    hasher.finalize().to_hex().to_string()
}

/// Lengthens the hashes of symbols whose four-digit hashes collide within the
/// same path part, until they differ. Symbols with the same kind and
/// signature keep the same ID. Parent references are left to the caller,
/// which knows which symbol each one means.
pub fn disambiguate(rows: &mut [SymbolRow], prefix: &str, path: &str) {
    let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, row) in rows.iter().enumerate() {
        groups.entry(row.symbol_id.clone()).or_default().push(i);
    }
    for members in groups.values().filter(|m| m.len() > 1) {
        let hashes: Vec<String> = members
            .iter()
            .map(|&i| full_hash(&rows[i].kind, rows[i].signature.as_deref()))
            .collect();
        if hashes.iter().all(|h| *h == hashes[0]) {
            // Same kind and signature: the same symbol twice, which a longer
            // hash cannot tell apart; resolution reports it as ambiguous.
            continue;
        }
        let distinct = hashes.iter().collect::<HashSet<_>>().len();
        let digits = (HASH_DIGITS..=hashes[0].len())
            .find(|&n| hashes.iter().map(|h| &h[..n]).collect::<HashSet<_>>().len() == distinct)
            .unwrap_or(hashes[0].len());
        for (&i, hash) in members.iter().zip(&hashes) {
            rows[i].symbol_id = format!(
                "{}{}",
                path_part(prefix, path, &rows[i].qualified_name),
                &hash[..digits]
            );
        }
    }
}

/// How a command names a symbol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymbolRef {
    /// A full ID, or its path part when `hash` is `None`.
    Id {
        /// Language prefix, such as `java`.
        prefix: String,
        /// Root-relative path.
        path: String,
        /// Qualified name.
        qualified_name: String,
        /// Hash, when given.
        hash: Option<String>,
    },
    /// `file:line:col`, the file relative to the cwd or the root.
    Location {
        /// File as given.
        file: String,
        /// 1-based line.
        line: u32,
        /// 1-based column.
        column: u32,
    },
    /// A simple or qualified name, such as `total` or `Invoice.total`.
    Name(String),
}

impl SymbolRef {
    /// Reads how a symbol was named.
    pub fn parse(input: &str) -> Result<Self, Error> {
        let usage = |cause: &str| {
            Error::Usage {
            cause: format!("`{input}`: {cause}"),
            hint: "name a symbol by full ID (lang:path::Name#hash), by path part (lang:path::Name), by file:line:col, or by name".to_owned(),
        }
        };
        if let Some((location, column)) = input.rsplit_once(':')
            && let Some((file, line)) = location.rsplit_once(':')
            && let (Ok(line), Ok(column)) = (line.parse::<u32>(), column.parse::<u32>())
            && !file.is_empty()
            && !file.contains("::")
        {
            if line == 0 || column == 0 {
                return Err(usage("lines and columns start at 1"));
            }
            return Ok(Self::Location {
                file: file.to_owned(),
                line,
                column,
            });
        }
        let Some((head, qualified)) = input.split_once("::") else {
            return Ok(Self::Name(input.to_owned()));
        };
        let Some((prefix, path)) = head.split_once(':') else {
            // `a::b` names a C++ or Rust qualified name, not an ID.
            return Ok(Self::Name(input.to_owned()));
        };
        let (qualified_name, hash) = match qualified.rsplit_once('#') {
            Some((name, hash))
                if !hash.is_empty() && hash.chars().all(|c| c.is_ascii_hexdigit()) =>
            {
                (name, Some(hash.to_owned()))
            }
            Some(_) => return Err(usage("the hash after `#` must be hexadecimal")),
            None => (qualified, None),
        };
        if prefix.is_empty() || path.is_empty() || qualified_name.is_empty() {
            return Err(usage("an ID needs a language, a path and a name"));
        }
        Ok(Self::Id {
            prefix: prefix.to_owned(),
            path: path.to_owned(),
            qualified_name: qualified_name.to_owned(),
            hash,
        })
    }
}

/// Finds the one symbol `input` names. Several matches are
/// `ambiguous_symbol`, listing them; none is `unknown_symbol`.
pub fn resolve(
    snapshot: &Snapshot<'_>,
    project: &Project,
    cwd: &Path,
    input: &str,
) -> Result<SymbolHit, Error> {
    resolve_where(snapshot, project, cwd, input, &|_| true)
}

/// Like [`resolve`], among the symbols `keep` accepts: `--kind` and `--path`
/// narrowing an ambiguous name.
pub fn resolve_where(
    snapshot: &Snapshot<'_>,
    project: &Project,
    cwd: &Path,
    input: &str,
    keep: &dyn Fn(&SymbolHit) -> bool,
) -> Result<SymbolHit, Error> {
    let matches = match SymbolRef::parse(input)? {
        SymbolRef::Id {
            prefix,
            path,
            qualified_name,
            hash: Some(hash),
        } => snapshot.symbols_with_id_prefix(&format!(
            "{}{hash}",
            path_part(&prefix, &path, &qualified_name)
        ))?,
        SymbolRef::Id {
            prefix,
            path,
            qualified_name,
            hash: None,
        } => snapshot.symbols_with_id_prefix(&path_part(&prefix, &path, &qualified_name))?,
        SymbolRef::Location { file, line, column } => {
            let id = project.file_id(cwd, &file)?;
            innermost_at(snapshot.symbols_in(id.as_str())?, line, column)
        }
        SymbolRef::Name(name) => snapshot.symbols_named(&name)?,
    };
    one(input, matches.into_iter().filter(|hit| keep(hit)).collect())
}

/// The deepest symbol whose range holds the position: the one defined there.
fn innermost_at(symbols: Vec<SymbolHit>, line: u32, column: u32) -> Vec<SymbolHit> {
    let at = (line, column);
    symbols
        .into_iter()
        .filter(|hit| {
            let r = &hit.symbol.range;
            (r.start.line, r.start.column) <= at && at < (r.end.line, r.end.column)
        })
        .max_by_key(|hit| hit.symbol.range.start.byte)
        .into_iter()
        .collect()
}

fn one(input: &str, mut matches: Vec<SymbolHit>) -> Result<SymbolHit, Error> {
    match matches.len() {
        0 => Err(Error::UnknownSymbol {
            query: input.to_owned(),
        }),
        1 => Ok(matches.remove(0)),
        _ => Err(Error::AmbiguousSymbol {
            query: input.to_owned(),
            candidates: matches
                .into_iter()
                .take(MAX_CANDIDATES)
                .map(|hit| hit.symbol.symbol_id)
                .collect(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(prefix: &str, path: &str, name: &str, hash: Option<&str>) -> SymbolRef {
        SymbolRef::Id {
            prefix: prefix.to_owned(),
            path: path.to_owned(),
            qualified_name: name.to_owned(),
            hash: hash.map(str::to_owned),
        }
    }

    #[test]
    fn parses_every_input_form() {
        let parse = |s| SymbolRef::parse(s).unwrap();
        assert_eq!(
            parse("py:src/billing/invoice.py::Invoice.total#a3f9"),
            id(
                "py",
                "src/billing/invoice.py",
                "Invoice.total",
                Some("a3f9")
            )
        );
        assert_eq!(
            parse("py:src/billing/invoice.py::Invoice.total"),
            id("py", "src/billing/invoice.py", "Invoice.total", None)
        );
        assert_eq!(
            parse("cpp:src/a.cpp::ns::Foo::bar#0be4"),
            id("cpp", "src/a.cpp", "ns::Foo::bar", Some("0be4"))
        );
        assert_eq!(
            parse("src/Invoice.java:42:9"),
            SymbolRef::Location {
                file: "src/Invoice.java".to_owned(),
                line: 42,
                column: 9
            }
        );
        assert_eq!(
            parse("Invoice.total"),
            SymbolRef::Name("Invoice.total".to_owned())
        );
        assert_eq!(parse("ns::Foo"), SymbolRef::Name("ns::Foo".to_owned()));
    }

    fn row(name: &str, kind: &str, signature: &str, id: &str) -> SymbolRow {
        use crate::store::{Confidence, Point, Span};
        let p = Point {
            line: 1,
            column: 1,
            byte: 0,
        };
        SymbolRow {
            symbol_id: id.to_owned(),
            name: name.to_owned(),
            kind: kind.to_owned(),
            qualified_name: name.to_owned(),
            parent: None,
            range: Span { start: p, end: p },
            name_range: Span { start: p, end: p },
            signature: Some(signature.to_owned()),
            visibility: None,
            confidence: Confidence::Normal,
        }
    }

    /// Two signatures whose hashes share their first four digits.
    fn colliding_signatures() -> (String, String) {
        let mut seen: HashMap<String, String> = HashMap::new();
        for i in 0.. {
            let signature = format!("void f(T{i})");
            let short = full_hash("method", Some(&signature))[..HASH_DIGITS].to_owned();
            if let Some(other) = seen.insert(short, signature.clone()) {
                return (other, signature);
            }
        }
        unreachable!("the loop only ends on a collision")
    }

    #[test]
    fn colliding_hashes_are_lengthened() {
        let (a, b) = colliding_signatures();
        let short = format("j", "a", "f", "method", Some(&a));
        assert_eq!(short, format("j", "a", "f", "method", Some(&b)));
        let mut rows = vec![
            row("f", "method", &a, &short),
            row("f", "method", &b, &short),
        ];
        disambiguate(&mut rows, "j", "a");
        assert_ne!(rows[0].symbol_id, rows[1].symbol_id);
        assert!(
            rows[0].symbol_id.len() > short.len(),
            "{}",
            rows[0].symbol_id
        );
        assert!(rows[0].symbol_id.starts_with(&short));
    }

    #[test]
    fn identical_definitions_keep_one_id() {
        let mut rows = vec![
            row("f", "method", "void f()", "j:a::f#0000"),
            row("f", "method", "void f()", "j:a::f#0000"),
        ];
        disambiguate(&mut rows, "j", "a");
        assert_eq!(rows[0].symbol_id, rows[1].symbol_id);
    }

    #[test]
    fn malformed_ids_are_usage_errors() {
        for bad in [
            "py:src/a.py::A#xyz",
            "py:src/a.py::#a3f9",
            ":src/a.py::A",
            "src/a.java:0:1",
        ] {
            assert_eq!(SymbolRef::parse(bad).unwrap_err().code(), "usage", "{bad}");
        }
    }
}
