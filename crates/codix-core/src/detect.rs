//! Language detection. In order: an override in `codix.toml`, a modeline, a
//! shebang, the exact file name, the extension, and for ambiguous extensions
//! (`.h`, `.m`, `.pl`) a content heuristic. Unknown files are plain text.

use codix_provider::{LanguageDescriptor, known, registry};
use globset::GlobMatcher;
use serde::{Deserialize, Serialize};

use crate::config::{Config, override_matcher};

/// Plain text: indexed for search only.
pub const TEXT: &str = "text";

/// Lines read at each end of a file for modelines.
const MODELINE_LINES: usize = 5;

/// Which step of the chain decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    /// `languages.overrides` in `codix.toml`.
    Override,
    /// A Vim or Emacs modeline.
    Modeline,
    /// A `#!` line.
    Shebang,
    /// The exact file name.
    Filename,
    /// The extension.
    Extension,
    /// A content heuristic for an ambiguous extension.
    Heuristic,
    /// Nothing matched: plain text.
    Unknown,
}

/// A detection result. The language may have no pack in this build: a
/// language codix knows but this build leaves out, or one it does not support
/// at all, such as Objective-C; both are indexed as plain text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    /// Language name.
    pub language: String,
    /// The step that decided.
    pub step: Step,
}

/// Detects languages from a project's overrides and the compiled-in packs.
#[derive(Debug)]
pub struct Detector {
    overrides: Vec<(GlobMatcher, String)>,
    languages: &'static [LanguageDescriptor],
}

impl Detector {
    /// A detector for a project's configuration and this build's packs.
    pub fn new(config: &Config) -> Self {
        Self::with_languages(config, &registry().languages)
    }

    /// A detector over the given packs.
    pub fn with_languages(config: &Config, languages: &'static [LanguageDescriptor]) -> Self {
        // When several globs match, the first in sorted order wins.
        let overrides = config
            .language_overrides()
            .into_iter()
            .filter_map(|(glob, language)| Some((override_matcher(&glob)?, language)))
            .collect();
        Self {
            overrides,
            languages,
        }
    }

    /// Detects the language of `path` (root-relative) with `content`.
    pub fn detect(&self, path: &str, content: &[u8]) -> Detection {
        let found = |language: &str, step| Detection {
            language: language.to_owned(),
            step,
        };
        if let Some((_, language)) = self.overrides.iter().find(|(m, _)| m.is_match(path)) {
            return found(language, Step::Override);
        }
        let text = String::from_utf8_lossy(&content[..content.len().min(64 << 10)]);
        if let Some(language) =
            modeline(&text).and_then(|name| self.by(name, |d| &d.detect.modelines))
        {
            return found(language, Step::Modeline);
        }
        if let Some(language) = shebang(&text).and_then(|name| {
            self.by(name, |d| &d.detect.shebangs).or_else(|| {
                self.by(
                    name.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.'),
                    |d| &d.detect.shebangs,
                )
            })
        }) {
            return found(language, Step::Shebang);
        }
        let name = path.rsplit('/').next().unwrap_or(path);
        if let Some(language) = self
            .by(name, |d| &d.detect.filenames)
            .or_else(|| known::language_for_filename(name))
        {
            return found(language, Step::Filename);
        }
        if let Some((_, extension)) = name.rsplit_once('.') {
            if let Some(heuristic) = heuristic(extension) {
                return found(heuristic(&text), Step::Heuristic);
            }
            if let Some(language) = self
                .by(extension, |d| &d.detect.extensions)
                .or_else(|| known::language_for_extension(extension))
            {
                return found(language, Step::Extension);
            }
        }
        found(TEXT, Step::Unknown)
    }

    fn by(
        &self,
        value: &str,
        rules: impl Fn(&LanguageDescriptor) -> &Vec<String>,
    ) -> Option<&'static str> {
        self.languages
            .iter()
            .find(|d| rules(d).iter().any(|rule| rule == value))
            .map(|d| d.name.as_str())
    }
}

/// The language named by a Vim (`vim: ft=x`, `vim: set filetype=x:`) or Emacs
/// (`-*- mode: x -*-`, `-*- x -*-`) modeline in the first or last lines.
fn modeline(text: &str) -> Option<&str> {
    let lines: Vec<&str> = text.lines().collect();
    let tail = lines
        .len()
        .saturating_sub(MODELINE_LINES)
        .max(MODELINE_LINES.min(lines.len()));
    lines[..MODELINE_LINES.min(lines.len())]
        .iter()
        .chain(&lines[tail..])
        .find_map(|line| vim_modeline(line).or_else(|| emacs_modeline(line)))
}

fn vim_modeline(line: &str) -> Option<&str> {
    let start = ["vim:", "vi:", "ex:"]
        .iter()
        .find_map(|m| line.find(m).map(|i| i + m.len()))?;
    let rest = &line[start..];
    let value = ["filetype=", "ft="]
        .iter()
        .find_map(|key| rest.find(key).map(|i| &rest[i + key.len()..]))?;
    let end = value
        .find(|c: char| c.is_whitespace() || c == ':')
        .unwrap_or(value.len());
    (end > 0).then(|| &value[..end])
}

fn emacs_modeline(line: &str) -> Option<&str> {
    let start = line.find("-*-")? + 3;
    let inner = &line[start..start + line[start..].find("-*-")?];
    let inner = inner.trim();
    if !inner.contains(':') {
        return (!inner.is_empty()).then_some(inner);
    }
    inner.split(';').find_map(|pair| {
        let (key, value) = pair.split_once(':')?;
        key.trim()
            .eq_ignore_ascii_case("mode")
            .then(|| value.trim())
    })
}

/// The interpreter of a `#!` line: the program's base name, or with `env` the
/// command it runs (`env -S` included).
fn shebang(text: &str) -> Option<&str> {
    let line = text.lines().next()?.strip_prefix("#!")?;
    let mut words = line.split_whitespace();
    let program = words.next()?.rsplit('/').next()?;
    if program != "env" {
        return Some(program);
    }
    words
        .find(|w| !w.starts_with('-') && !w.contains('='))
        .map(|w| w.rsplit('/').next().unwrap_or(w))
}

/// The heuristic for an ambiguous extension, if it is one.
fn heuristic(extension: &str) -> Option<fn(&str) -> &'static str> {
    match extension {
        "h" => Some(c_or_cpp),
        "m" => Some(objective_c_or_matlab),
        "pl" => Some(perl_or_prolog),
        _ => None,
    }
}

fn c_or_cpp(text: &str) -> &'static str {
    const CPP: &[&str] = &[
        "class ",
        "namespace ",
        "template<",
        "template <",
        "std::",
        "public:",
        "private:",
        "#include <iostream>",
        "nullptr",
        "constexpr",
    ];
    if CPP.iter().any(|marker| text.contains(marker)) {
        "cpp"
    } else {
        "c"
    }
}

fn objective_c_or_matlab(text: &str) -> &'static str {
    const OBJECTIVE_C: &[&str] = &[
        "@interface",
        "@implementation",
        "@property",
        "#import",
        "@end",
    ];
    if OBJECTIVE_C.iter().any(|marker| text.contains(marker)) {
        "objective-c"
    } else if text.lines().any(|l| {
        let l = l.trim_start();
        l.starts_with("function ") || l.starts_with('%')
    }) {
        "matlab"
    } else {
        "objective-c"
    }
}

fn perl_or_prolog(text: &str) -> &'static str {
    const PERL: &[&str] = &["use strict", "use warnings", "my $", "sub ", "print "];
    if PERL.iter().any(|marker| text.contains(marker)) {
        "perl"
    } else if text.contains(":-") {
        "prolog"
    } else {
        "perl"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modelines() {
        assert_eq!(modeline("# vim: set ft=python:\n"), Some("python"));
        assert_eq!(modeline("x\n// vim: filetype=cpp\n"), Some("cpp"));
        assert_eq!(
            modeline("# -*- mode: ruby; coding: utf-8 -*-\n"),
            Some("ruby")
        );
        assert_eq!(modeline("/* -*- c++ -*- */\n"), Some("c++"));
        let tail = format!("{}# vim: ft=go\n", "line\n".repeat(20));
        assert_eq!(modeline(&tail), Some("go"));
        let middle = format!(
            "{}# vim: ft=go\n{}",
            "line\n".repeat(10),
            "line\n".repeat(10)
        );
        assert_eq!(modeline(&middle), None);
    }

    #[test]
    fn shebangs() {
        assert_eq!(shebang("#!/usr/bin/python3\n"), Some("python3"));
        assert_eq!(shebang("#!/usr/bin/env ruby -w\n"), Some("ruby"));
        assert_eq!(shebang("#!/usr/bin/env -S node --harmony\n"), Some("node"));
        assert_eq!(shebang("#!/usr/bin/env FOO=1 python\n"), Some("python"));
        assert_eq!(shebang("print 1\n"), None);
    }

    #[test]
    fn heuristics() {
        assert_eq!(c_or_cpp("int f(void);\n"), "c");
        assert_eq!(c_or_cpp("namespace a { class B; }\n"), "cpp");
        assert_eq!(
            objective_c_or_matlab("@interface A : NSObject\n@end\n"),
            "objective-c"
        );
        assert_eq!(
            objective_c_or_matlab("function y = f(x)\n  y = x;\nend\n"),
            "matlab"
        );
        assert_eq!(perl_or_prolog("use strict;\nmy $x = 1;\n"), "perl");
        assert_eq!(
            perl_or_prolog("parent(a, b).\nancestor(X, Y) :- parent(X, Y).\n"),
            "prolog"
        );
    }
}
