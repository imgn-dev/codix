//! Bounded, ordered result lists.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::op::Outcome;

/// Default number of results per page.
pub const DEFAULT_LIMIT: usize = 50;

/// Which slice of the results to return.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page {
    limit: usize,
    offset: usize,
}

impl Page {
    /// A page of at most `limit` results, starting after `offset` results.
    pub fn new(limit: usize, offset: usize) -> Result<Self, Error> {
        if limit == 0 {
            return Err(Error::Usage {
                cause: "--limit must be at least 1".to_owned(),
                hint: format!("omit --limit for the default of {DEFAULT_LIMIT}"),
            });
        }
        Ok(Self { limit, offset })
    }

    /// Maximum number of results.
    pub fn limit(&self) -> usize {
        self.limit
    }

    /// Number of results skipped.
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// Keeps this page of `all`, which must already be sorted.
    pub fn apply<T>(&self, all: Vec<T>) -> Paged<T> {
        let total = all.len();
        let items = all.into_iter().skip(self.offset).take(self.limit).collect();
        Paged {
            items,
            total,
            offset: self.offset,
        }
    }
}

impl Default for Page {
    fn default() -> Self {
        Self {
            limit: DEFAULT_LIMIT,
            offset: 0,
        }
    }
}

/// One page of results and the total they were taken from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Paged<T> {
    /// The results in this page.
    pub items: Vec<T>,
    /// Number of results before paging.
    pub total: usize,
    /// Number of results skipped.
    pub offset: usize,
}

impl<T> Paged<T> {
    /// The `--offset` of the next page, if any results remain.
    pub fn next_offset(&self) -> Option<usize> {
        let end = self.offset + self.items.len();
        (end < self.total).then_some(end)
    }
}

impl<T> Outcome for Paged<T> {
    fn has_results(&self) -> bool {
        self.total > 0
    }
}

/// A position in a file: root-relative path, 1-based line and column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    /// Root-relative path with `/` separators.
    pub path: String,
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

/// Sorts for usefulness: definitions first, then results in `anchor` (the file
/// the query is about, if any), then by path, line and column.
pub fn sort_results<T: Ranked>(results: &mut [T], anchor: Option<&str>) {
    results.sort_by(|a, b| compare(a, b, anchor));
}

fn compare<T: Ranked>(a: &T, b: &T, anchor: Option<&str>) -> Ordering {
    let in_anchor = |r: &T| anchor.is_some_and(|path| r.location().path == path);
    let (la, lb) = (a.location(), b.location());
    b.is_definition()
        .cmp(&a.is_definition())
        .then_with(|| in_anchor(b).cmp(&in_anchor(a)))
        .then_with(|| la.path.cmp(&lb.path))
        .then_with(|| la.line.cmp(&lb.line))
        .then_with(|| la.column.cmp(&lb.column))
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn definitions_then_anchor_file_then_position() {
        let mut results = vec![
            r(false, "b.rs", 3, 1),
            r(false, "a.rs", 9, 2),
            r(false, "a.rs", 9, 1),
            r(true, "z.rs", 1, 1),
            r(false, "c.rs", 1, 1),
        ];
        sort_results(&mut results, Some("c.rs"));
        assert_eq!(
            keys(&results),
            ["D z.rs:1:1", "c.rs:1:1", "a.rs:9:1", "a.rs:9:2", "b.rs:3:1"]
        );
    }

    #[test]
    fn page_keeps_slice_and_total() {
        let page = Page::new(2, 1).unwrap().apply(vec![1, 2, 3, 4]);
        assert_eq!(page.items, [2, 3]);
        assert_eq!(page.total, 4);
        assert_eq!(page.next_offset(), Some(3));
    }

    #[test]
    fn last_page_has_no_next_offset() {
        let page = Page::new(2, 2).unwrap().apply(vec![1, 2, 3, 4]);
        assert_eq!(page.next_offset(), None);
    }

    #[test]
    fn offset_past_the_end_gives_empty_page_with_total() {
        let page = Page::new(5, 10).unwrap().apply(vec![1, 2]);
        assert_eq!(page.items, Vec::<i32>::new());
        assert_eq!(page.total, 2);
        assert!(page.has_results());
    }

    #[test]
    fn zero_limit_is_a_usage_error() {
        assert_eq!(Page::new(0, 0).unwrap_err().code(), "usage");
    }
}
