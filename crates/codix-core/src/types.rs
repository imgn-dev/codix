//! `types`: the type hierarchy, from the supertypes each type writes.
//!
//! A supertype's name binds to a type of its file by scope (`scoped`), to
//! one of another file by a resolver (`bound`), or, when nothing binds it,
//! to every type of that name (`textual`).

use std::collections::{HashSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::op::{Context, Operation, Outcome};
use crate::page::{Location, Page, Paged, Ranked, sort_results};
use crate::project::{FileId, ScopeRequest};
use crate::query::{Renamed, SymbolResult, complete, following, indexed};
use crate::refs::{Call, CallReceiver, DefAt};
use crate::store::{
    MatchMode, NameQuery, Snapshot, Store, Supertype, SupertypeBinding, SymbolHit, simple_name,
};
use crate::symbol_id::{SymbolRef, resolve_all_where};

/// The kinds that can have subtypes and supertypes.
pub const TYPE_KINDS: &[&str] = &["class", "struct", "interface", "trait", "enum"];

/// The kinds a call can mean.
const CALLABLE_KINDS: &[&str] = &["method", "function", "constructor", "declaration"];

/// `types`: the subtypes of a type, or its supertypes.
pub struct TypesOp;

/// Request for [`TypesOp`].
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TypesRequest {
    /// Type ID, its path part, `file:line:col`, or a name.
    pub symbol: String,
    /// List its supertypes, up to the roots, instead of what extends or implements it.
    #[serde(default, rename = "super", skip_serializing_if = "std::ops::Not::not")]
    pub supertypes: bool,
    /// Levels followed, 1 for the direct ones only.
    #[serde(default)]
    pub depth: Option<usize>,
    /// Consider types of this kind only; repeatable.
    #[serde(default, rename = "kind")]
    pub kinds: Vec<String>,
    /// Directories to keep.
    #[serde(flatten)]
    pub scope: ScopeRequest,
    /// Page to return.
    #[serde(flatten)]
    pub page: Page,
}

/// A type in the hierarchy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TypeResult {
    /// Where its name is.
    #[serde(flatten)]
    pub location: Location,
    /// Symbol ID.
    pub symbol_id: String,
    /// Kind from the shared vocabulary.
    pub kind: String,
    /// Qualified name within the file.
    pub qualified_name: String,
    /// Levels from the type asked about: 1 for a direct subtype or supertype.
    pub depth: usize,
    /// The type it is reached from, one level closer: the one it extends or
    /// implements, as written when only the name matched; for supertypes,
    /// the one extending it.
    pub via: String,
    /// How that relation was found: `scoped` when the name written binds to
    /// it in its file, `bound` when a resolver bound it, `textual` when only
    /// the name matched.
    pub confidence: String,
}

impl Ranked for TypeResult {
    fn is_definition(&self) -> bool {
        false
    }

    fn location(&self) -> &Location {
        &self.location
    }
}

/// What [`TypesOp`] answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TypesResponse {
    /// Every type the input names.
    pub targets: Vec<SymbolResult>,
    /// Whether these are supertypes rather than subtypes.
    #[serde(rename = "super")]
    pub supertypes: bool,
    /// The types found, by path and position.
    pub types: Paged<TypeResult>,
    /// Relations bound by scope or by a resolver, before paging.
    pub bound: usize,
    /// Relations matched by name only, before paging.
    pub textual: usize,
    /// Supertypes named but not in the project, such as a library's.
    pub outside: Vec<String>,
    /// The next page, when results remain: the request with these fields
    /// changed.
    pub next: Option<TypesNext>,
    /// See [`crate::query::FindResponse::possibly_stale`].
    pub possibly_stale: bool,
    /// When the ID given was the type's before codix renamed it.
    #[serde(default)]
    pub renamed: Option<Renamed>,
}

/// What changes in a [`TypesRequest`] for its next page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TypesNext {
    /// Where the next page starts.
    pub after: String,
}

impl Outcome for TypesResponse {
    fn has_results(&self) -> bool {
        self.types.has_results() || !self.outside.is_empty()
    }
}

const SCOPED: &str = "scoped";
const BOUND: &str = "bound";
const TEXTUAL: &str = "textual";

impl Operation for TypesOp {
    const NAME: &'static str = "types";
    type Request = TypesRequest;
    type Response = TypesResponse;

    fn run(&self, ctx: &Context, request: TypesRequest) -> Result<TypesResponse, Error> {
        let (project, config) = indexed(ctx)?;
        let scope = ctx.scope(&request.scope)?;
        let possibly_stale = complete(ctx, project, &config)?;
        let mut store = Store::open(&project.index_path())?;
        let snapshot = store.snapshot()?;
        let is_type = |hit: &SymbolHit| {
            TYPE_KINDS.contains(&hit.symbol.kind.as_str())
                && (request.kinds.is_empty() || request.kinds.contains(&hit.symbol.kind))
        };
        let (targets, renamed) = following(&request.symbol, |input| {
            let input = match SymbolRef::parse(input)? {
                SymbolRef::Location { file, line, column } => {
                    match crate::refs::def_at(ctx, project, &config, &file, line, column)? {
                        Some(DefAt::Found(found)) => found[0].symbol_id.clone(),
                        Some(DefAt::ByName(name, _)) => name,
                        None => input.to_owned(),
                    }
                }
                _ => input.to_owned(),
            };
            resolve_all_where(&snapshot, project, ctx.cwd(), &input, &is_type)
        })?;
        let walk = Walk {
            snapshot: &snapshot,
            depth: request.depth.unwrap_or(usize::MAX),
        };
        let (found, outside) = if request.supertypes {
            walk.up(&targets)?
        } else {
            (walk.down(&targets)?, Vec::new())
        };
        let mut results: Vec<TypeResult> = found
            .into_iter()
            .map(|(hit, depth, via, confidence)| {
                let start = hit.symbol.name_range.start;
                TypeResult {
                    location: ctx.location(
                        &FileId::from_canonical(hit.path.as_str()),
                        start.line,
                        start.column,
                    ),
                    symbol_id: hit.symbol.symbol_id,
                    kind: hit.symbol.kind,
                    qualified_name: hit.symbol.qualified_name,
                    depth,
                    via,
                    confidence: confidence.to_owned(),
                }
            })
            .filter(|r| scope.contains(&r.location.root_path))
            .collect();
        sort_results(&mut results, None);
        let textual = results.iter().filter(|r| r.confidence == TEXTUAL).count();
        let bound = results.len() - textual;
        let (types, after) = request.page.apply(results, None);
        Ok(TypesResponse {
            targets: targets
                .into_iter()
                .map(|hit| SymbolResult::new(ctx, hit))
                .collect(),
            supertypes: request.supertypes,
            types,
            bound,
            textual,
            outside,
            next: after.map(|after| TypesNext { after }),
            possibly_stale,
            renamed,
        })
    }
}

/// Orders the symbols a call's name matches: first those that fit the call,
/// methods of the types it may be made on, closest first, that take as many
/// arguments; then the others, as they came. Returns how many fit. The types
/// it may be made on are those around it, or the receiver's when codix knows
/// them, and their supertypes; when neither is known, any type is.
pub(crate) fn rank_for_call(
    snapshot: &Snapshot<'_>,
    hits: Vec<SymbolHit>,
    call: &Call,
) -> Result<(Vec<SymbolHit>, usize), Error> {
    let owners = owners(snapshot, call)?;
    let rank = |hit: &SymbolHit| -> Option<usize> {
        if !CALLABLE_KINDS.contains(&hit.symbol.kind.as_str()) {
            return None;
        }
        let owner = match &owners {
            Some(owners) => owners
                .iter()
                .position(|o| hit.symbol.parent.as_deref() == Some(o.as_str()))?,
            None => 0,
        };
        let fits = call.arity.is_none_or(|arity| {
            crate::symbol_id::fits_arity(&hit.symbol.name, hit.symbol.signature.as_deref(), arity)
                != Some(false)
        });
        fits.then_some(owner)
    };
    let mut ranked: Vec<(usize, SymbolHit)> = hits
        .into_iter()
        .map(|hit| (rank(&hit).unwrap_or(usize::MAX), hit))
        .collect();
    // Stable: the fitting ones by type, closest first; the others as they came.
    ranked.sort_by_key(|(rank, _)| *rank);
    let fitting: Vec<(usize, SymbolHit)> = ranked
        .iter()
        .take_while(|(rank, _)| *rank != usize::MAX)
        .cloned()
        .collect();
    let others: Vec<(usize, SymbolHit)> = ranked.into_iter().skip(fitting.len()).collect();
    let count = fitting.len();
    Ok((
        fitting
            .into_iter()
            .chain(others)
            .map(|(_, hit)| hit)
            .collect(),
        count,
    ))
}

/// The IDs of the types a call may be made on, closest first: the types
/// it starts from and their supertypes; `None` when codix cannot tell.
fn owners(snapshot: &Snapshot<'_>, call: &Call) -> Result<Option<Vec<String>>, Error> {
    let walk = Walk {
        snapshot,
        depth: usize::MAX,
    };
    let by_id = |ids: &[String]| -> Result<Vec<SymbolHit>, Error> {
        let mut found = Vec::new();
        for id in ids {
            found.extend(walk.by_id(id)?);
        }
        Ok(found)
    };
    let starts = match &call.receiver {
        CallReceiver::None | CallReceiver::Own => by_id(&call.enclosing)?,
        CallReceiver::Other {
            text: Some(text), ..
        } if text == "super" => {
            let own = by_id(&call.enclosing[..call.enclosing.len().min(1)])?;
            walk.up(&own)?.0.into_iter().map(|f| f.0).collect()
        }
        CallReceiver::Other {
            declared_type: Some(declared),
            ..
        } => walk.by_name(simple_name(declared))?,
        // A receiver that names a type: a static call.
        CallReceiver::Other {
            text: Some(text), ..
        } => walk.by_name(simple_name(text))?,
        CallReceiver::Other { .. } => Vec::new(),
    };
    if starts.is_empty() {
        return Ok(None);
    }
    let supertypes = walk.up(&starts)?.0;
    Ok(Some(
        starts
            .into_iter()
            .chain(supertypes.into_iter().map(|f| f.0))
            .map(|hit| hit.symbol.symbol_id)
            .collect(),
    ))
}

/// A type found: the type, its depth, the type it is reached from, and how.
type Found = (SymbolHit, usize, String, &'static str);

/// A breadth-first walk of the hierarchy, each type once.
struct Walk<'a> {
    snapshot: &'a Snapshot<'a>,
    depth: usize,
}

impl Walk<'_> {
    /// What extends or implements the targets, transitively.
    fn down(&self, targets: &[SymbolHit]) -> Result<Vec<Found>, Error> {
        let mut seen: HashSet<String> =
            targets.iter().map(|t| t.symbol.symbol_id.clone()).collect();
        let mut queue: VecDeque<(SymbolHit, usize)> =
            targets.iter().map(|t| (t.clone(), 0)).collect();
        let mut found = Vec::new();
        while let Some((parent, depth)) = queue.pop_front() {
            if depth >= self.depth {
                continue;
            }
            for Supertype {
                subtype,
                name,
                binding,
            } in self.snapshot.supertypes_named(&parent.symbol.name)?
            {
                let confidence = match &binding {
                    SupertypeBinding::Scoped(id) if *id == parent.symbol.symbol_id => SCOPED,
                    SupertypeBinding::Bound(id) if *id == parent.symbol.symbol_id => BOUND,
                    SupertypeBinding::Unbound => TEXTUAL,
                    // Bound to another type of the name, or outside.
                    _ => continue,
                };
                if !seen.insert(subtype.symbol.symbol_id.clone()) {
                    continue;
                }
                // A name alone may mean any type of that name: it is shown
                // as written.
                let via = if confidence == TEXTUAL {
                    name
                } else {
                    parent.symbol.qualified_name.clone()
                };
                found.push((subtype.clone(), depth + 1, via, confidence));
                queue.push_back((subtype, depth + 1));
            }
        }
        Ok(found)
    }

    /// The supertypes of the targets, up to the roots, and the names of
    /// those outside the project.
    fn up(&self, targets: &[SymbolHit]) -> Result<(Vec<Found>, Vec<String>), Error> {
        let mut seen: HashSet<String> =
            targets.iter().map(|t| t.symbol.symbol_id.clone()).collect();
        let mut queue: VecDeque<(SymbolHit, usize)> =
            targets.iter().map(|t| (t.clone(), 0)).collect();
        let mut found = Vec::new();
        let mut outside: Vec<String> = Vec::new();
        while let Some((child, depth)) = queue.pop_front() {
            if depth >= self.depth {
                continue;
            }
            for Supertype { name, binding, .. } in
                self.snapshot.supertypes_of(&child.symbol.symbol_id)?
            {
                let (candidates, confidence) = match binding {
                    SupertypeBinding::Scoped(id) => (self.by_id(&id)?, SCOPED),
                    SupertypeBinding::Bound(id) => (self.by_id(&id)?, BOUND),
                    SupertypeBinding::External => (Vec::new(), BOUND),
                    SupertypeBinding::Unbound => (self.by_name(simple_name(&name))?, TEXTUAL),
                };
                if candidates.is_empty() {
                    if !outside.contains(&name) {
                        outside.push(name);
                    }
                    continue;
                }
                for supertype in candidates {
                    if !seen.insert(supertype.symbol.symbol_id.clone()) {
                        continue;
                    }
                    found.push((
                        supertype.clone(),
                        depth + 1,
                        child.symbol.qualified_name.clone(),
                        confidence,
                    ));
                    queue.push_back((supertype, depth + 1));
                }
            }
        }
        Ok((found, outside))
    }

    fn by_id(&self, id: &str) -> Result<Vec<SymbolHit>, Error> {
        Ok(self
            .snapshot
            .symbols_with_id_prefix(id)?
            .into_iter()
            .filter(|hit| hit.symbol.symbol_id == id)
            .collect())
    }

    /// The types named `name`, whatever `--kind` keeps: it narrows the
    /// types asked about only.
    fn by_name(&self, name: &str) -> Result<Vec<SymbolHit>, Error> {
        Ok(self
            .snapshot
            .find(&NameQuery {
                text: name.to_owned(),
                mode: MatchMode::Exact,
                case_sensitive: true,
            })?
            .into_iter()
            .filter(|hit| TYPE_KINDS.contains(&hit.symbol.kind.as_str()))
            .collect())
    }
}
