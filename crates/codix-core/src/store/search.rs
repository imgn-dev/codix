//! Name search over the trigram full-text index.
//!
//! FTS5 offers token and substring matching, not fuzzy matching, so fuzzy
//! search takes the symbols sharing a trigram with the query as candidates and
//! ranks them by edit distance.

use rusqlite::{Transaction, params_from_iter};
use serde::{Deserialize, Serialize};

use super::{SYMBOL_COLUMNS, SymbolHit, symbol_hit};

/// Fuzzy candidates read from the full-text index before ranking.
const FUZZY_CANDIDATES: usize = 500;

/// Shortest query the trigram index can match.
const TRIGRAM: usize = 3;

/// How a name query matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MatchMode {
    /// The name equals the query.
    Exact,
    /// The name starts with the query, ignoring case.
    Prefix,
    /// The qualified name matches a glob (`*`, `?`, `[…]`), case-sensitively.
    Glob,
    /// The name is within a few edits of the query, ignoring case.
    Fuzzy,
}

/// A name query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameQuery {
    /// The query text.
    pub text: String,
    /// How it matches.
    pub mode: MatchMode,
}

pub(super) fn find(tx: &Transaction<'_>, query: &NameQuery) -> rusqlite::Result<Vec<SymbolHit>> {
    let from = "FROM symbols s JOIN files f ON f.id = s.file_id";
    let fts = "FROM symbols_fts JOIN symbols s ON s.id = symbols_fts.rowid JOIN files f ON f.id = s.file_id";
    match query.mode {
        MatchMode::Exact => select(
            tx,
            &format!("SELECT {SYMBOL_COLUMNS} {from} WHERE s.name = ?1"),
            std::slice::from_ref(&query.text),
        ),
        MatchMode::Prefix => prefix(tx, fts, &query.text),
        MatchMode::Glob => select(
            tx,
            &format!("SELECT {SYMBOL_COLUMNS} {fts} WHERE symbols_fts.qualified_name GLOB ?1"),
            std::slice::from_ref(&query.text),
        ),
        MatchMode::Fuzzy => fuzzy(tx, fts, &query.text),
    }
}

fn select(tx: &Transaction<'_>, sql: &str, args: &[String]) -> rusqlite::Result<Vec<SymbolHit>> {
    let mut stmt = tx.prepare(sql)?;
    stmt.query_map(params_from_iter(args), symbol_hit)?
        .collect()
}

fn prefix(tx: &Transaction<'_>, fts: &str, text: &str) -> rusqlite::Result<Vec<SymbolHit>> {
    let escaped: String = text
        .chars()
        .flat_map(|c| match c {
            '%' | '_' | '\\' => vec!['\\', c],
            c => vec![c],
        })
        .collect();
    select(
        tx,
        &format!("SELECT {SYMBOL_COLUMNS} {fts} WHERE symbols_fts.name LIKE ?1 ESCAPE '\\'"),
        &[format!("{escaped}%")],
    )
}

fn fuzzy(tx: &Transaction<'_>, fts: &str, text: &str) -> rusqlite::Result<Vec<SymbolHit>> {
    let query: Vec<char> = text.to_lowercase().chars().collect();
    if query.len() < TRIGRAM {
        return prefix(tx, fts, text);
    }
    let trigrams = trigrams(&query);
    let expression = trigrams
        .iter()
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ");
    let candidates = select(
        tx,
        &format!(
            "SELECT {SYMBOL_COLUMNS} {fts} WHERE symbols_fts MATCH ?1
             ORDER BY bm25(symbols_fts) LIMIT {FUZZY_CANDIDATES}"
        ),
        &[format!("name : ({expression})")],
    )?;
    let max_distance = query.len() / 3 + 1;
    let mut ranked: Vec<(usize, usize, SymbolHit)> = candidates
        .into_iter()
        .filter_map(|hit| {
            let name: Vec<char> = hit.symbol.name.to_lowercase().chars().collect();
            let distance = levenshtein(&query, &name);
            let shared = trigrams
                .iter()
                .filter(|t| hit.symbol.name.to_lowercase().contains(t.as_str()))
                .count();
            (distance <= max_distance).then_some((distance, shared, hit))
        })
        .collect();
    ranked.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(b.1.cmp(&a.1))
            .then_with(|| a.2.symbol.name.cmp(&b.2.symbol.name))
            .then_with(|| a.2.path.cmp(&b.2.path))
    });
    Ok(ranked.into_iter().map(|(_, _, hit)| hit).collect())
}

fn trigrams(chars: &[char]) -> Vec<String> {
    let mut all: Vec<String> = chars.windows(TRIGRAM).map(|w| w.iter().collect()).collect();
    all.sort_unstable();
    all.dedup();
    all
}

/// Edit distance between two strings of Unicode scalar values.
fn levenshtein(a: &[char], b: &[char]) -> usize {
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(ca != cb);
            current[j + 1] = substitution.min(previous[j + 1] + 1).min(current[j] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn distance(a: &str, b: &str) -> usize {
        levenshtein(
            &a.chars().collect::<Vec<_>>(),
            &b.chars().collect::<Vec<_>>(),
        )
    }

    #[test]
    fn edit_distance() {
        assert_eq!(distance("invoice", "invoice"), 0);
        assert_eq!(distance("invioce", "invoice"), 2);
        assert_eq!(distance("total", "totals"), 1);
        assert_eq!(distance("", "abc"), 3);
        assert_eq!(distance("café", "cafe"), 1);
    }

    #[test]
    fn trigrams_are_deduplicated() {
        assert_eq!(trigrams(&"aaaa".chars().collect::<Vec<_>>()), ["aaa"]);
    }
}
