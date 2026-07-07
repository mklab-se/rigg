//! Resource kind definitions. Per-kind metadata lives in [`crate::registry`].

pub mod traits;

pub use traits::{Resource, ResourceKind, ResourceRef, validate_resource_name};
