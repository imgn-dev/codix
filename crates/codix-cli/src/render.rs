//! Text and JSON rendering of responses and errors.

use std::io::{self, Write};

use codix_core::Error;
use codix_core::debug::EchoItem;
use codix_core::op::Warning;
use codix_core::page::{Location, Paged};
use serde::Serialize;

/// Version of the JSON output format; incremented on breaking changes only.
pub(crate) const SCHEMA_VERSION: u32 = 1;

const COLUMN_GAP: &str = "  ";

/// Whether to color a stream.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Style {
    pub(crate) color: bool,
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

/// A response that has a text rendering.
pub(crate) trait TextRender {
    fn render_text(&self, w: &mut dyn Write, style: Style) -> io::Result<()>;
}

/// A result shown as one line: its location, then its other columns.
pub(crate) trait Row {
    fn location(&self) -> &Location;
    fn columns(&self) -> Vec<String>;
}

impl Row for EchoItem {
    fn location(&self) -> &Location {
        &self.location
    }

    fn columns(&self) -> Vec<String> {
        vec![self.kind.clone(), self.name.clone()]
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
pub(crate) fn json_error(w: &mut dyn Write, error: &Error) -> io::Result<()> {
    let envelope = ErrorEnvelope {
        schema_version: SCHEMA_VERSION,
        error: ErrorBody {
            code: error.code(),
            cause: error.cause(),
            hint: error.hint(),
            candidates: error.candidates(),
        },
    };
    serde_json::to_writer(&mut *w, &envelope)?;
    writeln!(w)
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
    use super::*;

    fn page(n: u32) -> Paged<EchoItem> {
        Paged {
            items: (1..=n)
                .map(|i| EchoItem {
                    location: Location {
                        path: "src/a.rs".to_owned(),
                        line: i,
                        column: 1,
                    },
                    kind: "def".to_owned(),
                    name: format!("f{i}"),
                })
                .collect(),
            total: n as usize,
            offset: 0,
        }
    }

    fn text(page: &Paged<EchoItem>, color: bool) -> String {
        let mut out = Vec::new();
        page.render_text(&mut out, Style { color }).unwrap();
        String::from_utf8(out).unwrap()
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
    fn colored_errors_paint_labels() {
        let mut out = Vec::new();
        let error = Error::UnknownSymbol {
            query: "x".to_owned(),
        };
        text_error(&mut out, &error, Style { color: true }).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(out.starts_with("\x1b[1;31merror\x1b[0m[unknown_symbol]: "));
        assert!(out.contains("\x1b[1mhint\x1b[0m: "));
    }
}
