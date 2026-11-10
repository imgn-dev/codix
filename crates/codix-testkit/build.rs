//! Records the target, for the plugins the test kit compiles at run time.

use std::io::Write as _;

fn main() {
    let target = std::env::var("TARGET").expect("cargo sets TARGET for build scripts");
    writeln!(
        std::io::stdout(),
        "cargo::rustc-env=CODIX_TESTKIT_TARGET={target}"
    )
    .expect("write the target for cargo");
}
