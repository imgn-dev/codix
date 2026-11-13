//! A resolver plugin runs as its own process after each scan, with the same
//! results as in process. One that crashes, answers another protocol, or
//! writes rows it should not is refused with a `plugin_invalid` warning and
//! changes nothing; after three failures in a row it is not started again.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use codix_provider::{Plugin, PluginSource, Rejected};

const PLUGINS: &str = concat!(env!("CARGO_TARGET_TMPDIR"), "/resolver-plugin");

fn plugins() -> Vec<Result<Plugin, Rejected>> {
    codix_plugin::discover(&[PathBuf::from(PLUGINS)])
}

codix_provider::inventory::submit! {
    PluginSource { discover: plugins }
}

/// The Java plugin, with the test resolver in place of its own; returns its
/// `bin` directory.
fn install() -> PathBuf {
    let plugins = Path::new(PLUGINS);
    if plugins.exists() {
        fs::remove_dir_all(plugins).expect("remove old plugins");
    }
    let dir = codix_testkit::plugin::language_plugin("java", plugins);
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).expect("bin");
    let name = format!("codix-test-resolver{}", std::env::consts::EXE_SUFFIX);
    fs::copy(env!("CARGO_BIN_EXE_codix-test-resolver"), bin.join(&name)).expect("copy");
    let manifest = fs::read_to_string(dir.join("plugin.toml")).expect("manifest");
    let manifest = manifest
        .split_once("\n[resolver]")
        .map_or(manifest.as_str(), |(before, _)| before);
    fs::write(
        dir.join("plugin.toml"),
        format!("{manifest}\n[resolver]\nexecutable = \"bin/{name}\"\nprotocol = 1\nviews = 1\n"),
    )
    .expect("manifest");
    bin
}

// One test: the registry, and the resolver process, last as long as the
// test binary.
#[test]
fn a_resolver_plugin_gives_the_in_process_results_and_failures_are_refused() {
    let bin = install();
    let (_dir, root) = common::project();
    let report = common::scan_project(&root);
    assert_eq!(report.warnings, [], "{report:?}");
    common::check_resolutions(&root);

    common::change_util(&root, 1);
    let report = common::scan_project(&root);
    assert_eq!(report.warnings, [], "{report:?}");
    common::check_resolutions(&root);
    let log = fs::read_to_string(bin.join("requests.log")).expect("log");
    assert_eq!(
        log.lines().last(),
        Some("src/a/Util.java -> src/b/Main.java")
    );

    let mut round = 1;
    let mut fails = |mode: &str, causes: &[&str]| {
        fs::write(bin.join("mode"), mode).expect("mode");
        round += 1;
        common::change_util(&root, round);
        let before = common::resolutions(&root);
        let report = common::scan_project(&root);
        let [warning] = report.warnings.as_slice() else {
            panic!("{mode}: {report:?}");
        };
        assert_eq!(warning.code, "plugin_invalid");
        assert!(
            warning.message.starts_with("the java resolver failed: ")
                && causes.iter().any(|c| warning.message.contains(c)),
            "{mode}: {}",
            warning.message
        );
        // Util's symbols were replaced, so what pointed to them went with
        // them; nothing else changed.
        let kept: String = before
            .split_inclusive('\n')
            .filter(|l| !l.contains("src/a/Util.java::"))
            .collect();
        assert_eq!(common::resolutions(&root), kept, "{mode}");
    };
    fails("foreign", &["outside the files it was given"]);
    fails("dangling", &["bound to no symbol of the index"]);
    fs::write(bin.join("mode"), "").expect("mode");
    common::change_util(&root, 10);
    let report = common::scan_project(&root);
    assert_eq!(
        report.warnings,
        [],
        "a success resets the count: {report:?}"
    );
    common::check_resolutions(&root);

    fails(
        "crash",
        &[
            "the resolver exited",
            "closed its output",
            "stopped reading",
        ],
    );
    fails(
        "future",
        &["protocol 2 and views 1, this codix speaks protocol 1 and views 1"],
    );
    fails(
        "crash",
        &[
            "the resolver exited",
            "closed its output",
            "stopped reading",
        ],
    );
    fails("", &["not restarted after 3 failures in a row"]);
}
