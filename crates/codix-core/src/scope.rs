//! Tier 2: a file's scopes, its definitions and references, and what each
//! reference binds to, from the pack's `locals` query.
//!
//! The query's captures follow tree-sitter's names, with codix suffixes:
//!
//! - `@local.scope.<kind>` marks a scope, such as `class` or `block`;
//!   `(#set! local.opaque)` marks one where a name not found may be a member
//!   codix cannot see, such as an inherited one, and
//!   `(#set! local.opaque "members")` one where such a member can hide
//!   symbols but not locals, as with a Kotlin lambda's receiver. A name
//!   found nowhere in the file after crossing an opaque scope is uncertain.
//!   `(#set! local.transparent)` marks a scope whose symbols belong to the
//!   scope around it, as a C++ template's do.
//! - `@local.definition.<kind>` marks the name of a local definition, such
//!   as a `parameter` or `variable`. `(#set! local.reuse)` makes it a write
//!   to an earlier definition of the name in the same scope, if there is
//!   one, as Go's `:=` does; `(#set! local.reuse "visible")`, to any
//!   definition visible from there, as a Ruby assignment in a block does. `@local.after` in the same match makes the
//!   definition visible only after that node, as in Go's `x := x + 1`.
//!   `(#set! local.scope "<kind>")` defines it in the nearest enclosing
//!   scope of that kind rather than the innermost one, as JavaScript's `var`
//!   is defined in its function. The symbols of the `tags` query are
//!   definitions too.
//! - `@local.global` and `@local.nonlocal` mark a name a scope declares as
//!   the file's or an enclosing function's, as Python's `global` and
//!   `nonlocal` do: assignments to it there are writes to that one.
//! - `@local.reference.<context>` marks a reference, `@local.receiver` the
//!   object of a member access in the same match, `@local.arguments` the
//!   arguments of a call in the same match, and `@local.skip` a name that is
//!   no reference.
//! - `@local.type`, in a local definition's match, marks its declared type.
//!
//! When several patterns capture the same node as a reference, the one
//! written last wins; a definition always wins over a reference.
//!
//! Binding never guesses: a reference is bound to a definition of the file,
//! free when the file has none, or uncertain when an opaque scope lies
//! between it and the definition its name would otherwise reach.

use std::collections::{HashMap, HashSet};

use codix_provider::{Grammar, LanguageDescriptor};
use serde::{Deserialize, Serialize};
use streaming_iterator::StreamingIterator;
use tree_sitter::{Node, QueryCursor, Tree};

use crate::Error;
use crate::parse::{self, span};
use crate::store::{Confidence, ImportRow, RefRow, RefState, Span, SymbolRow};

/// The kind of the scope covering a whole file.
pub const FILE_SCOPE: &str = "file";

/// The kind of scope a self receiver such as `this` refers to the members of.
pub const CLASS_SCOPE: &str = "class";

/// The kind of scope of a function, method or lambda body.
pub const FUNCTION_SCOPE: &str = "function";

/// What a scope may hide that codix cannot see.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Opacity {
    /// Nothing: every name in scope here is in the file.
    Clear,
    /// Members that can hide symbols of the file, not its locals.
    Members,
    /// Members that can hide any name.
    All,
}

/// Where a reference appears.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum RefContext {
    /// A call.
    Call,
    /// A type.
    Type,
    /// An import.
    Import,
    /// A read.
    Read,
    /// A write.
    Write,
}

impl RefContext {
    /// The context a capture suffix names.
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "call" => Self::Call,
            "type" => Self::Type,
            "import" => Self::Import,
            "read" => Self::Read,
            "write" => Self::Write,
            _ => return None,
        })
    }

    /// The name used in queries, output and the index.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Call => "call",
            Self::Type => "type",
            Self::Import => "import",
            Self::Read => "read",
            Self::Write => "write",
        }
    }
}

/// A lexical scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeNode {
    /// Kind, from the capture suffix; [`FILE_SCOPE`] for the whole file.
    pub kind: String,
    /// First byte.
    pub start: usize,
    /// Byte after the last.
    pub end: usize,
    /// Enclosing scope; `None` for the file scope only.
    pub parent: Option<usize>,
    /// What members codix cannot see may hide here.
    pub opacity: Opacity,
    /// Whether symbols defined directly in it belong to its parent.
    pub transparent: bool,
}

/// What a definition defines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Defines {
    /// A symbol of the file, by index into [`FileModel::symbols`].
    Symbol(usize),
    /// A local, such as a parameter or a variable, which has no symbol ID.
    Local {
        /// Kind, from the capture suffix.
        kind: String,
    },
}

/// A definition of a name in a scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Definition {
    /// The name.
    pub name: String,
    /// What it defines.
    pub defines: Defines,
    /// Its namespace.
    pub namespace: String,
    /// The scope it is defined in.
    pub scope: usize,
    /// Where the name is.
    pub name_range: Span,
    /// First byte from which references see it, in scopes that are not
    /// hoisted.
    pub visible_from: usize,
    /// In or around a parse error.
    pub low: bool,
    /// The declared type, as written without type arguments, when captured.
    pub declared_type: Option<String>,
}

/// What a member access goes through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Receiver {
    /// No receiver: a plain name.
    None,
    /// A self receiver, such as `this`.
    Own,
    /// Any other object.
    Other,
}

/// What a reference binds to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Binding {
    /// A definition of the file, by index into [`FileModel::definitions`];
    /// for overloads, the first of the set.
    Bound(usize),
    /// No definition in the file: another file, a builtin, or a member of
    /// another object.
    Free,
    /// An opaque scope may hide the definition the name means.
    Uncertain,
}

/// A reference to a name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    /// The name.
    pub name: String,
    /// Its context.
    pub context: RefContext,
    /// Where it is.
    pub range: Span,
    /// The scope it appears in.
    pub scope: usize,
    /// What it is reached through.
    pub receiver: Receiver,
    /// The receiver as written when a name or a qualified name, `…` when
    /// another expression.
    pub receiver_text: Option<String>,
    /// The receiver's first and last byte.
    pub receiver_range: Option<(usize, usize)>,
    /// The receiver's declared type, when the file declares it.
    pub receiver_type: Option<String>,
    /// A call's argument count.
    pub arity: Option<u32>,
    /// What it binds to.
    pub binding: Binding,
    /// In or around a parse error.
    pub low: bool,
}

/// A file's symbols, scopes, definitions and bound references.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileModel {
    /// The symbols of the `tags` query.
    pub symbols: Vec<SymbolRow>,
    /// Scopes, the file scope first, each after its parent.
    pub scopes: Vec<ScopeNode>,
    /// Definitions, symbols and locals, in file order.
    pub definitions: Vec<Definition>,
    /// References, in file order.
    pub references: Vec<Reference>,
    /// The package, imports and includes of the `tags` query.
    pub imports: Vec<ImportRow>,
}

impl FileModel {
    /// The definitions of the overload set `definition` belongs to: those
    /// with its name and namespace in its scope, in file order.
    pub fn overloads(&self, definition: usize) -> Vec<usize> {
        let d = &self.definitions[definition];
        self.definitions
            .iter()
            .enumerate()
            .filter(|(_, o)| o.scope == d.scope && o.name == d.name && o.namespace == d.namespace)
            .map(|(i, _)| i)
            .collect()
    }

    /// The index of the definition whose name is at `byte`, if any.
    pub fn definition_at(&self, byte: usize) -> Option<usize> {
        self.definitions.iter().position(|d| {
            (d.name_range.start.byte as usize) <= byte && byte < d.name_range.end.byte as usize
        })
    }

    /// The index of the reference at `byte`, if any.
    pub fn reference_at(&self, byte: usize) -> Option<usize> {
        self.references
            .iter()
            .position(|r| (r.range.start.byte as usize) <= byte && byte < r.range.end.byte as usize)
    }

    /// The references to store: those not bound to a local, which nothing
    /// outside the file can refer to.
    pub fn stored_refs(&self) -> Vec<RefRow> {
        self.references
            .iter()
            .filter_map(|r| {
                let (state, target) = match r.binding {
                    Binding::Bound(d) => match self.definitions[d].defines {
                        Defines::Symbol(s) => (RefState::Bound, Some(s)),
                        Defines::Local { .. } => return None,
                    },
                    Binding::Free => (RefState::Free, None),
                    Binding::Uncertain => (RefState::Uncertain, None),
                };
                Some(RefRow {
                    name: r.name.clone(),
                    context: r.context,
                    range: r.range,
                    enclosing: self.enclosing_symbol(r.range.start.byte as usize),
                    target,
                    state,
                    confidence: if r.low {
                        Confidence::Low
                    } else {
                        Confidence::Normal
                    },
                    receiver: r.receiver_text.clone(),
                    receiver_type: r.receiver_type.clone(),
                    arity: r.arity,
                })
            })
            .collect()
    }

    /// The innermost symbol whose range holds `byte`.
    pub fn enclosing_symbol(&self, byte: usize) -> Option<usize> {
        self.symbols
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                (s.range.start.byte as usize) <= byte && byte < s.range.end.byte as usize
            })
            .min_by_key(|(_, s)| s.range.end.byte - s.range.start.byte)
            .map(|(i, _)| i)
    }
}

/// Builds the model of one file: parses it, extracts its symbols, and binds
/// its references when the pack has a `locals` query.
pub fn model(
    descriptor: &LanguageDescriptor,
    grammar: &Grammar,
    path: &str,
    content: &str,
) -> Result<FileModel, Error> {
    let tree = parse::parse(descriptor, grammar, path, content)?;
    let (symbols, imports) = parse::symbols_of(descriptor, grammar, path, content, &tree)?;
    let mut model = if let Some(query) = grammar
        .locals_query()
        .map_err(parse::unusable(&descriptor.name))?
    {
        Builder::new(descriptor, content, symbols).build(query, &tree)
    } else {
        // Tier 1: the symbols are definitions of the file scope, and no
        // reference is known.
        let mut builder = Builder::new(descriptor, content, symbols);
        builder.scopes(Vec::new());
        builder.definitions(Vec::new(), &mut HashMap::new(), &tree);
        builder.model
    };
    model.imports = imports;
    Ok(model)
}

/// A local definition captured by the query.
struct LocalCapture<'tree> {
    node: Node<'tree>,
    start: usize,
    end: usize,
    span: Span,
    kind: String,
    reuse: Option<Reuse>,
    after: Option<usize>,
    /// The kind of enclosing scope to define it in, if not the innermost.
    place_in: Option<String>,
    declared_type: Option<String>,
}

/// Which earlier definition a definition may write instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reuse {
    /// One in the same scope.
    SameScope,
    /// Any one visible from there.
    Visible,
}

/// A reference captured by the query, with the pattern that captured it.
struct RefCapture<'tree> {
    node: Node<'tree>,
    start: usize,
    end: usize,
    span: Span,
    context: Option<RefContext>,
    via: Via,
    pattern: usize,
}

/// What a match says a reference goes through, and its arguments.
#[derive(Debug, Clone)]
struct Via {
    receiver: Receiver,
    /// The receiver as written, when a name or a qualified name.
    text: Option<String>,
    /// The receiver's first and last byte.
    range: Option<(usize, usize)>,
    arity: Option<u32>,
}

impl Via {
    const NONE: Self = Self {
        receiver: Receiver::None,
        text: None,
        range: None,
        arity: None,
    };
}

/// What one match tells about its captures.
struct Pattern<'q> {
    index: usize,
    settings: &'q [tree_sitter::QueryProperty],
    via: Via,
    after: Option<usize>,
    declared_type: Option<String>,
}

impl Pattern<'_> {
    fn set(&self, key: &str) -> bool {
        self.settings.iter().any(|p| &*p.key == key)
    }

    fn value(&self, key: &str) -> Option<&str> {
        self.settings
            .iter()
            .find(|p| &*p.key == key)
            .and_then(|p| p.value.as_deref())
    }
}

/// Everything the query captured in a file.
#[derive(Default)]
struct Captures<'tree> {
    scopes: Vec<ScopeNode>,
    locals: Vec<LocalCapture<'tree>>,
    refs: HashMap<(usize, usize), RefCapture<'tree>>,
    declarations: Vec<(Node<'tree>, Declared)>,
}

impl<'tree> RefCapture<'tree> {
    fn new(
        text: &str,
        node: Node<'tree>,
        context: Option<RefContext>,
        via: Via,
        pattern: usize,
    ) -> Self {
        Self {
            node,
            start: node.start_byte(),
            end: node.end_byte(),
            span: span(text, node),
            context,
            via,
            pattern,
        }
    }
}

struct Builder<'a> {
    descriptor: &'a LanguageDescriptor,
    content: &'a str,
    model: FileModel,
    /// Definitions by scope, name and namespace, in file order.
    by_name: HashMap<(usize, String, String), Vec<usize>>,
    /// Names scopes declare as the file's (`global`) or an enclosing
    /// function's (`nonlocal`), by scope and name.
    declared: HashMap<(usize, String), Declared>,
}

/// Where a `global` or `nonlocal` declaration sends a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Declared {
    Global,
    Nonlocal,
}

impl<'a> Builder<'a> {
    fn new(descriptor: &'a LanguageDescriptor, content: &'a str, symbols: Vec<SymbolRow>) -> Self {
        Self {
            descriptor,
            content,
            model: FileModel {
                symbols,
                ..FileModel::default()
            },
            by_name: HashMap::new(),
            declared: HashMap::new(),
        }
    }

    fn build(mut self, query: &tree_sitter::Query, tree: &Tree) -> FileModel {
        let names = query.capture_names();
        let mut captures = Captures::default();
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(query, tree.root_node(), self.content.as_bytes());
        while let Some(m) = matches.next() {
            let settings = query.property_settings(m.pattern_index);
            let find = |name: &str| {
                m.captures()
                    .iter()
                    .find(|c| names[c.index as usize] == name)
                    .map(|c| c.node)
            };
            let receiver_node = find("local.receiver");
            let receiver = receiver_node.map_or(Receiver::None, |node| {
                let text = node_text(self.content, node);
                if self
                    .descriptor
                    .scopes
                    .self_receivers
                    .iter()
                    .any(|s| s == text)
                {
                    Receiver::Own
                } else {
                    Receiver::Other
                }
            });
            let pattern = Pattern {
                index: m.pattern_index,
                settings,
                via: Via {
                    receiver,
                    // Any other expression is written `…`: a receiver all
                    // the same.
                    text: receiver_node.map(|n| {
                        written_name(node_text(self.content, n)).unwrap_or_else(|| "…".to_owned())
                    }),
                    range: receiver_node.map(|n| (n.start_byte(), n.end_byte())),
                    arity: find("local.arguments").map(arguments),
                },
                after: find("local.after").map(|node| node.end_byte()),
                declared_type: find("local.type")
                    .map(|node| parse::written_type(node_text(self.content, node)))
                    // A type keyword, such as Java's `var`, leaves it to inference.
                    .filter(|t| !self.descriptor.names.type_keywords.contains(t)),
            };
            for capture in m.captures() {
                self.collect(
                    names[capture.index as usize],
                    capture.node,
                    &pattern,
                    &mut captures,
                );
            }
        }
        self.scopes(captures.scopes);
        for (node, declared) in captures.declarations {
            let scope = self.scope_of(node.start_byte(), node.end_byte(), false);
            self.declared
                .insert((scope, node_text(self.content, node).to_owned()), declared);
        }
        self.definitions(captures.locals, &mut captures.refs, tree);
        self.references(captures.refs);
        self.bind();
        self.model
    }

    /// Records one capture of a match.
    fn collect<'tree>(
        &self,
        name: &str,
        node: Node<'tree>,
        pattern: &Pattern<'_>,
        captures: &mut Captures<'tree>,
    ) {
        if let Some(kind) = name.strip_prefix("local.scope.") {
            captures.scopes.push(ScopeNode {
                kind: kind.to_owned(),
                start: node.start_byte(),
                end: node.end_byte(),
                parent: None,
                opacity: match pattern.value("local.opaque") {
                    Some("members") => Opacity::Members,
                    _ if pattern.set("local.opaque") => Opacity::All,
                    _ => Opacity::Clear,
                },
                transparent: pattern.set("local.transparent"),
            });
        } else if let Some(kind) = name.strip_prefix("local.definition.") {
            captures.locals.push(LocalCapture {
                node,
                start: node.start_byte(),
                end: node.end_byte(),
                span: span(self.content, node),
                kind: kind.to_owned(),
                reuse: match pattern.value("local.reuse") {
                    Some("visible") => Some(Reuse::Visible),
                    _ if pattern.set("local.reuse") => Some(Reuse::SameScope),
                    _ => None,
                },
                after: pattern.after,
                place_in: pattern.value("local.scope").map(str::to_owned),
                declared_type: pattern.declared_type.clone(),
            });
        } else if let Some(context) = name.strip_prefix("local.reference.") {
            let capture = RefCapture::new(
                self.content,
                node,
                RefContext::parse(context),
                pattern.via.clone(),
                pattern.index,
            );
            keep_last(&mut captures.refs, capture);
        } else if name == "local.global" || name == "local.nonlocal" {
            let declared = if name == "local.global" {
                Declared::Global
            } else {
                Declared::Nonlocal
            };
            captures.declarations.push((node, declared));
            let capture = RefCapture::new(
                self.content,
                node,
                Some(RefContext::Write),
                Via::NONE,
                usize::MAX,
            );
            keep_last(&mut captures.refs, capture);
        } else if name == "local.skip" {
            let capture = RefCapture::new(self.content, node, None, Via::NONE, pattern.index);
            keep_last(&mut captures.refs, capture);
        }
    }

    /// Orders the scopes outermost first and links each to its parent.
    fn scopes(&mut self, mut captured: Vec<ScopeNode>) {
        captured.sort_by_key(|s| (s.start, std::cmp::Reverse(s.end)));
        captured.dedup_by(|b, a| {
            let same = (a.start, a.end) == (b.start, b.end);
            if same {
                a.opacity = a.opacity.max(b.opacity);
                a.transparent |= b.transparent;
            }
            same
        });
        let mut scopes = vec![ScopeNode {
            kind: FILE_SCOPE.to_owned(),
            start: 0,
            end: self.content.len(),
            parent: None,
            opacity: Opacity::Clear,
            transparent: false,
        }];
        let mut open: Vec<usize> = vec![0];
        for mut scope in captured {
            while let Some(&top) = open.last() {
                if top == 0 || (scopes[top].start <= scope.start && scope.end <= scopes[top].end) {
                    break;
                }
                open.pop();
            }
            scope.parent = open.last().copied();
            scopes.push(scope);
            open.push(scopes.len() - 1);
        }
        self.model.scopes = scopes;
    }

    /// The innermost scope holding `start..end`; with `strict`, one that is
    /// not exactly that range, since a definition that is itself a scope,
    /// such as a method, is defined in the scope around it.
    fn scope_of(&self, start: usize, end: usize, strict: bool) -> usize {
        let scopes = &self.model.scopes;
        // Scopes are sorted by start, outer before inner: the innermost one
        // holding the range is the last that starts before it, or one of
        // its ancestors.
        let mut i = scopes
            .partition_point(|s| s.start <= start)
            .saturating_sub(1);
        loop {
            let s = &scopes[i];
            let holds = s.start <= start && end <= s.end;
            let same = (s.start, s.end) == (start, end);
            if i == 0 || (holds && !(strict && same)) {
                return i;
            }
            i = s.parent.unwrap_or(0);
        }
    }

    fn definitions<'tree>(
        &mut self,
        locals: Vec<LocalCapture<'tree>>,
        refs: &mut HashMap<(usize, usize), RefCapture<'tree>>,
        tree: &Tree,
    ) {
        let mut taken: HashSet<(usize, usize)> = HashSet::new();
        let mut definitions = Vec::new();
        self.symbol_definitions(&mut definitions, &mut taken, refs, tree);
        self.local_definitions(&mut definitions, &mut taken, locals, refs);
        self.order(definitions);
    }

    /// The symbols of the `tags` query, as definitions; their names are no
    /// references.
    fn symbol_definitions(
        &mut self,
        definitions: &mut Vec<Definition>,
        taken: &mut HashSet<(usize, usize)>,
        refs: &mut HashMap<(usize, usize), RefCapture<'_>>,
        tree: &Tree,
    ) {
        let scopes = &self.descriptor.scopes;
        for (i, symbol) in self.model.symbols.iter().enumerate() {
            let name = (
                symbol.name_range.start.byte as usize,
                symbol.name_range.end.byte as usize,
            );
            taken.insert(name);
            refs.remove(&name);
            let named_as_member = !scopes.member_name_parents.is_empty()
                && tree
                    .root_node()
                    .descendant_for_byte_range(name.0, name.1)
                    .and_then(|n| n.parent())
                    .is_some_and(|p| scopes.member_name_parents.iter().any(|k| k == p.kind()));
            let namespace = if named_as_member {
                codix_provider::descriptor::MEMBER_NAMESPACE.to_owned()
            } else {
                scopes.namespace_of(&symbol.kind).to_owned()
            };
            let mut scope = self.scope_of(
                symbol.range.start.byte as usize,
                symbol.range.end.byte as usize,
                true,
            );
            while scope != 0 && self.model.scopes[scope].transparent {
                scope = self.model.scopes[scope].parent.unwrap_or(0);
            }
            self.by_name
                .entry((scope, symbol.name.clone(), namespace.clone()))
                .or_default()
                .push(definitions.len());
            definitions.push(Definition {
                name: symbol.name.clone(),
                defines: Defines::Symbol(i),
                namespace,
                scope,
                name_range: symbol.name_range,
                visible_from: if scopes.symbols_hoisted { 0 } else { name.0 },
                low: symbol.confidence == Confidence::Low,
                declared_type: symbol.declared_type.clone(),
            });
        }
    }

    /// The locals the query captured, or writes where one reuses an earlier
    /// definition.
    fn local_definitions<'tree>(
        &mut self,
        definitions: &mut Vec<Definition>,
        taken: &mut HashSet<(usize, usize)>,
        mut locals: Vec<LocalCapture<'tree>>,
        refs: &mut HashMap<(usize, usize), RefCapture<'tree>>,
    ) {
        let scopes = &self.descriptor.scopes;
        locals.sort_by_key(|l| l.start);
        locals.dedup_by_key(|l| (l.start, l.end));
        for local in locals {
            let range = (local.start, local.end);
            if !taken.insert(range) {
                continue;
            }
            let mut scope = self.scope_of(local.start, local.end, false);
            if let Some(kind) = &local.place_in {
                while scope != 0 && self.model.scopes[scope].kind != *kind {
                    scope = self.model.scopes[scope].parent.unwrap_or(0);
                }
            }
            let name = self.content[local.start..local.end].to_owned();
            let namespace = scopes.namespace_of(&local.kind).to_owned();
            let key = (scope, name.clone(), namespace.clone());
            let earlier = match local.reuse {
                Some(Reuse::SameScope) => self.by_name.contains_key(&key),
                Some(Reuse::Visible) => {
                    self.visible_outward(definitions, scope, &name, &namespace, local.start)
                }
                None => false,
            } || self.declared.contains_key(&(scope, name.clone()));
            if earlier {
                // A redeclaration that reuses the variable: a write to it.
                refs.insert(
                    range,
                    RefCapture {
                        node: local.node,
                        start: local.start,
                        end: local.end,
                        span: local.span,
                        context: Some(RefContext::Write),
                        via: Via::NONE,
                        pattern: usize::MAX,
                    },
                );
                continue;
            }
            refs.remove(&range);
            self.by_name.entry(key).or_default().push(definitions.len());
            definitions.push(Definition {
                name,
                defines: Defines::Local { kind: local.kind },
                namespace,
                scope,
                name_range: local.span,
                visible_from: local.after.unwrap_or(local.start),
                low: self.near_error(local.node),
                declared_type: local.declared_type,
            });
        }
    }

    /// Puts the definitions in file order, and their index with them.
    fn order(&mut self, definitions: Vec<Definition>) {
        let mut order: Vec<usize> = (0..definitions.len()).collect();
        order.sort_by_key(|&i| definitions[i].name_range.start.byte);
        let mut new_index = vec![0; definitions.len()];
        for (new, &old) in order.iter().enumerate() {
            new_index[old] = new;
        }
        for indices in self.by_name.values_mut() {
            for i in indices.iter_mut() {
                *i = new_index[*i];
            }
            indices.sort_unstable();
        }
        let mut slots: Vec<Option<Definition>> = definitions.into_iter().map(Some).collect();
        self.model.definitions = order.into_iter().filter_map(|i| slots[i].take()).collect();
    }

    fn references(&mut self, refs: HashMap<(usize, usize), RefCapture<'_>>) {
        let mut references: Vec<Reference> = refs
            .into_values()
            .filter_map(|r| {
                let context = r.context?;
                Some(Reference {
                    name: self.content[r.start..r.end].to_owned(),
                    context,
                    range: r.span,
                    scope: self.scope_of(r.start, r.end, false),
                    receiver: r.via.receiver,
                    receiver_text: r.via.text,
                    receiver_range: r.via.range,
                    receiver_type: None,
                    arity: r.via.arity,
                    binding: Binding::Free,
                    low: self.near_error(r.node),
                })
            })
            .collect();
        references.sort_by_key(|r| r.range.start.byte);
        self.model.references = references;
    }

    fn bind(&mut self) {
        let bindings: Vec<Binding> = self
            .model
            .references
            .iter()
            .map(|r| self.binding(r))
            .collect();
        for (reference, binding) in self.model.references.iter_mut().zip(bindings) {
            reference.binding = binding;
        }
        self.receiver_types();
    }

    /// The declared type of each receiver the file declares: the definition
    /// the receiver's last name binds to, a local, a parameter or a field.
    fn receiver_types(&mut self) {
        let ends: HashMap<usize, usize> = self
            .model
            .references
            .iter()
            .enumerate()
            .map(|(i, r)| (r.range.end.byte as usize, i))
            .collect();
        let types: Vec<Option<String>> = self
            .model
            .references
            .iter()
            .map(|r| {
                let (start, end) = r.receiver_range?;
                let last = &self.model.references[*ends.get(&end)?];
                if (last.range.start.byte as usize) < start {
                    return None;
                }
                match last.binding {
                    Binding::Bound(d) => self.model.definitions[d].declared_type.clone(),
                    Binding::Free | Binding::Uncertain => None,
                }
            })
            .collect();
        for (reference, found) in self.model.references.iter_mut().zip(types) {
            reference.receiver_type = found;
        }
    }

    fn binding(&self, reference: &Reference) -> Binding {
        let rules = &self.descriptor.scopes;
        let namespaces = rules.read_by(reference.context.as_str());
        match reference.receiver {
            Receiver::Other => Binding::Free,
            Receiver::Own => {
                let Some(class) = self.enclosing_class(reference.scope) else {
                    return Binding::Free;
                };
                rules
                    .members_read_by(reference.context.as_str())
                    .iter()
                    .find_map(|ns| self.visible(class, &reference.name, ns, None))
                    .map_or(Binding::Free, Binding::Bound)
            }
            Receiver::None => {
                // Whether a scope that may hide members was crossed on the
                // way out: a name found nowhere may be one of those members.
                let mut maybe_hidden = false;
                for ns in namespaces {
                    let mut crossed = Opacity::Clear;
                    let mut in_function = false;
                    let mut scope = Some(reference.scope);
                    while let Some(mut s) = scope {
                        match self.declared.get(&(s, reference.name.clone())) {
                            Some(Declared::Global) => s = 0,
                            Some(Declared::Nonlocal) => {
                                in_function = true;
                                scope = self.model.scopes[s].parent;
                                continue;
                            }
                            None => {}
                        }
                        let node = &self.model.scopes[s];
                        let hidden =
                            in_function && rules.hidden_from_functions.contains(&node.kind);
                        if !hidden
                            && let Some(found) = self.visible(
                                s,
                                &reference.name,
                                ns,
                                Some(reference.range.start.byte as usize),
                            )
                        {
                            let is_symbol =
                                matches!(self.model.definitions[found].defines, Defines::Symbol(_));
                            return match crossed {
                                Opacity::All => Binding::Uncertain,
                                Opacity::Members if is_symbol => Binding::Uncertain,
                                _ => Binding::Bound(found),
                            };
                        }
                        in_function |= node.kind == FUNCTION_SCOPE;
                        if hidden {
                            scope = node.parent;
                            continue;
                        }
                        if node.kind == CLASS_SCOPE
                            && rules
                                .implicit_members
                                .get(ns)
                                .is_some_and(|names| names.contains(&reference.name))
                        {
                            // An inherited member, defined in another file.
                            return Binding::Free;
                        }
                        crossed = crossed.max(node.opacity);
                        if rules
                            .closed
                            .get(&node.kind)
                            .is_some_and(|closed| closed.iter().any(|c| c == ns))
                        {
                            break;
                        }
                        scope = node.parent;
                    }
                    maybe_hidden |= crossed == Opacity::All;
                }
                if maybe_hidden {
                    Binding::Uncertain
                } else {
                    Binding::Free
                }
            }
        }
    }

    /// In or around a parse error: missing, inside an `ERROR` node, or in a
    /// construct below its scope that contains an error, such as the `x` of
    /// `int x = ;`.
    fn near_error(&self, node: Node<'_>) -> bool {
        if node.is_missing() || node.is_error() {
            return true;
        }
        let scope = &self.model.scopes[self.scope_of(node.start_byte(), node.end_byte(), false)];
        let mut below_scope = true;
        let mut current = node.parent();
        while let Some(ancestor) = current {
            if ancestor.is_error() {
                return true;
            }
            if (ancestor.start_byte(), ancestor.end_byte()) == (scope.start, scope.end)
                || ancestor.parent().is_none()
            {
                below_scope = false;
            }
            if below_scope && ancestor.has_error() {
                return true;
            }
            current = ancestor.parent();
        }
        false
    }

    /// Whether a definition of `name` defined so far is visible at `at` from
    /// `scope` or the scopes around it, up to a scope closed to `namespace`.
    fn visible_outward(
        &self,
        definitions: &[Definition],
        scope: usize,
        name: &str,
        namespace: &str,
        at: usize,
    ) -> bool {
        let mut current = Some(scope);
        while let Some(s) = current {
            if self
                .visible_among(definitions, s, name, namespace, Some(at))
                .is_some()
            {
                return true;
            }
            let node = &self.model.scopes[s];
            if self
                .descriptor
                .scopes
                .closed
                .get(&node.kind)
                .is_some_and(|closed| closed.iter().any(|c| c == namespace))
            {
                return false;
            }
            current = node.parent;
        }
        false
    }

    fn enclosing_class(&self, mut scope: usize) -> Option<usize> {
        loop {
            let node = &self.model.scopes[scope];
            if node.kind == CLASS_SCOPE {
                return Some(scope);
            }
            scope = node.parent?;
        }
    }

    /// The definition of `name` in `namespace` that a reference at `at` sees
    /// in `scope`: in a hoisted scope the first one, otherwise the latest
    /// one visible before it. `at` is `None` for member access, which sees
    /// every member.
    fn visible(
        &self,
        scope: usize,
        name: &str,
        namespace: &str,
        at: Option<usize>,
    ) -> Option<usize> {
        self.visible_among(&self.model.definitions, scope, name, namespace, at)
    }

    /// [`Builder::visible`] among `definitions`, which `by_name` indexes.
    fn visible_among(
        &self,
        definitions: &[Definition],
        scope: usize,
        name: &str,
        namespace: &str,
        at: Option<usize>,
    ) -> Option<usize> {
        let hoisted = scope == 0
            || self
                .descriptor
                .scopes
                .hoisted
                .iter()
                .any(|k| *k == self.model.scopes[scope].kind);
        let candidates = self
            .by_name
            .get(&(scope, name.to_owned(), namespace.to_owned()))?;
        match at {
            Some(at) if !hoisted => candidates
                .iter()
                .copied()
                .rev()
                .find(|&i| definitions[i].visible_from <= at),
            _ => candidates.first().copied(),
        }
    }
}

/// Keeps, per node, the capture of the pattern written last.
fn keep_last<'tree>(
    refs: &mut HashMap<(usize, usize), RefCapture<'tree>>,
    capture: RefCapture<'tree>,
) {
    let key = (capture.start, capture.end);
    match refs.get(&key) {
        Some(existing) if existing.pattern > capture.pattern => {}
        _ => {
            refs.insert(key, capture);
        }
    }
}

/// The number of arguments in an argument list: its named children but
/// comments.
fn arguments(list: Node<'_>) -> u32 {
    let mut cursor = list.walk();
    let count = list
        .named_children(&mut cursor)
        .filter(|c| !c.kind().contains("comment"))
        .count();
    u32::try_from(count).unwrap_or(u32::MAX)
}

/// A receiver as written, when a name or names joined by `.`, `::` or `->`;
/// whitespace dropped.
fn written_name(text: &str) -> Option<String> {
    let name: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let mut rest = name.as_str();
    loop {
        let end = rest
            .char_indices()
            .find(|&(_, c)| !(c.is_alphanumeric() || c == '_' || c == '$'))
            .map_or(rest.len(), |(i, _)| i);
        if end == 0 || rest.starts_with(|c: char| c.is_ascii_digit()) {
            return None;
        }
        rest = &rest[end..];
        if rest.is_empty() {
            return Some(name);
        }
        rest = ["::", "->", "."]
            .iter()
            .find_map(|sep| rest.strip_prefix(sep))?;
    }
}

fn node_text<'a>(content: &'a str, node: Node<'_>) -> &'a str {
    content
        .get(node.start_byte()..node.end_byte())
        .unwrap_or_default()
}
