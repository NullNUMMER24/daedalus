//! The Daedalus domain model.
//!
//! Pure types and pure functions: no async runtime, no database, no network.
//! That is what lets the interesting logic — validation, diffing, graph
//! ordering — be tested in microseconds without fixtures. See D-012 in
//! `docs/decisions.md`; `just core-purity` enforces it.

pub mod error;
pub mod name;
pub mod quantity;

pub use error::{CoreError, Result, ValidationError};
pub use name::Name;
pub use quantity::ByteSize;
