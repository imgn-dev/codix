//! Benchmark tasks: `eval/tasks/<id>.toml`, each a question about the pinned
//! spring-framework checkout and the strings a right answer contains.

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
    pub(crate) expect: Vec<String>,
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
        assert_eq!(tasks.len(), 20);
        for task in &tasks {
            assert!(!task.expect.is_empty(), "{}", task.id);
            assert!(!task.prompt.trim().is_empty(), "{}", task.id);
            let prefix = match task.category {
                Category::Locate => "locate-",
                Category::Trace => "trace-",
                Category::Outline => "outline-",
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
        };
        assert!(task.answered_by("It declares preHandle and postHandle."));
        assert!(!task.answered_by("It declares preHandle."));
    }
}
