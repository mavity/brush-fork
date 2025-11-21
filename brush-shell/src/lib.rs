//! Create for brush, an executable bash-compatible shell.

#![allow(dead_code)]

pub mod args;
mod brushctl;
pub mod entry;
mod error_formatter;
pub mod events;
mod productinfo;
mod shell_factory;

/// Embedded shell entry point.
pub mod embedded;

pub use brush_builtins;
pub use brush_core;
