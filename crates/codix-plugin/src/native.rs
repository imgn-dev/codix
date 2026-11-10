//! The native grammar loader: the only `unsafe` code of codix.

use std::path::Path;

use codix_provider::PluginError;
use tree_sitter_language::LanguageFn;

/// The type of the function a tree-sitter grammar exports,
/// `const TSLanguage *tree_sitter_<name>(void)`, with the pointer left opaque.
type LanguageFunction = unsafe extern "C" fn() -> *const ();

/// Loads the grammar that `library` exports as `symbol`. The library stays
/// loaded for the rest of the process: the grammar points into it, and every
/// parser and query made from it may outlive any caller.
pub(crate) fn load(library: &Path, symbol: &str) -> Result<tree_sitter::Language, PluginError> {
    // SAFETY: opening a library runs its initialisers. Plugins come only from
    // the search path, never from a project, so this library was installed
    // by the user and is trusted as codix itself is. A grammar library holds
    // parse tables and C functions, and has no initialisers.
    let handle = unsafe { libloading::Library::new(library) }
        .map_err(|e| PluginError::new(format!("{}: {}", library.display(), describe(&e))))?;
    // SAFETY: tree-sitter grammars export their language function under this
    // symbol with the signature `LanguageFunction` declares. A symbol of
    // another type is a broken plugin, as a corrupt library would be.
    let function = *unsafe { handle.get::<LanguageFunction>(symbol) }
        .map_err(|e| PluginError::new(format!("symbol {symbol}: {}", describe(&e))))?;
    std::mem::forget(handle);
    // SAFETY: as above, the function returns a pointer to the grammar's
    // static `TSLanguage`, and has no other effect.
    if unsafe { function() }.is_null() {
        return Err(PluginError::new(format!("symbol {symbol}: no grammar")));
    }
    // SAFETY: the function is a grammar's language function, as above, and
    // the library holding it is never unloaded. tree-sitter checks the
    // grammar's ABI version before parsing with it.
    let function = unsafe { LanguageFn::from_raw(function) };
    Ok(tree_sitter::Language::new(function))
}

/// The system's own message, which names what failed, when it gave one.
fn describe(error: &libloading::Error) -> String {
    use std::error::Error as _;
    error
        .source()
        .map_or_else(|| error.to_string(), ToString::to_string)
}
