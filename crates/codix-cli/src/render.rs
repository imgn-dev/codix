//! Text and JSON rendering of responses and errors.

use std::io::{self, Write};

use codix_core::Error;
use codix_core::capabilities::{Capabilities, Tier};
use codix_core::config::{ConfigResponse, Setting};
use codix_core::indexing::{InitResponse, StatusResponse};
use codix_core::op::Warning;
use codix_core::page::{Location, Paged};
use codix_core::query::{
    DefResponse, FindResponse, GrepMatch, GrepResponse, OutlineNode, OutlineResponse, ShowResponse,
    SymbolResult,
};
use codix_core::scan::ScanReport;
use codix_core::store::MatchMode;
use codix_daemon::control::{ListResponse, StartResponse, StopResponse};
use codix_provider::ProviderKind;
use serde::Serialize;

/// Version of the JSON output format; incremented on breaking changes only.
pub(crate) const SCHEMA_VERSION: u32 = 1;

const COLUMN_GAP: &str = "  ";

/// How to render to a stream.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Style {
    pub(crate) color: bool,
    /// `config --list --show-origin`.
    pub(crate) show_origin: bool,
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
pub(crate) fn since_epoch() -> u64 {
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
pub(crate) fn elapsed(seconds: u64) -> String {
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
pub(crate) trait TextRender {
    fn render_text(&self, w: &mut dyn Write, style: Style) -> io::Result<()>;
}

/// A result shown as one line: its location, then its other columns.
pub(crate) trait Row {
    fn location(&self) -> &Location;
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
        let d = &self.definition;
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
        stale(w, self.possibly_stale)
    }
}

fn location(l: &Location) -> String {
    format!("{}:{}:{}", l.path, l.line, l.column)
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
            let tier = match language.tier {
                Tier::Symbols => "tier 1 (symbols)",
                Tier::Search => "search only",
                Tier::NotInBuild => "not in this build",
            };
            writeln!(w, "  {:width$}{COLUMN_GAP}{tier}", language.name)?;
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
        self.scan.render_text(w, style)
    }
}

impl TextRender for StartResponse {
    fn render_text(&self, w: &mut dyn Write, _: Style) -> io::Result<()> {
        if let Some(old) = &self.replaced {
            writeln!(
                w,
                "stopped the codix {} daemon (pid {}), which this codix cannot talk to",
                old.version, old.pid
            )?;
        }
        let d = &self.daemon;
        writeln!(
            w,
            "daemon started for {}: pid {}, socket {}",
            d.root.display(),
            d.pid,
            d.socket
        )
    }
}

impl TextRender for StopResponse {
    fn render_text(&self, w: &mut dyn Write, _: Style) -> io::Result<()> {
        if self.stopped.is_empty() {
            return writeln!(w, "no daemon running");
        }
        for d in &self.stopped {
            writeln!(w, "stopped daemon for {} (pid {})", d.root.display(), d.pid)?;
        }
        Ok(())
    }
}

impl TextRender for ListResponse {
    fn render_text(&self, w: &mut dyn Write, _: Style) -> io::Result<()> {
        if self.daemons.is_empty() {
            return writeln!(w, "no daemon running");
        }
        for listed in &self.daemons {
            let d = &listed.daemon;
            let stats = listed.stats.as_ref().map_or_else(String::new, |s| {
                format!(
                    "  {} MB  idle {}",
                    s.memory_bytes >> 20,
                    elapsed(s.idle_seconds)
                )
            });
            writeln!(
                w,
                "{}  pid {}{stats}  codix {}",
                d.root.display(),
                d.pid,
                d.version
            )?;
        }
        writeln!(w, "{} running", self.daemons.len())
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

#[derive(Serialize)]
struct Envelope<'a, R> {
    schema_version: u32,
    command: &'a str,
    result: &'a R,
    warnings: &'a [Warning],
}

#[derive(Serialize)]
struct ErrorEnvelope<'a> {
    schema_version: u32,
    error: ErrorBody<'a>,
    warnings: &'a [Warning],
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    code: &'a str,
    cause: String,
    hint: String,
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    candidates: &'a [String],
}

/// Writes a response as one line of JSON.
pub(crate) fn json_response<R: Serialize>(
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
pub(crate) fn json_error(w: &mut dyn Write, error: &Error, warnings: &[Warning]) -> io::Result<()> {
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
pub(crate) fn text_warnings(
    w: &mut dyn Write,
    warnings: &[Warning],
    style: Style,
) -> io::Result<()> {
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
pub(crate) fn text_error(w: &mut dyn Write, error: &Error, style: Style) -> io::Result<()> {
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
    use codix_core::project::FileId;

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
            },
        )
        .unwrap();
        String::from_utf8(out).unwrap()
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
            },
        )
        .unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(out.starts_with("\x1b[1;31merror\x1b[0m[unknown_symbol]: "));
        assert!(out.contains("\x1b[1mhint\x1b[0m: "));
    }
}
