//! The codix engine: operation layer, index store, scanner, parser pool,
//! resolver framework and edit engine.

pub mod build;
pub mod capabilities;
pub mod config;
pub mod daemon;
pub mod describe;
pub mod detect;
pub mod dispatch;
pub mod edit;
pub mod error;
pub mod indexing;
pub mod lock;
pub mod op;
pub mod page;
pub mod parse;
pub mod preflight;
pub mod project;
pub mod providers;
pub mod query;
pub mod refs;
pub mod render;
pub mod scan;
pub mod scope;
pub mod store;
pub mod symbol_id;
pub mod vcs;
pub mod walk;

pub use error::Error;
