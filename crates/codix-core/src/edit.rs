//! The edit engine: edit sets, the regions they touch, line endings, unified
//! diffs, and the preview every edit command returns.
//!
//! An edit replaces a byte range of one file. A region is the whole lines an
//! edit touches: a preview records each file's hash and its regions'
//! content, so that `apply` can tell a file changed elsewhere from one
//! changed where the edits go.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::page::Location;

/// Lines of context around each change in a diff.
const CONTEXT: usize = 3;

/// One replacement in one file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edit {
    /// First byte replaced.
    pub start: usize,
    /// Byte after the last one replaced; equal to `start` for an insertion.
    pub end: usize,
    /// The new text. Its line breaks take the ending of the line it lands
    /// in.
    pub replacement: String,
}

/// The edits of one file, sorted and not overlapping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEdits {
    /// Root-relative path.
    pub path: String,
    /// The edits, in file order.
    pub edits: Vec<Edit>,
}

/// Every edit of a plan, by file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditSet {
    /// Files in path order.
    pub files: Vec<FileEdits>,
}

impl EditSet {
    /// Groups edits by file and sorts them. Overlapping edits are a bug in
    /// the planner.
    pub fn new(edits: impl IntoIterator<Item = (String, Edit)>) -> Result<Self, Error> {
        let mut by_file: std::collections::BTreeMap<String, Vec<Edit>> =
            std::collections::BTreeMap::new();
        for (path, edit) in edits {
            by_file.entry(path).or_default().push(edit);
        }
        let mut files = Vec::with_capacity(by_file.len());
        for (path, mut edits) in by_file {
            edits.sort_by_key(|e| (e.start, e.end));
            edits.dedup();
            if let Some(pair) = edits.windows(2).find(|w| w[0].end > w[1].start) {
                return Err(Error::Internal {
                    message: format!(
                        "{path}: edits at bytes {}..{} and {}..{} overlap",
                        pair[0].start, pair[0].end, pair[1].start, pair[1].end
                    ),
                });
            }
            files.push(FileEdits { path, edits });
        }
        Ok(Self { files })
    }

    /// Number of edits.
    pub fn sites(&self) -> usize {
        self.files.iter().map(|f| f.edits.len()).sum()
    }
}

/// The whole lines one edit touches, as they were when it was planned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Region {
    /// First line, 1-based.
    pub first_line: u32,
    /// Last line, 1-based, inclusive.
    pub last_line: u32,
    /// Their content, line endings included.
    pub text: String,
}

/// A file as a plan found it: its hash and the regions of its edits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedFile {
    /// Root-relative path.
    pub path: String,
    /// BLAKE3 hash of the content, in hex.
    pub hash: String,
    /// One region per edit, in file order.
    pub regions: Vec<Region>,
}

/// Where a planned file stands now.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum PreviewState {
    /// Unchanged since planning: applies as planned.
    Ready,
    /// Changed outside the edited regions: `apply` rebases it.
    Rebase,
    /// An edited region changed, or the file is gone: plan again.
    Stale,
}

/// Where a planned file stands, given its current content (`None` when it
/// is gone).
pub fn file_state(planned: &PlannedFile, content: Option<&str>) -> PreviewState {
    let Some(content) = content else {
        return PreviewState::Stale;
    };
    if hash(content.as_bytes()) == planned.hash {
        return PreviewState::Ready;
    }
    let lines: Vec<&str> = content.split_inclusive('\n').collect();
    let intact = planned.regions.iter().all(|region| {
        let first = region.first_line as usize - 1;
        let last = region.last_line as usize;
        // A region may have moved: look for its exact lines anywhere.
        let count = last - first;
        lines
            .windows(count.max(1))
            .any(|window| window.concat() == region.text)
    });
    if intact {
        PreviewState::Rebase
    } else {
        PreviewState::Stale
    }
}

/// BLAKE3 of a file's content, in hex.
pub fn hash(content: &[u8]) -> String {
    blake3::hash(content).to_hex().to_string()
}

/// Line starts of `content`: byte offsets of each line's first byte.
fn line_starts(content: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(content.match_indices('\n').map(|(i, _)| i + 1))
        .filter(|&i| i < content.len() || i == 0)
        .collect()
}

/// The 0-based line holding `byte`.
fn line_of(starts: &[usize], byte: usize) -> usize {
    starts.partition_point(|&s| s <= byte).saturating_sub(1)
}

/// Byte range of the 0-based lines `first..=last`, line endings included.
fn lines_range(content: &str, starts: &[usize], first: usize, last: usize) -> (usize, usize) {
    let end = starts.get(last + 1).copied().unwrap_or(content.len());
    (starts[first], end)
}

/// The 0-based lines an edit touches. An insertion touches the line it
/// lands in; a replacement ending right after a line break does not touch
/// the next line.
fn touched(starts: &[usize], edit: &Edit) -> (usize, usize) {
    let first = line_of(starts, edit.start);
    let last = if edit.end > edit.start {
        line_of(starts, edit.end - 1)
    } else {
        first
    };
    (first, last)
}

/// The region of each edit, in order.
pub fn regions(content: &str, edits: &[Edit]) -> Vec<Region> {
    let starts = line_starts(content);
    edits
        .iter()
        .map(|edit| {
            let (first, last) = touched(&starts, edit);
            let (from, to) = lines_range(content, &starts, first, last);
            Region {
                first_line: u32::try_from(first + 1).unwrap_or(u32::MAX),
                last_line: u32::try_from(last + 1).unwrap_or(u32::MAX),
                text: content[from..to].to_owned(),
            }
        })
        .collect()
}

/// The file as a plan records it.
pub fn plan_file(path: &str, content: &str, edits: &[Edit]) -> PlannedFile {
    PlannedFile {
        path: path.to_owned(),
        hash: hash(content.as_bytes()),
        regions: regions(content, edits),
    }
}

/// The line ending of the line holding `byte`: that line's own, else the
/// previous line's for a last line without one, else `\n`.
fn ending_at(content: &str, starts: &[usize], byte: usize) -> &'static str {
    let line = line_of(starts, byte);
    let ending = |line: usize| {
        let (from, to) = lines_range(content, starts, line, line);
        let text = &content[from..to];
        if text.ends_with("\r\n") {
            Some("\r\n")
        } else if text.ends_with('\n') {
            Some("\n")
        } else {
            None
        }
    };
    ending(line)
        .or_else(|| line.checked_sub(1).and_then(ending))
        .unwrap_or("\n")
}

/// A replacement with its line breaks in the ending of the line it lands in.
fn with_ending(replacement: &str, ending: &str) -> String {
    replacement.replace("\r\n", "\n").replace('\n', ending)
}

/// The content after the edits, given sorted and not overlapping, as an
/// [`EditSet`] holds them.
pub fn apply(content: &str, edits: &[Edit]) -> String {
    let starts = line_starts(content);
    let mut out = String::with_capacity(content.len());
    let mut at = 0;
    for edit in edits {
        out.push_str(&content[at..edit.start]);
        out.push_str(&with_ending(
            &edit.replacement,
            ending_at(content, &starts, edit.start),
        ));
        at = edit.end;
    }
    out.push_str(&content[at..]);
    out
}

/// Edits whose lines overlap, merged into one change of those lines.
struct Block {
    /// 0-based lines, inclusive.
    first: usize,
    last: usize,
    edits: Vec<Edit>,
}

fn blocks(starts: &[usize], edits: &[Edit]) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    for edit in edits {
        let (first, last) = touched(starts, edit);
        match blocks.last_mut() {
            // Adjacent lines join too, so a diff shows their old lines, then
            // their new ones.
            Some(block) if first <= block.last + 1 => {
                block.last = block.last.max(last);
                block.edits.push(edit.clone());
            }
            _ => blocks.push(Block {
                first,
                last,
                edits: vec![edit.clone()],
            }),
        }
    }
    blocks
}

/// A unified diff of one file's edits, with root-relative `a/` and `b/`
/// paths so that `git apply` accepts it.
pub fn diff(path: &str, content: &str, edits: &[Edit]) -> String {
    if edits.is_empty() {
        return String::new();
    }
    let starts = line_starts(content);
    let line_count = if content.is_empty() { 0 } else { starts.len() };
    let line = |i: usize| {
        let (from, to) = lines_range(content, &starts, i, i);
        &content[from..to]
    };
    let mut out = format!("--- a/{path}\n+++ b/{path}\n");
    let blocks = blocks(&starts, edits);
    // Hunks: blocks whose context lines touch or overlap.
    let mut hunks: Vec<Vec<&Block>> = Vec::new();
    for block in &blocks {
        match hunks.last_mut() {
            Some(hunk) if block.first <= hunk[hunk.len() - 1].last + 2 * CONTEXT + 1 => {
                hunk.push(block);
            }
            _ => hunks.push(vec![block]),
        }
    }
    let mut delta: isize = 0;
    for hunk in hunks {
        let first = hunk[0].first.saturating_sub(CONTEXT);
        let last = (hunk[hunk.len() - 1].last + CONTEXT).min(line_count.saturating_sub(1));
        let mut body = String::new();
        let (mut old_count, mut new_count) = (0usize, 0usize);
        let mut at = first;
        for block in &hunk {
            for i in at..block.first {
                push_line(&mut body, ' ', line(i));
                old_count += 1;
                new_count += 1;
            }
            let (from, to) = lines_range(content, &starts, block.first, block.last);
            for i in block.first..=block.last {
                push_line(&mut body, '-', line(i));
                old_count += 1;
            }
            let shifted: Vec<Edit> = block
                .edits
                .iter()
                .map(|e| Edit {
                    start: e.start - from,
                    end: e.end - from,
                    replacement: with_ending(&e.replacement, ending_at(content, &starts, e.start)),
                })
                .collect();
            let new_text = apply(&content[from..to], &shifted);
            for new_line in new_text.split_inclusive('\n') {
                push_line(&mut body, '+', new_line);
                new_count += 1;
            }
            at = block.last + 1;
        }
        for i in at..=last {
            push_line(&mut body, ' ', line(i));
            old_count += 1;
            new_count += 1;
        }
        let old_start = first + 1;
        let new_start = old_start.saturating_add_signed(delta);
        let start = |start: usize, count: usize| if count == 0 { start - 1 } else { start };
        // Writing to a String cannot fail.
        let _ = writeln!(
            out,
            "@@ -{},{old_count} +{},{new_count} @@",
            start(old_start, old_count),
            start(new_start, new_count)
        );
        out.push_str(&body);
        delta += isize::try_from(new_count).unwrap_or(isize::MAX)
            - isize::try_from(old_count).unwrap_or(isize::MAX);
    }
    out
}

fn push_line(out: &mut String, prefix: char, line: &str) {
    out.push(prefix);
    out.push_str(line);
    if !line.ends_with('\n') {
        out.push_str("\n\\ No newline at end of file\n");
    }
}

/// A new preview ID: 8 hex digits, from the time, the process and a counter.
/// The store checks it is unused.
pub fn preview_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let mut hasher = blake3::Hasher::new();
    hasher.update(&nanos.to_le_bytes());
    hasher.update(&std::process::id().to_le_bytes());
    hasher.update(&COUNTER.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    hasher.finalize().to_hex()[..8].to_owned()
}

/// An override used by a plan, with the sites it allowed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct OverrideUse {
    /// The flag, such as `--allow-shadowing`.
    pub flag: String,
    /// The condition it allowed.
    pub condition: String,
    /// Where.
    pub sites: Vec<Location>,
}

/// Why a site is not edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Exclusion {
    /// A reference codix cannot bind, left unchanged under
    /// `--exclude-unresolved`.
    Unresolved,
    /// The name in a comment, a string or a macro body.
    Mention,
    /// The name in another file: in a comment or a string of code, or
    /// anywhere in a text file.
    MentionElsewhere,
}

/// A site the plan leaves unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ExcludedSite {
    /// Where.
    pub location: Location,
    /// Why.
    pub reason: Exclusion,
    /// The line, trimmed.
    pub text: String,
}

/// What every edit command returns: a preview to apply with `apply <id>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PreviewResponse {
    /// The preview ID, for `apply` and `discard`.
    pub id: String,
    /// What the plan does, such as `rename method Invoice.total to sum`.
    pub description: String,
    /// Files edited.
    pub files: usize,
    /// Sites edited.
    pub sites: usize,
    /// Overrides used, with the sites each allowed.
    pub overrides: Vec<OverrideUse>,
    /// Sites left unchanged, with the reason.
    pub excluded: Vec<ExcludedSite>,
    /// Edited sites whose meaning codix could not settle.
    pub ambiguous: Vec<Location>,
    /// Unified diff of every edited file.
    pub diff: String,
}

impl crate::op::Outcome for PreviewResponse {
    fn has_results(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(start: usize, end: usize, replacement: &str) -> Edit {
        Edit {
            start,
            end,
            replacement: replacement.to_owned(),
        }
    }

    /// The edits replacing each `old` in `content` by `new`.
    fn renaming(content: &str, old: &str, new: &str) -> Vec<Edit> {
        content
            .match_indices(old)
            .map(|(i, _)| edit(i, i + old.len(), new))
            .collect()
    }

    #[test]
    fn overlapping_edits_are_refused_and_duplicates_merged() {
        let set = EditSet::new([
            ("b".to_owned(), edit(0, 1, "x")),
            ("a".to_owned(), edit(4, 5, "y")),
            ("a".to_owned(), edit(0, 2, "z")),
            ("a".to_owned(), edit(0, 2, "z")),
        ])
        .unwrap();
        assert_eq!(
            set.files
                .iter()
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(set.files[0].edits, [edit(0, 2, "z"), edit(4, 5, "y")]);
        assert_eq!(set.sites(), 3);
        assert!(
            EditSet::new([
                ("a".to_owned(), edit(0, 3, "")),
                ("a".to_owned(), edit(2, 4, ""))
            ])
            .is_err()
        );
    }

    #[test]
    fn a_region_is_the_whole_lines_an_edit_touches() {
        let content = "one\ntwo three\nfour\n";
        let regions = regions(
            content,
            &[
                edit(4, 7, "2"),
                edit(8, 19, "3\n4\n"),
                edit(19, 19, "five\n"),
            ],
        );
        assert_eq!(
            regions,
            [
                Region {
                    first_line: 2,
                    last_line: 2,
                    text: "two three\n".to_owned()
                },
                Region {
                    first_line: 2,
                    last_line: 3,
                    text: "two three\nfour\n".to_owned()
                },
                Region {
                    first_line: 3,
                    last_line: 3,
                    text: "four\n".to_owned()
                },
            ]
        );
        let planned = plan_file("a.txt", content, &[edit(0, 3, "1")]);
        assert_eq!(planned.hash, hash(content.as_bytes()));
        assert_eq!(planned.regions[0].text, "one\n");
    }

    #[test]
    fn replacements_take_the_line_ending_of_their_line() {
        let crlf = "a\r\nb\r\n";
        assert_eq!(apply(crlf, &[edit(0, 1, "x\ny")]), "x\r\ny\r\nb\r\n");
        let mixed = "a\nb\r\nc";
        assert_eq!(
            apply(
                mixed,
                &[edit(0, 1, "m\nn"), edit(2, 3, "x\ny"), edit(5, 6, "p\r\nq")]
            ),
            "m\nn\nx\r\ny\r\np\r\nq"
        );
        assert_eq!(apply("", &[edit(0, 0, "x\ny\n")]), "x\ny\n");
    }

    #[test]
    fn columns_and_bytes_differ_for_multibyte_text() {
        let content = "é total = 1;\n";
        let edits = renaming(content, "total", "sum");
        assert_eq!(edits[0].start, 3);
        assert_eq!(apply(content, &edits), "é sum = 1;\n");
    }

    #[test]
    fn hunks_merge_when_their_context_overlaps() {
        let content = (1..=20).fold(String::new(), |mut text, i| {
            let _ = writeln!(text, "line {i}");
            text
        });
        let edits: Vec<Edit> = ["line 5\n", "line 9\n", "line 18\n"]
            .iter()
            .map(|l| {
                let at = content.find(l).unwrap();
                edit(at + 5, at + l.len() - 1, "X")
            })
            .collect();
        assert_text(
            &diff("src/a.txt", &content, &edits),
            "--- a/src/a.txt
+++ b/src/a.txt
@@ -2,11 +2,11 @@
 line 2
 line 3
 line 4
-line 5
+line X
 line 6
 line 7
 line 8
-line 9
+line X
 line 10
 line 11
 line 12
@@ -15,6 +15,6 @@
 line 15
 line 16
 line 17
-line 18
+line X
 line 19
 line 20
",
        );
    }

    #[test]
    fn a_last_line_without_newline_is_marked() {
        let content = "a\nlast";
        assert_text(
            &diff("f", content, &renaming(content, "last", "end")),
            "--- a/f
+++ b/f
@@ -1,2 +1,2 @@
 a
-last
\\ No newline at end of file
+end
\\ No newline at end of file
",
        );
    }

    #[test]
    fn diff_line_counts_follow_lines_added_and_removed() {
        let content = "a\nb\nc\n";
        assert_text(
            &diff("f", content, &[edit(2, 4, "")]),
            "--- a/f
+++ b/f
@@ -1,3 +1,2 @@
 a
-b
 c
",
        );
        assert_text(
            &diff("f", content, &[edit(2, 2, "x\ny\n")]),
            "--- a/f
+++ b/f
@@ -1,3 +1,5 @@
 a
-b
+x
+y
+b
 c
",
        );
    }

    #[test]
    fn preview_ids_are_eight_hex_digits_and_differ() {
        let (a, b) = (preview_id(), preview_id());
        assert_eq!(a.len(), 8);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    /// Compares texts line by line, for readable failures.
    fn assert_text(actual: &str, expected: &str) {
        assert_eq!(
            actual.lines().collect::<Vec<_>>(),
            expected.lines().collect::<Vec<_>>()
        );
        assert_eq!(actual, expected);
    }
}
