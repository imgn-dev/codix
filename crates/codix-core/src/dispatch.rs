//! Every operation by name, with requests and responses as JSON: how a daemon
//! runs what a client sends it, through the same code as in-process.

use serde_json::Value;

use crate::Error;
use crate::capabilities::{CapabilitiesOp, LanguagesOp};
use crate::config::ConfigOp;
use crate::indexing::{IndexOp, InitOp, StatusOp};
use crate::op::{Context, Operation};
use crate::previews::{DiscardOp, PreviewsOp};
use crate::query::{DefOp, FindOp, GrepOp, OutlineOp, ShowOp};
use crate::refs::RefsOp;

macro_rules! operations {
    ($($op:ident),* $(,)?) => {
        /// The name of every operation.
        pub const OPERATIONS: &[&str] = &[$(<$op as Operation>::NAME),*];

        /// Every operation's name, request schema and response schema.
        pub fn schemas() -> Vec<(&'static str, schemars::Schema, schemars::Schema)> {
            vec![$((
                <$op as Operation>::NAME,
                schemars::schema_for!(<$op as Operation>::Request),
                schemars::schema_for!(<$op as Operation>::Response),
            )),*]
        }

        /// Runs the operation `name` on `request`.
        pub fn dispatch(ctx: &Context, name: &str, request: Value) -> Result<Value, Error> {
            $(
                if name == <$op as Operation>::NAME {
                    return run(&$op, ctx, request);
                }
            )*
            Err(Error::Usage {
                cause: format!("unknown operation `{name}`"),
                hint: format!("operations: {}", OPERATIONS.join(", ")),
            })
        }
    };
}

operations!(
    InitOp,
    IndexOp,
    StatusOp,
    CapabilitiesOp,
    LanguagesOp,
    ConfigOp,
    FindOp,
    DefOp,
    RefsOp,
    OutlineOp,
    ShowOp,
    GrepOp,
    PreviewsOp,
    DiscardOp,
);

fn run<O: Operation>(op: &O, ctx: &Context, request: Value) -> Result<Value, Error> {
    let request: O::Request = serde_json::from_value(request).map_err(|e| Error::Usage {
        cause: format!("invalid {} request: {e}", O::NAME),
        hint: "the client and the daemon disagree; restart the daemon".to_owned(),
    })?;
    let response = op.run(ctx, request)?;
    serde_json::to_value(response).map_err(|e| Error::Internal {
        message: format!("cannot serialize the {} response: {e}", O::NAME),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_operation_is_dispatched_by_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ctx = Context::discover(dir.path(), None).expect("context");
        for name in OPERATIONS {
            // A request of the wrong shape: refused by the operation's own
            // request type, not as an unknown operation.
            let error = dispatch(&ctx, name, Value::from(42)).expect_err(name);
            assert!(
                error
                    .cause()
                    .starts_with(&format!("invalid {name} request")),
                "{name}: {}",
                error.cause()
            );
        }
        let error = dispatch(&ctx, "frobnicate", Value::Null).expect_err("unknown");
        assert_eq!(error.cause(), "unknown operation `frobnicate`");
    }
}
