//! The providers compiled into this binary, as the engine sees them.

use codix_provider::{LanguageDescriptor, ProviderKind, known, registry};

use crate::Error;

/// The language pack named `language`, needed for `path`. A language codix
/// knows but this build leaves out is `not_supported_in_build`.
pub fn require_language(language: &str, path: &str) -> Result<&'static LanguageDescriptor, Error> {
    if let Some(descriptor) = registry().languages.iter().find(|d| d.name == language) {
        return Ok(descriptor);
    }
    if known::is_known(ProviderKind::Language, language) {
        Err(Error::NotSupportedInBuild {
            provider: language.to_owned(),
            path: path.to_owned(),
        })
    } else {
        Err(Error::Internal {
            message: format!("unknown language `{language}` for {path}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_language_left_out_of_the_build_is_not_supported() {
        let error = require_language("kotlin", "src/Main.kt").unwrap_err();
        assert_eq!(error.code(), "not_supported_in_build");
        assert_eq!(
            error.cause(),
            "src/Main.kt: kotlin is not supported in this build"
        );
    }

    #[test]
    fn an_unknown_language_is_a_bug() {
        assert_eq!(
            require_language("cobol", "x.cob").unwrap_err().code(),
            "internal"
        );
    }
}
