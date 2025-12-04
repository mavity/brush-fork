//! Platform abstraction facilities

#![allow(unused)]

#[cfg(unix)]
pub(crate) mod unix;
#[cfg(unix)]
pub(crate) use unix as platform;

#[cfg(windows)]
pub(crate) mod windows;
#[cfg(windows)]
pub(crate) use windows as platform;

#[cfg(target_family = "wasm")]
#[path = "sys/wasm_shim.rs"]
pub(crate) mod wasm;
#[cfg(target_family = "wasm")]
pub(crate) use wasm as platform;

// When testing the wasm shim on a native host, make the `wasm` module public
// so integration tests can access it.

#[cfg(not(unix))]
pub(crate) mod stubs;

#[cfg(any(unix, windows))]
pub(crate) mod hostname;
#[cfg(any(unix, windows))]
pub mod tokio_process;

pub mod fs;

pub use platform::commands;
pub use platform::fd;
pub use platform::input;
pub(crate) use platform::network;
pub use platform::process;
pub use platform::resource;
pub use platform::signal;
pub use platform::terminal;
pub(crate) use platform::users;

pub use platform::PlatformError;
