//! `debug-echo`: a stub operation that exercises rendering, paging, sorting and
//! every error class until the real commands exist.

use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::op::{Context, Operation};
use crate::page::{Location, Page, Paged, Ranked, sort_results};
use crate::project::{FileId, ScopeRequest};

/// The `debug-echo` operation.
pub struct DebugEcho;

/// Request for [`DebugEcho`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugEchoRequest {
    /// Number of synthetic results to produce.
    pub count: usize,
    /// File the results are sorted around, if any.
    pub anchor: Option<String>,
    /// Error code to fail with instead of answering.
    pub fail: Option<String>,
    /// Directories to limit results to.
    #[serde(default)]
    pub scope: ScopeRequest,
    /// Page to return.
    pub page: Page,
}

/// One synthetic result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EchoItem {
    /// Where the result is.
    #[serde(flatten)]
    pub location: Location,
    /// `def` or `call`.
    pub kind: String,
    /// Symbol name.
    pub name: String,
}

impl Ranked for EchoItem {
    fn is_definition(&self) -> bool {
        self.kind == "def"
    }

    fn location(&self) -> &Location {
        &self.location
    }
}

impl Operation for DebugEcho {
    const NAME: &'static str = "debug-echo";
    type Request = DebugEchoRequest;
    type Response = Paged<EchoItem>;

    fn run(&self, ctx: &Context, request: DebugEchoRequest) -> Result<Paged<EchoItem>, Error> {
        if let Some(code) = &request.fail {
            return Err(sample_error(ctx, code));
        }
        let scope = ctx.scope(&request.scope)?;
        let mut items: Vec<EchoItem> = (0..request.count)
            .map(|i| item(ctx, i))
            .filter(|item| scope.contains(&item.location.root_path))
            .collect();
        sort_results(&mut items, request.anchor.as_deref());
        Ok(request.page.apply(items))
    }
}

fn item(ctx: &Context, i: usize) -> EchoItem {
    let n = u32::try_from(i).unwrap_or(u32::MAX);
    let dir = if i % 4 == 3 { "tests" } else { "src" };
    let file = FileId::from_canonical(format!("{dir}/file{}.rs", i % 4));
    EchoItem {
        location: ctx.location(&file, 10 + n, 1 + n % 5),
        kind: if i.is_multiple_of(7) { "def" } else { "call" }.to_owned(),
        name: format!("item{i}"),
    }
}

fn sample_error(ctx: &Context, code: &str) -> Error {
    match code {
        "no_project" => Error::NoProject {
            start: ctx.cwd().to_path_buf(),
        },
        "not_indexed" => Error::NotIndexed {
            root: PathBuf::from("/work/project"),
        },
        "ambiguous_symbol" => Error::AmbiguousSymbol {
            query: "total".to_owned(),
            candidates: vec![
                "py:src/billing/invoice.py::Invoice.total#a3f9".to_owned(),
                "py:src/billing/cart.py::Cart.total#71c2".to_owned(),
            ],
        },
        "unknown_symbol" => Error::UnknownSymbol {
            query: "totl".to_owned(),
        },
        "invalid_config" => Error::InvalidConfig {
            cause: "unknown key `previews.expiri`".to_owned(),
            hint: "valid keys: previews.expiry".to_owned(),
        },
        "not_supported_in_build" => Error::NotSupportedInBuild {
            provider: "kotlin".to_owned(),
            path: "src/Main.kt".to_owned(),
        },
        "io" => Error::Io {
            path: PathBuf::from("src/missing.rs"),
            source: io::Error::new(io::ErrorKind::NotFound, "file not found"),
        },
        "internal" => Error::Internal {
            message: "sample internal error".to_owned(),
        },
        other => Error::Usage {
            cause: format!("unknown error code `{other}`"),
            hint: "pass one of the stable error codes".to_owned(),
        },
    }
}
