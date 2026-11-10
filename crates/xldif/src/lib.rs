//! `xldif`: reading and content-aligned comparison of Excel workbooks.
//!
//! The library owns every workbook rule and has no process I/O; the
//! `xldif-cli` crate adapts it to the process contract. See `ARCHITECTURE.md`
//! for the module map and `PLAN.md` for the milestones.

pub mod align;
pub mod cellref;
pub mod diff;
mod diff_objects;
mod error;
pub mod formula;
pub mod html;
pub mod model;
mod package;
pub mod reader;
mod sheet_objects;
pub mod styles;
pub mod views;
mod xml;

pub use error::{Error, ErrorCode};
pub use package::Limits;
pub use reader::{read_bytes, read_bytes_with, read_path, read_path_with};
