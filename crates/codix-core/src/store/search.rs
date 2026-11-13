//! Name search over the trigram full-text index.
//!
//! FTS5 offers token and substring matching, not fuzzy matching, so fuzzy
//! search takes the symbols sharing one of the query's rarest trigrams as
//! candidates and ranks them by edit distance.

use std::collections::HashMap;

use regex::{Regex, RegexBuilder};
use rusqlite::{OptionalExtension, Transaction, params_from_iter};
use serde::{Deserialize, Serialize};

use super::{SYMBOL_COLUMNS, SymbolHit, symbol_hit};

/// The most fuzzy matches read in full, closest first.
const FUZZY_CANDIDATES: usize = 500;

/// Shortest query the trigram index can match.
const TRIGRAM: usize = 3;

/// The query's trigrams the candidates are drawn from, rarest first. One edit
/// changes at most three trigrams, so a name within two edits of the query
/// still holds one of any seven of its trigrams; drawing from the rarest keeps
/// the candidates few even when the query's other trigrams are everywhere.
const FUZZY_TRIGRAMS: usize = 7;

/// How a name query matches; every mode ignores case unless asked to match it, `exact` and `prefix` for ASCII letters only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MatchMode {
    /// The name equals the query.
    Exact,
    /// The name starts with the query.
    Prefix,
    /// The qualified name matches a glob (`*`, `?`, `[…]`).
    Glob,
    /// The name is within a few edits of the query.
    Fuzzy,
    /// The qualified name holds a match of a regular expression; `^` and `$` anchor it.
    Regex,
}

/// A name query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameQuery {
    /// The query text.
    pub text: String,
    /// How it matches.
    pub mode: MatchMode,
    /// Whether case matters.
    pub case_sensitive: bool,
}

/// The regular expression a query's qualified names must match, for the
/// modes SQLite cannot match alone: a regex, and a glob ignoring case.
pub(super) fn pattern(query: &NameQuery) -> Result<Option<Regex>, regex::Error> {
    let source = match query.mode {
        MatchMode::Regex => query.text.clone(),
        MatchMode::Glob if !query.case_sensitive => glob_to_regex(&query.text),
        _ => return Ok(None),
    };
    RegexBuilder::new(&source)
        .case_insensitive(!query.case_sensitive)
        .build()
        .map(Some)
}

/// `pattern` is [`pattern`]'s for `query`.
pub(super) fn find(
    tx: &Transaction<'_>,
    query: &NameQuery,
    pattern: Option<&Regex>,
) -> rusqlite::Result<Vec<SymbolHit>> {
    let from = "FROM symbols s JOIN files f ON f.id = s.file_id";
    let fts = "FROM symbols_fts JOIN symbols s ON s.id = symbols_fts.rowid JOIN files f ON f.id = s.file_id";
    let text = std::slice::from_ref(&query.text);
    match (query.mode, pattern) {
        (MatchMode::Exact, _) => {
            let collate = if query.case_sensitive {
                ""
            } else {
                " COLLATE NOCASE"
            };
            select(
                tx,
                &format!("SELECT {SYMBOL_COLUMNS} {from} WHERE s.name = ?1{collate}"),
                text,
            )
        }
        (MatchMode::Prefix, _) => prefix(tx, from, &query.text, query.case_sensitive),
        (MatchMode::Fuzzy, _) => fuzzy(tx, from, fts, &query.text, query.case_sensitive),
        // The trigram index ignores case, so LIKE draws the candidates of a
        // glob ignoring case quickly; the pattern then checks them.
        (MatchMode::Glob, Some(pattern)) => matching(
            tx,
            "FROM symbols_fts JOIN symbols s ON s.id = symbols_fts.rowid
             WHERE symbols_fts.qualified_name LIKE ?1 ESCAPE '\\'",
            &[glob_to_like(&query.text)],
            pattern,
        ),
        (MatchMode::Glob, None) => select(
            tx,
            &format!("SELECT {SYMBOL_COLUMNS} {fts} WHERE symbols_fts.qualified_name GLOB ?1"),
            text,
        ),
        (MatchMode::Regex, Some(pattern)) => matching(tx, "FROM symbols s", &[], pattern),
        (MatchMode::Regex, None) => Ok(Vec::new()),
    }
}

/// The symbols `from` yields whose qualified name `pattern` matches: names
/// first, whole rows only for the matches.
fn matching(
    tx: &Transaction<'_>,
    from: &str,
    args: &[String],
    pattern: &Regex,
) -> rusqlite::Result<Vec<SymbolHit>> {
    let mut names = tx.prepare(&format!("SELECT s.id, s.qualified_name {from}"))?;
    let mut ids = Vec::new();
    let rows = names.query_map(params_from_iter(args), |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (id, name) = row?;
        if pattern.is_match(&name) {
            ids.push(id);
        }
    }
    let mut row = tx.prepare_cached(&format!(
        "SELECT {SYMBOL_COLUMNS} FROM symbols s JOIN files f ON f.id = s.file_id WHERE s.id = ?1"
    ))?;
    ids.into_iter()
        .map(|id| row.query_row([id], symbol_hit))
        .collect()
}

/// A SQLite glob as an anchored regular expression: `*`, `?`, and `[…]`
/// with `^` negating it and `]` first standing for itself.
fn glob_to_regex(glob: &str) -> String {
    let chars: Vec<char> = glob.chars().collect();
    let mut out = String::from("^(?s:");
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' => out.push_str(".*"),
            '?' => out.push('.'),
            '[' => {
                if let Some((class, end)) = glob_class(&chars, i) {
                    out.push_str(&class);
                    i = end;
                } else {
                    out.push_str("\\[");
                }
            }
            c => out.push_str(&regex::escape(c.encode_utf8(&mut [0; 4]))),
        }
        i += 1;
    }
    out.push_str(")$");
    out
}

/// The class opening at `chars[start]`, as a regex class, and where it ends.
fn glob_class(chars: &[char], start: usize) -> Option<(String, usize)> {
    let mut i = start + 1;
    let negated = chars.get(i) == Some(&'^');
    if negated {
        i += 1;
    }
    let first = i;
    // A `]` first stands for itself.
    let end = (first..chars.len()).find(|&j| chars[j] == ']' && j > first)?;
    let mut class = String::from(if negated { "[^" } else { "[" });
    let members = &chars[first..end];
    for (j, &c) in members.iter().enumerate() {
        let range = c == '-' && j > 0 && j + 1 < members.len();
        if range {
            class.push('-');
        } else if matches!(c, '\\' | '[' | ']' | '^' | '-' | '&' | '~') {
            class.push('\\');
            class.push(c);
        } else {
            class.push(c);
        }
    }
    class.push(']');
    Some((class, end))
}

/// A SQLite glob as a LIKE pattern matching at least what it matches, with
/// `\` escaping.
fn glob_to_like(glob: &str) -> String {
    let chars: Vec<char> = glob.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' => out.push('%'),
            '?' => out.push('_'),
            '[' => {
                if let Some((_, end)) = glob_class(&chars, i) {
                    out.push('_');
                    i = end;
                } else {
                    out.push('[');
                }
            }
            c @ ('%' | '_' | '\\') => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
        i += 1;
    }
    out
}

fn select(tx: &Transaction<'_>, sql: &str, args: &[String]) -> rusqlite::Result<Vec<SymbolHit>> {
    let mut stmt = tx.prepare(sql)?;
    stmt.query_map(params_from_iter(args), symbol_hit)?
        .collect()
}

/// Names starting with `text`: a range of the name index, the case-blind one
/// unless case matters, from `text` to `text` followed by the highest
/// character.
fn prefix(
    tx: &Transaction<'_>,
    from: &str,
    text: &str,
    case_sensitive: bool,
) -> rusqlite::Result<Vec<SymbolHit>> {
    let collate = if case_sensitive {
        ""
    } else {
        " COLLATE NOCASE"
    };
    select(
        tx,
        &format!(
            "SELECT {SYMBOL_COLUMNS} {from}
             WHERE s.name >= ?1{collate} AND s.name < ?2{collate}"
        ),
        &[text.to_owned(), format!("{text}{}", char::MAX)],
    )
}

fn fuzzy(
    tx: &Transaction<'_>,
    from: &str,
    fts: &str,
    text: &str,
    case_sensitive: bool,
) -> rusqlite::Result<Vec<SymbolHit>> {
    // The trigram index ignores case: candidates are drawn ignoring it, and
    // ranked by it when it matters.
    let fold = |s: &str| {
        if case_sensitive {
            s.to_owned()
        } else {
            s.to_lowercase()
        }
    };
    let query: Vec<char> = fold(text).chars().collect();
    if query.len() < TRIGRAM {
        return prefix(tx, from, text, case_sensitive);
    }
    let trigrams = trigrams(&text.to_lowercase().chars().collect::<Vec<_>>());
    let held = rarest(tx, &trigrams, FUZZY_TRIGRAMS)?;
    if held.is_empty() {
        return Ok(Vec::new());
    }
    let expression = held
        .iter()
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ");
    // Names first, ranked here; whole rows only for the closest.
    let mut names = tx.prepare(&format!(
        "SELECT s.id, s.name {fts} WHERE symbols_fts MATCH ?1"
    ))?;
    let max_distance = query.len() / 3 + 1;
    let mut seen: HashMap<String, Option<(usize, usize)>> = HashMap::new();
    let mut close: Vec<(usize, usize, i64)> = Vec::new();
    let rows = names.query_map([format!("name : ({expression})")], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (id, name) = row?;
        let score = *seen.entry(name).or_insert_with_key(|name| {
            let lower = name.to_lowercase();
            let chars: Vec<char> = fold(name).chars().collect();
            let distance = edit_distance(&query, &chars);
            let shared = trigrams
                .iter()
                .filter(|t| lower.contains(t.as_str()))
                .count();
            (distance <= max_distance).then_some((distance, shared))
        });
        if let Some((distance, shared)) = score {
            close.push((distance, shared, id));
        }
    }
    close.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    close.truncate(FUZZY_CANDIDATES);
    let mut row = tx.prepare_cached(&format!(
        "SELECT {SYMBOL_COLUMNS} FROM symbols s JOIN files f ON f.id = s.file_id WHERE s.id = ?1"
    ))?;
    let mut ranked: Vec<(usize, usize, SymbolHit)> = Vec::with_capacity(close.len());
    for (distance, shared, id) in close {
        ranked.push((distance, shared, row.query_row([id], symbol_hit)?));
    }
    ranked.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(b.1.cmp(&a.1))
            .then_with(|| a.2.symbol.name.cmp(&b.2.symbol.name))
            .then_with(|| a.2.path.cmp(&b.2.path))
    });
    Ok(ranked.into_iter().map(|(_, _, hit)| hit).collect())
}

/// The `count` trigrams fewest symbols hold, among those some symbol holds.
fn rarest<'t>(
    tx: &Transaction<'_>,
    trigrams: &'t [String],
    count: usize,
) -> rusqlite::Result<Vec<&'t String>> {
    let mut lookup = tx.prepare_cached("SELECT doc FROM symbols_fts_vocab WHERE term = ?1")?;
    let mut held: Vec<(i64, &String)> = Vec::new();
    for trigram in trigrams {
        let docs: Option<i64> = lookup.query_row([trigram], |row| row.get(0)).optional()?;
        if let Some(docs) = docs {
            held.push((docs, trigram));
        }
    }
    held.sort();
    Ok(held.into_iter().take(count).map(|(_, t)| t).collect())
}

fn trigrams(chars: &[char]) -> Vec<String> {
    let mut all: Vec<String> = chars.windows(TRIGRAM).map(|w| w.iter().collect()).collect();
    all.sort_unstable();
    all.dedup();
    all
}

/// Edit distance between two strings of Unicode scalar values, counting the
/// swap of two adjacent characters, the most common typo, as one edit
/// (optimal string alignment).
pub(crate) fn edit_distance(a: &[char], b: &[char]) -> usize {
    let mut before: Vec<usize> = vec![0; b.len() + 1];
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(ca != cb);
            let mut best = substitution.min(previous[j + 1] + 1).min(current[j] + 1);
            if i > 0 && j > 0 && *ca == b[j - 1] && a[i - 1] == *cb {
                best = best.min(before[j - 1] + 1);
            }
            current[j + 1] = best;
        }
        std::mem::swap(&mut before, &mut previous);
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn distance(a: &str, b: &str) -> usize {
        edit_distance(
            &a.chars().collect::<Vec<_>>(),
            &b.chars().collect::<Vec<_>>(),
        )
    }

    #[test]
    fn edit_distances_count_a_swap_as_one() {
        assert_eq!(distance("invoice", "invoice"), 0);
        assert_eq!(distance("invioce", "invoice"), 1);
        assert_eq!(distance("getbaen", "getbean"), 1);
        assert_eq!(distance("getbaen", "getbar"), 2);
        assert_eq!(distance("ab", "ba"), 1);
        assert_eq!(distance("total", "totals"), 1);
        assert_eq!(distance("", "abc"), 3);
        assert_eq!(distance("café", "cafe"), 1);
    }

    #[test]
    fn globs_translate_to_regexes() {
        assert_eq!(glob_to_regex("Invoice.*"), "^(?s:Invoice\\..*)$");
        assert_eq!(glob_to_regex("a?[^]x-z-]"), "^(?s:a.[^\\]x-z\\-])$");
        assert_eq!(glob_to_regex("[a"), "^(?s:\\[a)$");
    }

    #[test]
    fn globs_translate_to_like_patterns() {
        assert_eq!(glob_to_like("Invoice.*"), "Invoice.%");
        assert_eq!(glob_to_like("a_?[xy]%"), r"a\___\%");
    }

    #[test]
    fn trigrams_are_deduplicated() {
        assert_eq!(trigrams(&"aaaa".chars().collect::<Vec<_>>()), ["aaa"]);
    }
}
