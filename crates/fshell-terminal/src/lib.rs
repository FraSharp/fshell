//! Fshell-owned terminal runtime boundary.
//!
//! Owns terminal input normalization, process-level lifecycle guards,
//! scoped terminal sessions (fullscreen and inline), and the unified TUI runner.

pub mod input;
pub mod lifecycle;
pub mod runner;
pub mod session;

pub use input::*;
pub use lifecycle::*;
pub use runner::*;
pub use session::*;
