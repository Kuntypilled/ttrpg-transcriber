//! The terminal front end, as a library so the whole program can be driven by tests.
//! `main.rs` only wires it to a real terminal.

pub mod app;
pub mod log;
pub mod picker;
pub mod resolve;
pub mod text;
pub mod ui;
