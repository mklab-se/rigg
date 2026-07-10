//! Azure authentication providers

use std::process::Command;
use thiserror::Error;

/// Authentication errors
#[derive(Debug, Error)]
pub enum AuthError {
    #[error(
        "Azure CLI not found. Please install it: https://docs.microsoft.com/cli/azure/install-azure-cli"
    )]
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

/// Build an actionable error for a failure to parse `az account show` output.
///
/// A bare serde error (e.g. `expected value at line 1 column 1`) gives the
/// user no idea what to do; this is almost always a transient Azure CLI
/// hiccup (extension update noise, empty stdout, etc.), so point them at the
/// obvious next steps.
fn account_parse_error(e: serde_json::Error) -> AuthError {
    AuthError::TokenError(format!(
        "could not parse `az account show` output ({e}); this is usually a \
         transient Azure CLI issue — try again, and if it persists run `az login`"
    ))
}

/// Turn `az`'s stderr into a `TokenError` detail, substituting an actionable
/// fallback message when stderr is empty or whitespace-only (which otherwise
/// surfaces to the user as a blank cause).
fn token_error_detail(stderr: &str, status: std::process::ExitStatus) -> String {
    if stderr.trim().is_empty() {
        format!(
            "az returned no error detail (exit {status}); usually transient — try again, or run `az login`"
        )
    } else {
        stderr.trim().to_string()
    }
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

    /// Create an auth provider for Azure Cognitive Services (OpenAI)
    pub fn for_cognitive_services() -> Self {
        Self {
            resource_scope: "https://cognitiveservices.azure.com",
        }
    }

    /// Create an auth provider for Azure Cosmos DB
    pub fn for_cosmos() -> Self {
        Self {
            resource_scope: "https://cosmos.azure.com",
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
        let account_json: serde_json::Value =
            serde_json::from_slice(&account_output.stdout).map_err(account_parse_error)?;

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
            return Err(AuthError::TokenError(token_error_detail(
                &stderr,
                output.status,
            )));
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
            if stderr.contains("not logged in") {
                return Err(AuthError::NotLoggedIn);
            }
            if stderr.contains("AADSTS") {
                // Extract the first AADSTS error line for a concise message
                let detail = stderr
                    .lines()
                    .find(|l| l.contains("AADSTS"))
                    .unwrap_or(&stderr)
                    .trim();
                return Err(AuthError::TokenError(format!(
                    "Failed to get access token for {}: {}\n  \
                     Debug: az account get-access-token --resource {}\n  \
                     Fix: Ensure 'Cognitive Services User' role is assigned on the AI Services resource",
                    self.resource_scope, detail, self.resource_scope
                )));
            }
            return Err(AuthError::TokenError(token_error_detail(
                &stderr,
                output.status,
            )));
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
    domain: rigg_core::ServiceDomain,
) -> Result<Box<dyn AuthProvider>, AuthError> {
    let scope = match domain {
        rigg_core::ServiceDomain::Search => "https://search.azure.com",
        rigg_core::ServiceDomain::Foundry => "https://ai.azure.com",
    };
    get_auth_provider_for_scope(scope)
}

/// Get the best available authentication provider for Azure Cognitive Services (OpenAI)
pub fn get_cognitive_services_auth() -> Result<Box<dyn AuthProvider>, AuthError> {
    get_auth_provider_for_scope("https://cognitiveservices.azure.com")
}

/// Static bearer token from the environment (`RIGG_ACCESS_TOKEN`).
/// Useful for CI systems that mint tokens out-of-band, and for tests.
pub struct StaticTokenAuth {
    token: String,
}

impl AuthProvider for StaticTokenAuth {
    fn get_token(&self) -> Result<String, AuthError> {
        Ok(self.token.clone())
    }
    fn method_name(&self) -> &'static str {
        "Static token (RIGG_ACCESS_TOKEN)"
    }
}

/// Get the best available authentication provider for a specific resource scope
fn get_auth_provider_for_scope(scope: &'static str) -> Result<Box<dyn AuthProvider>, AuthError> {
    // A pre-minted token wins over everything.
    if let Ok(token) = std::env::var("RIGG_ACCESS_TOKEN") {
        if !token.is_empty() {
            return Ok(Box::new(StaticTokenAuth { token }));
        }
    }
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

    /// # Safety
    /// Must be called while holding ENV_MUTEX to avoid data races.
    unsafe fn clear_azure_env_vars() {
        unsafe {
            std::env::remove_var("AZURE_CLIENT_ID");
            std::env::remove_var("AZURE_CLIENT_SECRET");
            std::env::remove_var("AZURE_TENANT_ID");
        }
    }

    /// # Safety
    /// Must be called while holding ENV_MUTEX to avoid data races.
    unsafe fn set_azure_env_vars() {
        unsafe {
            std::env::set_var("AZURE_CLIENT_ID", "test-client-id");
            std::env::set_var("AZURE_CLIENT_SECRET", "test-client-secret");
            std::env::set_var("AZURE_TENANT_ID", "test-tenant-id");
        }
    }

    #[test]
    fn test_env_auth_from_env_success() {
        let _lock = ENV_MUTEX.lock().unwrap();
        unsafe { set_azure_env_vars() };

        let result = EnvAuth::from_env();
        assert!(result.is_ok());
        let auth = result.unwrap();
        assert_eq!(auth.client_id, "test-client-id");
        assert_eq!(auth.client_secret, "test-client-secret");
        assert_eq!(auth.tenant_id, "test-tenant-id");

        unsafe { clear_azure_env_vars() };
    }

    #[test]
    fn test_env_auth_from_env_missing_client_id() {
        let _lock = ENV_MUTEX.lock().unwrap();
        unsafe {
            clear_azure_env_vars();
            std::env::set_var("AZURE_CLIENT_SECRET", "test-secret");
            std::env::set_var("AZURE_TENANT_ID", "test-tenant");
        }

        let result = EnvAuth::from_env();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, AuthError::MissingEnvVar(ref v) if v == "AZURE_CLIENT_ID"));

        unsafe { clear_azure_env_vars() };
    }

    #[test]
    fn test_env_auth_from_env_missing_client_secret() {
        let _lock = ENV_MUTEX.lock().unwrap();
        unsafe {
            clear_azure_env_vars();
            std::env::set_var("AZURE_CLIENT_ID", "test-id");
            std::env::set_var("AZURE_TENANT_ID", "test-tenant");
        }

        let result = EnvAuth::from_env();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, AuthError::MissingEnvVar(ref v) if v == "AZURE_CLIENT_SECRET"));

        unsafe { clear_azure_env_vars() };
    }

    #[test]
    fn test_env_auth_from_env_missing_tenant_id() {
        let _lock = ENV_MUTEX.lock().unwrap();
        unsafe {
            clear_azure_env_vars();
            std::env::set_var("AZURE_CLIENT_ID", "test-id");
            std::env::set_var("AZURE_CLIENT_SECRET", "test-secret");
        }

        let result = EnvAuth::from_env();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, AuthError::MissingEnvVar(ref v) if v == "AZURE_TENANT_ID"));

        unsafe { clear_azure_env_vars() };
    }

    #[test]
    fn test_env_auth_is_configured_all_set() {
        let _lock = ENV_MUTEX.lock().unwrap();
        unsafe { set_azure_env_vars() };

        assert!(EnvAuth::is_configured());

        unsafe { clear_azure_env_vars() };
    }

    #[test]
    fn test_env_auth_is_configured_none_set() {
        let _lock = ENV_MUTEX.lock().unwrap();
        unsafe { clear_azure_env_vars() };

        assert!(!EnvAuth::is_configured());
    }

    #[test]
    fn test_env_auth_is_configured_partial() {
        let _lock = ENV_MUTEX.lock().unwrap();
        unsafe {
            clear_azure_env_vars();
            std::env::set_var("AZURE_CLIENT_ID", "test-id");
            std::env::set_var("AZURE_CLIENT_SECRET", "test-secret");
        }
        // AZURE_TENANT_ID intentionally missing

        assert!(!EnvAuth::is_configured());

        unsafe { clear_azure_env_vars() };
    }

    #[test]
    fn test_env_auth_method_name() {
        let _lock = ENV_MUTEX.lock().unwrap();
        unsafe { set_azure_env_vars() };

        let auth = EnvAuth::from_env().unwrap();
        assert_eq!(
            auth.method_name(),
            "Environment Variables (Service Principal)"
        );

        unsafe { clear_azure_env_vars() };
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
    fn test_az_cli_auth_cognitive_services_scope() {
        let auth = AzCliAuth::for_cognitive_services();
        assert_eq!(auth.resource_scope, "https://cognitiveservices.azure.com");
    }

    #[test]
    fn test_az_cli_auth_new_defaults_to_search() {
        let auth = AzCliAuth::new();
        assert_eq!(auth.resource_scope, "https://search.azure.com");
    }

    #[test]
    fn test_env_auth_from_env_scope_foundry() {
        let _lock = ENV_MUTEX.lock().unwrap();
        unsafe { set_azure_env_vars() };

        let result = EnvAuth::from_env_for_scope("https://ai.azure.com");
        assert!(result.is_ok());
        let auth = result.unwrap();
        assert_eq!(auth.resource_scope, "https://ai.azure.com");

        unsafe { clear_azure_env_vars() };
    }

    #[test]
    fn test_env_auth_from_env_default_scope_is_search() {
        let _lock = ENV_MUTEX.lock().unwrap();
        unsafe { set_azure_env_vars() };

        let auth = EnvAuth::from_env().unwrap();
        assert_eq!(auth.resource_scope, "https://search.azure.com");

        unsafe { clear_azure_env_vars() };
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

    #[test]
    fn test_for_cosmos_uses_cosmos_scope() {
        let auth = AzCliAuth::for_cosmos();
        assert_eq!(auth.resource_scope, "https://cosmos.azure.com");
    }

    #[test]
    fn account_parse_error_is_actionable_not_raw_serde() {
        let serde_err = serde_json::from_slice::<serde_json::Value>(b"").unwrap_err();
        let err = account_parse_error(serde_err);
        match err {
            AuthError::TokenError(msg) => {
                assert!(msg.contains("az login"), "{msg}");
                assert!(msg.contains("transient"), "{msg}");
                assert!(msg.contains("az account show"), "{msg}");
            }
            other => panic!("expected TokenError, got {other:?}"),
        }
    }

    #[test]
    fn token_error_detail_falls_back_when_stderr_empty() {
        let status = std::process::Command::new("true")
            .status()
            .expect("failed to run `true`");
        let detail = token_error_detail("   \n", status);
        assert!(detail.contains("az login"), "{detail}");
        assert!(detail.contains("transient"), "{detail}");
        assert!(!detail.trim().is_empty());
    }

    #[test]
    fn token_error_detail_preserves_nonempty_stderr() {
        let status = std::process::Command::new("false")
            .status()
            .expect("failed to run `false`");
        let detail = token_error_detail("  ERROR: something specific broke  ", status);
        assert_eq!(detail, "ERROR: something specific broke");
    }
}
