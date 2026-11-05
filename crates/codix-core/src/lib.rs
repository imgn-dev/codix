//! The codix engine: operation layer, index store, scanner, parser pool,
//! resolver framework and edit engine.

pub mod config;
pub mod debug;
pub mod error;
pub mod op;
pub mod page;
pub mod project;
pub mod providers;
pub mod vcs;
pub mod walk;

pub use error::Error;
