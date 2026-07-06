//! rigg-core - Core types and logic for Azure AI Search and Microsoft Foundry configuration management
//!
//! This crate provides:
//! - Resource trait definitions and models (Search and Foundry)
//! - Configuration management
//! - JSON normalization
//! - Constraint validation

pub mod config;
pub mod constraints;
pub mod copy;
pub mod graph;
pub mod normalize;
pub mod registry;
pub mod resources;
pub mod scaffold;
pub mod service;
pub mod sidecar;
pub mod state;
pub mod templates;
pub mod workspace;

pub use config::{
    Config, ConfigError, EnvironmentConfig, FoundryServiceConfig, ResolvedEnvironment,
    SearchServiceConfig, SyncConfig,
};
pub use resources::ResourceKind;
pub use service::ServiceDomain;
