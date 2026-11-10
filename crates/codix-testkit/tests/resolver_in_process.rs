//! A compiled-in resolver runs in process after each scan: its results are
//! merged, and a scan that changes one file resolves that file and the
//! dependents it names, no other.

mod common;

use std::fs;
use std::path::Path;

use codix_lang_java as _;
use codix_testkit::resolver::Unique;

const LOG: &str = concat!(env!("CARGO_TARGET_TMPDIR"), "/resolver-in-process.log");

codix_resolver::register!("java", || Unique {
    log: Some(Path::new(LOG).to_path_buf()),
});

#[test]
fn a_compiled_in_resolver_resolves_changed_files_and_their_dependents() {
    // Absent on the first run.
    let _ = fs::remove_file(LOG);
    let (_dir, root) = common::project();
    let report = common::scan_project(&root);
    assert_eq!(report.warnings, [], "{report:?}");
    common::check_resolutions(&root);

    common::change_util(&root, 1);
    let report = common::scan_project(&root);
    assert_eq!(report.warnings, [], "{report:?}");
    common::check_resolutions(&root);
    let log = fs::read_to_string(LOG).expect("log");
    let last = log.lines().last().expect("a request");
    assert_eq!(last, "src/a/Util.java -> src/b/Main.java");
}
