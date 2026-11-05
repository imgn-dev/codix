//! The query commands: `find`, `def`, `outline`, `show` and `grep`. They read
//! the index, keeping it fresh first: lookups recheck the files they return,
//! and `grep`, which must not miss a change anywhere, scans first.

use std::collections::{BTreeMap, HashSet};
use std::fs;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::Error;
use crate::config::Config;
use crate::lock::QUERY_WAIT;
use crate::op::{Context, Operation, Outcome};
use crate::page::{Location, Page, Paged, Ranked, sort_results};
use crate::parse::PackIndexer;
use crate::project::{FileId, Project, ScopeRequest};
use crate::scan::{rescan, scan_within};
use crate::store::{FileStatus, MatchMode, NameQuery, Snapshot, Store, SymbolHit};
use crate::symbol_id::resolve_where;

/// Longest line text shown with a `grep` match, in characters.
const MAX_LINE: usize = 200;

/// A symbol in query results.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SymbolResult {
    /// Where its name is.
    #[serde(flatten)]
    pub location: Location,
    /// Symbol ID, accepted by every command naming a symbol.
    pub symbol_id: String,
    /// Kind from the shared vocabulary.
    pub kind: String,
    /// Simple name.
    pub name: String,
    /// Qualified name within the file.
    pub qualified_name: String,
    /// Declaration without the body, when captured.
    pub signature: Option<String>,
    /// Language of the file.
    pub language: String,
    /// How it was found: `textual` for tier 1 symbols.
    pub confidence: String,
}

impl Ranked for SymbolResult {
    fn is_definition(&self) -> bool {
        true
    }

    fn location(&self) -> &Location {
        &self.location
    }
}

impl SymbolResult {
    fn new(ctx: &Context, hit: SymbolHit) -> Self {
        let start = hit.symbol.name_range.start;
        Self {
            location: ctx.location(&FileId::from_canonical(hit.path), start.line, start.column),
            symbol_id: hit.symbol.symbol_id,
            kind: hit.symbol.kind,
            name: hit.symbol.name,
            qualified_name: hit.symbol.qualified_name,
            signature: hit.symbol.signature,
            language: hit.language,
            confidence: match hit.symbol.confidence {
                crate::store::Confidence::Normal => "textual".to_owned(),
                crate::store::Confidence::Low => "low".to_owned(),
            },
        }
    }
}

/// The project, indexed, with its configuration.
fn indexed(ctx: &Context) -> Result<(&Project, Config), Error> {
    let project = ctx.project()?;
    project.require_indexed()?;
    Ok((project, Config::load(project)?))
}

/// Runs a lookup, rechecks the files it returned, and when any had changed,
/// reindexes them and runs it again. The flag is set when the write lock
/// stayed busy, so changed files could not be reindexed.
fn lookup<T>(
    ctx: &Context,
    project: &Project,
    config: &Config,
    query: impl Fn(&Snapshot<'_>) -> Result<(T, Vec<String>), Error>,
) -> Result<(T, bool), Error> {
    let mut store = Store::open(&project.index_path())?;
    let (first, paths) = query(&store.snapshot()?)?;
    match rescan(
        project,
        &PackIndexer::new(config),
        ctx.gate(),
        &paths,
        QUERY_WAIT,
    )? {
        None => Ok((first, true)),
        Some(r) if r.indexed + r.touched + r.deleted + r.pending == 0 => Ok((first, false)),
        Some(_) => Ok((query(&store.snapshot()?)?.0, false)),
    }
}

fn distinct(paths: impl Iterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    paths.filter(|p| seen.insert(p.clone())).collect()
}

/// `find`: symbols by name.
pub struct FindOp;

/// Request for [`FindOp`].
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FindRequest {
    /// Name to look for.
    pub query: String,
    /// How to match: `exact`, `prefix` and `fuzzy` ignore case, `glob` matches
    /// the qualified name; by default exact, then prefix, then fuzzy.
    #[serde(default)]
    pub mode: Option<MatchMode>,
    /// Keep symbols of this kind, such as method; repeatable.
    #[serde(default, rename = "kind")]
    pub kinds: Vec<String>,
    /// Keep symbols of this language; repeatable.
    #[serde(default, rename = "lang")]
    pub languages: Vec<String>,
    /// Directories to keep.
    #[serde(flatten)]
    pub scope: ScopeRequest,
    /// Page to return.
    #[serde(flatten)]
    pub page: Page,
}

/// What [`FindOp`] answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FindResponse {
    /// The mode that matched, or the last one tried.
    pub mode: MatchMode,
    /// Matching symbols.
    pub results: Paged<SymbolResult>,
    /// Whether another process held the write lock, so changed files could
    /// not be reindexed first.
    pub possibly_stale: bool,
}

impl Outcome for FindResponse {
    fn has_results(&self) -> bool {
        self.results.has_results()
    }
}

impl Operation for FindOp {
    const NAME: &'static str = "find";
    type Request = FindRequest;
    type Response = FindResponse;

    fn run(&self, ctx: &Context, request: FindRequest) -> Result<FindResponse, Error> {
        let (project, config) = indexed(ctx)?;
        let scope = ctx.scope(&request.scope)?;
        let modes = match request.mode {
            Some(mode) => vec![mode],
            None => vec![MatchMode::Exact, MatchMode::Prefix, MatchMode::Fuzzy],
        };
        let keep = |hit: &SymbolHit| {
            scope.contains(&FileId::from_canonical(hit.path.as_str()))
                && (request.kinds.is_empty() || request.kinds.contains(&hit.symbol.kind))
                && (request.languages.is_empty() || request.languages.contains(&hit.language))
        };
        let ((mode, hits), possibly_stale) = lookup(ctx, project, &config, |snapshot| {
            let mut last = (modes[modes.len() - 1], Vec::new());
            for &mode in &modes {
                let hits: Vec<SymbolHit> = snapshot
                    .find(&NameQuery {
                        text: request.query.clone(),
                        mode,
                    })?
                    .into_iter()
                    .filter(|hit| keep(hit))
                    .collect();
                if !hits.is_empty() {
                    last = (mode, hits);
                    break;
                }
            }
            let paths = distinct(last.1.iter().map(|hit| hit.path.clone()));
            Ok((last, paths))
        })?;
        let mut results: Vec<SymbolResult> = hits
            .into_iter()
            .map(|hit| SymbolResult::new(ctx, hit))
            .collect();
        if mode != MatchMode::Fuzzy {
            sort_results(&mut results, None);
        }
        Ok(FindResponse {
            mode,
            results: request.page.apply(results),
            possibly_stale,
        })
    }
}

/// `def`: where a symbol is defined.
pub struct DefOp;

/// Request for [`DefOp`].
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DefRequest {
    /// Symbol ID, its path part, `file:line:col`, or a name.
    pub symbol: String,
    /// Consider symbols of this kind only; repeatable.
    #[serde(default, rename = "kind")]
    pub kinds: Vec<String>,
    /// Directories to consider, narrowing an ambiguous name.
    #[serde(flatten)]
    pub scope: ScopeRequest,
}

/// What [`DefOp`] answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DefResponse {
    /// The definition.
    pub definition: SymbolResult,
    /// See [`FindResponse::possibly_stale`].
    pub possibly_stale: bool,
}

impl Outcome for DefResponse {
    fn has_results(&self) -> bool {
        true
    }
}

impl Operation for DefOp {
    const NAME: &'static str = "def";
    type Request = DefRequest;
    type Response = DefResponse;

    fn run(&self, ctx: &Context, request: DefRequest) -> Result<DefResponse, Error> {
        let (project, config) = indexed(ctx)?;
        let scope = ctx.scope(&request.scope)?;
        let (hit, possibly_stale) = lookup(ctx, project, &config, |snapshot| {
            let hit = resolve_where(snapshot, project, ctx.cwd(), &request.symbol, &|hit| {
                (request.kinds.is_empty() || request.kinds.contains(&hit.symbol.kind))
                    && scope.contains(&FileId::from_canonical(hit.path.as_str()))
            })?;
            let path = hit.path.clone();
            Ok((hit, vec![path]))
        })?;
        Ok(DefResponse {
            definition: SymbolResult::new(ctx, hit),
            possibly_stale,
        })
    }
}

/// `outline`: a file's or directory's symbol tree, signatures only.
pub struct OutlineOp;

/// Request for [`OutlineOp`].
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct OutlineRequest {
    /// File or directory.
    pub path: String,
}

/// One symbol in an outline, with the symbols it contains.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct OutlineNode {
    /// The symbol.
    #[serde(flatten)]
    pub symbol: SymbolResult,
    /// Symbols defined inside it.
    pub children: Vec<OutlineNode>,
}

/// The outline of one file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct OutlineFile {
    /// Path relative to the cwd.
    pub path: String,
    /// Root-relative path.
    pub root_path: FileId,
    /// Language of the file.
    pub language: String,
    /// Top-level symbols, in source order.
    pub symbols: Vec<OutlineNode>,
}

/// What [`OutlineOp`] answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct OutlineResponse {
    /// Files with symbols, sorted by path.
    pub files: Vec<OutlineFile>,
    /// See [`FindResponse::possibly_stale`].
    pub possibly_stale: bool,
}

impl Outcome for OutlineResponse {
    fn has_results(&self) -> bool {
        !self.files.is_empty()
    }
}

impl Operation for OutlineOp {
    const NAME: &'static str = "outline";
    type Request = OutlineRequest;
    type Response = OutlineResponse;

    fn run(&self, ctx: &Context, request: OutlineRequest) -> Result<OutlineResponse, Error> {
        let (project, config) = indexed(ctx)?;
        let target = project.file_id(ctx.cwd(), &request.path)?;
        {
            // A file of a language this build leaves out has no outline here.
            let mut store = Store::open(&project.index_path())?;
            if let Some(row) = store.snapshot()?.file(target.as_str())?
                && row.language != crate::detect::TEXT
                && row.status != FileStatus::Indexed
                && let Err(e @ Error::NotSupportedInBuild { .. }) =
                    crate::providers::require_language(&row.language, &request.path)
            {
                return Err(e);
            }
        }
        let (files, possibly_stale) = lookup(ctx, project, &config, |snapshot| {
            let paths: Vec<(String, String)> = snapshot
                .files()?
                .into_iter()
                .filter(|row| FileId::from_canonical(row.path.as_str()).is_within(&target))
                .filter(|row| row.status == FileStatus::Indexed)
                .map(|row| (row.path, row.language))
                .collect();
            let mut files = Vec::new();
            for (path, language) in &paths {
                let hits = snapshot.symbols_in(path)?;
                if hits.is_empty() {
                    continue;
                }
                files.push((path.clone(), language.clone(), hits));
            }
            let returned = paths.into_iter().map(|(p, _)| p).collect();
            Ok((files, returned))
        })?;
        let files = files
            .into_iter()
            .map(|(path, language, hits)| {
                let id = FileId::from_canonical(path);
                OutlineFile {
                    path: ctx.location(&id, 1, 1).path,
                    root_path: id,
                    language,
                    symbols: tree(ctx, hits),
                }
            })
            .collect();
        Ok(OutlineResponse {
            files,
            possibly_stale,
        })
    }
}

/// Symbols by parent ID, each with its position in source order.
type Children = BTreeMap<Option<String>, Vec<(usize, SymbolHit)>>;

/// Nests symbols under their parents, keeping source order.
fn tree(ctx: &Context, hits: Vec<SymbolHit>) -> Vec<OutlineNode> {
    let mut children = Children::new();
    for (i, hit) in hits.into_iter().enumerate() {
        children
            .entry(hit.symbol.parent.clone())
            .or_default()
            .push((i, hit));
    }
    level(ctx, None, &mut children)
}

fn level(ctx: &Context, parent: Option<&str>, children: &mut Children) -> Vec<OutlineNode> {
    let Some(mut nodes) = children.remove(&parent.map(str::to_owned)) else {
        return Vec::new();
    };
    nodes.sort_by_key(|(i, _)| *i);
    nodes
        .into_iter()
        .map(|(_, hit)| {
            let id = hit.symbol.symbol_id.clone();
            OutlineNode {
                symbol: SymbolResult::new(ctx, hit),
                children: level(ctx, Some(&id), children),
            }
        })
        .collect()
}

/// `show`: a symbol's source, optionally with lines around it.
pub struct ShowOp;

/// Request for [`ShowOp`].
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ShowRequest {
    /// Symbol ID, its path part, `file:line:col`, or a name.
    pub symbol: String,
    /// Consider symbols of this kind only; repeatable.
    #[serde(default, rename = "kind")]
    pub kinds: Vec<String>,
    /// Directories to consider, narrowing an ambiguous name.
    #[serde(flatten)]
    pub scope: ScopeRequest,
    /// Lines shown before and after the symbol.
    #[serde(default)]
    pub context: usize,
}

/// What [`ShowOp`] answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ShowResponse {
    /// The symbol.
    pub symbol: SymbolResult,
    /// 1-based number of the first line in `lines`.
    pub first_line: u32,
    /// The symbol's lines, with the context asked for.
    pub lines: Vec<String>,
    /// See [`FindResponse::possibly_stale`].
    pub possibly_stale: bool,
}

impl Outcome for ShowResponse {
    fn has_results(&self) -> bool {
        true
    }
}

impl Operation for ShowOp {
    const NAME: &'static str = "show";
    type Request = ShowRequest;
    type Response = ShowResponse;

    fn run(&self, ctx: &Context, request: ShowRequest) -> Result<ShowResponse, Error> {
        let (project, config) = indexed(ctx)?;
        let scope = ctx.scope(&request.scope)?;
        let (hit, possibly_stale) = lookup(ctx, project, &config, |snapshot| {
            let hit = resolve_where(snapshot, project, ctx.cwd(), &request.symbol, &|hit| {
                (request.kinds.is_empty() || request.kinds.contains(&hit.symbol.kind))
                    && scope.contains(&FileId::from_canonical(hit.path.as_str()))
            })?;
            let path = hit.path.clone();
            Ok((hit, vec![path]))
        })?;
        let path = project.path_of(&FileId::from_canonical(hit.path.as_str()));
        let text = fs::read_to_string(&path).map_err(|source| Error::Io { path, source })?;
        let all: Vec<&str> = text.lines().collect();
        let range = hit.symbol.range;
        let first = (range.start.line as usize)
            .saturating_sub(request.context)
            .max(1);
        // A range ending at column 1 stops before that line.
        let end_line = if range.end.column == 1 && range.end.line > range.start.line {
            range.end.line - 1
        } else {
            range.end.line
        } as usize;
        let last = (end_line + request.context).min(all.len());
        let lines = all
            .get(first - 1..last)
            .unwrap_or_default()
            .iter()
            .map(|l| (*l).to_owned())
            .collect();
        Ok(ShowResponse {
            symbol: SymbolResult::new(ctx, hit),
            first_line: u32::try_from(first).unwrap_or(u32::MAX),
            lines,
            possibly_stale,
        })
    }
}

/// `grep`: a regular expression over the indexed files, with the enclosing
/// symbol of each match.
pub struct GrepOp;

/// Request for [`GrepOp`].
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GrepRequest {
    /// Regular expression, matched line by line.
    pub pattern: String,
    /// Search files of this language; repeatable.
    #[serde(default, rename = "lang")]
    pub languages: Vec<String>,
    /// Keep matches inside a symbol of this kind; repeatable.
    #[serde(default, rename = "kind")]
    pub kinds: Vec<String>,
    /// Directories to search.
    #[serde(flatten)]
    pub scope: ScopeRequest,
    /// Page to return.
    #[serde(flatten)]
    pub page: Page,
}

/// One match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GrepMatch {
    /// Where the match starts.
    #[serde(flatten)]
    pub location: Location,
    /// The line, cut to 200 characters.
    pub text: String,
    /// Qualified name of the innermost symbol around the match.
    pub enclosing: Option<String>,
    /// Kind of that symbol.
    pub enclosing_kind: Option<String>,
}

impl Ranked for GrepMatch {
    fn is_definition(&self) -> bool {
        false
    }

    fn location(&self) -> &Location {
        &self.location
    }
}

/// What [`GrepOp`] answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GrepResponse {
    /// Matches, by path and position.
    pub matches: Paged<GrepMatch>,
    /// Whether another process held the write lock, so the scan before the
    /// search could not run.
    pub possibly_stale: bool,
}

impl Outcome for GrepResponse {
    fn has_results(&self) -> bool {
        self.matches.has_results()
    }
}

impl Operation for GrepOp {
    const NAME: &'static str = "grep";
    type Request = GrepRequest;
    type Response = GrepResponse;

    fn run(&self, ctx: &Context, request: GrepRequest) -> Result<GrepResponse, Error> {
        let (project, config) = indexed(ctx)?;
        let regex = Regex::new(&request.pattern).map_err(|e| Error::Usage {
            cause: format!(
                "invalid regular expression: {}",
                e.to_string().lines().last().unwrap_or_default()
            ),
            hint: "grep takes Rust regex syntax; escape special characters with \\".to_owned(),
        })?;
        let scope = ctx.scope(&request.scope)?;
        // A daemon whose watcher covers every directory has the index
        // current once its pending changes are in; otherwise the whole tree
        // is scanned, as without a daemon.
        let watched = ctx.watch().is_some_and(crate::daemon::Watch::flush);
        let possibly_stale = !watched
            && scan_within(
                project,
                &config,
                &PackIndexer::new(&config),
                ctx.gate(),
                Some(QUERY_WAIT),
            )?
            .is_none();
        let mut store = Store::open(&project.index_path())?;
        let snapshot = store.snapshot()?;
        let mut matches = Vec::new();
        for row in snapshot.files()? {
            let id = FileId::from_canonical(row.path.as_str());
            let searchable = matches!(row.status, FileStatus::Indexed | FileStatus::SearchOnly);
            if !searchable
                || !scope.contains(&id)
                || !(request.languages.is_empty() || request.languages.contains(&row.language))
            {
                continue;
            }
            let Ok(text) = fs::read_to_string(project.path_of(&id)) else {
                continue;
            };
            let found = search(ctx, &id, &text, &regex);
            if found.is_empty() {
                continue;
            }
            let symbols = snapshot.symbols_in(&row.path)?;
            for (mut m, byte) in found {
                if let Some(enclosing) = enclosing(&symbols, byte) {
                    m.enclosing = Some(enclosing.symbol.qualified_name.clone());
                    m.enclosing_kind = Some(enclosing.symbol.kind.clone());
                }
                if request.kinds.is_empty()
                    || m.enclosing_kind
                        .as_ref()
                        .is_some_and(|k| request.kinds.contains(k))
                {
                    matches.push(m);
                }
            }
        }
        sort_results(&mut matches, None);
        Ok(GrepResponse {
            matches: request.page.apply(matches),
            possibly_stale,
        })
    }
}

/// Every match in `text`, with its byte offset.
fn search(ctx: &Context, id: &FileId, text: &str, regex: &Regex) -> Vec<(GrepMatch, usize)> {
    let mut found = Vec::new();
    let mut offset = 0;
    for (number, line) in text.split_inclusive('\n').enumerate() {
        let content = line.trim_end_matches(['\n', '\r']);
        for m in regex.find_iter(content) {
            let column = content[..m.start()].chars().count() + 1;
            let shown: String = content.chars().take(MAX_LINE).collect();
            found.push((
                GrepMatch {
                    location: ctx.location(
                        id,
                        u32::try_from(number + 1).unwrap_or(u32::MAX),
                        u32::try_from(column).unwrap_or(u32::MAX),
                    ),
                    text: shown,
                    enclosing: None,
                    enclosing_kind: None,
                },
                offset + m.start(),
            ));
        }
        offset += line.len();
    }
    found
}

/// The innermost symbol whose range holds `byte`.
fn enclosing(symbols: &[SymbolHit], byte: usize) -> Option<&SymbolHit> {
    symbols
        .iter()
        .filter(|hit| {
            let r = hit.symbol.range;
            (r.start.byte as usize) <= byte && byte < r.end.byte as usize
        })
        .min_by_key(|hit| hit.symbol.range.end.byte - hit.symbol.range.start.byte)
}
