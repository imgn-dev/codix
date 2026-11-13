//! The C resolver as a plugin's executable.

use std::process::ExitCode;

fn main() -> ExitCode {
    codix_resolver::serve(&mut codix_resolve_c::CResolver::new())
}
