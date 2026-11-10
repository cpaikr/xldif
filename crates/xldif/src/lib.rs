//! `xldif`: reading and content-aligned comparison of Excel workbooks.
//!
//! The library owns every workbook rule and has no process I/O; the
//! `xldif-cli` crate adapts it to the process contract. See `ARCHITECTURE.md`
//! for the planned modules and `PLAN.md` for the milestone they arrive in.

mod error;

pub use error::{Error, ErrorCode};
