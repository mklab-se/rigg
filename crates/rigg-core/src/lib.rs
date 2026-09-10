//! rigg-core - Core types and logic for Azure AI Search and Microsoft Foundry configuration management
//!
//! This crate provides:
//! - Resource trait definitions and models (Search and Foundry)
//! - Workspace and project model
//! - JSON normalization
//! - Constraint validation

pub mod binding;
pub mod graph;
pub mod identity;
pub mod infra;
pub mod migrate;
pub mod normalize;
pub mod openapi;
pub mod promote;
pub mod registry;
pub mod resources;
pub mod scaffold;
pub mod schema;
pub mod service;
pub mod sidecar;
pub mod store;
pub mod workspace;

pub use resources::ResourceKind;
pub use service::ServiceDomain;
