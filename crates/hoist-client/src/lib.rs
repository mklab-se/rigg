//! hoist-client - Azure AI Search and Microsoft Foundry REST API client
//!
//! This crate provides:
//! - Authentication handling (Azure CLI, environment variables)
//! - REST API operations for all resource types (Search and Foundry)
//! - ARM discovery for subscriptions, search services, AI Services accounts, and Foundry projects
//! - Response parsing and error handling

pub mod ai;
pub mod arm;
pub mod auth;
pub mod client;
pub mod error;
pub mod foundry;
pub mod local_agent;
pub mod ollama;
pub mod openai;

pub use arm::ArmClient;
pub use auth::AuthProvider;
pub use client::AzureSearchClient;
pub use error::ClientError;
pub use foundry::FoundryClient;
pub use openai::AzureOpenAIClient;
