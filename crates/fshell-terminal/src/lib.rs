//! Fshell-owned terminal input semantics backed by Crossterm.
//!
//! This crate owns event acquisition and normalization. Terminal-mode
//! lifetimes and Ratatui rendering remain with their existing owners.

pub mod input;
