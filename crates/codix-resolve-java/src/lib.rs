//! The Java resolver: binds the references its files' scopes leave unbound
//! to symbols of other files, from the index's views.
//!
//! Type names resolve as Java does: nested and inherited member types, the
//! file's types, single-type imports, the package, on-demand imports, and
//! anything else is outside the project (`java.lang` among it). Members
//! resolve in the type a reference is made on and its supertypes: the
//! enclosing types for a plain name or `this`, their supertypes for
//! `super`, the declared type of a local, a parameter or a field, a type
//! named as the receiver, and static imports; calls by argument count.
//! What needs inference stays unbound: `var`, lambda parameters, generic type
//! arguments, chained calls.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};

use codix_resolver::{Resolver, Scratch};
use rusqlite::Connection;

codix_resolver::register!("java", JavaResolver::new);

/// The Java resolver.
#[derive(Debug, Default)]
pub struct JavaResolver;

impl JavaResolver {
    /// A resolver.
    pub fn new() -> Self {
        Self
    }
}

impl Resolver for JavaResolver {
    fn resolve(
        &mut self,
        index: &Connection,
        scratch: &mut Scratch,
        files: &[String],
    ) -> Result<Vec<String>, String> {
        let model = Model::load(index).map_err(|e| e.to_string())?;
        let changed: HashSet<i64> = files
            .iter()
            .filter_map(|path| model.file_ids.get(path).copied())
            .collect();
        let dependents = dependents(index, &model, &changed).map_err(|e| e.to_string())?;
        let ids: Vec<i64> = changed.iter().chain(&dependents).copied().collect();
        let refs = unbound_refs(index, &ids).map_err(|e| e.to_string())?;
        scratch.batch(|scratch| {
            for r in &refs {
                match model.resolve(r) {
                    Some(Found::Symbol(target)) => scratch.bind(r.id, target)?,
                    Some(Found::External) => scratch.external(r.id)?,
                    None => {}
                }
            }
            Ok(())
        })?;
        Ok(dependents
            .iter()
            .filter_map(|id| model.files.get(id).map(|f| f.path.clone()))
            .collect())
    }
}

/// What a name resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Found {
    /// A symbol of the project, by `v1_symbols.id`.
    Symbol(i64),
    /// Something outside the project.
    External,
}

const TYPE_KINDS: &[&str] = &["class", "interface", "enum", "struct", "trait"];
const METHOD_KINDS: &[&str] = &["method"];
const VALUE_KINDS: &[&str] = &["field", "constant", "enum_member"];

#[derive(Debug)]
struct Symbol {
    file: i64,
    name: String,
    kind: String,
    parent: Option<i64>,
    signature: Option<String>,
    declared_type: Option<String>,
}

#[derive(Debug, Default)]
struct File {
    path: String,
    package: String,
    imports: Vec<Import>,
}

#[derive(Debug)]
struct Import {
    name: String,
    is_static: bool,
    wildcard: bool,
}

/// A reference to resolve.
#[derive(Debug)]
struct Ref {
    id: i64,
    file: i64,
    name: String,
    context: String,
    enclosing: Option<i64>,
    receiver: Option<String>,
    receiver_type: Option<String>,
    arity: Option<u32>,
}

/// The Java part of the index.
#[derive(Debug, Default)]
struct Model {
    files: HashMap<i64, File>,
    file_ids: HashMap<String, i64>,
    symbols: HashMap<i64, Symbol>,
    /// Members by their type's ID.
    children: HashMap<i64, Vec<i64>>,
    /// Top-level types by file and name.
    top_types: HashMap<(i64, String), i64>,
    /// Types by fully qualified name, nested ones with dots.
    types: HashMap<String, i64>,
    /// Packages of the project, and their prefixes.
    packages: HashSet<String>,
    /// The simple names of the project's types.
    simple_types: HashSet<String>,
    /// Supertypes as written, by type.
    supertypes: HashMap<i64, Vec<String>>,
    /// Resolved supertypes, transitively, by type.
    ancestors: RefCell<HashMap<i64, Vec<i64>>>,
}

impl Model {
    fn load(index: &Connection) -> rusqlite::Result<Self> {
        let mut model = Self::default();
        model.load_files(index)?;
        let qualified = model.load_symbols(index)?;
        model.index_types(&qualified);
        let mut stmt = index.prepare(
            "SELECT t.symbol, t.name FROM v1_supertypes t
             JOIN v1_symbols s ON s.id = t.symbol JOIN v1_files f ON f.id = s.file_id
             WHERE f.language = 'java'",
        )?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
            let (symbol, name) = row?;
            model.supertypes.entry(symbol).or_default().push(name);
        }
        Ok(model)
    }

    /// The Java files, with their package and imports.
    fn load_files(&mut self, index: &Connection) -> rusqlite::Result<()> {
        let model = self;
        let mut stmt = index.prepare("SELECT id, path FROM v1_files WHERE language = 'java'")?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
            let (id, path) = row?;
            model.file_ids.insert(path.clone(), id);
            model.files.insert(
                id,
                File {
                    path,
                    ..File::default()
                },
            );
        }
        let mut stmt = index.prepare(
            "SELECT i.file_id, i.kind, i.name, i.is_static, i.wildcard FROM v1_imports i
             JOIN v1_files f ON f.id = i.file_id WHERE f.language = 'java'",
        )?;
        for row in stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, bool>(3)?,
                r.get::<_, bool>(4)?,
            ))
        })? {
            let (file, kind, name, is_static, wildcard) = row?;
            let Some(f) = model.files.get_mut(&file) else {
                continue;
            };
            if kind == "package" {
                f.package = name;
            } else if kind == "import" {
                f.imports.push(Import {
                    name,
                    is_static,
                    wildcard,
                });
            }
        }
        Ok(())
    }

    /// The symbols of the Java files, linked to their parents; returns
    /// their qualified names.
    fn load_symbols(&mut self, index: &Connection) -> rusqlite::Result<HashMap<i64, String>> {
        let model = self;
        let mut stmt = index.prepare(
            "SELECT s.id, s.symbol_id, s.file_id, s.name, s.kind, s.qualified_name, s.parent,
                    s.signature, s.declared_type
             FROM v1_symbols s JOIN v1_files f ON f.id = s.file_id WHERE f.language = 'java'",
        )?;
        let mut by_symbol_id: HashMap<String, i64> = HashMap::new();
        let mut parents: Vec<(i64, String)> = Vec::new();
        let mut qualified: HashMap<i64, String> = HashMap::new();
        for row in stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, Option<String>>(7)?,
                r.get::<_, Option<String>>(8)?,
            ))
        })? {
            let (id, symbol_id, file, name, kind, qualified_name, parent, signature, declared) =
                row?;
            by_symbol_id.insert(symbol_id, id);
            if let Some(parent) = parent {
                parents.push((id, parent));
            }
            qualified.insert(id, qualified_name);
            model.symbols.insert(
                id,
                Symbol {
                    file,
                    name,
                    kind,
                    parent: None,
                    signature,
                    declared_type: declared,
                },
            );
        }
        for (id, parent) in parents {
            if let Some(&p) = by_symbol_id.get(&parent) {
                if let Some(s) = model.symbols.get_mut(&id) {
                    s.parent = Some(p);
                }
                model.children.entry(p).or_default().push(id);
            }
        }
        Ok(qualified)
    }

    /// The types by qualified and simple name, and the packages.
    fn index_types(&mut self, qualified: &HashMap<i64, String>) {
        let model = self;
        for (&id, s) in &model.symbols {
            if !TYPE_KINDS.contains(&s.kind.as_str()) {
                continue;
            }
            let package = model.files.get(&s.file).map_or("", |f| f.package.as_str());
            let name = &qualified[&id];
            let fqn = if package.is_empty() {
                name.clone()
            } else {
                format!("{package}.{name}")
            };
            model.types.insert(fqn, id);
            model.simple_types.insert(s.name.clone());
            if s.parent.is_none() {
                model.top_types.insert((s.file, s.name.clone()), id);
            }
        }
        for f in model.files.values() {
            let mut prefix = String::new();
            for part in f.package.split('.').filter(|p| !p.is_empty()) {
                if !prefix.is_empty() {
                    prefix.push('.');
                }
                prefix.push_str(part);
                model.packages.insert(prefix.clone());
            }
        }
    }

    fn resolve(&self, r: &Ref) -> Option<Found> {
        let enclosing = self.enclosing_types(r.enclosing);
        match r.context.as_str() {
            "import" => Some(self.import(r)),
            "type" => match &r.receiver {
                Some(receiver) => {
                    self.qualified_type(r.file, &enclosing, &format!("{receiver}.{}", r.name))
                }
                None => self.type_named(r.file, &enclosing, &r.name),
            },
            _ => self.member_use(r, &enclosing),
        }
    }

    /// The types around a symbol, innermost first.
    fn enclosing_types(&self, mut symbol: Option<i64>) -> Vec<i64> {
        let mut types = Vec::new();
        while let Some(id) = symbol {
            let Some(s) = self.symbols.get(&id) else {
                break;
            };
            if TYPE_KINDS.contains(&s.kind.as_str()) {
                types.push(id);
            }
            symbol = s.parent;
        }
        types
    }

    /// A name of an import: a type, a static member, or a package.
    fn import(&self, r: &Ref) -> Found {
        let full = match &r.receiver {
            Some(receiver) => format!("{receiver}.{}", r.name),
            None => r.name.clone(),
        };
        if let Some(&id) = self.types.get(&full) {
            return Found::Symbol(id);
        }
        if let Some(receiver) = &r.receiver
            && let Some(&owner) = self.types.get(receiver)
        {
            // A static import of a member.
            return self
                .member(owner, &r.name, None, Want::Any)
                .map_or(Found::External, Found::Symbol);
        }
        Found::External
    }

    /// A simple type name used in `file` within the types `enclosing`.
    fn type_named(&self, file: i64, enclosing: &[i64], name: &str) -> Option<Found> {
        for &t in enclosing {
            for owner in std::iter::once(t).chain(self.ancestors(t)) {
                if let Some(id) = self.child(owner, name, TYPE_KINDS) {
                    return Some(Found::Symbol(id));
                }
            }
        }
        if let Some(&id) = self.top_types.get(&(file, name.to_owned())) {
            return Some(Found::Symbol(id));
        }
        let f = self.files.get(&file)?;
        for import in f.imports.iter().filter(|i| !i.is_static && !i.wildcard) {
            if import.name.rsplit('.').next() == Some(name) {
                return Some(
                    self.types
                        .get(&import.name)
                        .map_or(Found::External, |&id| Found::Symbol(id)),
                );
            }
        }
        let in_package = if f.package.is_empty() {
            name.to_owned()
        } else {
            format!("{}.{name}", f.package)
        };
        if let Some(&id) = self.types.get(&in_package) {
            return Some(Found::Symbol(id));
        }
        for import in f.imports.iter().filter(|i| i.wildcard) {
            if let Some(&id) = self.types.get(&format!("{}.{name}", import.name)) {
                return Some(Found::Symbol(id));
            }
        }
        // Outside the project, unless a type of the project has the name:
        // code missing an import may mean it.
        (!self.simple_types.contains(name)).then_some(Found::External)
    }

    /// A type written with qualifiers: a fully qualified name, or a type
    /// followed by its nested types.
    fn qualified_type(&self, file: i64, enclosing: &[i64], written: &str) -> Option<Found> {
        if let Some(&id) = self.types.get(written) {
            return Some(Found::Symbol(id));
        }
        let mut parts = written.split('.');
        let first = parts.next()?;
        if self.packages.contains(first) || first.starts_with(char::is_lowercase) {
            // A package outside the project, or one this name does not reach.
            return Some(Found::External);
        }
        let mut current = self.type_named(file, enclosing, first)?;
        for part in parts {
            current = match current {
                Found::Symbol(t) => self
                    .child(t, part, TYPE_KINDS)
                    .map_or(Found::External, Found::Symbol),
                Found::External => Found::External,
            };
        }
        Some(current)
    }

    /// A call, a read or a write of a member.
    fn member_use(&self, r: &Ref, enclosing: &[i64]) -> Option<Found> {
        let want = if r.context == "call" {
            Want::Method
        } else {
            Want::Value
        };
        let in_types = |types: &[i64]| -> Option<Found> {
            types
                .iter()
                .find_map(|&t| self.member(t, &r.name, r.arity, want))
                .map(Found::Symbol)
        };
        match r.receiver.as_deref() {
            None => in_types(enclosing)
                .or_else(|| self.static_import(r.file, &r.name, r.arity, want))
                .or_else(|| {
                    (want == Want::Value && is_type_like(&r.name))
                        .then(|| self.type_named(r.file, enclosing, &r.name))
                        .flatten()
                }),
            Some("this") => in_types(&enclosing[..enclosing.len().min(1)]),
            Some("super") => {
                let own = enclosing.first()?;
                self.ancestors(*own)
                    .iter()
                    .find_map(|&t| self.member_here(t, &r.name, r.arity, want))
                    .map(Found::Symbol)
            }
            // A call's result or another expression: its type needs
            // inference.
            Some("…") => None,
            Some(receiver) => {
                let owner = match &r.receiver_type {
                    Some(declared) => self.qualified_type(r.file, enclosing, declared)?,
                    None => self.receiver_owner(r.file, enclosing, receiver)?,
                };
                match owner {
                    Found::Symbol(t) => self.member(t, &r.name, r.arity, want).map(Found::Symbol),
                    Found::External => Some(Found::External),
                }
            }
        }
    }

    /// The type a receiver written without a declared type designates: an
    /// inherited field's declared type, `this.field`'s, or a type named.
    fn receiver_owner(&self, file: i64, enclosing: &[i64], receiver: &str) -> Option<Found> {
        let field = receiver.strip_prefix("this.").unwrap_or(receiver);
        if !field.contains('.')
            && let Some(id) = enclosing
                .iter()
                .find_map(|&t| self.member(t, field, None, Want::Value))
        {
            let symbol = &self.symbols[&id];
            let declared = symbol.declared_type.as_deref()?;
            let around = self.enclosing_types(symbol.parent);
            return self.qualified_type(symbol.file, &around, declared);
        }
        if receiver.split('.').all(is_type_like) || receiver.contains('.') {
            return self.qualified_type(file, enclosing, receiver);
        }
        None
    }

    /// A member of a type imported statically into `file`.
    fn static_import(
        &self,
        file: i64,
        name: &str,
        arity: Option<u32>,
        want: Want,
    ) -> Option<Found> {
        let f = self.files.get(&file)?;
        for import in f.imports.iter().filter(|i| i.is_static) {
            let owner = if import.wildcard {
                import.name.as_str()
            } else {
                match import.name.rsplit_once('.') {
                    Some((owner, member)) if member == name => owner,
                    _ => continue,
                }
            };
            match self.types.get(owner) {
                Some(&t) => {
                    if let Some(id) = self.member(t, name, arity, want) {
                        return Some(Found::Symbol(id));
                    }
                }
                None if !import.wildcard => return Some(Found::External),
                None => {}
            }
        }
        None
    }

    /// The member `name` of `owner` or of its closest supertype that has
    /// one fitting; a call fits by its argument count. When methods of
    /// several types fit with other parameters, only types could tell which
    /// is meant: none is. An override, with the same parameters, is the
    /// closest.
    fn member(&self, owner: i64, name: &str, arity: Option<u32>, want: Want) -> Option<i64> {
        let mut found: Option<i64> = None;
        let mut parameters: HashSet<String> = HashSet::new();
        for t in std::iter::once(owner).chain(self.ancestors(owner)) {
            for id in self.members_here(t, name, arity, want) {
                let key = parameter_text(name, self.symbols[&id].signature.as_deref());
                if found.is_none() {
                    found = Some(id);
                    parameters.insert(key);
                } else if want == Want::Method
                    && arity.is_some()
                    && parameters.insert(key)
                    && self.symbols[&id].parent != found.and_then(|f| self.symbols[&f].parent)
                {
                    return None;
                }
            }
        }
        found
    }

    fn member_here(&self, owner: i64, name: &str, arity: Option<u32>, want: Want) -> Option<i64> {
        self.members_here(owner, name, arity, want)
            .into_iter()
            .next()
    }

    fn members_here(&self, owner: i64, name: &str, arity: Option<u32>, want: Want) -> Vec<i64> {
        let kinds: &[&str] = match want {
            Want::Method => METHOD_KINDS,
            Want::Value => VALUE_KINDS,
            Want::Any => &["method", "field", "constant", "enum_member"],
        };
        self.children
            .get(&owner)
            .into_iter()
            .flatten()
            .copied()
            .filter(|id| {
                let s = &self.symbols[id];
                s.name == name
                    && kinds.contains(&s.kind.as_str())
                    && arity.is_none_or(|a| fits(name, s.signature.as_deref(), a))
            })
            .collect()
    }

    fn child(&self, owner: i64, name: &str, kinds: &[&str]) -> Option<i64> {
        self.children.get(&owner)?.iter().copied().find(|id| {
            let s = &self.symbols[id];
            s.name == name && kinds.contains(&s.kind.as_str())
        })
    }

    /// The supertypes of a type in the project, transitively, closest first.
    fn ancestors(&self, t: i64) -> Vec<i64> {
        if let Some(found) = self.ancestors.borrow().get(&t) {
            return found.clone();
        }
        // A cycle, in code that does not compile, ends here.
        self.ancestors.borrow_mut().insert(t, Vec::new());
        let mut found = Vec::new();
        let mut seen = HashSet::from([t]);
        let mut queue = VecDeque::from([t]);
        while let Some(current) = queue.pop_front() {
            let Some(symbol) = self.symbols.get(&current) else {
                continue;
            };
            let around = self.enclosing_types(symbol.parent);
            for written in self.supertypes.get(&current).into_iter().flatten() {
                if let Some(Found::Symbol(s)) = self.qualified_type(symbol.file, &around, written)
                    && seen.insert(s)
                {
                    found.push(s);
                    queue.push_back(s);
                }
            }
        }
        self.ancestors.borrow_mut().insert(t, found.clone());
        found
    }
}

/// What a member use may mean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Want {
    Method,
    Value,
    Any,
}

/// A name written like a type: an upper case letter, then not all upper
/// case, as constants are.
fn is_type_like(name: &str) -> bool {
    name.starts_with(char::is_uppercase) && name.chars().any(char::is_lowercase)
}

/// A method's parameters as written, whitespace dropped: overrides share
/// them.
fn parameter_text(name: &str, signature: Option<&str>) -> String {
    let Some(signature) = signature else {
        return String::new();
    };
    let start = signature
        .find(&format!("{name}("))
        .map_or(0, |i| i + name.len());
    signature[start..]
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

/// Whether a call with `arity` arguments fits the parameters `signature`
/// declares for `name`: as many, or more with varargs.
fn fits(name: &str, signature: Option<&str>, arity: u32) -> bool {
    let Some(signature) = signature else {
        return true;
    };
    let Some(after) = signature
        .find(&format!("{name}("))
        .map(|i| i + name.len() + 1)
    else {
        return true;
    };
    let mut depth = 1;
    let mut parts = vec![String::new()];
    for c in signature[after..].chars() {
        match c {
            '(' | '[' | '{' | '<' => depth += 1,
            ')' | ']' | '}' | '>' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            ',' if depth == 1 => {
                parts.push(String::new());
                continue;
            }
            _ => {}
        }
        if let Some(last) = parts.last_mut() {
            last.push(c);
        }
    }
    let parts: Vec<&str> = parts
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect();
    let arity = arity as usize;
    if parts.last().is_some_and(|p| p.contains("...")) {
        arity + 1 >= parts.len()
    } else {
        arity == parts.len()
    }
}

/// The files to resolve again because they use a name the changed files
/// define, but the changed files.
fn dependents(
    index: &Connection,
    model: &Model,
    changed: &HashSet<i64>,
) -> rusqlite::Result<Vec<i64>> {
    let names: HashSet<&str> = model
        .symbols
        .values()
        .filter(|s| changed.contains(&s.file))
        .map(|s| s.name.as_str())
        .collect();
    let mut found = HashSet::new();
    let names: Vec<&str> = names.into_iter().collect();
    for chunk in names.chunks(500) {
        let marks = vec!["?"; chunk.len()].join(", ");
        let mut stmt = index.prepare(&format!(
            "SELECT DISTINCT r.file_id FROM v1_refs r JOIN v1_files f ON f.id = r.file_id
             WHERE f.language = 'java' AND r.state != 'bound' AND r.name IN ({marks})"
        ))?;
        for row in stmt.query_map(rusqlite::params_from_iter(chunk), |r| r.get::<_, i64>(0))? {
            let file = row?;
            if !changed.contains(&file) {
                found.insert(file);
            }
        }
    }
    let mut found: Vec<i64> = found.into_iter().collect();
    found.sort_unstable();
    Ok(found)
}

/// The references of `files` their scopes leave unbound.
fn unbound_refs(index: &Connection, files: &[i64]) -> rusqlite::Result<Vec<Ref>> {
    let mut refs = Vec::new();
    let mut stmt = index.prepare(
        "SELECT id, file_id, name, context, enclosing, receiver, receiver_type, arity
         FROM v1_refs WHERE file_id = ?1 AND state != 'bound'",
    )?;
    for &file in files {
        for row in stmt.query_map([file], |r| {
            Ok(Ref {
                id: r.get(0)?,
                file: r.get(1)?,
                name: r.get(2)?,
                context: r.get(3)?,
                enclosing: r.get(4)?,
                receiver: r.get(5)?,
                receiver_type: r.get(6)?,
                arity: r.get(7)?,
            })
        })? {
            refs.push(row?);
        }
    }
    Ok(refs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calls_fit_by_argument_count_and_varargs() {
        let sig = Some("public Object getBean(String name, Class<?> type, Object... args)");
        assert!(fits("getBean", sig, 2));
        assert!(fits("getBean", sig, 4));
        assert!(!fits("getBean", sig, 1));
        assert!(fits("f", Some("void f(Map<String, Integer> m)"), 1));
        assert!(!fits("f", Some("void f()"), 1));
        assert!(fits("f", None, 3));
    }

    #[test]
    fn type_like_names_have_lower_case_letters() {
        assert!(is_type_like("BeanFactory"));
        assert!(!is_type_like("MAX_VALUE"));
        assert!(!is_type_like("factory"));
    }
}
