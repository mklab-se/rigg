//! hoist-client - Azure AI Search REST API client
//!
//! This crate provides:
//! - Authentication handling (Azure CLI, environment variables)
//! - REST API operations for all resource types
//! - Response parsing and error handling

pub mod arm;
pub mod auth;
pub mod client;
pub mod error;

pub use arm::ArmClient;
pub use auth::AuthProvider;
pub use client::AzureSearchClient;
pub use error::ClientError;
