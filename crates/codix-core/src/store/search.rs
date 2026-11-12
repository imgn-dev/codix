//! Name search over the trigram full-text index.
//!
//! FTS5 offers token and substring matching, not fuzzy matching, so fuzzy
//! search takes the symbols sharing one of the query's rarest trigrams as
//! candidates and ranks them by edit distance.

use std::collections::HashMap;

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
        MatchMode::Prefix => prefix(tx, from, &query.text),
        MatchMode::Glob => select(
            tx,
            &format!("SELECT {SYMBOL_COLUMNS} {fts} WHERE symbols_fts.qualified_name GLOB ?1"),
            std::slice::from_ref(&query.text),
        ),
        MatchMode::Fuzzy => fuzzy(tx, from, fts, &query.text),
    }
}

fn select(tx: &Transaction<'_>, sql: &str, args: &[String]) -> rusqlite::Result<Vec<SymbolHit>> {
    let mut stmt = tx.prepare(sql)?;
    stmt.query_map(params_from_iter(args), symbol_hit)?
        .collect()
}

/// Names starting with `text`, ignoring case: a range of the case-blind name
/// index, from `text` to `text` followed by the highest character. SQLite
/// folds the case of ASCII letters only.
fn prefix(tx: &Transaction<'_>, from: &str, text: &str) -> rusqlite::Result<Vec<SymbolHit>> {
    select(
        tx,
        &format!(
            "SELECT {SYMBOL_COLUMNS} {from}
             WHERE s.name >= ?1 COLLATE NOCASE AND s.name < ?2 COLLATE NOCASE"
        ),
        &[text.to_owned(), format!("{text}{}", char::MAX)],
    )
}

fn fuzzy(
    tx: &Transaction<'_>,
    from: &str,
    fts: &str,
    text: &str,
) -> rusqlite::Result<Vec<SymbolHit>> {
    let query: Vec<char> = text.to_lowercase().chars().collect();
    if query.len() < TRIGRAM {
        return prefix(tx, from, text);
    }
    let trigrams = trigrams(&query);
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
            let chars: Vec<char> = lower.chars().collect();
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
    fn trigrams_are_deduplicated() {
        assert_eq!(trigrams(&"aaaa".chars().collect::<Vec<_>>()), ["aaa"]);
    }
}
