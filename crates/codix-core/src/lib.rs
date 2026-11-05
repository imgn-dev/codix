//! The codix engine: operation layer, index store, scanner, parser pool,
//! resolver framework and edit engine.

pub mod build;
pub mod capabilities;
pub mod config;
pub mod daemon;
pub mod detect;
pub mod dispatch;
pub mod error;
pub mod indexing;
pub mod lock;
pub mod op;
pub mod page;
pub mod parse;
pub mod project;
pub mod providers;
pub mod query;
pub mod scan;
pub mod store;
pub mod symbol_id;
pub mod vcs;
pub mod walk;

pub use error::Error;
