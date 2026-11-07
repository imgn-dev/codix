//! The results of a benchmark run: per run, and summed per model.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use serde::Serialize;

use crate::score::{Defect, DefectKind};
use crate::task::Category;

/// One task run by one model.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Run {
    /// The model.
    pub(crate) model: String,
    /// The task.
    pub(crate) task: String,
    /// Its category.
    pub(crate) category: Category,
    /// Whether the answer contained every expected string.
    pub(crate) success: bool,
    /// Turns.
    pub(crate) turns: u64,
    /// Tool calls.
    pub(crate) tool_calls: usize,
    /// Input tokens, cache included.
    pub(crate) input_tokens: u64,
    /// Output tokens.
    pub(crate) output_tokens: u64,
    /// Cost in US dollars.
    pub(crate) cost_usd: f64,
    /// Defects found.
    pub(crate) defects: Vec<Defect>,
    /// The final answer.
    pub(crate) answer: String,
    /// Why the run failed, if Claude Code says it did.
    pub(crate) error: Option<String>,
}

impl Run {
    /// Replaces this machine's paths in what the model wrote.
    pub(crate) fn redact(&mut self, redactions: &Redactions) {
        self.answer = redactions.apply(&self.answer);
        self.error = self.error.as_deref().map(|e| redactions.apply(e));
        for defect in &mut self.defects {
            defect.detail = redactions.apply(&defect.detail);
        }
    }
}

/// Paths of this machine that a model may quote, which name its user, each
/// with the placeholder the published results show instead.
#[derive(Debug)]
pub(crate) struct Redactions(Vec<(String, &'static str)>);

impl Redactions {
    /// Redacts the benchmark checkouts' directory, then the home directory.
    /// A home directory of `/` is left alone: it would replace every slash.
    pub(crate) fn new(bench_repos: &Path, home: Option<&Path>) -> Self {
        let mut paths = vec![(bench_repos.display().to_string(), "<bench-repos>")];
        if let Some(home) = home.filter(|h| h.parent().is_some()) {
            paths.push((home.display().to_string(), "~"));
        }
        Self(paths)
    }

    /// `text` with each path replaced, most specific first.
    pub(crate) fn apply(&self, text: &str) -> String {
        self.0
            .iter()
            .fold(text.to_owned(), |text, (path, placeholder)| {
                text.replace(path.as_str(), placeholder)
            })
    }
}

/// One model's totals.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub(crate) struct Summary {
    /// Tasks run.
    pub(crate) tasks: usize,
    /// Tasks answered.
    pub(crate) succeeded: usize,
    /// Tool calls, all tasks.
    pub(crate) tool_calls: usize,
    /// Defects by kind.
    pub(crate) defects: BTreeMap<DefectKind, usize>,
    /// Cost in US dollars, all tasks.
    pub(crate) cost_usd: f64,
}

/// Totals per model.
pub(crate) fn summarize(runs: &[Run]) -> BTreeMap<String, Summary> {
    let mut summaries: BTreeMap<String, Summary> = BTreeMap::new();
    for run in runs {
        let s = summaries.entry(run.model.clone()).or_default();
        s.tasks += 1;
        s.succeeded += usize::from(run.success);
        s.tool_calls += run.tool_calls;
        s.cost_usd += run.cost_usd;
        for defect in &run.defects {
            *s.defects.entry(defect.kind).or_default() += 1;
        }
    }
    summaries
}

/// The summary as a Markdown table.
pub(crate) fn table(summaries: &BTreeMap<String, Summary>) -> String {
    let mut out = String::from(
        "| Model | Answered | Tool calls | Misused commands | Guessed flags | Grep fallbacks | Malformed calls | Cost |\n\
         | --- | --- | --- | --- | --- | --- | --- | --- |\n",
    );
    for (model, s) in summaries {
        let count = |kind| s.defects.get(&kind).copied().unwrap_or(0);
        // Writing to a String cannot fail.
        let _ = writeln!(
            out,
            "| {model} | {}/{} | {} | {} | {} | {} | {} | ${:.2} |",
            s.succeeded,
            s.tasks,
            s.tool_calls,
            count(DefectKind::MisusedCommand),
            count(DefectKind::GuessedFlag),
            count(DefectKind::GrepFallback),
            count(DefectKind::MalformedCall),
            s.cost_usd
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(model: &str, success: bool, defects: Vec<Defect>) -> Run {
        Run {
            model: model.to_owned(),
            task: "t".to_owned(),
            category: Category::Locate,
            success,
            turns: 2,
            tool_calls: 3,
            input_tokens: 100,
            output_tokens: 10,
            cost_usd: 0.5,
            defects,
            answer: String::new(),
            error: None,
        }
    }

    #[test]
    fn runs_are_summed_per_model_into_a_table() {
        let grep = Defect {
            kind: DefectKind::GrepFallback,
            tool: "grep".to_owned(),
            detail: String::new(),
        };
        let runs = [
            run("haiku", true, vec![grep.clone()]),
            run("haiku", false, vec![grep]),
            run("opus", true, vec![]),
        ];
        let summaries = summarize(&runs);
        assert_eq!(summaries["haiku"].succeeded, 1);
        assert_eq!(summaries["haiku"].defects[&DefectKind::GrepFallback], 2);
        assert_eq!(
            table(&summaries).lines().nth(2),
            Some("| haiku | 1/2 | 6 | 0 | 0 | 2 | 0 | $1.00 |")
        );
    }

    #[test]
    fn paths_of_this_machine_are_redacted() {
        // Literal paths: `join` would add a backslash on Windows.
        let redactions = Redactions::new(
            Path::new("/home/someone/codix/target/bench-repos"),
            Some(Path::new("/home/someone")),
        );
        let mut run = run(
            "haiku",
            true,
            vec![Defect {
                kind: DefectKind::GrepFallback,
                tool: "grep".to_owned(),
                detail: "in /home/someone/codix/target/bench-repos/spring-framework".to_owned(),
            }],
        );
        run.answer =
            "See `/home/someone/codix/target/bench-repos/spring-framework/A.java`.".to_owned();
        run.error = Some("cannot read /home/someone/.config".to_owned());
        run.redact(&redactions);
        assert_eq!(run.answer, "See `<bench-repos>/spring-framework/A.java`.");
        assert_eq!(run.error.as_deref(), Some("cannot read ~/.config"));
        assert_eq!(run.defects[0].detail, "in <bench-repos>/spring-framework");
    }

    #[test]
    fn a_root_home_directory_is_not_redacted() {
        let redactions = Redactions::new(Path::new("/bench-repos"), Some(Path::new("/")));
        assert_eq!(redactions.apply("/a/b"), "/a/b");
    }
}
