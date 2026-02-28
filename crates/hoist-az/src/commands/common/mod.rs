//! Shared utilities used by multiple commands.

mod fields;
mod ordering;
mod selection;

pub use fields::{get_read_only_fields, get_volatile_fields};
pub use ordering::{order_by_dependencies, read_agent_yaml};
pub use selection::{ResourceSelection, SingularFlags, resolve_resource_selection_from_flags};

// Test-only re-export
#[cfg(test)]
pub use selection::resolve_resource_kinds;
