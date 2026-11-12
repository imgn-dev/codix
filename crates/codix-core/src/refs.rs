//! `refs`, and `def` by location: both read what a name binds to from its
//! file's model, built from the file as it is on disk.
//!
//! References bound by scope are `scoped`. References that may mean the
//! symbol but that codix cannot bind, in other files or through another
//! object, are `textual`: they match by name only.

use std::fs;

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::config::Config;
use crate::detect::Detector;
use crate::op::{Context, Operation, Outcome};
use crate::page::{Location, Page, Paged, Ranked, sort_results};
use crate::project::{FileId, Project, ScopeRequest};
use crate::query::{Renamed, SymbolResult, complete, following, indexed};
use crate::scope::{Binding, Defines, FileModel, Receiver};
use crate::store::{FileStatus, RefState, Snapshot, Store, SymbolHit};
use crate::symbol_id::{SymbolRef, resolve_overloads_where};

/// A file read from disk, with its model.
pub struct Analyzed {
    /// The file.
    pub id: FileId,
    /// Its language.
    pub language: String,
    /// Its content.
    pub content: String,
    /// Its scopes, definitions and references.
    pub model: FileModel,
}

/// Reads a file and builds its model; `None` when its language has no
/// grammar in this build or it is not UTF-8.
pub fn analyze(project: &Project, config: &Config, id: &FileId) -> Result<Option<Analyzed>, Error> {
    let path = project.path_of(id);
    let bytes = fs::read(&path).map_err(|source| Error::Io { path, source })?;
    let language = Detector::new(config).detect(id.as_str(), &bytes).language;
    let registry = codix_provider::registry();
    let (Some(descriptor), Some(grammar)) = (
        registry.languages.iter().find(|d| d.name == language),
        registry.grammars.get(&language),
    ) else {
        return Ok(None);
    };
    let Ok(content) = String::from_utf8(bytes) else {
        return Ok(None);
    };
    let model = crate::scope::model(descriptor, grammar, id.as_str(), &content)?;
    Ok(Some(Analyzed {
        id: id.clone(),
        language,
        content,
        model,
    }))
}

/// The byte at a 1-based line and column, columns counting Unicode scalar
/// values; `None` past the end of the line or the file.
pub(crate) fn byte_at(content: &str, line: u32, column: u32) -> Option<usize> {
    let start = if line == 1 {
        0
    } else {
        content
            .match_indices('\n')
            .nth(usize::try_from(line).ok()? - 2)
            .map(|(i, _)| i + 1)?
    };
    let text = &content[start..];
    let text = &text[..text.find('\n').unwrap_or(text.len())];
    let skip = usize::try_from(column).ok()? - 1;
    match text.char_indices().nth(skip) {
        Some((i, _)) => Some(start + i),
        None if text.chars().count() == skip => Some(start + text.len()),
        None => None,
    }
}

/// What a name at a location means.
pub(crate) enum AtLocation {
    /// A definition of the file, by index into its model's definitions.
    Definition(usize),
    /// A name the file does not define, or cannot tell: its name.
    Unbound(String),
    /// No name there.
    Nothing,
}

/// What the identifier at `byte` defines or binds to.
pub(crate) fn at(model: &FileModel, byte: usize) -> AtLocation {
    if let Some(d) = model.definition_at(byte) {
        return AtLocation::Definition(d);
    }
    match model.reference_at(byte).map(|r| &model.references[r]) {
        Some(r) => match r.binding {
            Binding::Bound(d) => AtLocation::Definition(d),
            Binding::Free | Binding::Uncertain => AtLocation::Unbound(r.name.clone()),
        },
        None => AtLocation::Nothing,
    }
}

/// A symbol in results, from a file's model rather than the index.
pub(crate) fn symbol_result(
    ctx: &Context,
    file: &Analyzed,
    definition: usize,
    confidence: &str,
) -> SymbolResult {
    let d = &file.model.definitions[definition];
    let at = d.name_range.start;
    let location = ctx.location(&file.id, at.line, at.column);
    match &d.defines {
        Defines::Symbol(i) => {
            let s = &file.model.symbols[*i];
            SymbolResult {
                location,
                symbol_id: s.symbol_id.clone(),
                kind: s.kind.clone(),
                name: s.name.clone(),
                qualified_name: s.qualified_name.clone(),
                signature: s.signature.clone(),
                language: file.language.clone(),
                confidence: confidence.to_owned(),
            }
        }
        // A local has no symbol ID; its location names it in every command.
        Defines::Local { kind } => SymbolResult {
            location,
            symbol_id: format!("{}:{}:{}", file.id.as_str(), at.line, at.column),
            kind: kind.clone(),
            name: d.name.clone(),
            qualified_name: d.name.clone(),
            signature: None,
            language: file.language.clone(),
            confidence: confidence.to_owned(),
        },
    }
}

/// What a command's symbol argument names.
pub(crate) enum Target {
    /// Definitions of one file, overloads of one name or a single one.
    Definitions {
        /// The file, as it is on disk.
        file: Analyzed,
        /// Indices into its model's definitions.
        definitions: Vec<usize>,
    },
}

/// Resolves a symbol argument: by location, the binding of the name there;
/// otherwise through the index, as `def` does, with every overload of the
/// name in its file.
pub(crate) fn target(
    ctx: &Context,
    project: &Project,
    config: &Config,
    snapshot: &Snapshot<'_>,
    input: &str,
    keep: &dyn Fn(&SymbolHit) -> bool,
) -> Result<Target, Error> {
    let name = match SymbolRef::parse(input)? {
        SymbolRef::Location { file, line, column } => {
            let id = project.file_id(ctx.cwd(), &file)?;
            let unknown = || Error::UnknownSymbol {
                query: input.to_owned(),
            };
            let Some(analyzed) = analyze(project, config, &id)? else {
                return Err(unknown());
            };
            let byte = byte_at(&analyzed.content, line, column).ok_or_else(unknown)?;
            match at(&analyzed.model, byte) {
                AtLocation::Definition(d) => {
                    let definitions = analyzed.model.overloads(d);
                    return Ok(Target::Definitions {
                        file: analyzed,
                        definitions,
                    });
                }
                AtLocation::Unbound(name) => name,
                AtLocation::Nothing => return Err(unknown()),
            }
        }
        _ => input.to_owned(),
    };
    let hits = resolve_overloads_where(snapshot, project, ctx.cwd(), &name, keep)?;
    let id = FileId::from_canonical(hits[0].path.as_str());
    let unknown = || Error::UnknownSymbol {
        query: input.to_owned(),
    };
    let file = analyze(project, config, &id)?.ok_or_else(unknown)?;
    let wanted: Vec<&str> = hits.iter().map(|h| h.symbol.symbol_id.as_str()).collect();
    let first = file
        .model
        .definitions
        .iter()
        .position(|d| match d.defines {
            Defines::Symbol(i) => wanted.contains(&file.model.symbols[i].symbol_id.as_str()),
            Defines::Local { .. } => false,
        })
        .ok_or_else(unknown)?;
    let definitions = file.model.overloads(first);
    Ok(Target::Definitions { file, definitions })
}

/// `refs`: every reference to a symbol or a local.
pub struct RefsOp;

/// Request for [`RefsOp`].
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RefsRequest {
    /// Symbol ID, its path part, `file:line:col`, or a name.
    pub symbol: String,
    /// Consider symbols of this kind only; repeatable.
    #[serde(default, rename = "kind")]
    pub kinds: Vec<String>,
    /// Directories to keep.
    #[serde(flatten)]
    pub scope: ScopeRequest,
    /// Page to return.
    #[serde(flatten)]
    pub page: Page,
}

/// One reference, or the definition itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RefResult {
    /// Where the name is.
    #[serde(flatten)]
    pub location: Location,
    /// `def` for the definition; otherwise `call`, `type`, `import`, `read`
    /// or `write`.
    pub context: String,
    /// `scoped`: bound to the symbol by scope; `textual`: matched by name
    /// only.
    pub confidence: String,
    /// Qualified name of the innermost enclosing symbol.
    pub enclosing: Option<String>,
}

impl Ranked for RefResult {
    fn is_definition(&self) -> bool {
        self.context == "def"
    }

    fn location(&self) -> &Location {
        &self.location
    }
}

/// What [`RefsOp`] answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RefsResponse {
    /// The symbol's ID, and its overloads' after it; or a local's
    /// `file:line:col`.
    pub targets: Vec<String>,
    /// The definitions first, then references in their file, then by path.
    pub references: Paged<RefResult>,
    /// Definitions among the results, before paging.
    pub definitions: usize,
    /// References bound by scope, before paging.
    pub scoped: usize,
    /// References matched by name only, before paging.
    pub textual: usize,
    /// The next page, when results remain: the request with these fields
    /// changed.
    pub next: Option<RefsNext>,
    /// See [`crate::query::FindResponse::possibly_stale`].
    pub possibly_stale: bool,
    /// When the ID given was the symbol's before codix renamed it.
    #[serde(default)]
    pub renamed: Option<Renamed>,
}

/// What changes in a [`RefsRequest`] for its next page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RefsNext {
    /// Where the next page starts.
    pub after: String,
    /// The symbol's ID, so the next page keeps it through a rename.
    pub symbol: String,
}

impl Outcome for RefsResponse {
    fn has_results(&self) -> bool {
        self.references.has_results()
    }
}

const SCOPED: &str = "scoped";
const TEXTUAL: &str = "textual";

impl Operation for RefsOp {
    const NAME: &'static str = "refs";
    type Request = RefsRequest;
    type Response = RefsResponse;

    fn run(&self, ctx: &Context, request: RefsRequest) -> Result<RefsResponse, Error> {
        let (project, config) = indexed(ctx)?;
        let scope = ctx.scope(&request.scope)?;
        let possibly_stale = complete(ctx, project, &config)?;
        let mut store = Store::open(&project.index_path())?;
        let snapshot = store.snapshot()?;
        let (Target::Definitions { file, definitions }, renamed) =
            following(&request.symbol, |input| {
                target(ctx, project, &config, &snapshot, input, &|hit| {
                    request.kinds.is_empty() || request.kinds.contains(&hit.symbol.kind)
                })
            })?;
        let model = &file.model;
        let first = &model.definitions[definitions[0]];
        let targets: Vec<String> = definitions
            .iter()
            .map(|&d| symbol_result(ctx, &file, d, SCOPED).symbol_id)
            .collect();
        let enclosing = |byte: usize| {
            model
                .enclosing_symbol(byte)
                .map(|i| model.symbols[i].qualified_name.clone())
        };
        let mut results = Vec::new();
        for &d in &definitions {
            let at = model.definitions[d].name_range.start;
            results.push(RefResult {
                location: ctx.location(&file.id, at.line, at.column),
                context: "def".to_owned(),
                confidence: SCOPED.to_owned(),
                enclosing: None,
            });
        }
        let visible_elsewhere = match &first.defines {
            Defines::Symbol(i) => model.symbols[*i].visibility.as_deref() != Some("private"),
            Defines::Local { .. } => false,
        };
        for r in &model.references {
            let confidence = match r.binding {
                Binding::Bound(d) if definitions.contains(&d) => SCOPED,
                Binding::Free | Binding::Uncertain
                    if visible_elsewhere
                        && r.name == first.name
                        && (r.receiver != Receiver::None || r.binding == Binding::Uncertain) =>
                {
                    TEXTUAL
                }
                _ => continue,
            };
            let at = r.range.start;
            results.push(RefResult {
                location: ctx.location(&file.id, at.line, at.column),
                context: r.context.as_str().to_owned(),
                confidence: confidence.to_owned(),
                enclosing: enclosing(at.byte as usize),
            });
        }
        if visible_elsewhere {
            for r in snapshot.refs_named(&first.name)? {
                if r.path == file.id.as_str() || r.state == RefState::Bound {
                    continue;
                }
                results.push(RefResult {
                    location: ctx.location(
                        &FileId::from_canonical(r.path.as_str()),
                        r.start.line,
                        r.start.column,
                    ),
                    context: r.context.as_str().to_owned(),
                    confidence: TEXTUAL.to_owned(),
                    enclosing: r.enclosing.as_deref().map(qualified_part),
                });
            }
            let defined: Vec<Location> = results.iter().map(|r| r.location.clone()).collect();
            results.extend(
                tier_1_matches(ctx, project, &snapshot, &first.name)?
                    .into_iter()
                    .filter(|r| !defined.contains(&r.location)),
            );
        }
        results.retain(|r| scope.contains(&r.location.root_path));
        sort_results(&mut results, Some(file.id.as_str()));
        let (definitions, scoped, textual) = counts(&results);
        // A symbol is pinned by its ID, which follows renames; a local has
        // only its place.
        let symbol = match first.defines {
            Defines::Symbol(_) => targets[0].clone(),
            Defines::Local { .. } => request.symbol.clone(),
        };
        let (references, after) = request.page.apply(results, Some(file.id.as_str()));
        Ok(RefsResponse {
            targets,
            references,
            definitions,
            scoped,
            textual,
            next: after.map(|after| RefsNext { after, symbol }),
            possibly_stale,
            renamed,
        })
    }
}

/// The definitions, the references bound by scope, and those matched by name
/// among `results`.
fn counts(results: &[RefResult]) -> (usize, usize, usize) {
    let definitions = results.iter().filter(|r| r.is_definition()).count();
    let scoped = results
        .iter()
        .filter(|r| !r.is_definition() && r.confidence == SCOPED)
        .count();
    (definitions, scoped, results.len() - definitions - scoped)
}

/// The qualified name in a symbol ID, `prefix:path::Qualified.name#hash`.
fn qualified_part(id: &str) -> String {
    let after = id.split_once("::").map_or(id, |(_, rest)| rest);
    after
        .rsplit_once('#')
        .map_or(after, |(name, _)| name)
        .to_owned()
}

/// Whole-word matches of `name` in files of languages without scopes (tier
/// 1).
fn tier_1_matches(
    ctx: &Context,
    project: &Project,
    snapshot: &Snapshot<'_>,
    name: &str,
) -> Result<Vec<RefResult>, Error> {
    let registry = codix_provider::registry();
    let word = regex::Regex::new(&format!(r"\b{}\b", regex::escape(name))).map_err(|e| {
        Error::Internal {
            message: format!("cannot match `{name}`: {e}"),
        }
    })?;
    let mut found = Vec::new();
    for row in snapshot.files()? {
        let tier_1 = registry
            .grammars
            .get(&row.language)
            .is_some_and(|g| g.locals.is_none());
        if !tier_1 || row.status != FileStatus::Indexed {
            continue;
        }
        let id = FileId::from_canonical(row.path.as_str());
        let Ok(text) = fs::read_to_string(project.path_of(&id)) else {
            continue;
        };
        for (number, line) in text.lines().enumerate() {
            for m in word.find_iter(line) {
                found.push(RefResult {
                    location: ctx.location(
                        &id,
                        u32::try_from(number + 1).unwrap_or(u32::MAX),
                        u32::try_from(line[..m.start()].chars().count() + 1).unwrap_or(u32::MAX),
                    ),
                    context: "read".to_owned(),
                    confidence: TEXTUAL.to_owned(),
                    enclosing: None,
                });
            }
        }
    }
    Ok(found)
}

/// `def` by location: the definition the name there binds to, read from
/// its file; `None` when no name is there, so `def` falls back to the
/// symbol enclosing the location.
pub(crate) fn def_at(
    ctx: &Context,
    project: &Project,
    config: &Config,
    file: &str,
    line: u32,
    column: u32,
) -> Result<Option<DefAt>, Error> {
    let id = project.file_id(ctx.cwd(), file)?;
    let Some(analyzed) = analyze(project, config, &id)? else {
        return Ok(None);
    };
    let Some(byte) = byte_at(&analyzed.content, line, column) else {
        return Ok(None);
    };
    Ok(match at(&analyzed.model, byte) {
        AtLocation::Definition(d) => {
            let results = analyzed
                .model
                .overloads(d)
                .into_iter()
                .map(|o| symbol_result(ctx, &analyzed, o, SCOPED))
                .collect();
            Some(DefAt::Found(results))
        }
        AtLocation::Unbound(name) => Some(DefAt::ByName(name)),
        AtLocation::Nothing => None,
    })
}

/// What a location says for `def`.
pub(crate) enum DefAt {
    /// The definitions, overloads included, first one first.
    Found(Vec<SymbolResult>),
    /// A name its file does not bind: look it up by name.
    ByName(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations_count_characters_and_stop_at_line_ends() {
        let text = "ab\né x\n";
        assert_eq!(byte_at(text, 1, 1), Some(0));
        assert_eq!(byte_at(text, 1, 3), Some(2));
        assert_eq!(byte_at(text, 1, 4), None);
        assert_eq!(byte_at(text, 2, 3), Some(6));
        assert_eq!(byte_at(text, 3, 1), Some(8));
        assert_eq!(byte_at(text, 9, 1), None);
    }

    #[test]
    fn the_qualified_part_of_an_id() {
        assert_eq!(
            qualified_part("java:A.java::Invoice.total#a3f9"),
            "Invoice.total"
        );
        assert_eq!(qualified_part("Invoice"), "Invoice");
    }
}
