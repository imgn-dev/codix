//! Plugins built from the packs, as a package would ship them: the pack's
//! descriptor and queries, and its grammar compiled from the grammar crate's
//! C sources into a shared library.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A language pack, and where its grammar comes from.
struct Pack {
    /// Language name.
    language: &'static str,
    /// The pack's crate, under `crates/`.
    crate_dir: &'static str,
    /// Its queries: `tags`, then the files `locals` concatenates.
    tags: &'static str,
    locals: &'static [&'static str],
    /// The grammar crate, and its directory holding `parser.c`.
    grammar: &'static str,
    sources: &'static str,
    /// The function the grammar exports.
    symbol: &'static str,
}

const PACKS: &[Pack] = &[Pack {
    language: "java",
    crate_dir: "codix-lang-java",
    tags: "queries/tags.scm",
    locals: &["queries/locals.scm"],
    grammar: "tree-sitter-java",
    sources: "src",
    symbol: "tree_sitter_java",
}];

/// The target this test kit was built for.
const TARGET: &str = env!("CODIX_TESTKIT_TARGET");

/// Builds the plugin of `language` into `<out>/language/<language>/`,
/// replacing any earlier one, and returns its directory.
///
/// # Panics
///
/// When the language has no pack here, or the grammar does not compile.
pub fn language_plugin(language: &str, out: &Path) -> PathBuf {
    let pack = PACKS
        .iter()
        .find(|p| p.language == language)
        .unwrap_or_else(|| panic!("no pack for {language}"));
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let pack_dir = crates.join(pack.crate_dir);
    let dir = out.join("language").join(language);
    if dir.exists() {
        fs::remove_dir_all(&dir).expect("remove the old plugin");
    }
    fs::create_dir_all(dir.join("queries")).expect("plugin directory");
    let library = format!("{language}.{}", std::env::consts::DLL_EXTENSION);
    fs::write(
        dir.join("plugin.toml"),
        format!(
            "schema = 1\nkind = \"language\"\nname = \"{language}\"\nversion = \"{}\"\nlibrary = \"{library}\"\nsymbol = \"{}\"\n",
            env!("CARGO_PKG_VERSION"),
            pack.symbol
        ),
    )
    .expect("write plugin.toml");
    fs::copy(
        pack_dir.join("descriptor.toml"),
        dir.join("descriptor.toml"),
    )
    .expect("copy the descriptor");
    fs::copy(pack_dir.join(pack.tags), dir.join("queries/tags.scm")).expect("copy tags");
    if !pack.locals.is_empty() {
        let locals: String = pack
            .locals
            .iter()
            .map(|f| fs::read_to_string(pack_dir.join(f)).expect("read locals"))
            .collect();
        fs::write(dir.join("queries/locals.scm"), locals).expect("write locals");
    }
    compile_grammar(
        &crate_dir(pack.grammar).join(pack.sources),
        &dir.join(library),
    );
    dir
}

/// The directory of a dependency of the workspace, from `cargo metadata`.
fn crate_dir(name: &str) -> PathBuf {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
    let output = Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1", "--offline"])
        .args(["--filter-platform", TARGET, "--manifest-path"])
        .arg(workspace)
        .output()
        .expect("run cargo metadata");
    assert!(
        output.status.success(),
        "cargo metadata: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("cargo metadata JSON");
    let manifest = metadata["packages"]
        .as_array()
        .expect("packages")
        .iter()
        .find(|p| p["name"] == name)
        .and_then(|p| p["manifest_path"].as_str())
        .unwrap_or_else(|| panic!("{name} is not a dependency of the workspace"));
    Path::new(manifest)
        .parent()
        .expect("crate directory")
        .to_path_buf()
}

/// Compiles a grammar's `parser.c`, and `scanner.c` when it has one, into
/// the shared library `library`, with the compiler `cc` finds for this
/// target.
fn compile_grammar(sources: &Path, library: &Path) {
    let mut build = cc::Build::new();
    build
        .cargo_metadata(false)
        .cargo_warnings(false)
        .emit_rerun_if_env_changed(false)
        .target(TARGET)
        .host(TARGET)
        .opt_level(2)
        .debug(false)
        .std("c11")
        .include(sources);
    if TARGET.contains("msvc") {
        build.flag("-utf-8");
    }
    let compiler = build.try_get_compiler().expect("a C compiler");
    let files: Vec<PathBuf> = ["parser.c", "scanner.c"]
        .iter()
        .map(|f| sources.join(f))
        .filter(|f| f.is_file())
        .collect();
    let objects = tempfile::tempdir().expect("temp dir");
    let mut command = compiler.to_command();
    command.current_dir(objects.path()).args(&files);
    if compiler.is_like_msvc() {
        command
            .arg("/LD")
            .arg(format!("/Fe{}", library.display()))
            .args(["/link", "/NOIMPLIB", "/NOEXP"]);
    } else {
        command.arg("-shared").arg("-o").arg(library);
    }
    let output = command.output().expect("run the C compiler");
    assert!(
        output.status.success(),
        "{command:?}: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// An executable script `<dir>/<name>` running `unix` with `sh`, or on
/// Windows `<dir>/<name>.cmd` running `windows` with `cmd`, for fake build
/// plugins. Writing an executable while another thread starts a process can
/// make that start fail on Linux: write scripts before starting any.
///
/// # Panics
///
/// When the script cannot be written.
pub fn script(dir: &Path, name: &str, unix: &str, windows: &str) -> PathBuf {
    if cfg!(windows) {
        let path = dir.join(format!("{name}.cmd"));
        fs::write(&path, format!("@echo off\r\n{windows}\r\n")).expect("write");
        path
    } else {
        let path = dir.join(name);
        fs::write(&path, format!("#!/bin/sh\n{unix}\n")).expect("write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        path
    }
}
