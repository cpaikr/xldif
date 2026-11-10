//! `xldif`: reading and content-aligned comparison of Excel workbooks.
//!
//! The library owns every workbook rule and has no process I/O; the
//! `xldif-cli` crate adapts it to the process contract. See `ARCHITECTURE.md`
//! for the module map and `PLAN.md` for the milestones.

pub mod cellref;
mod error;
pub mod model;
mod package;
pub mod reader;
pub mod views;
mod xml;

pub use error::{Error, ErrorCode};
pub use reader::{read_bytes, read_path};
