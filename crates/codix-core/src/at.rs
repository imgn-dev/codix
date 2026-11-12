//! `at`: what encloses a place, for each place given, such as the matches
//! another tool printed.

use std::fs;

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::op::{Context, Operation, Outcome};
use crate::query::{SymbolResult, indexed, lookup};
use crate::store::SymbolHit;

/// `at`: the innermost symbol around each place.
pub struct AtOp;

/// Request for [`AtOp`].
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AtRequest {
    /// Places as `path:line` or `path:line:col`, any text after a further colon kept, as `rg -n`, `rg --vimgrep` and compilers print them.
    pub places: Vec<String>,
}

/// One place and what encloses it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AtPlace {
    /// The place as given, without its text.
    pub place: String,
    /// The text that followed the place, if any.
    pub text: Option<String>,
    /// The innermost symbol holding the place; none outside every symbol.
    pub enclosing: Option<SymbolResult>,
    /// Why the place could not be read: not a place, a file codix does not
    /// know, or a line past the end.
    pub error: Option<String>,
}

/// What [`AtOp`] answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AtResponse {
    /// Every place, in the order given.
    pub places: Vec<AtPlace>,
}

impl Outcome for AtResponse {
    fn has_results(&self) -> bool {
        self.places.iter().any(|p| p.enclosing.is_some())
    }
}

/// A place read from `input`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Place<'a> {
    path: &'a str,
    line: u32,
    column: Option<u32>,
    /// `path:line[:col]`.
    shown: &'a str,
    text: Option<&'a str>,
}

/// Reads `path:line`, `path:line:col`, and either followed by `:` and text.
/// The path ends at the first colon followed by digits and then a colon or
/// the end.
fn place(input: &str) -> Option<Place<'_>> {
    let input = input.trim_end_matches(['\n', '\r']);
    let number = |s: &str| -> Option<(u32, usize)> {
        let digits = s.bytes().take_while(u8::is_ascii_digit).count();
        let value = s.get(..digits)?.parse().ok()?;
        let rest = &s[digits..];
        (rest.is_empty() || rest.starts_with(':')).then_some((value, digits))
    };
    for (colon, _) in input.match_indices(':') {
        if colon == 0 {
            continue;
        }
        let Some((line, digits)) = number(&input[colon + 1..]) else {
            continue;
        };
        let mut end = colon + 1 + digits;
        let mut column = None;
        if let Some(after) = input.get(end + 1..)
            && let Some((col, digits)) = number(after)
            && col > 0
        {
            column = Some(col);
            end += 1 + digits;
        }
        let text = input.get(end + 1..);
        return (line > 0).then_some(Place {
            path: &input[..colon],
            line,
            column,
            shown: &input[..end],
            text,
        });
    }
    None
}

impl Operation for AtOp {
    const NAME: &'static str = "at";
    type Request = AtRequest;
    type Response = AtResponse;

    fn run(&self, ctx: &Context, request: AtRequest) -> Result<AtResponse, Error> {
        let (project, config) = indexed(ctx)?;
        let read: Vec<Option<Place<'_>>> = request.places.iter().map(|p| place(p)).collect();
        let mut paths: Vec<String> = Vec::new();
        for p in read.iter().flatten() {
            if let Ok(id) = project.file_id(ctx.cwd(), p.path)
                && !paths.contains(&id.as_str().to_owned())
            {
                paths.push(id.as_str().to_owned());
            }
        }
        let (places, _) = lookup(ctx, project, &config, |snapshot| {
            let mut places = Vec::new();
            for (input, p) in request.places.iter().zip(&read) {
                let Some(p) = p else {
                    places.push(failed(input, "not a place: path:line or path:line:col"));
                    continue;
                };
                let answer = |error: Option<String>, enclosing: Option<SymbolResult>| AtPlace {
                    place: p.shown.to_owned(),
                    text: p.text.map(str::to_owned),
                    enclosing,
                    error,
                };
                let id = match project.file_id(ctx.cwd(), p.path) {
                    Ok(id) => id,
                    Err(Error::Io { source, .. })
                        if source.kind() == std::io::ErrorKind::NotFound =>
                    {
                        places.push(answer(Some("no such file".to_owned()), None));
                        continue;
                    }
                    Err(e) => {
                        places.push(answer(Some(e.cause()), None));
                        continue;
                    }
                };
                if snapshot.file(id.as_str())?.is_none() {
                    places.push(answer(Some("a file codix does not index".to_owned()), None));
                    continue;
                }
                let lines =
                    fs::read_to_string(project.path_of(&id)).map_or(0, |text| text.lines().count());
                if p.line as usize > lines {
                    let error = format!("line {} is past the end ({lines} lines)", p.line);
                    places.push(answer(Some(error), None));
                    continue;
                }
                let enclosing = innermost(snapshot.symbols_in(id.as_str())?, p.line, p.column)
                    .map(|hit| SymbolResult::new(ctx, hit));
                places.push(answer(None, enclosing));
            }
            Ok((places, paths.clone()))
        })?;
        Ok(AtResponse { places })
    }
}

fn failed(input: &str, why: &str) -> AtPlace {
    AtPlace {
        place: input.trim_end().to_owned(),
        text: None,
        enclosing: None,
        error: Some(why.to_owned()),
    }
}

/// The deepest symbol holding the line, or the line and column.
fn innermost(symbols: Vec<SymbolHit>, line: u32, column: Option<u32>) -> Option<SymbolHit> {
    symbols
        .into_iter()
        .filter(|hit| {
            let r = &hit.symbol.range;
            match column {
                Some(column) => {
                    (r.start.line, r.start.column) <= (line, column)
                        && (line, column) < (r.end.line, r.end.column)
                }
                // A range ending at column 1 stops before that line.
                None => {
                    r.start.line <= line
                        && (line < r.end.line || (line == r.end.line && r.end.column > 1))
                }
            }
        })
        .max_by_key(|hit| hit.symbol.range.start.byte)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at<'a>(
        path: &'a str,
        line: u32,
        column: Option<u32>,
        shown: &'a str,
        text: Option<&'a str>,
    ) -> Place<'a> {
        Place {
            path,
            line,
            column,
            shown,
            text,
        }
    }

    #[test]
    fn places_are_read_as_tools_print_them() {
        let read = |input| place(input).expect("a place");
        assert_eq!(
            read("mm/slub.c:42"),
            at("mm/slub.c", 42, None, "mm/slub.c:42", None)
        );
        assert_eq!(
            read("mm/slub.c:42:7"),
            at("mm/slub.c", 42, Some(7), "mm/slub.c:42:7", None)
        );
        assert_eq!(
            read("fs/a.c:12:\tmutex_lock(&l);\n"),
            at("fs/a.c", 12, None, "fs/a.c:12", Some("\tmutex_lock(&l);"))
        );
        assert_eq!(
            read("fs/a.c:12:5:x = 1;"),
            at("fs/a.c", 12, Some(5), "fs/a.c:12:5", Some("x = 1;"))
        );
        assert_eq!(
            read("odd:name.c:3: y"),
            at("odd:name.c", 3, None, "odd:name.c:3", Some(" y"))
        );
        assert_eq!(place("no place here"), None);
        assert_eq!(place("a.c:0"), None);
    }
}
