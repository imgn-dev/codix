//! `previews` and `discard`: the pending edit plans, and dropping them.
//! Previews expire after `previews.expiry`, or once an edited region
//! changes; both are removed when found.

use std::fs;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::config::Config;
use crate::edit::{PreviewState, file_state};
use crate::op::{Context, Operation, Outcome};
use crate::project::{FileId, Project};
use crate::render::since_epoch;
use crate::store::{PreviewRow, Store};

/// How a pending preview stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Applies as planned.
    Ready,
    /// A file changed outside the edited regions: `apply` rebases it.
    Rebase,
    /// An edited region changed: plan it again. Removed once listed.
    Stale,
    /// Older than `previews.expiry`. Removed once listed.
    Expired,
}

/// One pending preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PreviewSummary {
    /// Its ID.
    pub id: String,
    /// What it does.
    pub description: String,
    /// Seconds since it was planned.
    pub age: u64,
    /// Files it edits.
    pub files: usize,
    /// Sites it edits.
    pub sites: usize,
    /// How it stands.
    pub state: State,
}

/// Whether a preview is past the expiry.
pub(crate) fn expired(row: &PreviewRow, expiry: Duration, now: u64) -> bool {
    now.saturating_sub(row.created) > expiry.as_secs()
}

/// How a preview's files stand now.
pub(crate) fn state(project: &Project, row: &PreviewRow) -> PreviewState {
    row.files
        .iter()
        .map(|planned| {
            let path = project.path_of(&FileId::from_canonical(planned.path.as_str()));
            file_state(planned, fs::read_to_string(path).ok().as_deref())
        })
        .max()
        .unwrap_or(PreviewState::Ready)
}

fn project_and_config(ctx: &Context) -> Result<(&Project, Config), Error> {
    let project = ctx.project()?;
    project.require_indexed()?;
    Ok((project, Config::load(project)?))
}

/// `previews`: the pending previews.
pub struct PreviewsOp;

/// Request for [`PreviewsOp`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PreviewsRequest {}

/// What [`PreviewsOp`] answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PreviewsResponse {
    /// Pending previews, oldest first.
    pub previews: Vec<PreviewSummary>,
}

impl Outcome for PreviewsResponse {
    fn has_results(&self) -> bool {
        !self.previews.is_empty()
    }
}

impl Operation for PreviewsOp {
    const NAME: &'static str = "previews";
    type Request = PreviewsRequest;
    type Response = PreviewsResponse;

    fn run(&self, ctx: &Context, _: PreviewsRequest) -> Result<PreviewsResponse, Error> {
        let (project, config) = project_and_config(ctx)?;
        let mut store = Store::open(&project.index_path())?;
        let now = since_epoch();
        let mut previews = Vec::new();
        for row in store.previews()? {
            let state = if expired(&row, config.preview_expiry(), now) {
                State::Expired
            } else {
                match state(project, &row) {
                    PreviewState::Ready => State::Ready,
                    PreviewState::Rebase => State::Rebase,
                    PreviewState::Stale => State::Stale,
                }
            };
            if matches!(state, State::Stale | State::Expired) {
                store.delete_preview(&row.response.id)?;
            }
            previews.push(PreviewSummary {
                id: row.response.id.clone(),
                description: row.response.description.clone(),
                age: now.saturating_sub(row.created),
                files: row.response.files,
                sites: row.response.sites,
                state,
            });
        }
        Ok(PreviewsResponse { previews })
    }
}

/// `discard`: drops previews.
pub struct DiscardOp;

/// Request for [`DiscardOp`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DiscardRequest {
    /// Preview IDs to drop.
    #[serde(default)]
    pub ids: Vec<String>,
    /// Drop every pending preview.
    #[serde(default)]
    pub all: bool,
}

/// What [`DiscardOp`] answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DiscardResponse {
    /// The previews dropped.
    pub discarded: Vec<String>,
}

impl Outcome for DiscardResponse {
    fn has_results(&self) -> bool {
        !self.discarded.is_empty()
    }
}

/// An unknown or no longer stored preview ID.
pub(crate) fn unknown_preview(id: &str) -> Error {
    Error::Refused {
        code: "unknown_preview",
        cause: format!("no pending preview `{id}`"),
        hint: "list pending previews with `codix previews`; expired and stale ones are removed"
            .to_owned(),
        sites: Vec::new(),
    }
}

impl Operation for DiscardOp {
    const NAME: &'static str = "discard";
    type Request = DiscardRequest;
    type Response = DiscardResponse;

    fn run(&self, ctx: &Context, request: DiscardRequest) -> Result<DiscardResponse, Error> {
        let (project, _) = project_and_config(ctx)?;
        if request.all != request.ids.is_empty() {
            return Err(Error::Usage {
                cause: "name the previews to discard, or pass --all, not both".to_owned(),
                hint: "codix discard <id>… or codix discard --all".to_owned(),
            });
        }
        let mut store = Store::open(&project.index_path())?;
        let ids = if request.all {
            store
                .previews()?
                .into_iter()
                .map(|row| row.response.id)
                .collect()
        } else {
            request.ids
        };
        if let Some(unknown) = ids
            .iter()
            .find(|id| store.preview(id).map_or(true, |row| row.is_none()))
        {
            return Err(unknown_preview(unknown));
        }
        for id in &ids {
            store.delete_preview(id)?;
        }
        Ok(DiscardResponse { discarded: ids })
    }
}
