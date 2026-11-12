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
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    count: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    files: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    sort: Option<Sort>,
}

/// Another order than the results' own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Sort {
    /// With `files`, the files with the most results first.
    Count,
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
    /// Return how many results there are, not the results.
    #[serde(default)]
    count: bool,
    /// Return one row per file, with its number of results, paged by file.
    #[serde(default)]
    files: bool,
    /// With files, `count` puts the files with the most results first, on one page.
    #[serde(default)]
    sort: Option<Sort>,
}

fn default_limit() -> usize {
    DEFAULT_LIMIT
}

impl TryFrom<PageFields> for Page {
    type Error = String;

    fn try_from(fields: PageFields) -> Result<Self, String> {
        let mut page = Self::new(fields.limit, fields.after).map_err(|e| e.cause())?;
        page.count = fields.count;
        page.files = fields.files;
        if let Some(sort) = fields.sort {
            page = page.sorted(sort).map_err(|e| e.cause())?;
        }
        Ok(page)
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
        Ok(Self {
            limit,
            after,
            count: false,
            files: false,
            sort: None,
        })
    }

    /// The same page, in another order: one page, with no next one, since
    /// the order changes as results do.
    pub fn sorted(self, sort: Sort) -> Result<Self, Error> {
        if !self.files {
            return Err(Error::Usage {
                cause: "--sort count orders files: it needs --files".to_owned(),
                hint: "add --files".to_owned(),
            });
        }
        if self.after.is_some() {
            return Err(Error::Usage {
                cause: "a sorted page has no next page to start after".to_owned(),
                hint: "drop --after, or --sort for pages in path order".to_owned(),
            });
        }
        Ok(Self {
            sort: Some(sort),
            ..self
        })
    }

    /// The same page, by file rather than by result.
    #[must_use]
    pub fn by_file(self) -> Self {
        Self {
            files: true,
            ..self
        }
    }

    /// The same page, counted rather than listed.
    #[must_use]
    pub fn counted(self) -> Self {
        Self {
            count: true,
            ..self
        }
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
        if self.count {
            let breakdown = breakdown(&places(&all));
            let items = Vec::new();
            return (
                Paged {
                    items,
                    total,
                    breakdown,
                    files: Vec::new(),
                },
                None,
            );
        }
        let start = self.after.as_deref().map_or(0, |after| {
            let cursor = cursor(&all, after, anchor);
            all.partition_point(|r| key(r, anchor) <= cursor)
        });
        if self.files {
            return self.files_page(&all, start);
        }
        let mut end = start.saturating_add(self.limit).min(total);
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
        // Paging on, a reader has chosen to read through.
        let breakdown = if self.after.is_none() && end - start < total {
            breakdown(&places(&all))
        } else {
            Vec::new()
        };
        let items = all.into_iter().skip(start).take(end - start).collect();
        (
            Paged {
                items,
                total,
                breakdown,
                files: Vec::new(),
            },
            next,
        )
    }

    /// The page of the files of `all` from the result `start` on.
    fn files_page<T: Ranked>(&self, all: &[T], start: usize) -> (Paged<T>, Option<String>) {
        let mut every = files(all);
        if self.sort == Some(Sort::Count) {
            every.sort_by(|a, b| {
                b.count
                    .cmp(&a.count)
                    .then_with(|| a.root_path.cmp(&b.root_path))
            });
            let total = every.len();
            let breakdown = if self.limit < total {
                let each: Vec<(&str, &str)> = every
                    .iter()
                    .map(|f| (f.path.as_str(), f.root_path.as_str()))
                    .collect();
                breakdown(&each)
            } else {
                Vec::new()
            };
            every.truncate(self.limit);
            return (
                Paged {
                    items: Vec::new(),
                    total,
                    breakdown,
                    files: every,
                },
                None,
            );
        }
        let first = files(&all[..start]).len();
        let total = every.len();
        let end = first.saturating_add(self.limit).min(total);
        let next = (end < total).then(|| every[end - 1].root_path.clone());
        let breakdown = if self.after.is_none() && end - first < total {
            let each: Vec<(&str, &str)> = every
                .iter()
                .map(|f| (f.path.as_str(), f.root_path.as_str()))
                .collect();
            breakdown(&each)
        } else {
            Vec::new()
        };
        let files = every[first..end].to_vec();
        (
            Paged {
                items: Vec::new(),
                total,
                breakdown,
                files,
            },
            next,
        )
    }
}

/// The files of sorted `all`, in order, with their number of results.
fn files<T: Ranked>(all: &[T]) -> Vec<Place> {
    let mut files: Vec<Place> = Vec::new();
    for r in all {
        let l = r.location();
        match files.last_mut() {
            Some(last) if last.root_path == l.root_path.as_str() => last.count += 1,
            _ => files.push(Place {
                path: l.path.clone(),
                root_path: l.root_path.as_str().to_owned(),
                count: 1,
            }),
        }
    }
    files
}

/// Each result's path as shown from the cwd, and root-relative.
fn places<T: Ranked>(all: &[T]) -> Vec<(&str, &str)> {
    all.iter()
        .map(|r| {
            let l = r.location();
            (l.path.as_str(), l.root_path.as_str())
        })
        .collect()
}

impl Default for Page {
    fn default() -> Self {
        Self {
            limit: DEFAULT_LIMIT,
            after: None,
            count: false,
            files: false,
            sort: None,
        }
    }
}

/// One page of results and the total they were taken from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Paged<T> {
    /// The results in this page.
    pub items: Vec<T>,
    /// Number of results before paging; by file, of files.
    pub total: usize,
    /// Where the results are, when a first page does not hold them all or
    /// they are counted: the places below the directory they share, most
    /// results first, at most 8; by file, counting files.
    pub breakdown: Vec<Place>,
    /// By file, this page's files with their number of results, in the
    /// results' order; `items` is then empty.
    pub files: Vec<Place>,
}

/// A directory or a file holding some of the results.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Place {
    /// Path relative to the cwd, as `--path` takes it.
    pub path: String,
    /// Root-relative path.
    pub root_path: String,
    /// Results there.
    pub count: usize,
}

/// The places a breakdown shows.
const PLACES: usize = 8;

/// The directories of a root-relative path, outermost first.
fn directory(root: &str) -> Vec<&str> {
    let mut parts: Vec<&str> = root.split('/').collect();
    parts.pop();
    parts
}

/// Where `entries` are, each a path as shown from the cwd and its
/// root-relative one: grouped by the first path component below the deepest
/// directory they all share, a directory or a file directly in it. Nothing
/// when they are all in one file.
fn breakdown(entries: &[(&str, &str)]) -> Vec<Place> {
    let Some((_, first)) = entries.first() else {
        return Vec::new();
    };
    let mut shared = directory(first);
    for (_, root) in &entries[1..] {
        let parts = directory(root);
        let same = shared
            .iter()
            .zip(&parts)
            .take_while(|(a, b)| a == b)
            .count();
        shared.truncate(same);
    }
    let mut places: Vec<Place> = Vec::new();
    for (shown, root) in entries {
        let parts: Vec<&str> = root.split('/').collect();
        let key = parts[..=shared.len()].join("/");
        if let Some(place) = places.iter_mut().find(|p| p.root_path == key) {
            place.count += 1;
            continue;
        }
        // The path as shown from the cwd ends as the root-relative one does.
        let tail = &root[key.len()..];
        let path = shown.strip_suffix(tail).unwrap_or(&key).to_owned();
        places.push(Place {
            path,
            root_path: key,
            count: 1,
        });
    }
    if places.len() < 2 {
        return Vec::new();
    }
    places.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.root_path.cmp(&b.root_path))
    });
    places.truncate(PLACES);
    places
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
    fn a_counted_page_has_the_total_only() {
        let (paged, next) = page(1, None)
            .counted()
            .apply(mentions(&["a.rs", "b.rs"], 2), None);
        assert_eq!(paged.items, Vec::<R>::new());
        assert_eq!(paged.total, 4);
        assert_eq!(next, None);
    }

    #[test]
    fn an_unbounded_page_takes_everything_after_its_place() {
        let (paged, next) =
            page(usize::MAX, Some("a.rs")).apply(mentions(&["a.rs", "b.rs"], 2), None);
        assert_eq!(keys(&paged.items), ["b.rs:1:1", "b.rs:2:1"]);
        assert_eq!(next, None);
    }

    fn grouped(all: &[R]) -> Vec<(String, String, usize)> {
        breakdown(&places(all))
            .into_iter()
            .map(|p| (p.path, p.root_path, p.count))
            .collect()
    }

    #[test]
    fn a_breakdown_groups_below_the_shared_directory() {
        let mut all = mentions(&["src/a/x.rs", "src/a/y.rs", "src/b/z.rs", "src/top.rs"], 1);
        all.push(r(false, "src/a/deep/w.rs", 1, 1));
        assert_eq!(
            grouped(&all),
            [
                ("src/a".to_owned(), "src/a".to_owned(), 3),
                ("src/b".to_owned(), "src/b".to_owned(), 1),
                ("src/top.rs".to_owned(), "src/top.rs".to_owned(), 1),
            ]
        );
    }

    #[test]
    fn a_breakdown_shows_paths_from_the_cwd() {
        let mut all = mentions(&["src/a/x.rs", "src/b/y.rs"], 1);
        for r in &mut all {
            r.1.path = format!("../{}", r.1.root_path.as_str());
        }
        assert_eq!(grouped(&all)[0].0, "../src/a");
    }

    #[test]
    fn a_breakdown_needs_two_places_and_keeps_eight() {
        assert_eq!(grouped(&mentions(&["src/a/x.rs"], 3)), []);
        let files: Vec<String> = (0..10).map(|i| format!("d{i}/f.rs")).collect();
        let files: Vec<&str> = files.iter().map(String::as_str).collect();
        assert_eq!(grouped(&mentions(&files, 1)).len(), PLACES);
    }

    #[test]
    fn only_a_first_page_that_does_not_fit_has_a_breakdown() {
        let all = || mentions(&["a/x.rs", "b/y.rs"], 2);
        assert_eq!(page(10, None).apply(all(), None).0.breakdown.len(), 0);
        assert_eq!(page(2, None).apply(all(), None).0.breakdown.len(), 2);
        assert_eq!(
            page(1, Some("a/x.rs:1:1"))
                .apply(all(), None)
                .0
                .breakdown
                .len(),
            0
        );
        assert_eq!(
            page(10, None)
                .counted()
                .apply(all(), None)
                .0
                .breakdown
                .len(),
            2
        );
    }

    #[test]
    fn by_file_a_page_holds_files_and_continues_after_the_last() {
        let all = || mentions(&["a/x.rs", "a/y.rs", "b/z.rs"], 2);
        let (paged, next) = page(2, None).by_file().apply(all(), None);
        let rows: Vec<(&str, usize)> = paged
            .files
            .iter()
            .map(|f| (f.root_path.as_str(), f.count))
            .collect();
        assert_eq!(rows, [("a/x.rs", 2), ("a/y.rs", 2)]);
        assert_eq!(paged.total, 3);
        assert_eq!(paged.items, Vec::<R>::new());
        assert_eq!(next.as_deref(), Some("a/y.rs"));
        assert_eq!(grouped_counts(&paged.breakdown), [("a", 2), ("b", 1)]);

        let (paged, next) = page(2, next.as_deref()).by_file().apply(all(), None);
        assert_eq!(paged.files.len(), 1);
        assert_eq!(paged.files[0].root_path, "b/z.rs");
        assert_eq!(next, None);
    }

    #[test]
    fn files_sorted_by_count_are_one_page_most_first() {
        let mut all = mentions(&["a.rs", "c.rs"], 1);
        all.extend(mentions(&["b.rs"], 3));
        sort_results(&mut all, None);
        let page = Page::new(2, None)
            .unwrap()
            .by_file()
            .sorted(Sort::Count)
            .unwrap();
        let (paged, next) = page.apply(all, None);
        let rows: Vec<(&str, usize)> = paged
            .files
            .iter()
            .map(|f| (f.root_path.as_str(), f.count))
            .collect();
        assert_eq!(rows, [("b.rs", 3), ("a.rs", 1)]);
        assert_eq!(paged.total, 3);
        assert_eq!(next, None);
        assert_eq!(
            Page::new(2, None)
                .unwrap()
                .sorted(Sort::Count)
                .unwrap_err()
                .code(),
            "usage"
        );
    }

    fn grouped_counts(places: &[Place]) -> Vec<(&str, usize)> {
        places
            .iter()
            .map(|p| (p.root_path.as_str(), p.count))
            .collect()
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
