//! Fshell-owned terminal runtime boundary.
//!
//! Owns terminal input normalization, process-level lifecycle guards,
//! scoped terminal sessions (fullscreen and inline), and the unified TUI runner.

pub mod ansi;
pub mod backend;
pub mod input;
pub mod lifecycle;
pub mod parse;
pub mod raw;
pub mod runner;
pub mod session;
#[cfg(unix)]
pub mod unix;

pub use ansi::*;
pub use backend::*;
pub use input::*;
pub use lifecycle::*;
pub use raw::*;
pub use runner::*;
pub use session::*;
