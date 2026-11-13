//! The C resolver: binds a use of a function or a global in one file to the
//! symbol of that name its `#include`s reach, directly or through other
//! headers.
//!
//! Include paths come from `compile_commands.json` at the project's root or
//! in `build/`, when there is one: every `-I`, `-iquote` and `-isystem`
//! directory of every entry. Otherwise they are the including file's
//! directory, then every `include` directory of the tree. A use binds when
//! exactly one symbol of the name is reached, or when those reached are all
//! in one file (a prototype and its inline definition); several in several
//! files, as configuration variants are, leave it unbound, and so do calls
//! through function pointers.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Component, Path, PathBuf};

use codix_resolver::{Resolver, Scratch};
use rusqlite::Connection;

codix_resolver::register!("c", CResolver::new);

/// The C resolver.
#[derive(Debug, Default)]
pub struct CResolver;

impl CResolver {
    /// A resolver.
    pub fn new() -> Self {
        Self
    }
}

impl Resolver for CResolver {
    fn resolve(
        &mut self,
        index: &Connection,
        scratch: &mut Scratch,
        files: &[String],
    ) -> Result<Vec<String>, String> {
        let sql = |e: rusqlite::Error| e.to_string();
        let mut model = Model::load(index).map_err(sql)?;
        let changed: HashSet<i64> = files
            .iter()
            .filter_map(|path| model.ids.get(path).copied())
            .filter(|id| model.c_files.contains(id))
            .collect();
        let dependents = dependents(index, &changed).map_err(sql)?;
        let targets: Vec<i64> = changed.iter().chain(&dependents).copied().collect();
        let refs = unbound_refs(index, &targets).map_err(sql)?;
        // Only the names used matter; all of them for a whole project.
        let names: HashSet<&str> = refs.iter().map(|(_, _, name)| name.as_str()).collect();
        let few = (names.len() < SELECTIVE).then_some(&names);
        model.load_symbols(index, few).map_err(sql)?;
        let mut reach = Reach::default();
        scratch.batch(|scratch| {
            let mut current: Option<(i64, HashSet<i64>)> = None;
            for (id, file, name) in &refs {
                // References come file by file: one file's reach at a time.
                if current.as_ref().is_none_or(|(f, _)| f != file) {
                    current = Some((*file, model.reachable(*file)));
                }
                let Some((_, reached)) = &current else {
                    continue;
                };
                if let Some(target) = model.resolve(&mut reach, *file, reached, name) {
                    scratch.bind(*id, target)?;
                }
            }
            Ok(())
        })?;
        Ok(dependents
            .iter()
            .filter_map(|id| model.paths.get(id).cloned())
            .collect())
    }
}

/// The kinds a plain name may mean across files.
const KINDS: &[&str] = &["function", "declaration", "variable", "macro"];

/// Below this many names used, only their symbols are read.
const SELECTIVE: usize = 20_000;

/// The definition files' reaches kept at most, to map prototypes.
const REACHES_KEPT: usize = 4_096;

#[derive(Debug, Default)]
struct Model {
    /// Every indexed file's path, by ID, and back.
    paths: HashMap<i64, String>,
    ids: HashMap<String, i64>,
    /// The C files.
    c_files: HashSet<i64>,
    /// The includes of each file, as written: path and whether `<…>`.
    includes: HashMap<i64, Vec<(String, bool)>>,
    /// Directories searched for includes, root-relative.
    include_dirs: Vec<String>,
    /// The same, to look up.
    include_set: HashSet<String>,
    /// Files by their last path component, to find an include's candidates
    /// without trying every include directory.
    by_basename: HashMap<String, Vec<i64>>,
    /// The files each include names, by the including file's directory, the
    /// path written and whether `<…>`.
    headers: std::cell::RefCell<HashMap<HeaderKey, Vec<i64>>>,
    /// Symbols that a name may mean across files, by name.
    symbols: HashMap<String, Vec<Sym>>,
}

/// An include as one directory sees it: that directory, the path written
/// and whether `<…>`.
type HeaderKey = (Option<String>, String, bool);

/// A symbol a name may mean across files.
#[derive(Debug, Clone, Copy)]
struct Sym {
    id: i64,
    file: i64,
    /// A prototype, declared without its body.
    declaration: bool,
    /// Declared `static`: visible in its file only.
    is_static: bool,
}

/// The files definition files reach, kept within [`REACHES_KEPT`], and what
/// each prototype stands for, once found.
#[derive(Debug, Default)]
struct Reach {
    files: HashMap<i64, HashSet<i64>>,
    prototypes: HashMap<i64, i64>,
}

impl Reach {
    fn of(&mut self, model: &Model, file: i64) -> &HashSet<i64> {
        if self.files.len() >= REACHES_KEPT && !self.files.contains_key(&file) {
            self.files.clear();
        }
        self.files
            .entry(file)
            .or_insert_with(|| model.reachable(file))
    }
}

impl Model {
    fn load(index: &Connection) -> rusqlite::Result<Self> {
        let mut model = Self::default();
        let mut stmt = index.prepare("SELECT id, path, language FROM v1_files")?;
        for row in stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })? {
            let (id, path, language) = row?;
            if language == "c" {
                model.c_files.insert(id);
            }
            model.ids.insert(path.clone(), id);
            model.paths.insert(id, path);
        }
        let mut stmt =
            index.prepare("SELECT file_id, name, system FROM v1_imports WHERE kind = 'include'")?;
        for row in stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, bool>(2)?,
            ))
        })? {
            let (file, name, system) = row?;
            model.includes.entry(file).or_default().push((name, system));
        }
        model.include_dirs = match project_root(index).and_then(|root| compile_commands(&root)) {
            Some(dirs) => dirs,
            None => include_directories(model.paths.values()),
        };
        model.include_set = model.include_dirs.iter().cloned().collect();
        for (&id, path) in &model.paths {
            let base = path.rsplit('/').next().unwrap_or(path);
            model
                .by_basename
                .entry(base.to_owned())
                .or_default()
                .push(id);
        }
        Ok(model)
    }

    /// The symbols a name may mean across files: those named `names`, or
    /// every one.
    fn load_symbols(
        &mut self,
        index: &Connection,
        names: Option<&HashSet<&str>>,
    ) -> rusqlite::Result<()> {
        let select =
            "SELECT s.id, s.file_id, s.name, s.kind, s.parent, s.signature FROM v1_symbols s
             JOIN v1_files f ON f.id = s.file_id WHERE f.language IN ('c', 'cpp')";
        match names {
            None => self.symbols_where(index, select, &[]),
            Some(names) => {
                let names: Vec<&str> = names.iter().copied().collect();
                for chunk in names.chunks(500) {
                    let marks = vec!["?"; chunk.len()].join(", ");
                    self.symbols_where(index, &format!("{select} AND s.name IN ({marks})"), chunk)?;
                }
                Ok(())
            }
        }
    }

    fn symbols_where(
        &mut self,
        index: &Connection,
        sql: &str,
        args: &[&str],
    ) -> rusqlite::Result<()> {
        let model = self;
        let mut stmt = index.prepare(sql)?;
        for row in stmt.query_map(rusqlite::params_from_iter(args), |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
            ))
        })? {
            let (id, file, name, kind, parent, signature) = row?;
            // Members and locals of a type are reached through it.
            if parent.is_none() && KINDS.contains(&kind.as_str()) {
                let is_static = signature
                    .as_deref()
                    .is_some_and(|s| s.split(['(', ' ', '\t', '\n']).any(|w| w == "static"));
                model.symbols.entry(name).or_default().push(Sym {
                    id,
                    file,
                    declaration: kind == "declaration",
                    is_static,
                });
            }
        }
        Ok(())
    }

    /// The files `file` includes, directly or through other headers.
    fn reachable(&self, file: i64) -> HashSet<i64> {
        let mut seen = HashSet::from([file]);
        let mut queue = VecDeque::from([file]);
        while let Some(current) = queue.pop_front() {
            for (written, system) in self.includes.get(&current).into_iter().flatten() {
                for header in self.header_cached(current, written, *system) {
                    if seen.insert(header) {
                        queue.push_back(header);
                    }
                }
            }
        }
        seen
    }

    /// [`Self::header`], computed once per directory and path written.
    fn header_cached(&self, from: i64, written: &str, system: bool) -> Vec<i64> {
        let dir = self
            .paths
            .get(&from)
            .map(|p| p.rsplit_once('/').map_or("", |(d, _)| d).to_owned());
        let key = (dir, written.to_owned(), system);
        if let Some(found) = self.headers.borrow().get(&key) {
            return found.clone();
        }
        let found = self.header(from, written, system);
        self.headers.borrow_mut().insert(key, found.clone());
        found
    }

    /// The files an include may name: beside the including file for `"…"`,
    /// then in each include directory.
    fn header(&self, from: i64, written: &str, system: bool) -> Vec<i64> {
        let mut found = Vec::new();
        let beside = (!system)
            .then(|| self.paths.get(&from))
            .flatten()
            .and_then(|path| Path::new(path).parent().map(Path::to_path_buf));
        // Beside the including file, a `"…"` include is searched first.
        if let Some(dir) = beside
            && let Some(path) = normalized(&dir.join(written))
            && let Some(&id) = self.ids.get(&path)
        {
            return vec![id];
        }
        let Some(written) = normalized(Path::new(written)) else {
            return found;
        };
        let from_dir = self
            .paths
            .get(&from)
            .and_then(|p| p.rsplit_once('/'))
            .map_or("", |(d, _)| d);
        // Of the include directories holding it, those nearest the including
        // file, then the shallowest: `tools/include` for a file of `tools/`,
        // `include` rather than `tools/include` for the others.
        let mut best: Option<(usize, isize)> = None;
        let base = written.rsplit('/').next().unwrap_or(&written);
        for &id in self.by_basename.get(base).into_iter().flatten() {
            let path = &self.paths[&id];
            let dir = if *path == written {
                Some("")
            } else {
                path.strip_suffix(written.as_str())
                    .and_then(|p| p.strip_suffix('/'))
            };
            let Some(dir) = dir.filter(|d| self.include_set.contains(*d)) else {
                continue;
            };
            let score = (shared_components(dir, from_dir), -depth(dir));
            match best {
                Some(b) if score < b => {}
                Some(b) if score == b => found.push(id),
                _ => {
                    best = Some(score);
                    found = vec![id];
                }
            }
        }
        found
    }

    /// What `name` used in `file` binds to: the one symbol of the name the
    /// file reaches, or several in one file, its own first; a prototype
    /// stands for the one definition whose file reaches it, when there is one.
    fn resolve(
        &self,
        reach: &mut Reach,
        file: i64,
        reached: &HashSet<i64>,
        name: &str,
    ) -> Option<i64> {
        let all = self.symbols.get(name)?;
        let candidates: Vec<Sym> = all
            .iter()
            .copied()
            .filter(|s| reached.contains(&s.file))
            .collect();
        let files: BTreeSet<i64> = candidates.iter().map(|s| s.file).collect();
        if files.len() != 1 {
            return None;
        }
        let chosen = candidates
            .iter()
            .find(|s| s.file == file && !s.declaration)
            .or_else(|| candidates.iter().find(|s| !s.declaration))
            .or_else(|| candidates.first())
            .copied()?;
        if !chosen.declaration {
            return Some(chosen.id);
        }
        if let Some(&mapped) = reach.prototypes.get(&chosen.id) {
            return Some(mapped);
        }
        let header = chosen.file;
        let definitions: Vec<i64> = all
            .iter()
            .filter(|s| !s.declaration && !s.is_static)
            .filter(|s| reach.of(self, s.file).contains(&header))
            .map(|s| s.id)
            .collect();
        let mapped = match definitions.as_slice() {
            [one] => *one,
            _ => chosen.id,
        };
        reach.prototypes.insert(chosen.id, mapped);
        Some(mapped)
    }
}

/// How many leading path components two directories share.
fn shared_components(a: &str, b: &str) -> usize {
    a.split('/')
        .zip(b.split('/'))
        .take_while(|(x, y)| x == y && !x.is_empty())
        .count()
}

/// How many components a directory has.
fn depth(dir: &str) -> isize {
    isize::try_from(dir.split('/').filter(|p| !p.is_empty()).count()).unwrap_or(isize::MAX)
}

/// The project's root: the directory holding `.codix/`, where the index is.
fn project_root(index: &Connection) -> Option<PathBuf> {
    let path: String = index
        .query_row(
            "SELECT file FROM pragma_database_list WHERE name = 'main'",
            [],
            |r| r.get(0),
        )
        .ok()?;
    Path::new(&path).parent()?.parent().map(Path::to_path_buf)
}

/// The include directories of `compile_commands.json`, root-relative; `None`
/// without one.
fn compile_commands(root: &Path) -> Option<Vec<String>> {
    let path = [
        root.join("compile_commands.json"),
        root.join("build/compile_commands.json"),
    ]
    .into_iter()
    .find(|p| p.is_file())?;
    let entries: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    let mut dirs: Vec<String> = Vec::new();
    for entry in entries.as_array()? {
        // A relative directory is the root's.
        let directory = root.join(entry["directory"].as_str().unwrap_or("."));
        let words: Vec<String> = match (&entry["arguments"], &entry["command"]) {
            (serde_json::Value::Array(args), _) => args
                .iter()
                .filter_map(|a| a.as_str().map(str::to_owned))
                .collect(),
            (_, serde_json::Value::String(command)) => {
                command.split_whitespace().map(str::to_owned).collect()
            }
            _ => continue,
        };
        let mut words = words.iter();
        while let Some(word) = words.next() {
            let dir = ["-I", "-iquote", "-isystem"].iter().find_map(|flag| {
                let rest = word.strip_prefix(flag)?;
                if rest.is_empty() {
                    words.clone().next().cloned()
                } else {
                    Some(rest.to_owned())
                }
            });
            let Some(dir) = dir else {
                continue;
            };
            let absolute = directory.join(dir);
            if let Ok(relative) = absolute.strip_prefix(root)
                && let Some(relative) = normalized(relative)
                && !dirs.contains(&relative)
            {
                dirs.push(relative);
            }
        }
    }
    Some(dirs)
}

/// Every directory named `include` among the files' ancestors, sorted.
fn include_directories<'a>(paths: impl Iterator<Item = &'a String>) -> Vec<String> {
    let mut dirs = BTreeSet::new();
    for path in paths {
        let mut prefix = PathBuf::new();
        for component in Path::new(path)
            .parent()
            .into_iter()
            .flat_map(Path::components)
        {
            prefix.push(component);
            if component.as_os_str() == "include"
                && let Some(dir) = normalized(&prefix)
            {
                dirs.insert(dir);
            }
        }
    }
    dirs.into_iter().collect()
}

/// A relative path with `.` and `..` removed and `/` between parts; `None`
/// when it leaves the root.
fn normalized(path: &Path) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_str()?.to_owned()),
            Component::ParentDir => {
                parts.pop()?;
            }
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(parts.join("/"))
}

/// The C files to resolve again because they use, unresolved, a name the
/// changed files define and other files can see, but the changed files:
/// reindexing a file drops what was bound to its symbols, and what nothing
/// bound may bind now.
fn dependents(index: &Connection, changed: &HashSet<i64>) -> rusqlite::Result<Vec<i64>> {
    let ids: Vec<i64> = changed.iter().copied().collect();
    let mut names: BTreeSet<String> = BTreeSet::new();
    for chunk in ids.chunks(500) {
        let marks = vec!["?"; chunk.len()].join(", ");
        let mut stmt = index.prepare(&format!(
            "SELECT name, kind, signature FROM v1_symbols
             WHERE parent IS NULL AND file_id IN ({marks})"
        ))?;
        for row in stmt.query_map(rusqlite::params_from_iter(chunk), |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })? {
            let (name, kind, signature) = row?;
            let is_static = signature
                .as_deref()
                .is_some_and(|s| s.split(['(', ' ', '\t', '\n']).any(|w| w == "static"));
            if KINDS.contains(&kind.as_str()) && !is_static {
                names.insert(name);
            }
        }
    }
    let mut found = HashSet::new();
    let names: Vec<String> = names.into_iter().collect();
    for chunk in names.chunks(500) {
        let marks = vec!["?"; chunk.len()].join(", ");
        let mut stmt = index.prepare(&format!(
            "SELECT DISTINCT r.file_id FROM v1_refs r JOIN v1_files f ON f.id = r.file_id
             LEFT JOIN v1_resolved x ON x.ref = r.id
             WHERE f.language = 'c' AND r.state != 'bound' AND x.ref IS NULL
               AND r.receiver IS NULL AND r.context IN ('call', 'read')
               AND r.name IN ({marks})"
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

/// The plain names of `files` their scopes leave unbound, called or read:
/// ID, file and name. Members and calls through pointers have a receiver.
fn unbound_refs(index: &Connection, files: &[i64]) -> rusqlite::Result<Vec<(i64, i64, String)>> {
    let mut refs = Vec::new();
    let mut stmt = index.prepare(
        "SELECT id, file_id, name FROM v1_refs
         WHERE file_id = ?1 AND state != 'bound' AND receiver IS NULL
           AND context IN ('call', 'read')",
    )?;
    for &file in files {
        for row in stmt.query_map([file], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))? {
            refs.push(row?);
        }
    }
    Ok(refs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_normalized_within_the_root() {
        assert_eq!(
            normalized(Path::new("kernel/sched/../../include/linux/sched.h")),
            Some("include/linux/sched.h".to_owned())
        );
        assert_eq!(normalized(Path::new("../x.h")), None);
    }

    #[test]
    fn nearer_and_shallower_include_directories_come_first() {
        assert_eq!(shared_components("tools/include", "tools/perf/util"), 1);
        assert_eq!(shared_components("include", "kernel/sched"), 0);
        assert_eq!(shared_components("", "kernel"), 0);
        assert!(depth("include") < depth("tools/include"));
    }

    #[test]
    fn include_directories_are_found_in_the_tree() {
        let paths = [
            "include/linux/sched.h".to_owned(),
            "arch/x86/include/asm/a.h".to_owned(),
            "kernel/sched/core.c".to_owned(),
        ];
        assert_eq!(
            include_directories(paths.iter()),
            ["arch/x86/include", "include"]
        );
    }
}
