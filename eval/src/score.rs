//! Scoring a run: whether it answered, and the defects in how it used codix,
//! each a sign of a name or description to improve.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::task::{Category, Task};
use crate::transcript::Transcript;

/// The tools that look up symbols, which resolve an ambiguous name.
const LOOKUPS: &[&str] = &["find", "def", "show", "outline"];

/// A kind of defect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DefectKind {
    /// A `usage` error, or an `ambiguous_symbol` never resolved.
    MisusedCommand,
    /// An argument the tool does not take.
    GuessedFlag,
    /// `grep` called for a task that `find` or `def` answers.
    GrepFallback,
}

/// One defect, with the call it comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Defect {
    /// Its kind.
    pub(crate) kind: DefectKind,
    /// The tool called.
    pub(crate) tool: String,
    /// What was wrong, with an excerpt of the call or result.
    pub(crate) detail: String,
}

/// The arguments each tool takes, from the request schema snapshots.
pub(crate) type Arguments = BTreeMap<String, BTreeSet<String>>;

/// The defects of a run of `task`, given each tool's `arguments`.
pub(crate) fn defects(task: &Task, transcript: &Transcript, arguments: &Arguments) -> Vec<Defect> {
    let mut found = Vec::new();
    for (i, call) in transcript.calls.iter().enumerate() {
        let excerpt: String = call
            .result
            .lines()
            .next()
            .unwrap_or_default()
            .chars()
            .take(160)
            .collect();
        if call.is_error && call.result.starts_with("error[usage]") {
            found.push(Defect {
                kind: DefectKind::MisusedCommand,
                tool: call.tool.clone(),
                detail: excerpt.clone(),
            });
        }
        let resolved_later = transcript.calls[i + 1..]
            .iter()
            .any(|later| !later.is_error && LOOKUPS.contains(&later.tool.as_str()));
        if call.result.starts_with("error[ambiguous_symbol]") && !resolved_later {
            found.push(Defect {
                kind: DefectKind::MisusedCommand,
                tool: call.tool.clone(),
                detail: format!("left unresolved: {excerpt}"),
            });
        }
        if let (Some(known), Some(given)) = (arguments.get(&call.tool), call.input.as_object()) {
            for name in given.keys().filter(|k| !known.contains(k.as_str())) {
                found.push(Defect {
                    kind: DefectKind::GuessedFlag,
                    tool: call.tool.clone(),
                    detail: format!("`{name}` is not an argument of {}", call.tool),
                });
            }
        }
        if call.tool == "grep" && task.category == Category::Locate {
            found.push(Defect {
                kind: DefectKind::GrepFallback,
                tool: call.tool.clone(),
                detail: format!("grep {}", call.input),
            });
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::transcript::ToolCall;

    fn call(tool: &str, input: serde_json::Value, result: &str, is_error: bool) -> ToolCall {
        ToolCall {
            tool: tool.to_owned(),
            input,
            result: result.to_owned(),
            is_error,
        }
    }

    fn task(category: Category) -> Task {
        Task {
            id: "t".to_owned(),
            category,
            prompt: "?".to_owned(),
            expect: vec!["x".to_owned()],
        }
    }

    fn arguments() -> Arguments {
        let mut arguments = Arguments::new();
        arguments.insert(
            "def".to_owned(),
            ["symbol", "kind", "here", "path", "cwd"]
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
        );
        arguments.insert(
            "grep".to_owned(),
            ["pattern", "cwd"].iter().map(|s| (*s).to_owned()).collect(),
        );
        arguments
    }

    fn kinds(found: &[Defect]) -> Vec<DefectKind> {
        found.iter().map(|d| d.kind).collect()
    }

    #[test]
    fn a_clean_run_has_no_defect() {
        let t = Transcript {
            calls: vec![call(
                "def",
                json!({ "symbol": "A" }),
                "A.java:1:1  class  A",
                false,
            )],
            ..Transcript::default()
        };
        assert_eq!(defects(&task(Category::Locate), &t, &arguments()), vec![]);
    }

    #[test]
    fn usage_errors_and_unresolved_ambiguity_are_misused_commands() {
        let t = Transcript {
            calls: vec![
                call("def", json!({ "symbol": "A" }), "error[usage]: bad", true),
                call(
                    "def",
                    json!({ "symbol": "B" }),
                    "error[ambiguous_symbol]: \"B\" matches 2",
                    true,
                ),
            ],
            ..Transcript::default()
        };
        assert_eq!(
            kinds(&defects(&task(Category::Trace), &t, &arguments())),
            [DefectKind::MisusedCommand, DefectKind::MisusedCommand]
        );
    }

    #[test]
    fn ambiguity_resolved_by_a_later_lookup_is_fine() {
        let t = Transcript {
            calls: vec![
                call(
                    "def",
                    json!({ "symbol": "B" }),
                    "error[ambiguous_symbol]: \"B\" matches 2",
                    true,
                ),
                call(
                    "def",
                    json!({ "symbol": "java:B.java::B#1234" }),
                    "B.java:1:1  class  B",
                    false,
                ),
            ],
            ..Transcript::default()
        };
        assert_eq!(defects(&task(Category::Trace), &t, &arguments()), vec![]);
    }

    #[test]
    fn unknown_arguments_are_guessed_flags() {
        let t = Transcript {
            calls: vec![call(
                "def",
                json!({ "symbol": "A", "file": "A.java" }),
                "ok",
                false,
            )],
            ..Transcript::default()
        };
        let found = defects(&task(Category::Trace), &t, &arguments());
        assert_eq!(kinds(&found), [DefectKind::GuessedFlag]);
        assert_eq!(found[0].detail, "`file` is not an argument of def");
    }

    #[test]
    fn grep_counts_as_a_fallback_for_locate_tasks_only() {
        let t = Transcript {
            calls: vec![call(
                "grep",
                json!({ "pattern": "class A" }),
                "A.java:1:1",
                false,
            )],
            ..Transcript::default()
        };
        assert_eq!(
            kinds(&defects(&task(Category::Locate), &t, &arguments())),
            [DefectKind::GrepFallback]
        );
        assert_eq!(defects(&task(Category::Trace), &t, &arguments()), vec![]);
    }
}
