//! The Daedalus domain model.
//!
//! Pure types and pure functions: no async runtime, no database, no network.
//! That is what lets the interesting logic — validation, diffing, graph
//! ordering — be tested in microseconds without fixtures. See D-012 in
//! `docs/decisions.md`; `just core-purity` enforces it.

pub mod diagnostic;
mod document;
pub mod error;
mod load;
mod manifest;
pub mod name;
pub mod quantity;
mod resolve;
pub mod resource;
mod suggest;
mod yaml;

pub use diagnostic::{Code, Source, ValidationError};
pub use error::{CoreError, Result};
pub use load::load;
pub use name::Name;
pub use quantity::ByteSize;
pub use resource::{Kind, Repository, Resource, ResourceKey, Scope};
