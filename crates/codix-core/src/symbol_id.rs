//! Symbol IDs: a readable path plus a short disambiguating hash, such as
//! `py:src/billing/invoice.py::Invoice.total#a3f9`, and the ways commands
//! name a symbol: a full ID, the path part alone, `file:line:col`, or a name.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::Error;
use crate::error::Candidate;
use crate::project::{FileId, Project};
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
/// signature, such as a C function defined in both branches of an `#if`,
/// are told apart by their order in the file: `#14e7`, then `#14e7~2`.
/// Parent references are left to the caller, which knows which symbol each
/// one means.
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
            // A longer hash cannot tell them apart; their order can.
            for (n, &i) in members.iter().enumerate().skip(1) {
                rows[i].symbol_id = format!("{}~{}", rows[i].symbol_id, n + 1);
            }
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
            Some((name, hash)) if is_hash(hash) => (name, Some(hash.to_owned())),
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

/// Hexadecimal digits, then `~` and the occurrence of a symbol whose kind and
/// signature repeat in its file.
fn is_hash(hash: &str) -> bool {
    let (digits, occurrence) = hash.split_once('~').unwrap_or((hash, "1"));
    !digits.is_empty()
        && digits.chars().all(|c| c.is_ascii_hexdigit())
        && !occurrence.is_empty()
        && occurrence.chars().all(|c| c.is_ascii_digit())
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
    let matches = candidates(snapshot, project, cwd, input, keep)?;
    one(project, cwd, input, matches)
}

/// Every symbol `input` names that `keep` accepts, by path and position:
/// what a lookup answers when a name is ambiguous. None is `unknown_symbol`.
pub fn resolve_all_where(
    snapshot: &Snapshot<'_>,
    project: &Project,
    cwd: &Path,
    input: &str,
    keep: &dyn Fn(&SymbolHit) -> bool,
) -> Result<Vec<SymbolHit>, Error> {
    let mut matches = candidates(snapshot, project, cwd, input, keep)?;
    if matches.is_empty() {
        return Err(Error::UnknownSymbol {
            query: input.to_owned(),
        });
    }
    // Definitions first; a C prototype after the functions it declares.
    matches.sort_by(|a, b| {
        (a.symbol.kind == DECLARATION)
            .cmp(&(b.symbol.kind == DECLARATION))
            .then(a.path.cmp(&b.path))
            .then(a.symbol.range.start.byte.cmp(&b.symbol.range.start.byte))
    });
    Ok(matches)
}

/// The kind of a function declared without its body.
pub(crate) const DECLARATION: &str = "declaration";

/// The other symbols that are the same C or C++ function as `hit`: its
/// declarations and definitions. They share the name, sit outside any type,
/// take as many parameters, and are visible across files, or, when one is
/// `static`, sit in the same file.
pub(crate) fn same_function(
    snapshot: &Snapshot<'_>,
    hit: &SymbolHit,
) -> Result<Vec<SymbolHit>, Error> {
    let function = |h: &SymbolHit| {
        matches!(h.language.as_str(), "c" | "cpp")
            && matches!(h.symbol.kind.as_str(), "function" | DECLARATION)
            && h.symbol.parent.is_none()
            && h.symbol.qualified_name == h.symbol.name
    };
    if !function(hit) {
        return Ok(Vec::new());
    }
    let arity = parameters(&hit.symbol.name, hit.symbol.signature.as_deref());
    let local = is_static(hit.symbol.signature.as_deref());
    Ok(snapshot
        .symbols_named(&hit.symbol.name)?
        .into_iter()
        .filter(|other| {
            function(other)
                && other.symbol.symbol_id != hit.symbol.symbol_id
                && parameters(&other.symbol.name, other.symbol.signature.as_deref()) == arity
                && ((!local && !is_static(other.symbol.signature.as_deref()))
                    || other.path == hit.path)
        })
        .collect())
}

/// How many parameters a C signature declares: the commas between the
/// parentheses after the name, `(void)` and `()` taking none.
fn parameters(name: &str, signature: Option<&str>) -> Option<usize> {
    let signature = signature?;
    let after = signature.find(&format!("{name}("))? + name.len() + 1;
    let mut depth = 1;
    let mut commas = 0;
    let mut inside = String::new();
    for c in signature[after..].chars() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            ',' if depth == 1 => commas += 1,
            _ => {}
        }
        inside.push(c);
    }
    let inside = inside.trim();
    Some(if inside.is_empty() || inside == "void" {
        0
    } else {
        commas + 1
    })
}

/// Whether a C signature declares a function `static`, visible in its file
/// only.
fn is_static(signature: Option<&str>) -> bool {
    signature.is_some_and(|s| s.split(['(', ' ', '\t', '\n']).any(|w| w == "static"))
}

/// Like [`resolve_where`], answering every overload when the matches are
/// overloads of one name in one file, first one first.
pub fn resolve_overloads_where(
    snapshot: &Snapshot<'_>,
    project: &Project,
    cwd: &Path,
    input: &str,
    keep: &dyn Fn(&SymbolHit) -> bool,
) -> Result<Vec<SymbolHit>, Error> {
    let mut matches = candidates(snapshot, project, cwd, input, keep)?;
    if matches.len() > 1 && overloads(&matches) {
        matches.sort_by_key(|hit| hit.symbol.range.start.byte);
        return Ok(matches);
    }
    one(project, cwd, input, matches).map(|hit| vec![hit])
}

/// Whether every match is a method, function or constructor of the same kind,
/// with the same qualified name, in the same file.
fn overloads(matches: &[SymbolHit]) -> bool {
    let first = &matches[0];
    matches!(
        first.symbol.kind.as_str(),
        "method" | "function" | "constructor"
    ) && matches.iter().all(|hit| {
        hit.path == first.path
            && hit.symbol.kind == first.symbol.kind
            && hit.symbol.qualified_name == first.symbol.qualified_name
    })
}

/// A type and its own constructors share its name; the name means the type.
fn prefer_types(matches: Vec<SymbolHit>) -> Vec<SymbolHit> {
    let (constructors, others): (Vec<&SymbolHit>, Vec<&SymbolHit>) = matches
        .iter()
        .partition(|hit| hit.symbol.kind == "constructor");
    match others.as_slice() {
        [ty] if !constructors.is_empty()
            && constructors
                .iter()
                .all(|c| c.symbol.parent.as_deref() == Some(ty.symbol.symbol_id.as_str())) =>
        {
            vec![(*ty).clone()]
        }
        _ => matches,
    }
}

/// Every symbol `input` names that `keep` accepts.
fn candidates(
    snapshot: &Snapshot<'_>,
    project: &Project,
    cwd: &Path,
    input: &str,
    keep: &dyn Fn(&SymbolHit) -> bool,
) -> Result<Vec<SymbolHit>, Error> {
    let matches = match SymbolRef::parse(input)? {
        SymbolRef::Id {
            prefix,
            path,
            qualified_name,
            hash: Some(hash),
        } => {
            let id = format!("{}{hash}", path_part(&prefix, &path, &qualified_name));
            let mut matches = snapshot.symbols_with_id_prefix(&id)?;
            // `#14e7` is a prefix of `#14e7~2`, but names `#14e7` itself.
            if matches.iter().any(|hit| hit.symbol.symbol_id == id) {
                matches.retain(|hit| hit.symbol.symbol_id == id);
            }
            matches
        }
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
    // A live symbol wins over an alias with its ID.
    if matches.is_empty()
        && let SymbolRef::Id {
            prefix,
            path,
            qualified_name,
            hash,
        } = SymbolRef::parse(input)?
        && let Some(new) = snapshot.alias(&format!(
            "{}{}",
            path_part(&prefix, &path, &qualified_name),
            hash.unwrap_or_default()
        ))?
    {
        return Err(Error::SymbolRenamed {
            old: input.to_owned(),
            new,
        });
    }
    Ok(prefer_types(
        matches.into_iter().filter(|hit| keep(hit)).collect(),
    ))
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

fn one(
    project: &Project,
    cwd: &Path,
    input: &str,
    mut matches: Vec<SymbolHit>,
) -> Result<SymbolHit, Error> {
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
                .map(|hit| candidate(project, cwd, hit))
                .collect(),
        }),
    }
}

/// A match as an ambiguity lists it.
pub(crate) fn candidate(project: &Project, cwd: &Path, hit: SymbolHit) -> Candidate {
    let at = hit.symbol.name_range.start;
    Candidate {
        path: project.display(cwd, &FileId::from_canonical(hit.path.as_str())),
        symbol_id: hit.symbol.symbol_id,
        kind: hit.symbol.kind,
        line: at.line,
        column: at.column,
        signature: hit.symbol.signature,
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
    fn identical_definitions_are_numbered_in_file_order() {
        let mut rows = vec![
            row("f", "function", "int f(int x)", "c:a::f#0000"),
            row("f", "function", "int f(int x)", "c:a::f#0000"),
            row("f", "function", "int f(int x)", "c:a::f#0000"),
        ];
        disambiguate(&mut rows, "c", "a");
        let ids: Vec<&str> = rows.iter().map(|r| r.symbol_id.as_str()).collect();
        assert_eq!(ids, ["c:a::f#0000", "c:a::f#0000~2", "c:a::f#0000~3"]);
        assert_eq!(
            SymbolRef::parse("c:a::f#0000~2").unwrap(),
            id("c", "a", "f", Some("0000~2"))
        );
    }

    #[test]
    fn parameters_are_counted_between_the_name_s_parentheses() {
        let count = |s| parameters("f", Some(s));
        assert_eq!(count("int f(void)"), Some(0));
        assert_eq!(count("int f()"), Some(0));
        assert_eq!(
            count("void *f(struct s *a, int (*cb)(int, int), ...)"),
            Some(3)
        );
        assert_eq!(
            count("static inline int f(int x) __attribute__((a, b))"),
            Some(1)
        );
        assert_eq!(parameters("f", None), None);
    }

    #[test]
    fn static_is_a_word_of_the_signature_before_its_parameters() {
        assert!(is_static(Some("static int f(int x)")));
        assert!(is_static(Some("static\tinline int f(void)")));
        assert!(!is_static(Some("int static_f(void)")));
        assert!(!is_static(None));
    }

    #[test]
    fn malformed_ids_are_usage_errors() {
        for bad in [
            "py:src/a.py::A#xyz",
            "py:src/a.py::#a3f9",
            "py:src/a.py::A#a3f9~",
            "py:src/a.py::A#a3f9~x",
            ":src/a.py::A",
            "src/a.java:0:1",
        ] {
            assert_eq!(SymbolRef::parse(bad).unwrap_err().code(), "usage", "{bad}");
        }
    }
}
