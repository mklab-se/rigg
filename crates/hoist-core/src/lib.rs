//! hoist-core - Core business logic for Azure AI Search configuration management
//!
//! This crate provides:
//! - Resource trait definitions and models
//! - Configuration management
//! - JSON normalization
//! - Constraint validation

pub mod config;
pub mod constraints;
pub mod copy;
pub mod normalize;
pub mod resources;
pub mod state;
pub mod templates;

pub use config::Config;
pub use resources::ResourceKind;
