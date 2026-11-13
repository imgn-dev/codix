//! The views resolvers read are a contract: their columns are those
//! `codix-resolver` declares, and match their snapshot. Once released, a
//! change is a new version of the views, beside the old one.

use std::fmt::Write as _;
use std::path::Path;

use codix_core::store::Store;

#[test]
fn the_views_have_the_declared_columns_and_match_their_snapshot() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("index.db");
    drop(Store::open(&path).expect("index"));
    let conn = rusqlite::Connection::open(&path).expect("open");
    let mut snapshot = String::new();
    for (view, declared) in codix_resolver::VIEW_COLUMNS {
        let columns: Vec<String> = conn
            .prepare(&format!("SELECT name FROM pragma_table_info('{view}')"))
            .and_then(|mut s| s.query_map([], |row| row.get(0))?.collect())
            .expect("columns");
        assert_eq!(&columns, declared, "{view}");
        writeln!(snapshot, "{view}: {}", columns.join(", ")).expect("write");
    }
    write!(snapshot, "\nscratch:{}", codix_resolver::SCRATCH_SQL).expect("write");
    codix_testkit::assert_golden(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas/plugins/resolver-1.txt"),
        &snapshot,
    );
}
