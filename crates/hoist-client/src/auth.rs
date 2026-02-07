//! Azure authentication providers

use std::process::Command;
use thiserror::Error;

/// Authentication errors
#[derive(Debug, Error)]
pub enum AuthError {
    #[error("Azure CLI not found. Please install it: https://docs.microsoft.com/cli/azure/install-azure-cli")]
    AzCliNotFound,
    #[error("Not logged in to Azure CLI. Run: az login")]
    NotLoggedIn,
    #[error("Failed to get access token: {0}")]
    TokenError(String),
    #[error("Missing environment variable: {0}")]
    MissingEnvVar(String),
    #[error("Authentication failed: {0}")]
    AuthFailed(String),
}

/// Authentication provider trait
pub trait AuthProvider: Send + Sync {
    /// Get an access token for Azure Search
    fn get_token(&self) -> Result<String, AuthError>;

    /// Get the authentication method name
    fn method_name(&self) -> &'static str;
}

/// Azure CLI authentication provider
pub struct AzCliAuth;

impl AzCliAuth {
    pub fn new() -> Self {
        Self
    }

    /// Check if Azure CLI is available and logged in
    pub fn check_status() -> Result<AuthStatus, AuthError> {
        // Check if az CLI is installed
        let version_output = Command::new("az").arg("--version").output();

        if version_output.is_err() {
            return Err(AuthError::AzCliNotFound);
        }

        // Check if logged in
        let account_output = Command::new("az")
            .args(["account", "show", "--output", "json"])
            .output()
            .map_err(|e| AuthError::TokenError(e.to_string()))?;

        if !account_output.status.success() {
            return Err(AuthError::NotLoggedIn);
        }

        // Parse account info
        let account_json: serde_json::Value = serde_json::from_slice(&account_output.stdout)
            .map_err(|e| AuthError::TokenError(e.to_string()))?;

        Ok(AuthStatus {
            logged_in: true,
            user: account_json
                .get("user")
                .and_then(|u| u.get("name"))
                .and_then(|n| n.as_str())
                .map(String::from),
            subscription: account_json
                .get("name")
                .and_then(|n| n.as_str())
                .map(String::from),
            subscription_id: account_json
                .get("id")
                .and_then(|i| i.as_str())
                .map(String::from),
        })
    }

    /// Get an access token for Azure Resource Manager (management.azure.com)
    pub fn get_arm_token() -> Result<String, AuthError> {
        let output = Command::new("az")
            .args([
                "account",
                "get-access-token",
                "--resource",
                "https://management.azure.com",
                "--query",
                "accessToken",
                "--output",
                "tsv",
            ])
            .output()
            .map_err(|e| AuthError::TokenError(e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("not logged in") || stderr.contains("AADSTS") {
                return Err(AuthError::NotLoggedIn);
            }
            return Err(AuthError::TokenError(stderr.to_string()));
        }

        let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if token.is_empty() {
            return Err(AuthError::TokenError(
                "Empty ARM token received".to_string(),
            ));
        }

        Ok(token)
    }
}

impl Default for AzCliAuth {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthProvider for AzCliAuth {
    fn get_token(&self) -> Result<String, AuthError> {
        let output = Command::new("az")
            .args([
                "account",
                "get-access-token",
                "--resource",
                "https://search.azure.com",
                "--query",
                "accessToken",
                "--output",
                "tsv",
            ])
            .output()
            .map_err(|e| AuthError::TokenError(e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("not logged in") || stderr.contains("AADSTS") {
                return Err(AuthError::NotLoggedIn);
            }
            return Err(AuthError::TokenError(stderr.to_string()));
        }

        let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if token.is_empty() {
            return Err(AuthError::TokenError("Empty token received".to_string()));
        }

        Ok(token)
    }

    fn method_name(&self) -> &'static str {
        "Azure CLI"
    }
}

/// Environment variable authentication provider
pub struct EnvAuth {
    client_id: String,
    client_secret: String,
    tenant_id: String,
}

impl EnvAuth {
    /// Create from environment variables
    pub fn from_env() -> Result<Self, AuthError> {
        let client_id = std::env::var("AZURE_CLIENT_ID")
            .map_err(|_| AuthError::MissingEnvVar("AZURE_CLIENT_ID".to_string()))?;
        let client_secret = std::env::var("AZURE_CLIENT_SECRET")
            .map_err(|_| AuthError::MissingEnvVar("AZURE_CLIENT_SECRET".to_string()))?;
        let tenant_id = std::env::var("AZURE_TENANT_ID")
            .map_err(|_| AuthError::MissingEnvVar("AZURE_TENANT_ID".to_string()))?;

        Ok(Self {
            client_id,
            client_secret,
            tenant_id,
        })
    }

    /// Check if environment variables are set
    pub fn is_configured() -> bool {
        std::env::var("AZURE_CLIENT_ID").is_ok()
            && std::env::var("AZURE_CLIENT_SECRET").is_ok()
            && std::env::var("AZURE_TENANT_ID").is_ok()
    }
}

impl AuthProvider for EnvAuth {
    fn get_token(&self) -> Result<String, AuthError> {
        // Use Azure CLI to get token with service principal
        let output = Command::new("az")
            .args([
                "account",
                "get-access-token",
                "--resource",
                "https://search.azure.com",
                "--query",
                "accessToken",
                "--output",
                "tsv",
                "--tenant",
                &self.tenant_id,
                "--username",
                &self.client_id,
            ])
            .env("AZURE_CLIENT_SECRET", &self.client_secret)
            .output()
            .map_err(|e| AuthError::TokenError(e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(AuthError::AuthFailed(stderr.to_string()));
        }

        let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(token)
    }

    fn method_name(&self) -> &'static str {
        "Environment Variables (Service Principal)"
    }
}

/// Authentication status
#[derive(Debug, Clone)]
pub struct AuthStatus {
    pub logged_in: bool,
    pub user: Option<String>,
    pub subscription: Option<String>,
    pub subscription_id: Option<String>,
}

/// Get the best available authentication provider
pub fn get_auth_provider() -> Result<Box<dyn AuthProvider>, AuthError> {
    // First try environment variables
    if EnvAuth::is_configured() {
        return Ok(Box::new(EnvAuth::from_env()?));
    }

    // Fall back to Azure CLI
    AzCliAuth::check_status()?;
    Ok(Box::new(AzCliAuth::new()))
}
