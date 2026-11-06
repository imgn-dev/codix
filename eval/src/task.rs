//! Benchmark tasks: `eval/tasks/<id>.toml`, each a question about the pinned
//! spring-framework checkout and the strings a right answer contains, or a
//! rename to make and the edits that make it.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// What a task exercises.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Category {
    /// Find where something is defined.
    Locate,
    /// Follow a definition to a fact in its source.
    Trace,
    /// Read the structure of a file or type.
    Outline,
    /// Rename something, writing the change.
    Rename,
}

/// One task.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Task {
    /// The file name without `.toml`.
    #[serde(skip)]
    pub(crate) id: String,
    /// What it exercises.
    pub(crate) category: Category,
    /// The question, as the model receives it.
    pub(crate) prompt: String,
    /// Strings the answer must all contain.
    #[serde(default)]
    pub(crate) expect: Vec<String>,
    /// For a rename task, the renames the files must show afterwards; none
    /// when the right outcome is to change nothing.
    #[serde(default)]
    pub(crate) renames: Vec<Rename>,
}

/// One rename a task expects in the checkout.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Rename {
    /// Root-relative path of the file.
    pub(crate) file: String,
    /// The name before.
    pub(crate) old: String,
    /// The name after, absent from the file before.
    pub(crate) new: String,
    /// How many times the name must change.
    pub(crate) sites: usize,
}

/// Byte offsets of `name` in `text` as a whole word.
fn words(text: &str, name: &str) -> Vec<usize> {
    let part = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    text.match_indices(name)
        .filter(|(i, _)| {
            !text[..*i].chars().next_back().is_some_and(part)
                && !text[i + name.len()..].chars().next().is_some_and(part)
        })
        .map(|(i, _)| i)
        .collect()
}

/// Whether `after` is `before` with exactly the expected renames: every
/// new name, put back to the old one, gives the file as it was, and each
/// changed as many times as expected.
pub(crate) fn renamed_as_expected(before: &str, after: &str, renames: &[&Rename]) -> bool {
    let mut restored = after.to_owned();
    for rename in renames {
        if words(after, &rename.new).len() != rename.sites || !words(before, &rename.new).is_empty()
        {
            return false;
        }
        let mut text = String::with_capacity(restored.len());
        let mut at = 0;
        for start in words(&restored, &rename.new) {
            text.push_str(&restored[at..start]);
            text.push_str(&rename.old);
            at = start + rename.new.len();
        }
        text.push_str(&restored[at..]);
        restored = text;
    }
    restored == before
}

impl Task {
    /// Whether `answer` contains every expected string.
    pub(crate) fn answered_by(&self, answer: &str) -> bool {
        self.expect.iter().all(|e| answer.contains(e.as_str()))
    }
}

/// Every task in `dir`, sorted by ID.
///
/// # Panics
///
/// When a task file cannot be read or parsed.
pub(crate) fn load(dir: &Path) -> Vec<Task> {
    let mut tasks: Vec<Task> = fs::read_dir(dir)
        .expect("read the task directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "toml"))
        .map(|path| {
            let text = fs::read_to_string(&path).expect("read a task");
            let mut task: Task =
                toml::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            task.id = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            task
        })
        .collect();
    tasks.sort_by(|a, b| a.id.cmp(&b.id));
    tasks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_task_parses_and_expects_something() {
        let tasks = load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tasks"));
        assert_eq!(tasks.len(), 27);
        for task in &tasks {
            assert!(
                !task.expect.is_empty() || !task.renames.is_empty(),
                "{}",
                task.id
            );
            assert!(!task.prompt.trim().is_empty(), "{}", task.id);
            let prefix = match task.category {
                Category::Locate => "locate-",
                Category::Trace => "trace-",
                Category::Outline => "outline-",
                Category::Rename => "rename-",
            };
            assert!(task.id.starts_with(prefix), "{}", task.id);
        }
    }

    #[test]
    fn an_answer_must_contain_every_expected_string() {
        let task = Task {
            id: "t".to_owned(),
            category: Category::Trace,
            prompt: "?".to_owned(),
            expect: vec!["preHandle".to_owned(), "postHandle".to_owned()],
            renames: Vec::new(),
        };
        assert!(task.answered_by("It declares preHandle and postHandle."));
        assert!(!task.answered_by("It declares preHandle."));
    }

    #[test]
    fn a_rename_must_change_exactly_the_expected_sites() {
        let rename = Rename {
            file: "A.java".to_owned(),
            old: "width".to_owned(),
            new: "columns".to_owned(),
            sites: 2,
        };
        let before = "int width = 1; // width\nreturn width + widths;\n";
        let right = "int columns = 1; // width\nreturn columns + widths;\n";
        assert!(renamed_as_expected(before, right, &[&rename]));
        let comment_too = "int columns = 1; // columns\nreturn columns + widths;\n";
        assert!(!renamed_as_expected(before, comment_too, &[&rename]));
        let other_change = "int columns = 2; // width\nreturn columns + widths;\n";
        assert!(!renamed_as_expected(before, other_change, &[&rename]));
    }
}
