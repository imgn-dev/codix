//! The per-project codix daemon: it holds the project's write lock, serves
//! requests over a local socket, and registers itself so every daemon of the
//! user can be listed and stopped.

pub mod client;
pub mod control;
pub mod paths;
pub mod protocol;
pub mod registry;
pub mod server;
pub mod watch;
