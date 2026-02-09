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
pub struct AzCliAuth {
    resource_scope: &'static str,
}

impl AzCliAuth {
    /// Create an auth provider for Azure Search
    pub fn for_search() -> Self {
        Self {
            resource_scope: "https://search.azure.com",
        }
    }

    /// Create an auth provider for Microsoft Foundry
    pub fn for_foundry() -> Self {
        Self {
            resource_scope: "https://ai.azure.com",
        }
    }

    /// Create a new auth provider (defaults to Search scope for backward compatibility)
    pub fn new() -> Self {
        Self::for_search()
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
                self.resource_scope,
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
#[derive(Debug)]
pub struct EnvAuth {
    client_id: String,
    client_secret: String,
    tenant_id: String,
    resource_scope: &'static str,
}

impl EnvAuth {
    /// Create from environment variables (defaults to Search scope)
    pub fn from_env() -> Result<Self, AuthError> {
        Self::from_env_for_scope("https://search.azure.com")
    }

    /// Create from environment variables for a specific resource scope
    pub fn from_env_for_scope(scope: &'static str) -> Result<Self, AuthError> {
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
            resource_scope: scope,
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
                self.resource_scope,
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

/// Get the best available authentication provider for Search (backward compat)
pub fn get_auth_provider() -> Result<Box<dyn AuthProvider>, AuthError> {
    get_auth_provider_for_scope("https://search.azure.com")
}

/// Get the best available authentication provider for a specific service domain
pub fn get_auth_provider_for(
    domain: hoist_core::ServiceDomain,
) -> Result<Box<dyn AuthProvider>, AuthError> {
    let scope = match domain {
        hoist_core::ServiceDomain::Search => "https://search.azure.com",
        hoist_core::ServiceDomain::Foundry => "https://ai.azure.com",
    };
    get_auth_provider_for_scope(scope)
}

/// Get the best available authentication provider for a specific resource scope
fn get_auth_provider_for_scope(scope: &'static str) -> Result<Box<dyn AuthProvider>, AuthError> {
    // First try environment variables
    if EnvAuth::is_configured() {
        return Ok(Box::new(EnvAuth::from_env_for_scope(scope)?));
    }

    // Fall back to Azure CLI
    AzCliAuth::check_status()?;
    Ok(Box::new(AzCliAuth {
        resource_scope: scope,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Env var tests must run serially since they share process-wide state.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    fn clear_azure_env_vars() {
        std::env::remove_var("AZURE_CLIENT_ID");
        std::env::remove_var("AZURE_CLIENT_SECRET");
        std::env::remove_var("AZURE_TENANT_ID");
    }

    fn set_azure_env_vars() {
        std::env::set_var("AZURE_CLIENT_ID", "test-client-id");
        std::env::set_var("AZURE_CLIENT_SECRET", "test-client-secret");
        std::env::set_var("AZURE_TENANT_ID", "test-tenant-id");
    }

    #[test]
    fn test_env_auth_from_env_success() {
        let _lock = ENV_MUTEX.lock().unwrap();
        set_azure_env_vars();

        let result = EnvAuth::from_env();
        assert!(result.is_ok());
        let auth = result.unwrap();
        assert_eq!(auth.client_id, "test-client-id");
        assert_eq!(auth.client_secret, "test-client-secret");
        assert_eq!(auth.tenant_id, "test-tenant-id");

        clear_azure_env_vars();
    }

    #[test]
    fn test_env_auth_from_env_missing_client_id() {
        let _lock = ENV_MUTEX.lock().unwrap();
        clear_azure_env_vars();
        std::env::set_var("AZURE_CLIENT_SECRET", "test-secret");
        std::env::set_var("AZURE_TENANT_ID", "test-tenant");

        let result = EnvAuth::from_env();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, AuthError::MissingEnvVar(ref v) if v == "AZURE_CLIENT_ID"));

        clear_azure_env_vars();
    }

    #[test]
    fn test_env_auth_from_env_missing_client_secret() {
        let _lock = ENV_MUTEX.lock().unwrap();
        clear_azure_env_vars();
        std::env::set_var("AZURE_CLIENT_ID", "test-id");
        std::env::set_var("AZURE_TENANT_ID", "test-tenant");

        let result = EnvAuth::from_env();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, AuthError::MissingEnvVar(ref v) if v == "AZURE_CLIENT_SECRET"));

        clear_azure_env_vars();
    }

    #[test]
    fn test_env_auth_from_env_missing_tenant_id() {
        let _lock = ENV_MUTEX.lock().unwrap();
        clear_azure_env_vars();
        std::env::set_var("AZURE_CLIENT_ID", "test-id");
        std::env::set_var("AZURE_CLIENT_SECRET", "test-secret");

        let result = EnvAuth::from_env();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, AuthError::MissingEnvVar(ref v) if v == "AZURE_TENANT_ID"));

        clear_azure_env_vars();
    }

    #[test]
    fn test_env_auth_is_configured_all_set() {
        let _lock = ENV_MUTEX.lock().unwrap();
        set_azure_env_vars();

        assert!(EnvAuth::is_configured());

        clear_azure_env_vars();
    }

    #[test]
    fn test_env_auth_is_configured_none_set() {
        let _lock = ENV_MUTEX.lock().unwrap();
        clear_azure_env_vars();

        assert!(!EnvAuth::is_configured());
    }

    #[test]
    fn test_env_auth_is_configured_partial() {
        let _lock = ENV_MUTEX.lock().unwrap();
        clear_azure_env_vars();
        std::env::set_var("AZURE_CLIENT_ID", "test-id");
        std::env::set_var("AZURE_CLIENT_SECRET", "test-secret");
        // AZURE_TENANT_ID intentionally missing

        assert!(!EnvAuth::is_configured());

        clear_azure_env_vars();
    }

    #[test]
    fn test_env_auth_method_name() {
        let _lock = ENV_MUTEX.lock().unwrap();
        set_azure_env_vars();

        let auth = EnvAuth::from_env().unwrap();
        assert_eq!(
            auth.method_name(),
            "Environment Variables (Service Principal)"
        );

        clear_azure_env_vars();
    }

    #[test]
    fn test_az_cli_auth_method_name() {
        let auth = AzCliAuth::new();
        assert_eq!(auth.method_name(), "Azure CLI");
    }

    #[test]
    fn test_az_cli_auth_search_scope() {
        let auth = AzCliAuth::for_search();
        assert_eq!(auth.resource_scope, "https://search.azure.com");
    }

    #[test]
    fn test_az_cli_auth_foundry_scope() {
        let auth = AzCliAuth::for_foundry();
        assert_eq!(auth.resource_scope, "https://ai.azure.com");
    }

    #[test]
    fn test_az_cli_auth_new_defaults_to_search() {
        let auth = AzCliAuth::new();
        assert_eq!(auth.resource_scope, "https://search.azure.com");
    }

    #[test]
    fn test_env_auth_from_env_scope_foundry() {
        let _lock = ENV_MUTEX.lock().unwrap();
        set_azure_env_vars();

        let result = EnvAuth::from_env_for_scope("https://ai.azure.com");
        assert!(result.is_ok());
        let auth = result.unwrap();
        assert_eq!(auth.resource_scope, "https://ai.azure.com");

        clear_azure_env_vars();
    }

    #[test]
    fn test_env_auth_from_env_default_scope_is_search() {
        let _lock = ENV_MUTEX.lock().unwrap();
        set_azure_env_vars();

        let auth = EnvAuth::from_env().unwrap();
        assert_eq!(auth.resource_scope, "https://search.azure.com");

        clear_azure_env_vars();
    }

    #[test]
    fn test_auth_status_fields() {
        let status = AuthStatus {
            logged_in: true,
            user: Some("testuser@example.com".to_string()),
            subscription: Some("My Subscription".to_string()),
            subscription_id: Some("00000000-0000-0000-0000-000000000000".to_string()),
        };

        assert!(status.logged_in);
        assert_eq!(status.user.as_deref(), Some("testuser@example.com"));
        assert_eq!(status.subscription.as_deref(), Some("My Subscription"));
        assert_eq!(
            status.subscription_id.as_deref(),
            Some("00000000-0000-0000-0000-000000000000")
        );
    }
}
