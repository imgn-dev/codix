//! Each step of the detection chain wins over the next one.

use std::fs;

use codix_core::config::Config;
use codix_core::detect::{Detection, Detector, Step};
use codix_core::project::Project;

codix_provider::register!(Language, "alpha", "tests/fixtures/alpha.toml");
codix_provider::register!(Language, "beta", "tests/fixtures/beta.toml");

fn detect(path: &str, content: &str) -> (String, Step) {
    detect_with(None, path, content)
}

fn detect_with(shared: Option<&str>, path: &str, content: &str) -> (String, Step) {
    let dir = tempfile::tempdir().expect("temp dir");
    fs::create_dir(dir.path().join(".codix")).expect("create .codix");
    if let Some(shared) = shared {
        fs::write(dir.path().join("codix.toml"), shared).expect("write codix.toml");
    }
    let project = Project::discover(dir.path())
        .expect("discover")
        .expect("project");
    let config = Config::load(&project).expect("config");
    let Detection { language, step } = Detector::new(&config).detect(path, content.as_bytes());
    (language, step)
}

fn is(language: &str, step: Step) -> (String, Step) {
    (language.to_owned(), step)
}

#[test]
fn override_beats_modeline() {
    let shared = "[languages.overrides]\n\"gen/*.b\" = \"ruby\"\n";
    assert_eq!(
        detect_with(Some(shared), "gen/x.b", "# vim: ft=alpha\n"),
        is("ruby", Step::Override)
    );
    assert_eq!(
        detect_with(Some(shared), "src/x.b", "# vim: ft=alpha\n"),
        is("alpha", Step::Modeline)
    );
}

#[test]
fn modeline_beats_shebang() {
    assert_eq!(
        detect("tool", "#!/usr/bin/env alpha\n# vim: ft=beta\n"),
        is("beta", Step::Modeline)
    );
}

#[test]
fn shebang_beats_filename() {
    assert_eq!(
        detect("Build.b", "#!/usr/bin/env beta\n"),
        is("beta", Step::Shebang)
    );
    assert_eq!(
        detect("tool", "#!/opt/alpha2.1 -x\n"),
        is("alpha", Step::Shebang),
        "version stripped"
    );
}

#[test]
fn filename_beats_extension() {
    assert_eq!(detect("dir/Build.b", ""), is("alpha", Step::Filename));
    assert_eq!(detect("dir/Other.b", ""), is("beta", Step::Extension));
}

#[test]
fn ambiguous_extensions_use_content() {
    assert_eq!(
        detect("x.h", "namespace n { class C; }\n"),
        is("cpp", Step::Heuristic)
    );
    assert_eq!(detect("x.h", "int f(void);\n"), is("c", Step::Heuristic));
    assert_eq!(
        detect("x.m", "@interface A\n@end\n"),
        is("objective-c", Step::Heuristic)
    );
    assert_eq!(detect("x.pl", "a :- b.\n"), is("prolog", Step::Heuristic));
}

#[test]
fn languages_left_out_of_the_build_are_still_recognized() {
    assert_eq!(detect("src/Main.kt", ""), is("kotlin", Step::Extension));
    assert_eq!(detect("Rakefile", ""), is("ruby", Step::Filename));
    assert_eq!(
        detect("build", "#!/usr/bin/env kotlinc -script\n"),
        is("kotlin", Step::Shebang)
    );
    assert_eq!(
        detect("tool", "#!/usr/bin/python3.12\n"),
        is("python", Step::Shebang)
    );
    assert_eq!(
        detect("x.conf", "# vim: ft=yaml\n"),
        is("yaml", Step::Modeline)
    );
}

#[test]
fn unknown_files_are_plain_text() {
    assert_eq!(
        detect("notes.unknownext", "hello\n"),
        is("text", Step::Unknown)
    );
    assert_eq!(detect("LICENSE", "Apache\n"), is("text", Step::Unknown));
}
