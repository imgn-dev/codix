//! Text and JSON rendering of responses and errors.

use std::io::{self, Write};

use codix_provider::ProviderKind;
use serde::Serialize;

use crate::Error;
use crate::capabilities::{Capabilities, LanguagesResponse, Tier};
use crate::config::{ConfigResponse, Setting};
use crate::edit::{Exclusion, PreviewResponse};
use crate::indexing::{InitResponse, StatusResponse};
use crate::op::Warning;
use crate::page::{Location, Paged};
use crate::query::{
    DefResponse, FindResponse, GrepMatch, GrepResponse, OutlineNode, OutlineResponse, ShowResponse,
    SymbolResult,
};
use crate::refs::{RefResult, RefsResponse};
use crate::scan::ScanReport;
use crate::store::MatchMode;

/// Version of the JSON output format; incremented on breaking changes only.
pub const SCHEMA_VERSION: u32 = 1;

const COLUMN_GAP: &str = "  ";

/// How to render to a stream.
#[derive(Debug, Clone, Copy)]
pub struct Style {
    /// Whether to paint with ANSI colors, on a terminal only.
    pub color: bool,
    /// `config --list --show-origin`.
    pub show_origin: bool,
    /// Whether next steps name MCP tools rather than CLI commands.
    pub tools: bool,
}

impl Style {
    fn paint(self, codes: &str, text: &str) -> String {
        if self.color {
            format!("\x1b[{codes}m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    }
}

/// Seconds since the Unix epoch.
pub fn since_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn directories(count: usize) -> String {
    if count == 1 {
        "1 directory".to_owned()
    } else {
        format!("{count} directories")
    }
}

/// A duration in its two largest units, such as `2h 5m`.
pub fn elapsed(seconds: u64) -> String {
    let (d, h, m, s) = (
        seconds / 86_400,
        seconds / 3600 % 24,
        seconds / 60 % 60,
        seconds % 60,
    );
    match (d, h, m) {
        (0, 0, 0) => format!("{s}s"),
        (0, 0, _) => format!("{m}m {s}s"),
        (0, _, _) => format!("{h}h {m}m"),
        _ => format!("{d}d {h}h"),
    }
}

/// A response that has a text rendering.
pub trait TextRender {
    /// Writes the response as text.
    fn render_text(&self, w: &mut dyn Write, style: Style) -> io::Result<()>;
}

/// A result shown as one line: its location, then its other columns.
pub trait Row {
    /// The location shown first.
    fn location(&self) -> &Location;
    /// The other columns.
    fn columns(&self) -> Vec<String>;
}

impl Row for SymbolResult {
    fn location(&self) -> &Location {
        &self.location
    }

    fn columns(&self) -> Vec<String> {
        let mut columns = vec![self.kind.clone(), self.qualified_name.clone()];
        columns.extend(self.signature.clone());
        columns
    }
}

impl Row for RefResult {
    fn location(&self) -> &Location {
        &self.location
    }

    fn columns(&self) -> Vec<String> {
        vec![
            self.context.clone(),
            self.enclosing.clone().unwrap_or_else(|| "-".to_owned()),
            self.confidence.clone(),
        ]
    }
}

impl TextRender for RefsResponse {
    fn render_text(&self, w: &mut dyn Write, style: Style) -> io::Result<()> {
        self.references.render_text(w, style)?;
        if self.references.total > 0 {
            writeln!(w, "{} scoped, {} textual", self.scoped, self.textual)?;
        }
        stale(w, self.possibly_stale)
    }
}

impl Row for GrepMatch {
    fn location(&self) -> &Location {
        &self.location
    }

    fn columns(&self) -> Vec<String> {
        vec![
            self.enclosing.clone().unwrap_or_else(|| "-".to_owned()),
            self.text.trim().to_owned(),
        ]
    }
}

const STALE: &str = "note: another codix process holds the write lock; results may be stale";

impl TextRender for FindResponse {
    fn render_text(&self, w: &mut dyn Write, style: Style) -> io::Result<()> {
        self.results.render_text(w, style)?;
        if self.results.total > 0 && self.mode != MatchMode::Exact {
            let mode = match self.mode {
                MatchMode::Exact => "exact",
                MatchMode::Prefix => "prefix",
                MatchMode::Fuzzy => "fuzzy",
                MatchMode::Glob => "glob",
            };
            writeln!(w, "matched by {mode}")?;
        }
        stale(w, self.possibly_stale)
    }
}

fn stale(w: &mut dyn Write, possibly_stale: bool) -> io::Result<()> {
    if possibly_stale {
        writeln!(w, "{STALE}")?;
    }
    Ok(())
}

impl TextRender for DefResponse {
    fn render_text(&self, w: &mut dyn Write, style: Style) -> io::Result<()> {
        for d in std::iter::once(&self.definition).chain(&self.overloads) {
            let mut line = format!(
                "{}  {}  {}",
                style.paint("36", &location(&d.location)),
                d.kind,
                d.qualified_name
            );
            if let Some(signature) = &d.signature {
                line.push_str(COLUMN_GAP);
                line.push_str(signature);
            }
            writeln!(w, "{line}")?;
            writeln!(w, "{}", d.symbol_id)?;
        }
        if !self.overloads.is_empty() {
            writeln!(w, "{} overloads", self.overloads.len() + 1)?;
        }
        stale(w, self.possibly_stale)
    }
}

fn location(l: &Location) -> String {
    format!("{}:{}:{}", l.path, l.line, l.column)
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

impl TextRender for PreviewResponse {
    fn render_text(&self, w: &mut dyn Write, style: Style) -> io::Result<()> {
        writeln!(
            w,
            "preview {}: {}",
            style.paint("1", &self.id),
            self.description
        )?;
        writeln!(
            w,
            "  {}, {}",
            plural(self.files, "file", "files"),
            plural(self.sites, "site", "sites")
        )?;
        for o in &self.overrides {
            writeln!(w, "  override {}: {}", o.flag, o.condition)?;
            for site in &o.sites {
                writeln!(w, "    {}", style.paint("36", &location(site)))?;
            }
        }
        for (reason, one, many) in [
            (Exclusion::Unresolved, "unresolved site", "unresolved sites"),
            (Exclusion::Mention, "mention", "mentions"),
        ] {
            let sites: Vec<_> = self
                .excluded
                .iter()
                .filter(|e| e.reason == reason)
                .collect();
            if sites.is_empty() {
                continue;
            }
            writeln!(w, "  not changed, {}:", plural(sites.len(), one, many))?;
            let width = sites
                .iter()
                .map(|e| location(&e.location).len())
                .max()
                .unwrap_or(0);
            for e in sites {
                let at = location(&e.location);
                writeln!(
                    w,
                    "    {}{}{COLUMN_GAP}{}",
                    style.paint("36", &at),
                    " ".repeat(width - at.len()),
                    e.text
                )?;
            }
        }
        if !self.ambiguous.is_empty() {
            writeln!(
                w,
                "  ambiguous, {}:",
                plural(self.ambiguous.len(), "site", "sites")
            )?;
            for site in &self.ambiguous {
                writeln!(w, "    {}", style.paint("36", &location(site)))?;
            }
        }
        w.write_all(self.diff.as_bytes())?;
        if style.tools {
            writeln!(w, "apply with: the apply tool, id \"{}\"", self.id)
        } else {
            writeln!(w, "apply with: codix apply {}", self.id)
        }
    }
}

impl TextRender for OutlineResponse {
    fn render_text(&self, w: &mut dyn Write, style: Style) -> io::Result<()> {
        fn node(w: &mut dyn Write, n: &OutlineNode, depth: usize) -> io::Result<()> {
            let s = &n.symbol;
            let signature = s.signature.as_deref().unwrap_or(&s.name);
            writeln!(
                w,
                "{}{}:{}  {}  {signature}",
                "  ".repeat(depth + 1),
                s.location.line,
                s.location.column,
                s.kind
            )?;
            n.children
                .iter()
                .try_for_each(|child| node(w, child, depth + 1))
        }
        for file in &self.files {
            writeln!(w, "{}", style.paint("36", &file.path))?;
            for n in &file.symbols {
                node(w, n, 0)?;
            }
        }
        if self.files.is_empty() {
            writeln!(w, "no symbols")?;
        }
        stale(w, self.possibly_stale)
    }
}

impl TextRender for ShowResponse {
    fn render_text(&self, w: &mut dyn Write, style: Style) -> io::Result<()> {
        let s = &self.symbol;
        writeln!(
            w,
            "{}  {}  {}",
            style.paint("36", &location(&s.location)),
            s.kind,
            s.qualified_name
        )?;
        let last = self.first_line as usize + self.lines.len();
        let width = last.saturating_sub(1).to_string().len();
        for (i, line) in self.lines.iter().enumerate() {
            writeln!(w, "{:>width$} | {line}", self.first_line as usize + i)?;
        }
        stale(w, self.possibly_stale)
    }
}

impl TextRender for GrepResponse {
    fn render_text(&self, w: &mut dyn Write, style: Style) -> io::Result<()> {
        self.matches.render_text(w, style)?;
        stale(w, self.possibly_stale)
    }
}

impl<T: Row> TextRender for Paged<T> {
    fn render_text(&self, w: &mut dyn Write, style: Style) -> io::Result<()> {
        let rows: Vec<Vec<String>> = self
            .items
            .iter()
            .map(|item| {
                let l = item.location();
                let mut row = vec![format!("{}:{}:{}", l.path, l.line, l.column)];
                row.extend(item.columns());
                row
            })
            .collect();
        let widths = column_widths(&rows);
        for row in &rows {
            let mut line = String::new();
            for (i, cell) in row.iter().enumerate() {
                let last = i + 1 == row.len();
                let painted = if i == 0 {
                    style.paint("36", cell)
                } else {
                    cell.clone()
                };
                line.push_str(&painted);
                if !last {
                    let pad = widths[i] - cell.chars().count();
                    line.push_str(&" ".repeat(pad));
                    line.push_str(COLUMN_GAP);
                }
            }
            writeln!(w, "{line}")?;
        }
        writeln!(w, "{}", footer(self))
    }
}

fn column_widths(rows: &[Vec<String>]) -> Vec<usize> {
    let mut widths = Vec::new();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            let width = cell.chars().count();
            if i == widths.len() {
                widths.push(width);
            } else {
                widths[i] = widths[i].max(width);
            }
        }
    }
    widths
}

fn footer<T>(page: &Paged<T>) -> String {
    let shown = page.items.len();
    let noun = if page.total == 1 { "result" } else { "results" };
    if page.total == 0 {
        return "no results".to_owned();
    }
    if page.offset == 0 && shown == page.total {
        return format!("{} {noun}", page.total);
    }
    match page.next_offset() {
        Some(next) => format!("{shown} of {} {noun}; next: --offset {next}", page.total),
        None => format!("{shown} of {} {noun}", page.total),
    }
}

impl TextRender for ConfigResponse {
    fn render_text(&self, w: &mut dyn Write, style: Style) -> io::Result<()> {
        match self {
            Self::Get {
                map: false,
                settings,
            } => settings
                .iter()
                .try_for_each(|s| writeln!(w, "{}", s.value.display())),
            Self::Get {
                map: true,
                settings,
            } => render_settings(w, settings, false),
            Self::Set {
                key,
                value,
                file,
                effect,
            } => {
                writeln!(w, "set {key} = {} in {file}", value.display())?;
                writeln!(w, "{effect}")
            }
            Self::Unset { key, files, effect } => {
                if files.is_empty() {
                    return writeln!(w, "{key} was not set");
                }
                writeln!(w, "unset {key} in {}", files.join(", "))?;
                if let Some(effect) = effect {
                    writeln!(w, "{effect}")?;
                }
                Ok(())
            }
            Self::List { settings } => render_settings(w, settings, style.show_origin),
            Self::Describe { keys } => {
                for (i, key) in keys.iter().enumerate() {
                    if i > 0 {
                        writeln!(w)?;
                    }
                    writeln!(w, "{}", key.key)?;
                    writeln!(w, "  {}", key.description)?;
                    writeln!(w, "  type: {}", key.kind)?;
                    writeln!(w, "  default: {}", key.default.as_deref().unwrap_or("none"))?;
                    writeln!(w, "  file: {}", key.file)?;
                }
                Ok(())
            }
        }
    }
}

fn render_settings(w: &mut dyn Write, settings: &[Setting], show_origin: bool) -> io::Result<()> {
    let width = settings
        .iter()
        .map(|s| s.origin.display().len())
        .max()
        .unwrap_or(0);
    for setting in settings {
        if show_origin {
            let origin = setting.origin.display();
            write!(
                w,
                "{origin}{}{COLUMN_GAP}",
                " ".repeat(width - origin.len())
            )?;
        }
        writeln!(w, "{} = {}", setting.key, setting.value.display())?;
    }
    Ok(())
}

impl TextRender for Capabilities {
    fn render_text(&self, w: &mut dyn Write, _: Style) -> io::Result<()> {
        let width = self
            .languages
            .iter()
            .map(|l| l.name.len())
            .max()
            .unwrap_or(0);
        writeln!(w, "languages:")?;
        for language in &self.languages {
            writeln!(
                w,
                "  {:width$}{COLUMN_GAP}{}",
                language.name,
                tier_text(language.tier)
            )?;
        }
        writeln!(w, "providers:")?;
        for kind in [
            ProviderKind::Language,
            ProviderKind::Build,
            ProviderKind::Vcs,
        ] {
            let names: Vec<&str> = self
                .providers
                .iter()
                .filter(|p| p.kind == kind)
                .map(|p| p.name.as_str())
                .collect();
            if !names.is_empty() {
                writeln!(w, "  {kind:8}{COLUMN_GAP}{}", names.join(", "))?;
            }
        }
        writeln!(w, "commands:")?;
        let width = self
            .commands
            .iter()
            .map(|c| c.name.len())
            .max()
            .unwrap_or(0);
        for command in &self.commands {
            let needs = match command.tier {
                Some(Tier::Symbols) => "needs tier 1 (symbols)",
                Some(Tier::Scopes) => "needs tier 2 (scopes)",
                Some(Tier::Search | Tier::NotInBuild) => "any indexed text",
                None => "any project",
            };
            writeln!(w, "  {:width$}{COLUMN_GAP}{needs}", command.name)?;
        }
        if let Some(project) = &self.project {
            writeln!(w, "project: {}", project.root.display())?;
            writeln!(
                w,
                "  {} indexed, {} pending",
                files(project.files),
                project.pending
            )?;
            for language in &project.languages {
                writeln!(
                    w,
                    "  {}{COLUMN_GAP}{}{COLUMN_GAP}{}",
                    language.name,
                    files(language.files),
                    tier_text(language.tier)
                )?;
            }
            match &project.daemon {
                Some(d) => writeln!(w, "  daemon: pid {}", d.pid)?,
                None => writeln!(w, "  daemon: not running")?,
            }
        }
        Ok(())
    }
}

fn files(count: usize) -> String {
    if count == 1 {
        "1 file".to_owned()
    } else {
        format!("{count} files")
    }
}

fn tier_text(tier: Tier) -> &'static str {
    match tier {
        Tier::Symbols => "tier 1 (symbols)",
        Tier::Scopes => "tier 2 (scopes)",
        Tier::Search => "search only",
        Tier::NotInBuild => "not in this build",
    }
}

impl TextRender for LanguagesResponse {
    fn render_text(&self, w: &mut dyn Write, _: Style) -> io::Result<()> {
        let width = self
            .languages
            .iter()
            .map(|l| l.name.len())
            .max()
            .unwrap_or(0);
        for language in &self.languages {
            let files: Vec<String> = language
                .extensions
                .iter()
                .map(|e| format!("*.{e}"))
                .chain(language.filenames.iter().cloned())
                .collect();
            let tier = tier_text(language.tier);
            if files.is_empty() {
                writeln!(w, "{:width$}{COLUMN_GAP}{tier}", language.name)?;
            } else {
                writeln!(
                    w,
                    "{:width$}{COLUMN_GAP}{tier:17}{COLUMN_GAP}{}",
                    language.name,
                    files.join(" ")
                )?;
            }
        }
        Ok(())
    }
}

impl TextRender for ScanReport {
    fn render_text(&self, w: &mut dyn Write, _: Style) -> io::Result<()> {
        writeln!(
            w,
            "indexed: {} parsed, {} touched, {} deleted, {} unchanged",
            self.indexed, self.touched, self.deleted, self.unchanged
        )?;
        if self.pending > 0 {
            writeln!(
                w,
                "{} files kept changing and are left for the next scan",
                self.pending
            )?;
        }
        for (path, error) in &self.errors {
            writeln!(w, "error: {path}: {error}")?;
        }
        Ok(())
    }
}

impl TextRender for InitResponse {
    fn render_text(&self, w: &mut dyn Write, style: Style) -> io::Result<()> {
        let verb = if self.created {
            "initialized"
        } else {
            "reinitialized"
        };
        writeln!(w, "{verb} codix in {}", self.root.display())?;
        if !self.vcs.is_empty() {
            writeln!(w, "version control: {}", self.vcs.join(", "))?;
        }
        for hint in &self.hints {
            writeln!(w, "hint: {hint}")?;
        }
        self.scan.render_text(w, style)?;
        if let Some(path) = &self.agent_docs {
            writeln!(
                w,
                "agent guide written to {}; add it to your agent's instructions",
                path.display()
            )?;
        }
        Ok(())
    }
}

impl TextRender for StatusResponse {
    fn render_text(&self, w: &mut dyn Write, _: Style) -> io::Result<()> {
        let f = &self.files;
        writeln!(w, "project: {}", self.root.display())?;
        writeln!(
            w,
            "files: {} indexed, {} search only, {} pending, {} errors, {} skipped",
            f.indexed, f.search_only, f.pending, f.errors, f.skipped
        )?;
        match &self.daemon {
            Some(status) => {
                let d = &status.record;
                let watch = match &status.watch {
                    None => String::new(),
                    Some(watch) if watch.recursive && watch.unwatched == 0 => {
                        "; watching every directory".to_owned()
                    }
                    Some(watch) if watch.unwatched == 0 => {
                        format!("; watching {}", directories(watch.watched))
                    }
                    Some(watch) => format!(
                        "; watching {}; {} unwatched, rescanned every {}",
                        directories(watch.watched),
                        watch.unwatched,
                        elapsed(watch.rescan_seconds)
                    ),
                };
                writeln!(
                    w,
                    "daemon: pid {}, up {}, codix {}{watch}",
                    d.pid,
                    elapsed(since_epoch().saturating_sub(d.started_at)),
                    d.version
                )?;
            }
            None => writeln!(w, "daemon: not running")?,
        }
        let c = &self.changed;
        if c.new.is_empty() && c.modified.is_empty() && c.deleted.is_empty() {
            writeln!(w, "changed since last scan: none")?;
        } else {
            writeln!(
                w,
                "changed since last scan: {} new, {} modified, {} deleted; run `codix index`",
                c.new.len(),
                c.modified.len(),
                c.deleted.len()
            )?;
            for (mark, paths) in [
                ("new", &c.new),
                ("modified", &c.modified),
                ("deleted", &c.deleted),
            ] {
                for path in paths {
                    writeln!(w, "  {mark:8}  {path}")?;
                }
            }
        }
        for path in &self.errors {
            writeln!(w, "error: {path}")?;
        }
        if !self.modules.is_empty() {
            writeln!(w, "modules:")?;
            for m in &self.modules {
                let root = if m.root.is_empty() {
                    "."
                } else {
                    m.root.as_str()
                };
                writeln!(w, "  {}  {}  {root}", m.build, m.name)?;
                writeln!(w, "    sources: {}", m.source_roots.join(", "))?;
                if !m.test_roots.is_empty() {
                    writeln!(w, "    tests: {}", m.test_roots.join(", "))?;
                }
                for note in &m.notes {
                    writeln!(w, "    note: {note}")?;
                }
            }
        }
        if !self.build_issues.is_empty() {
            writeln!(w, "build files codix could not interpret:")?;
            for issue in &self.build_issues {
                writeln!(w, "  {}:{}  {}", issue.file, issue.line, issue.construct)?;
            }
        }
        Ok(())
    }
}

/// What `--json` prints for a response.
#[derive(Serialize, schemars::JsonSchema)]
struct Envelope<'a, R> {
    /// Version of the JSON output format; incremented on breaking changes only.
    schema_version: u32,
    /// The command.
    command: &'a str,
    /// Its result; each command's own schema.
    result: &'a R,
    /// Warnings about the project.
    warnings: &'a [Warning],
}

/// What `--json` prints for an error.
#[derive(Serialize, schemars::JsonSchema)]
struct ErrorEnvelope<'a> {
    /// Version of the JSON output format; incremented on breaking changes only.
    schema_version: u32,
    /// The error.
    error: ErrorBody<'a>,
    /// Warnings about the project.
    warnings: &'a [Warning],
}

/// An error, as `--json` prints it.
#[derive(Serialize, schemars::JsonSchema)]
struct ErrorBody<'a> {
    /// Stable code, such as `ambiguous_symbol`.
    code: &'a str,
    /// One line saying what went wrong.
    cause: String,
    /// The next step.
    hint: String,
    /// Symbol IDs that match, for `ambiguous_symbol`.
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    candidates: &'a [String],
}

/// The schema of every response's JSON envelope, any result.
pub fn envelope_schema() -> schemars::Schema {
    schemars::schema_for!(Envelope<'static, serde_json::Value>)
}

/// The schema of every error's JSON envelope.
pub fn error_schema() -> schemars::Schema {
    schemars::schema_for!(ErrorEnvelope<'static>)
}

/// Writes a response as one line of JSON.
pub fn json_response<R: Serialize>(
    w: &mut dyn Write,
    command: &str,
    result: &R,
    warnings: &[Warning],
) -> io::Result<()> {
    let envelope = Envelope {
        schema_version: SCHEMA_VERSION,
        command,
        result,
        warnings,
    };
    serde_json::to_writer(&mut *w, &envelope)?;
    writeln!(w)
}

/// Writes an error as one line of JSON.
pub fn json_error(w: &mut dyn Write, error: &Error, warnings: &[Warning]) -> io::Result<()> {
    let envelope = ErrorEnvelope {
        schema_version: SCHEMA_VERSION,
        error: ErrorBody {
            code: error.code(),
            cause: error.cause(),
            hint: error.hint(),
            candidates: error.candidates(),
        },
        warnings,
    };
    serde_json::to_writer(&mut *w, &envelope)?;
    writeln!(w)
}

/// Writes each warning as `warning[code]: message`.
pub fn text_warnings(w: &mut dyn Write, warnings: &[Warning], style: Style) -> io::Result<()> {
    for warning in warnings {
        writeln!(
            w,
            "{}[{}]: {}",
            style.paint("1;33", "warning"),
            warning.code,
            warning.message
        )?;
    }
    Ok(())
}

/// Writes an error as `error[code]: cause`, its candidates, and `hint: …`.
pub fn text_error(w: &mut dyn Write, error: &Error, style: Style) -> io::Result<()> {
    writeln!(
        w,
        "{}[{}]: {}",
        style.paint("1;31", "error"),
        error.code(),
        error.cause()
    )?;
    for candidate in error.candidates() {
        writeln!(w, "  {candidate}")?;
    }
    writeln!(w, "{}: {}", style.paint("1", "hint"), error.hint())
}

#[cfg(test)]
mod tests {
    use crate::project::FileId;

    use super::*;

    fn page(n: u32) -> Paged<GrepMatch> {
        Paged {
            items: (1..=n)
                .map(|i| GrepMatch {
                    location: Location {
                        path: "src/a.rs".to_owned(),
                        root_path: FileId::from_canonical("src/a.rs"),
                        line: i,
                        column: 1,
                    },
                    text: format!("f{i}"),
                    enclosing: Some("def".to_owned()),
                    enclosing_kind: None,
                })
                .collect(),
            total: n as usize,
            offset: 0,
        }
    }

    fn text(page: &Paged<GrepMatch>, color: bool) -> String {
        let mut out = Vec::new();
        page.render_text(
            &mut out,
            Style {
                color,
                show_origin: false,
                tools: false,
            },
        )
        .unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn previews_name_the_next_step_for_each_frontend() {
        use crate::edit::{ExcludedSite, OverrideUse};
        use crate::project::FileId;

        let at = |path: &str, line| Location {
            path: path.to_owned(),
            root_path: FileId::from_canonical(path),
            line,
            column: 9,
        };
        let preview = PreviewResponse {
            id: "3f9a2c41".to_owned(),
            description: "rename method Invoice.total to sum".to_owned(),
            files: 1,
            sites: 2,
            overrides: vec![OverrideUse {
                flag: "--exclude-unresolved".to_owned(),
                condition: "references codix cannot bind, left unchanged".to_owned(),
                sites: vec![at("Report.java", 5)],
            }],
            excluded: vec![
                ExcludedSite {
                    location: at("Report.java", 5),
                    reason: Exclusion::Unresolved,
                    text: "return invoice.total();".to_owned(),
                },
                ExcludedSite {
                    location: at("Invoice.java", 12),
                    reason: Exclusion::Mention,
                    text: "// total of the lines".to_owned(),
                },
            ],
            ambiguous: Vec::new(),
            diff: "--- a/Invoice.java\n+++ b/Invoice.java\n".to_owned(),
        };
        let render = |tools| {
            let mut out = Vec::new();
            preview
                .render_text(
                    &mut out,
                    Style {
                        color: false,
                        show_origin: false,
                        tools,
                    },
                )
                .unwrap();
            String::from_utf8(out).unwrap()
        };
        assert_eq!(
            render(false),
            "preview 3f9a2c41: rename method Invoice.total to sum
  1 file, 2 sites
  override --exclude-unresolved: references codix cannot bind, left unchanged
    Report.java:5:9
  not changed, 1 unresolved site:
    Report.java:5:9  return invoice.total();
  not changed, 1 mention:
    Invoice.java:12:9  // total of the lines
--- a/Invoice.java
+++ b/Invoice.java
apply with: codix apply 3f9a2c41
"
        );
        assert!(render(true).ends_with("apply with: the apply tool, id \"3f9a2c41\"\n"));
    }

    #[test]
    fn elapsed_shows_the_two_largest_units() {
        assert_eq!(elapsed(42), "42s");
        assert_eq!(elapsed(125), "2m 5s");
        assert_eq!(elapsed(7_500), "2h 5m");
        assert_eq!(elapsed(273_600), "3d 4h");
    }

    #[test]
    fn color_marks_locations_only_when_enabled() {
        assert!(text(&page(1), true).starts_with("\x1b[36msrc/a.rs:1:1\x1b[0m  def  f1\n"));
        assert!(!text(&page(1), false).contains('\x1b'));
    }

    #[test]
    fn columns_align_on_the_widest_cell() {
        let out = text(&page(10), false);
        assert!(out.contains("src/a.rs:9:1   def  f9\n"));
        assert!(out.contains("src/a.rs:10:1  def  f10\n"));
    }

    #[test]
    fn internal_errors_render_in_both_formats() {
        let error = Error::Internal {
            message: "sample internal error".to_owned(),
        };
        assert_eq!(error.exit_code(), 5);
        let mut text = Vec::new();
        text_error(
            &mut text,
            &error,
            Style {
                color: false,
                show_origin: false,
                tools: false,
            },
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(text).unwrap(),
            "error[internal]: sample internal error\nhint: this is a bug in codix; please report it\n"
        );
        let mut json = Vec::new();
        json_error(&mut json, &error, &[]).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(value["error"]["code"], "internal");
        assert_eq!(value["schema_version"], 1);
    }

    #[test]
    fn colored_errors_paint_labels() {
        let mut out = Vec::new();
        let error = Error::UnknownSymbol {
            query: "x".to_owned(),
        };
        text_error(
            &mut out,
            &error,
            Style {
                color: true,
                show_origin: false,
                tools: false,
            },
        )
        .unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(out.starts_with("\x1b[1;31merror\x1b[0m[unknown_symbol]: "));
        assert!(out.contains("\x1b[1mhint\x1b[0m: "));
    }
}
