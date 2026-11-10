//! Records the target this tool runs on: the default target of the plugins
//! it builds.

use std::io::Write as _;

fn main() {
    let target = std::env::var("TARGET").expect("cargo sets TARGET for build scripts");
    writeln!(std::io::stdout(), "cargo::rustc-env=XTASK_TARGET={target}")
        .expect("write the target for cargo");
}
