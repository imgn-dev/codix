//! The plugin directories of the providers that are not in the core: each
//! language pack with its grammar compiled into a shared library, each build
//! system with its executable, and the version control systems but git with
//! their descriptors. A language with a resolver would also get its resolver
//! executable; none has one yet.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The target this tool was built for, the default target of the plugins.
pub const HOST: &str = env!("XTASK_TARGET");

/// The plugins' version: the workspace's.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// How to build.
#[derive(Debug, Clone)]
pub struct Options {
    /// Only the providers of these names; every one when empty.
    pub only: Vec<String>,
    /// The target triple.
    pub target: String,
    /// How executables and grammars are optimized.
    pub profile: Profile,
}

/// Optimized for shipping, or quick to build for tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// Cargo's `release` profile, grammars at `-O2`.
    Release,
    /// Cargo's `dev` profile, grammars at `-O0`.
    Debug,
}

/// A language pack, and where its grammar comes from.
#[derive(Debug, Clone, Copy)]
pub struct Pack {
    /// Language name.
    pub language: &'static str,
    /// The pack's crate, under `crates/`.
    pub crate_dir: &'static str,
    /// Its descriptor, in the crate.
    pub descriptor: &'static str,
    /// Its `tags` query, in the crate.
    pub tags: &'static str,
    /// The files its `locals` query concatenates, in the crate.
    pub locals: &'static [&'static str],
    /// The grammar crate.
    pub grammar: &'static str,
    /// The grammar crate's directory holding `parser.c`.
    pub sources: &'static str,
    /// The function the grammar exports.
    pub symbol: &'static str,
}

const fn pack(
    language: &'static str,
    crate_dir: &'static str,
    grammar: &'static str,
    sources: &'static str,
    symbol: &'static str,
    locals: &'static [&'static str],
) -> Pack {
    Pack {
        language,
        crate_dir,
        descriptor: "descriptor.toml",
        tags: "queries/tags.scm",
        locals,
        grammar,
        sources,
        symbol,
    }
}

const LOCALS: &[&str] = &["queries/locals.scm"];

/// Every language pack.
pub const PACKS: &[Pack] = &[
    pack(
        "c",
        "codix-lang-c",
        "tree-sitter-c",
        "src",
        "tree_sitter_c",
        LOCALS,
    ),
    pack(
        "cpp",
        "codix-lang-cpp",
        "tree-sitter-cpp",
        "src",
        "tree_sitter_cpp",
        LOCALS,
    ),
    pack(
        "csharp",
        "codix-lang-csharp",
        "tree-sitter-c-sharp",
        "src",
        "tree_sitter_c_sharp",
        LOCALS,
    ),
    pack(
        "go",
        "codix-lang-go",
        "tree-sitter-go",
        "src",
        "tree_sitter_go",
        LOCALS,
    ),
    pack(
        "java",
        "codix-lang-java",
        "tree-sitter-java",
        "src",
        "tree_sitter_java",
        LOCALS,
    ),
    pack(
        "javascript",
        "codix-lang-javascript",
        "tree-sitter-javascript",
        "src",
        "tree_sitter_javascript",
        LOCALS,
    ),
    pack(
        "json",
        "codix-lang-json",
        "tree-sitter-json",
        "src",
        "tree_sitter_json",
        &[],
    ),
    pack(
        "kotlin",
        "codix-lang-kotlin",
        "tree-sitter-kotlin-ng",
        "src",
        "tree_sitter_kotlin",
        LOCALS,
    ),
    pack(
        "markdown",
        "codix-lang-markdown",
        "tree-sitter-md",
        "tree-sitter-markdown/src",
        "tree_sitter_markdown",
        &[],
    ),
    pack(
        "php",
        "codix-lang-php",
        "tree-sitter-php",
        "php/src",
        "tree_sitter_php",
        LOCALS,
    ),
    pack(
        "python",
        "codix-lang-python",
        "tree-sitter-python",
        "src",
        "tree_sitter_python",
        LOCALS,
    ),
    pack(
        "ruby",
        "codix-lang-ruby",
        "tree-sitter-ruby",
        "src",
        "tree_sitter_ruby",
        LOCALS,
    ),
    pack(
        "rust",
        "codix-lang-rust",
        "tree-sitter-rust",
        "src",
        "tree_sitter_rust",
        LOCALS,
    ),
    pack(
        "sql",
        "codix-lang-sql",
        "tree-sitter-sequel",
        "src",
        "tree_sitter_sql",
        &[],
    ),
    pack(
        "toml",
        "codix-lang-toml",
        "tree-sitter-toml-ng",
        "src",
        "tree_sitter_toml",
        &[],
    ),
    Pack {
        descriptor: "tsx.toml",
        ..pack(
            "tsx",
            "codix-lang-typescript",
            "tree-sitter-typescript",
            "tsx/src",
            "tree_sitter_tsx",
            &["queries/locals.scm", "queries/locals-jsx.scm"],
        )
    },
    pack(
        "typescript",
        "codix-lang-typescript",
        "tree-sitter-typescript",
        "typescript/src",
        "tree_sitter_typescript",
        LOCALS,
    ),
    pack(
        "xml",
        "codix-lang-xml",
        "tree-sitter-xml",
        "xml/src",
        "tree_sitter_xml",
        &[],
    ),
    pack(
        "yaml",
        "codix-lang-yaml",
        "tree-sitter-yaml",
        "src",
        "tree_sitter_yaml",
        &[],
    ),
];

/// Build systems: (name, crate under `crates/`, executable).
pub const BUILDS: &[(&str, &str, &str)] = &[
    ("gradle", "codix-build-gradle", "codix-build-gradle"),
    ("maven", "codix-build-maven", "codix-build-maven"),
];

/// Version control systems outside the core: (name, crate under `crates/`).
pub const VCS: &[(&str, &str)] = &[
    ("mercurial", "codix-vcs-hg"),
    ("subversion", "codix-vcs-svn"),
];

/// Builds the plugins into `out`, replacing any there; returns their
/// directories.
pub fn build(out: &Path, options: &Options) -> Result<Vec<PathBuf>, String> {
    // The C compiler may run from another directory.
    let out = &absolute(out)?;
    let wanted = |name: &str| options.only.is_empty() || options.only.iter().any(|o| o == name);
    let known: Vec<&str> = PACKS
        .iter()
        .map(|p| p.language)
        .chain(BUILDS.iter().map(|b| b.0))
        .chain(VCS.iter().map(|v| v.0))
        .collect();
    if let Some(unknown) = options.only.iter().find(|o| !known.contains(&o.as_str())) {
        return Err(format!("no provider named {unknown}"));
    }
    let workspace = Workspace::read(&options.target)?;
    let packs: Vec<&Pack> = PACKS.iter().filter(|p| wanted(p.language)).collect();
    let mut dirs = in_parallel(&packs, |pack| {
        language_plugin_in(&workspace, pack, out, options)
    })?;
    for (name, crate_dir, executable) in BUILDS.iter().filter(|b| wanted(b.0)) {
        dirs.push(build_plugin(
            &workspace, name, crate_dir, executable, out, options,
        )?);
    }
    for (name, crate_dir) in VCS.iter().filter(|v| wanted(v.0)) {
        let dir = fresh(out, "vcs", name)?;
        manifest(&dir, "vcs", name, "")?;
        copy(
            &workspace.crates.join(crate_dir).join("descriptor.toml"),
            &dir.join("descriptor.toml"),
        )?;
        dirs.push(dir);
    }
    Ok(dirs)
}

/// Runs `work` on each of `items`, as many at a time as the machine has
/// cores, since compiling a grammar keeps one busy; the results in the
/// items' order, or the first error.
fn in_parallel<T: Sync, R: Send>(
    items: &[T],
    work: impl Fn(&T) -> Result<R, String> + Sync,
) -> Result<Vec<R>, String> {
    let next = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(items.len());
    let mut done: Vec<(usize, Result<R, String>)> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(i) else {
                            return done;
                        };
                        done.push((i, work(item)));
                    }
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().unwrap_or_default())
            .collect()
    });
    if done.len() != items.len() {
        return Err("a build thread panicked".to_owned());
    }
    done.sort_by_key(|(i, _)| *i);
    done.into_iter().map(|(_, result)| result).collect()
}

/// Builds the plugin of one language into `<out>/language/<language>/`, for
/// this machine, quick to build: what tests install.
pub fn language_plugin(language: &str, out: &Path) -> Result<PathBuf, String> {
    let out = &absolute(out)?;
    let pack = PACKS
        .iter()
        .find(|p| p.language == language)
        .ok_or_else(|| format!("no pack for {language}"))?;
    let options = Options {
        only: Vec::new(),
        target: HOST.to_owned(),
        profile: Profile::Debug,
    };
    language_plugin_in(&Workspace::read(HOST)?, pack, out, &options)
}

/// Builds the modular flavour into `<out>`, laid out as it installs:
/// `bin/codix` and, beside it, `lib/codix/plugins/`, where codix finds its
/// plugins with no configuration. Returns the binary.
pub fn dist(out: &Path, options: &Options) -> Result<PathBuf, String> {
    let workspace = Workspace::read(&options.target)?;
    let file = format!("codix{}", exe_suffix(&options.target));
    let built = cargo_build(
        &workspace,
        &[
            "--package",
            "codix-cli",
            "--bin",
            "codix",
            "--no-default-features",
            "--features",
            MODULAR_FEATURES,
        ],
        &file,
        options,
    )?;
    let bin = out.join("bin");
    fs::create_dir_all(&bin).map_err(|e| format!("{}: {e}", bin.display()))?;
    copy(&built, &bin.join(&file))?;
    build(&out.join("lib").join("codix").join("plugins"), options)?;
    Ok(bin.join(file))
}

/// The features of the modular flavour: the core with git, and plugins.
pub const MODULAR_FEATURES: &str = "plugins,vcs-git";

/// Runs `cargo build` with `args` for the target and profile, and returns
/// where it put `file`.
fn cargo_build(
    workspace: &Workspace,
    args: &[&str],
    file: &str,
    options: &Options,
) -> Result<PathBuf, String> {
    let mut command = Command::new(cargo());
    command.arg("build").args(args);
    command.args(["--target", &options.target]);
    if options.profile == Profile::Release {
        command.arg("--release");
    }
    let status = command.status().map_err(|e| format!("cargo build: {e}"))?;
    if !status.success() {
        return Err(format!("cargo build of {file} failed ({status})"));
    }
    let profile = match options.profile {
        Profile::Release => "release",
        Profile::Debug => "debug",
    };
    Ok(workspace
        .target_dir
        .join(&options.target)
        .join(profile)
        .join(file))
}

/// What `cargo metadata` says about the workspace.
struct Workspace {
    /// The `crates/` directory.
    crates: PathBuf,
    /// Cargo's target directory.
    target_dir: PathBuf,
    /// Every package's directory, by name.
    packages: Vec<(String, PathBuf)>,
}

impl Workspace {
    fn read(target: &str) -> Result<Self, String> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let output = Command::new(cargo())
            .args(["metadata", "--format-version", "1"])
            .args(["--filter-platform", target, "--manifest-path"])
            .arg(root.join("Cargo.toml"))
            .output()
            .map_err(|e| format!("cargo metadata: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "cargo metadata: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        let metadata: serde_json::Value =
            serde_json::from_slice(&output.stdout).map_err(|e| format!("cargo metadata: {e}"))?;
        let packages = metadata["packages"]
            .as_array()
            .ok_or("cargo metadata: no packages")?
            .iter()
            .filter_map(|p| {
                let name = p["name"].as_str()?;
                let manifest = Path::new(p["manifest_path"].as_str()?);
                Some((name.to_owned(), manifest.parent()?.to_path_buf()))
            })
            .collect();
        let target_dir = metadata["target_directory"]
            .as_str()
            .ok_or("cargo metadata: no target directory")?;
        Ok(Self {
            crates: root.join("crates"),
            target_dir: PathBuf::from(target_dir),
            packages,
        })
    }

    fn package(&self, name: &str) -> Result<&Path, String> {
        self.packages
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, dir)| dir.as_path())
            .ok_or_else(|| format!("{name} is not a dependency of the workspace"))
    }
}

fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned())
}

fn language_plugin_in(
    workspace: &Workspace,
    pack: &Pack,
    out: &Path,
    options: &Options,
) -> Result<PathBuf, String> {
    let crate_dir = workspace.crates.join(pack.crate_dir);
    let dir = fresh(out, "language", pack.language)?;
    let library = format!("{}.{}", pack.language, dll_extension(&options.target));
    manifest(
        &dir,
        "language",
        pack.language,
        &format!("library = \"{library}\"\nsymbol = \"{}\"\n", pack.symbol),
    )?;
    copy(
        &crate_dir.join(pack.descriptor),
        &dir.join("descriptor.toml"),
    )?;
    fs::create_dir(dir.join("queries")).map_err(|e| format!("{}: {e}", dir.display()))?;
    copy(&crate_dir.join(pack.tags), &dir.join("queries/tags.scm"))?;
    if !pack.locals.is_empty() {
        let mut locals = String::new();
        for file in pack.locals {
            let path = crate_dir.join(file);
            locals.push_str(
                &fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?,
            );
        }
        let path = dir.join("queries/locals.scm");
        fs::write(&path, locals).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    compile_grammar(
        &workspace.package(pack.grammar)?.join(pack.sources),
        &dir.join(library),
        options,
    )?;
    Ok(dir)
}

fn build_plugin(
    workspace: &Workspace,
    name: &str,
    crate_dir: &str,
    executable: &str,
    out: &Path,
    options: &Options,
) -> Result<PathBuf, String> {
    let file = format!("{executable}{}", exe_suffix(&options.target));
    let built = cargo_build(
        workspace,
        &["--package", crate_dir, "--bin", executable],
        &file,
        options,
    )?;
    let dir = fresh(out, "build", name)?;
    manifest(
        &dir,
        "build",
        name,
        &format!("executable = \"bin/{file}\"\n"),
    )?;
    copy(
        &workspace.crates.join(crate_dir).join("descriptor.toml"),
        &dir.join("descriptor.toml"),
    )?;
    fs::create_dir(dir.join("bin")).map_err(|e| format!("{}: {e}", dir.display()))?;
    copy(&built, &dir.join("bin").join(&file))?;
    Ok(dir)
}

fn absolute(path: &Path) -> Result<PathBuf, String> {
    std::path::absolute(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// An empty plugin directory `<out>/<kind>/<name>`.
fn fresh(out: &Path, kind: &str, name: &str) -> Result<PathBuf, String> {
    let dir = out.join(kind).join(name);
    if dir.exists() {
        fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(dir)
}

fn manifest(dir: &Path, kind: &str, name: &str, extra: &str) -> Result<(), String> {
    let path = dir.join("plugin.toml");
    fs::write(
        &path,
        format!(
            "schema = 1\nkind = \"{kind}\"\nname = \"{name}\"\nversion = \"{VERSION}\"\n{extra}"
        ),
    )
    .map_err(|e| format!("{}: {e}", path.display()))
}

fn copy(from: &Path, to: &Path) -> Result<(), String> {
    fs::copy(from, to)
        .map(|_| ())
        .map_err(|e| format!("{} to {}: {e}", from.display(), to.display()))
}

fn dll_extension(target: &str) -> &'static str {
    if target.contains("windows") {
        "dll"
    } else if target.contains("apple") {
        "dylib"
    } else {
        "so"
    }
}

fn exe_suffix(target: &str) -> &'static str {
    if target.contains("windows") {
        ".exe"
    } else {
        ""
    }
}

/// Compiles a grammar's `parser.c`, and `scanner.c` when it has one, into
/// the shared library `library`, with the C compiler `cc` finds for the
/// target.
fn compile_grammar(sources: &Path, library: &Path, options: &Options) -> Result<(), String> {
    let mut build = cc::Build::new();
    build
        .cargo_metadata(false)
        .cargo_warnings(false)
        .emit_rerun_if_env_changed(false)
        .target(&options.target)
        .host(HOST)
        .opt_level(match options.profile {
            Profile::Release => 2,
            Profile::Debug => 0,
        })
        .debug(false)
        .std("c11")
        .include(sources);
    if options.target.contains("msvc") {
        build.flag("-utf-8");
    }
    let compiler = build.try_get_compiler().map_err(|e| e.to_string())?;
    let files: Vec<PathBuf> = ["parser.c", "scanner.c"]
        .iter()
        .map(|f| sources.join(f))
        .filter(|f| f.is_file())
        .collect();
    if files.is_empty() {
        return Err(format!("{}: no parser.c", sources.display()));
    }
    let library_dir = library.parent().ok_or("no plugin directory")?;
    let mut command = compiler.to_command();
    command.args(&files);
    if compiler.is_like_msvc() {
        // Objects go to the current directory: the plugin's, then removed.
        command
            .current_dir(library_dir)
            .arg("/LD")
            .arg(format!("/Fe{}", library.display()))
            .args(["/link", "/NOIMPLIB", "/NOEXP"]);
    } else {
        command.arg("-shared").arg("-o").arg(library);
    }
    let output = command
        .output()
        .map_err(|e| format!("the C compiler: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "{command:?}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    if compiler.is_like_msvc() {
        for file in &files {
            if let Some(stem) = file.file_stem() {
                // Left by `cl`; absent ones are fine.
                let _ = fs::remove_file(library_dir.join(stem).with_extension("obj"));
            }
        }
    }
    Ok(())
}
