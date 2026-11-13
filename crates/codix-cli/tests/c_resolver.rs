//! The C resolver: a call binds to the function the file's includes reach;
//! several variants leave it textual.
#![cfg(with_lang_c)]

use std::path::Path;

use codix_testkit::fixture::{codix_in, indexed};

const CODIX: &str = env!("CARGO_BIN_EXE_codix");

const FILES: &[(&str, &str)] = &[
    ("include/linux/lock.h", "void mutex_lock(int *m);\n"),
    (
        "kernel/locking.c",
        "#include <linux/lock.h>\nvoid mutex_lock(int *m) { *m = 1; }\n",
    ),
    (
        "kernel/user.c",
        "#include <linux/lock.h>\n#include <asm/cpu.h>\nstatic int m;\nvoid go(void) { mutex_lock(&m); cpu_relax(); }\n",
    ),
    ("arch/x86/include/asm/cpu.h", "void cpu_relax(void);\n"),
    ("arch/arm/include/asm/cpu.h", "void cpu_relax(void);\n"),
    ("tools/copy.c", "void mutex_lock(int *m) { }\n"),
];

fn codix(root: &Path, args: &[&str]) -> codix_testkit::Run {
    codix_in(CODIX, root, root, args)
}

#[test]
fn a_call_binds_to_the_definition_its_includes_reach() {
    let (_guard, root) = indexed(CODIX, FILES);
    let out = codix(&root, &["refs", "kernel/locking.c:2:6"]);
    assert_eq!(
        out.stdout,
        "kernel/locking.c:2:6  def   -   scoped\n\
         kernel/user.c:4:17    call  go  bound\n\
         2 results\n\
         1 definition, 0 scoped, 1 bound, 0 textual\n"
    );
    // The copy in tools/ is reached by no include.
    let copy = codix(&root, &["refs", "tools/copy.c:1:6"]);
    assert!(!copy.stdout.contains("kernel/user.c"), "{}", copy.stdout);
    // The prototype stands for its definition.
    let prototype = codix(&root, &["refs", "include/linux/lock.h:1:6"]);
    assert!(
        prototype
            .stdout
            .contains("kernel/user.c:4:17        call  go  bound\n"),
        "{}",
        prototype.stdout
    );
}

#[test]
fn variants_in_several_headers_stay_textual() {
    let (_guard, root) = indexed(CODIX, FILES);
    let out = codix(&root, &["refs", "arch/x86/include/asm/cpu.h:1:6"]);
    assert!(
        out.stdout
            .contains("kernel/user.c:4:33              call  go  textual\n"),
        "{}",
        out.stdout
    );
}

#[test]
fn compile_commands_give_the_include_paths() {
    let mut files = FILES.to_vec();
    // Only the x86 headers are on the include path: its variant binds.
    files.push((
        "compile_commands.json",
        r#"[{"directory": ".", "file": "kernel/user.c", "command": "cc -Iinclude -I arch/x86/include -c kernel/user.c"}]"#,
    ));
    let (_guard, root) = indexed(CODIX, &files);
    let out = codix(&root, &["refs", "arch/x86/include/asm/cpu.h:1:6"]);
    assert!(
        out.stdout
            .contains("kernel/user.c:4:33              call  go  bound\n"),
        "{}",
        out.stdout
    );
}
