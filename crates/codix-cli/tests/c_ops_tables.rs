//! C function pointer members: an ops table's slots, and the designated
//! initializers that fill them.
#![cfg(with_lang_c)]

use std::path::Path;

use codix_testkit::fixture::{codix_in, indexed};

const CODIX: &str = env!("CARGO_BIN_EXE_codix");

const FILES: &[(&str, &str)] = &[
    (
        "include/sched.h",
        "struct rq;
struct task_struct;
struct sched_class {
\tvoid (*enqueue_task)(struct rq *rq, struct task_struct *p, int flags);
\tstruct task_struct *(*pick_next_task)(struct rq *rq);
\tint prio;
};
",
    ),
    (
        "kernel/sched/fair.c",
        "#include \"sched.h\"
static struct task_struct *__pick_next_task_fair(struct rq *rq) { return 0; }
static void enqueue_task_fair(struct rq *rq, struct task_struct *p, int flags) {}
const struct sched_class fair_sched_class = {
\t.enqueue_task = enqueue_task_fair,
\t.pick_next_task = __pick_next_task_fair,
};
",
    ),
    (
        "kernel/sched/core.c",
        "#include \"sched.h\"
struct task_struct *pick(const struct sched_class *class, struct rq *rq) {
\treturn class->pick_next_task(rq);
}
",
    ),
];

fn codix(root: &Path, args: &[&str]) -> codix_testkit::Run {
    codix_in(CODIX, root, root, args)
}

#[test]
fn function_pointer_members_are_fields() {
    let (_guard, root) = indexed(CODIX, FILES);
    let out = codix(&root, &["outline", "include/sched.h"]);
    assert!(
        out.stdout.contains(
            "    4:9  field  void (*enqueue_task)(struct rq *rq, struct task_struct *p, int flags)\n    5:24  field  struct task_struct *(*pick_next_task)(struct rq *rq)\n"
        ),
        "{}",
        out.stdout
    );
}

#[test]
fn designated_initializers_fill_a_slot_and_name_its_function() {
    let (_guard, root) = indexed(CODIX, FILES);
    let writes = codix(
        &root,
        &["refs", "sched_class.pick_next_task", "--context", "write"],
    );
    assert_eq!(writes.code, Some(0), "{}", writes.stderr);
    assert!(
        writes
            .stdout
            .contains("kernel/sched/fair.c:6:3  write  fair_sched_class  textual\n"),
        "{}",
        writes.stdout
    );
    let calls = codix(&root, &["refs", "sched_class.pick_next_task"]);
    assert!(
        calls
            .stdout
            .contains("kernel/sched/core.c:3:16  call   pick              textual\n"),
        "{}",
        calls.stdout
    );
    let function = codix(&root, &["refs", "__pick_next_task_fair"]);
    assert!(
        function
            .stdout
            .contains("kernel/sched/fair.c:6:20  read  fair_sched_class  scoped\n"),
        "{}",
        function.stdout
    );
}
