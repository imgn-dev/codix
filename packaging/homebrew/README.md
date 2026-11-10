# Homebrew formulas: examples only

Examples of how a tap would package the modular flavour: `codix.rb` for the
core and `codix-lang-java.rb` for one plugin; every other plugin follows the
second. Nothing here is published. The URL and checksum stand for a release
archive of the sources.

The core is built with `CODIX_INSTALL_HINT`, so a missing provider names the
formula to install. A plugin formula installs its directory under
`lib/codix/plugins/<kind>/<name>`; Homebrew links it into
`$(brew --prefix)/lib/codix/plugins`, beside the linked `bin/codix`, where
codix finds it with no configuration. TypeScript's formula installs two
plugins, `language/typescript` and `language/tsx`.
