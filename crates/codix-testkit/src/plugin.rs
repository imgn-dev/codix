//! Plugins for tests: built from the packs as `cargo xtask plugins` builds
//! them, and fake ones made of scripts.

use std::fs;
use std::path::{Path, PathBuf};

/// Builds the plugin of `language` into `<out>/language/<language>/`,
/// replacing any earlier one, with the code of `cargo xtask plugins`, and
/// returns its directory.
///
/// # Panics
///
/// When the language has no pack, or the grammar does not compile.
pub fn language_plugin(language: &str, out: &Path) -> PathBuf {
    xtask::plugins::language_plugin(language, out)
        .unwrap_or_else(|e| panic!("cannot build the {language} plugin: {e}"))
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
