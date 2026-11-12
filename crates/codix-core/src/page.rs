//! Bounded, ordered result lists, paged by position.
//!
//! A page is cut from the results as they are now, so it cannot be an offset:
//! results edited away before it would shift it. A page starts after a
//! place instead, the last result of the page before, and ends at a file
//! boundary when it can, so edits to files already seen do not move it.

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::op::Outcome;
use crate::project::FileId;

/// Default number of results per page.
pub const DEFAULT_LIMIT: usize = 50;

/// Which slice of the results to return.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(try_from = "PageFields")]
pub struct Page {
    limit: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    after: Option<String>,
}

/// [`Page`] as requests carry it, checked on the way in.
#[derive(Deserialize, schemars::JsonSchema)]
struct PageFields {
    /// Maximum number of results.
    #[serde(default = "default_limit")]
    #[schemars(range(min = 1))]
    limit: usize,
    /// Return the results after this place, as a page's `next` gives it.
    #[serde(default)]
    after: Option<String>,
}

fn default_limit() -> usize {
    DEFAULT_LIMIT
}

impl TryFrom<PageFields> for Page {
    type Error = String;

    fn try_from(fields: PageFields) -> Result<Self, String> {
        Self::new(fields.limit, fields.after).map_err(|e| e.cause())
    }
}

impl Page {
    /// A page of at most `limit` results, after the place `after`.
    pub fn new(limit: usize, after: Option<String>) -> Result<Self, Error> {
        if limit == 0 {
            return Err(Error::Usage {
                cause: "--limit must be at least 1".to_owned(),
                hint: format!("omit --limit for the default of {DEFAULT_LIMIT}"),
            });
        }
        if after.as_deref() == Some("") {
            return Err(Error::Usage {
                cause: "--after needs a place".to_owned(),
                hint: "pass the `after` of the previous response's next page".to_owned(),
            });
        }
        Ok(Self { limit, after })
    }

    /// Maximum number of results.
    pub fn limit(&self) -> usize {
        self.limit
    }

    /// The place the page starts after, if any.
    pub fn after(&self) -> Option<&str> {
        self.after.as_deref()
    }

    /// Keeps this page of `all`, sorted by [`sort_results`] with the same
    /// `anchor`, and returns where the next page starts, if any results
    /// remain.
    pub fn apply<T: Ranked>(
        &self,
        all: Vec<T>,
        anchor: Option<&str>,
    ) -> (Paged<T>, Option<String>) {
        let total = all.len();
        let start = self.after.as_deref().map_or(0, |after| {
            let cursor = cursor(&all, after, anchor);
            all.partition_point(|r| key(r, anchor) <= cursor)
        });
        let mut end = (start + self.limit).min(total);
        let mut next = None;
        if end < total {
            let last = all[end - 1].location();
            if all[end].location().root_path == last.root_path {
                // The page would cut a file: end it before that file when
                // that keeps at least half the page.
                let file_start = (start..end)
                    .rev()
                    .take_while(|&i| all[i].location().root_path == last.root_path)
                    .last()
                    .unwrap_or(start);
                if file_start > start && (file_start - start) * 2 >= self.limit {
                    end = file_start;
                    next = Some(all[end - 1].location().root_path.as_str().to_owned());
                } else {
                    next = Some(format!(
                        "{}:{}:{}",
                        last.root_path.as_str(),
                        last.line,
                        last.column
                    ));
                }
            } else {
                next = Some(last.root_path.as_str().to_owned());
            }
        }
        let items = all.into_iter().skip(start).take(end - start).collect();
        (Paged { items, total }, next)
    }
}

impl Default for Page {
    fn default() -> Self {
        Self {
            limit: DEFAULT_LIMIT,
            after: None,
        }
    }
}

/// One page of results and the total they were taken from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Paged<T> {
    /// The results in this page.
    pub items: Vec<T>,
    /// Number of results before paging.
    pub total: usize,
}

impl<T> Outcome for Paged<T> {
    fn has_results(&self) -> bool {
        self.total > 0
    }
}

/// A position in a file: its path as shown from the cwd, its root-relative
/// identity, and a 1-based line and column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Location {
    /// Path relative to the cwd, with `/` separators.
    pub path: String,
    /// Root-relative path.
    pub root_path: FileId,
    /// 1-based line.
    pub line: u32,
    /// 1-based column, in Unicode scalar values.
    pub column: u32,
}

/// A result that takes part in the shared sort order.
pub trait Ranked {
    /// Whether the result is a definition rather than a mention.
    fn is_definition(&self) -> bool;
    /// Where the result is.
    fn location(&self) -> &Location;
}

/// Sorts for usefulness and paging: results in `anchor` (the file the query
/// is about, if any) first, then by path; in a file, definitions first, then
/// by line and column.
pub fn sort_results<T: Ranked>(results: &mut [T], anchor: Option<&str>) {
    results.sort_by(|a, b| key(a, anchor).cmp(&key(b, anchor)));
}

/// Where a result sorts: fields in order of precedence.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Key<'a> {
    elsewhere: bool,
    path: &'a str,
    mention: bool,
    line: u32,
    column: u32,
}

fn key<'a, T: Ranked>(result: &'a T, anchor: Option<&str>) -> Key<'a> {
    let l = result.location();
    Key {
        elsewhere: anchor != Some(l.root_path.as_str()),
        path: l.root_path.as_str(),
        mention: !result.is_definition(),
        line: l.line,
        column: l.column,
    }
}

/// Where the place `after` sorts among `all`: the result there, or where a
/// mention there would sort, when edits removed it; after every result of
/// the file, for a path alone.
fn cursor<'a, T: Ranked>(all: &'a [T], after: &'a str, anchor: Option<&str>) -> Key<'a> {
    let (path, at) = place(after);
    let (line, column) = at.unwrap_or((u32::MAX, u32::MAX));
    let there = all.iter().find(|r| {
        let l = r.location();
        at.is_some() && l.root_path.as_str() == path && l.line == line && l.column == column
    });
    there.map_or(
        Key {
            elsewhere: anchor != Some(path),
            path,
            mention: true,
            line,
            column,
        },
        |r| key(r, anchor),
    )
}

/// A place: `path:line:col`, or a path alone.
fn place(after: &str) -> (&str, Option<(u32, u32)>) {
    let mut parts = after.rsplitn(3, ':');
    if let (Some(column), Some(line), Some(path)) = (parts.next(), parts.next(), parts.next())
        && let (Ok(line), Ok(column)) = (line.parse(), column.parse())
    {
        return (path, Some((line, column)));
    }
    (after, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    struct R(bool, Location);

    impl Ranked for R {
        fn is_definition(&self) -> bool {
            self.0
        }
        fn location(&self) -> &Location {
            &self.1
        }
    }

    fn r(definition: bool, path: &str, line: u32, column: u32) -> R {
        R(
            definition,
            Location {
                path: path.to_owned(),
                root_path: FileId::from_canonical(path),
                line,
                column,
            },
        )
    }

    fn keys(results: &[R]) -> Vec<String> {
        results
            .iter()
            .map(|R(d, l)| {
                format!(
                    "{}{}:{}:{}",
                    if *d { "D " } else { "" },
                    l.path,
                    l.line,
                    l.column
                )
            })
            .collect()
    }

    /// `per_file` mentions in each of `files`, sorted.
    fn mentions(files: &[&str], per_file: u32) -> Vec<R> {
        let mut all: Vec<R> = files
            .iter()
            .flat_map(|f| (1..=per_file).map(move |line| r(false, f, line, 1)))
            .collect();
        sort_results(&mut all, None);
        all
    }

    fn page(limit: usize, after: Option<&str>) -> Page {
        Page::new(limit, after.map(str::to_owned)).unwrap()
    }

    #[test]
    fn anchor_file_then_path_and_definitions_first_in_a_file() {
        let mut results = vec![
            r(false, "b.rs", 3, 1),
            r(false, "a.rs", 9, 2),
            r(false, "c.rs", 2, 1),
            r(false, "a.rs", 9, 1),
            r(true, "c.rs", 5, 1),
            r(false, "c.rs", 1, 1),
        ];
        sort_results(&mut results, Some("c.rs"));
        assert_eq!(
            keys(&results),
            [
                "D c.rs:5:1",
                "c.rs:1:1",
                "c.rs:2:1",
                "a.rs:9:1",
                "a.rs:9:2",
                "b.rs:3:1"
            ]
        );
    }

    #[test]
    fn a_page_ending_between_files_continues_after_the_last_file() {
        let (paged, next) = page(4, None).apply(mentions(&["a.rs", "b.rs"], 2), None);
        assert_eq!(paged.items.len(), 4);
        assert_eq!(paged.total, 4);
        assert_eq!(next, None);

        let (paged, next) = page(2, None).apply(mentions(&["a.rs", "b.rs"], 2), None);
        assert_eq!(keys(&paged.items), ["a.rs:1:1", "a.rs:2:1"]);
        assert_eq!(next.as_deref(), Some("a.rs"));
        let (paged, next) = page(2, next.as_deref()).apply(mentions(&["a.rs", "b.rs"], 2), None);
        assert_eq!(keys(&paged.items), ["b.rs:1:1", "b.rs:2:1"]);
        assert_eq!(next, None);
    }

    #[test]
    fn a_page_ends_before_a_file_it_would_cut_when_that_keeps_half() {
        let all = mentions(&["a.rs", "b.rs", "c.rs"], 2);
        let (paged, next) = page(3, None).apply(all, None);
        assert_eq!(keys(&paged.items), ["a.rs:1:1", "a.rs:2:1"]);
        assert_eq!(next.as_deref(), Some("a.rs"));
    }

    #[test]
    fn a_file_bigger_than_half_a_page_is_cut_at_a_place() {
        let all = mentions(&["a.rs", "b.rs"], 5);
        let (paged, next) = page(4, None).apply(all, None);
        assert_eq!(
            keys(&paged.items),
            ["a.rs:1:1", "a.rs:2:1", "a.rs:3:1", "a.rs:4:1"]
        );
        assert_eq!(next.as_deref(), Some("a.rs:4:1"));
        let (paged, next) = page(4, next.as_deref()).apply(mentions(&["a.rs", "b.rs"], 5), None);
        // Ending before b.rs would keep a single result: b.rs is cut too.
        assert_eq!(
            keys(&paged.items),
            ["a.rs:5:1", "b.rs:1:1", "b.rs:2:1", "b.rs:3:1"]
        );
        assert_eq!(next.as_deref(), Some("b.rs:3:1"));
    }

    #[test]
    fn results_edited_away_before_the_cursor_skip_nothing() {
        let (_, next) = page(2, None).apply(mentions(&["a.rs", "b.rs", "c.rs"], 2), None);
        assert_eq!(next.as_deref(), Some("a.rs"));
        // The first page's results were fixed: a.rs has none left.
        let (paged, _) = page(2, next.as_deref()).apply(mentions(&["b.rs", "c.rs"], 2), None);
        assert_eq!(keys(&paged.items), ["b.rs:1:1", "b.rs:2:1"]);
    }

    #[test]
    fn a_cursor_whose_result_is_gone_continues_where_it_would_sort() {
        let all = vec![
            r(false, "a.rs", 1, 1),
            r(false, "a.rs", 7, 1),
            r(false, "b.rs", 1, 1),
        ];
        let (paged, _) = page(10, Some("a.rs:4:2")).apply(all, None);
        assert_eq!(keys(&paged.items), ["a.rs:7:1", "b.rs:1:1"]);
    }

    #[test]
    fn a_path_cursor_skips_the_definitions_of_its_file() {
        let mut all = vec![
            r(true, "a.rs", 9, 1),
            r(false, "a.rs", 1, 1),
            r(false, "b.rs", 1, 1),
        ];
        sort_results(&mut all, Some("a.rs"));
        let (paged, _) = page(10, Some("a.rs")).apply(all, Some("a.rs"));
        assert_eq!(keys(&paged.items), ["b.rs:1:1"]);
    }

    #[test]
    fn places_parse_from_the_right() {
        assert_eq!(place("src/a.rs:12:3"), ("src/a.rs", Some((12, 3))));
        assert_eq!(place("src/a.rs"), ("src/a.rs", None));
        assert_eq!(place("odd:name.rs"), ("odd:name.rs", None));
        assert_eq!(place("odd:1:x"), ("odd:1:x", None));
    }

    #[test]
    fn zero_limit_and_empty_after_are_usage_errors() {
        assert_eq!(Page::new(0, None).unwrap_err().code(), "usage");
        assert_eq!(
            Page::new(1, Some(String::new())).unwrap_err().code(),
            "usage"
        );
    }
}
