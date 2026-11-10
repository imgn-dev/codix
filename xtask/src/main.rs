//! Workspace tasks:
//!
//! - `cargo xtask plugins <out> [--only <name>]... [--target <triple>] [--debug]`
//!   builds the plugin directories of every provider that is not in the
//!   core, as `<out>/<kind>/<name>/`, ready to install under
//!   `lib/codix/plugins`;
//! - `cargo xtask dist <out> [--target <triple>] [--debug]` builds the
//!   modular flavour as it installs: `<out>/bin/codix` and every plugin in
//!   `<out>/lib/codix/plugins`.

use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use xtask::plugins::{Options, Profile, build, dist};

const USAGE: &str =
    "usage: cargo xtask plugins <out> [--only <name>]... [--target <triple>] [--debug]
       cargo xtask dist <out> [--target <triple>] [--debug]";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let task = args.next();
    if !matches!(task.as_deref(), Some("plugins" | "dist")) {
        return usage();
    }
    let mut out = None;
    let mut options = Options {
        only: Vec::new(),
        target: xtask::plugins::HOST.to_owned(),
        profile: Profile::Release,
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--only" if task.as_deref() == Some("plugins") => match args.next() {
                Some(name) => options.only.push(name),
                None => return usage(),
            },
            "--target" => match args.next() {
                Some(target) => options.target = target,
                None => return usage(),
            },
            "--debug" => options.profile = Profile::Debug,
            _ if out.is_none() && !arg.starts_with('-') => out = Some(PathBuf::from(arg)),
            _ => return usage(),
        }
    }
    let Some(out) = out else {
        return usage();
    };
    let built = if task.as_deref() == Some("dist") {
        dist(&out, &options).map(|binary| vec![binary])
    } else {
        build(&out, &options)
    };
    match built {
        Ok(dirs) => {
            let mut stdout = std::io::stdout().lock();
            for dir in dirs {
                // Nothing to report if stdout is gone.
                let _ = writeln!(stdout, "{}", dir.display());
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "xtask: {e}");
            ExitCode::FAILURE
        }
    }
}

fn usage() -> ExitCode {
    let _ = writeln!(std::io::stderr(), "{USAGE}");
    ExitCode::from(2)
}
