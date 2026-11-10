//! The Maven build plugin: answers `discover --protocol 1 <root>` with the
//! build's modules as JSON on stdout.

fn main() -> std::process::ExitCode {
    codix_provider::serve(&codix_build_maven::Maven)
}
