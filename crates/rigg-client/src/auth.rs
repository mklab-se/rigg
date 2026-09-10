//! Azure authentication providers

use std::collections::HashMap;
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use thiserror::Error;

use rigg_core::registry;

/// Process-wide cache of Azure CLI tokens per resource scope. Every `az
/// account get-access-token` call spawns a subprocess; multi-env commands
/// (e.g. `rigg status` fanning out over all environments) would otherwise
/// pay that cost once per request. Entries are reused well below Azure's
/// token lifetime; the single lock also serializes fetches so concurrent
/// requests can't stampede `az`.
struct TokenCache {
    ttl: Duration,
    entries: Mutex<HashMap<String, (String, Instant)>>,
}

impl TokenCache {
    fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            entries: Mutex::new(HashMap::new()),
        }
    }

    fn get_or_fetch(
        &self,
        scope: &str,
        fetch: impl FnOnce() -> Result<String, AuthError>,
    ) -> Result<String, AuthError> {
        let mut entries = self.entries.lock().unwrap();
        if let Some((token, acquired)) = entries.get(scope)
            && acquired.elapsed() < self.ttl
        {
            return Ok(token.clone());
        }
        let token = fetch()?;
        entries.insert(scope.to_string(), (token.clone(), Instant::now()));
        Ok(token)
    }
}

fn az_token_cache() -> &'static TokenCache {
    static CACHE: OnceLock<TokenCache> = OnceLock::new();
    CACHE.get_or_init(|| TokenCache::new(Duration::from_secs(300)))
}

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

/// The ARM token audience, from the registry provider table.
fn arm_audience() -> &'static str {
    registry::provider(registry::Provider::ResourcesArm).audience
}

/// The Microsoft Graph audience, from the registry provider table. Graph is
/// the one audience the Azure CLI wants addressed by `--resource-type`
/// rather than `--scope`.
fn graph_audience() -> &'static str {
    registry::provider(registry::Provider::Graph).audience
}

/// Cache key for one `(tenant, audience)` pair. `None` (the operator's home
/// tenant) is `-`, so an explicit tenant can never collide with it.
fn token_cache_key(tenant: Option<&str>, audience: &str) -> String {
    format!("{}|{}", tenant.unwrap_or("-"), audience)
}

/// The `az account get-access-token` arguments for one `(tenant, audience)`.
///
/// Every audience is addressed as a scope (`<audience>/.default`) except
/// Microsoft Graph, which the CLI serves via `--resource-type ms-graph`.
///
/// There is deliberately no `--username`: `az account get-access-token`
/// rejects it, and a service principal never comes through here — it is
/// minted directly against Entra ID by
/// [`mint_service_principal_token`].
fn az_token_args(tenant: Option<&str>, audience: &str) -> Vec<String> {
    let mut args = vec!["account".to_string(), "get-access-token".to_string()];
    if let Some(t) = tenant {
        args.push("--tenant".to_string());
        args.push(t.to_string());
    }
    if audience == graph_audience() {
        args.push("--resource-type".to_string());
        args.push("ms-graph".to_string());
    } else {
        args.push("--scope".to_string());
        args.push(format!("{audience}/.default"));
    }
    args.extend(
        ["--query", "accessToken", "--output", "tsv"]
            .into_iter()
            .map(String::from),
    );
    args
}

/// Entra ID's token endpoint host. `RIGG_LOGIN_ENDPOINT` replaces it (tests,
/// and sovereign clouds).
const DEFAULT_LOGIN_ENDPOINT: &str = "https://login.microsoftonline.com";

/// The login base to use: `RIGG_LOGIN_ENDPOINT` when set (trimmed of a
/// trailing `/`), else [`DEFAULT_LOGIN_ENDPOINT`]. A pure function so it is
/// testable without mutating process env vars.
fn login_endpoint_from(env: Option<&str>) -> String {
    match env {
        Some(v) if !v.is_empty() => v.trim_end_matches('/').to_string(),
        _ => DEFAULT_LOGIN_ENDPOINT.to_string(),
    }
}

/// The configured Entra ID login base.
pub fn login_endpoint() -> String {
    login_endpoint_from(std::env::var("RIGG_LOGIN_ENDPOINT").ok().as_deref())
}

/// How a service principal proves itself to Entra ID.
///
/// Never `Debug`-derived and never logged: both variants are bearer
/// credentials.
#[derive(Clone)]
pub enum SpCredential {
    /// A client secret (`AZURE_CLIENT_SECRET`).
    Secret(String),
    /// A federated identity assertion — the OIDC token GitHub Actions (and
    /// any workload-identity issuer) writes to `AZURE_FEDERATED_TOKEN_FILE`.
    FederatedAssertion(String),
}

impl std::fmt::Debug for SpCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Secret(_) => f.write_str("SpCredential::Secret(<redacted>)"),
            Self::FederatedAssertion(_) => {
                f.write_str("SpCredential::FederatedAssertion(<redacted>)")
            }
        }
    }
}

/// The service-principal credential the environment carries.
///
/// A federated assertion wins when `AZURE_FEDERATED_TOKEN_FILE` names one:
/// a workload-identity runner (GitHub OIDC, AKS) sets that file and no
/// secret, and where both are present the short-lived assertion is the
/// better credential. The file is read at every token request, since the
/// issuer rewrites it as it rotates.
fn sp_credential_from_env() -> Result<SpCredential, AuthError> {
    if let Ok(path) = std::env::var("AZURE_FEDERATED_TOKEN_FILE")
        && !path.is_empty()
    {
        let assertion = std::fs::read_to_string(&path).map_err(|e| {
            AuthError::AuthFailed(format!(
                "could not read the federated token file AZURE_FEDERATED_TOKEN_FILE={path}: {e}"
            ))
        })?;
        return Ok(SpCredential::FederatedAssertion(
            assertion.trim().to_string(),
        ));
    }
    match std::env::var("AZURE_CLIENT_SECRET") {
        Ok(secret) if !secret.is_empty() => Ok(SpCredential::Secret(secret)),
        _ => Err(AuthError::MissingEnvVar("AZURE_CLIENT_SECRET".to_string())),
    }
}

/// The form body of an OAuth2 client-credentials request. Pure, so the wire
/// shape is unit-testable without a network.
fn client_credentials_form(
    client_id: &str,
    audience: &str,
    credential: &SpCredential,
) -> Vec<(&'static str, String)> {
    let mut form = vec![
        ("grant_type", "client_credentials".to_string()),
        ("client_id", client_id.to_string()),
        ("scope", format!("{audience}/.default")),
    ];
    match credential {
        SpCredential::Secret(secret) => form.push(("client_secret", secret.clone())),
        SpCredential::FederatedAssertion(assertion) => {
            form.push((
                "client_assertion_type",
                "urn:ietf:params:oauth:client-assertion-type:jwt-bearer".to_string(),
            ));
            form.push(("client_assertion", assertion.clone()));
        }
    }
    form
}

/// Mint a token for `audience` directly from Entra ID's v2 token endpoint,
/// `POST {login_base}/{tenant}/oauth2/v2.0/token`.
///
/// This is how a service principal authenticates: `az account
/// get-access-token` cannot do it (it has no `--username`/`--password` form
/// and would need `az login --service-principal` to have run first), so CI
/// that only sets `AZURE_CLIENT_ID` / `AZURE_TENANT_ID` / a credential is
/// served here rather than through the CLI.
///
/// The credential is sent in the form body and never appears in a log line,
/// an error message, or a process argument.
pub fn mint_service_principal_token(
    login_base: &str,
    tenant: &str,
    client_id: &str,
    credential: &SpCredential,
    audience: &str,
) -> Result<String, AuthError> {
    let url = format!(
        "{}/{tenant}/oauth2/v2.0/token",
        login_base.trim_end_matches('/')
    );
    let form = client_credentials_form(client_id, audience, credential);
    tracing::debug!("minting a service-principal token for {audience} at {url}");
    let (status, body) = post_form(&url, form)?;
    if !(200..300).contains(&status) {
        return Err(AuthError::AuthFailed(format!(
            "Entra ID refused the service-principal token request for {audience} \
             (client {client_id}, tenant {tenant}): {}",
            token_endpoint_error(status, &body)
        )));
    }
    let parsed: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
        AuthError::TokenError(format!("could not parse the Entra ID token response: {e}"))
    })?;
    match parsed.get("access_token").and_then(|t| t.as_str()) {
        Some(token) if !token.is_empty() => Ok(token.to_string()),
        _ => Err(AuthError::TokenError(
            "Entra ID returned no access_token".to_string(),
        )),
    }
}

/// Entra ID's own error text for a failed token request — `error` plus
/// `error_description` (which carries the AADSTS code) when the body is the
/// documented JSON envelope, else the raw body. Only ever the *response*, so
/// no credential can end up here.
fn token_endpoint_error(status: u16, body: &str) -> String {
    let detail = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            let code = v.get("error").and_then(|e| e.as_str()).map(str::to_string);
            let description = v
                .get("error_description")
                .and_then(|e| e.as_str())
                .map(|d| d.lines().next().unwrap_or(d).trim().to_string());
            match (code, description) {
                (Some(c), Some(d)) => Some(format!("{c}: {d}")),
                (Some(c), None) => Some(c),
                (None, Some(d)) => Some(d),
                (None, None) => None,
            }
        })
        .unwrap_or_else(|| body.trim().to_string());
    format!("HTTP {status} {detail}")
}

/// POST a form and return `(status, body)`.
///
/// `token_for` is synchronous (it backs the blocking `AuthProvider` trait),
/// so the request runs on its own thread with its own single-threaded
/// runtime rather than borrowing the caller's — exactly as blocking as the
/// `az` subprocess it replaces, and safe to call from inside an async
/// command.
fn post_form(url: &str, form: Vec<(&'static str, String)>) -> Result<(u16, String), AuthError> {
    let url = url.to_string();
    let worker = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| {
                AuthError::TokenError(format!("could not start the token request runtime: {e}"))
            })?;
        runtime.block_on(async move {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .map_err(|e| AuthError::TokenError(e.to_string()))?;
            let response = client.post(&url).form(&form).send().await.map_err(|e| {
                AuthError::TokenError(format!("token request to {url} failed: {e}"))
            })?;
            let status = response.status().as_u16();
            let body = response
                .text()
                .await
                .map_err(|e| AuthError::TokenError(format!("token response from {url}: {e}")))?;
            Ok((status, body))
        })
    });
    worker
        .join()
        .map_err(|_| AuthError::TokenError("the token request thread panicked".to_string()))?
}

/// An access token for one `(tenant, audience)` pair — the single entry point
/// every client uses (spec §8).
///
/// Resolution order, highest first:
/// 1. `RIGG_ACCESS_TOKEN` — a pre-minted token, honoured for any audience.
/// 2. Service-principal environment variables (`AZURE_CLIENT_ID` /
///    `AZURE_TENANT_ID` plus `AZURE_CLIENT_SECRET` or
///    `AZURE_FEDERATED_TOKEN_FILE`), minted straight from Entra ID.
/// 3. The operator's Azure CLI login.
///
/// Results are cached for 5 minutes keyed by `(tenant, audience)`; failures
/// are never cached. `tenant: None` means the CLI's current tenant.
pub fn token_for(tenant: Option<&str>, audience: &str) -> Result<String, AuthError> {
    // A pre-minted token wins over everything, for every audience — same
    // rule the data-plane providers follow.
    if let Ok(token) = std::env::var("RIGG_ACCESS_TOKEN")
        && !token.is_empty()
    {
        return Ok(token);
    }
    let key = token_cache_key(tenant, audience);
    az_token_cache().get_or_fetch(&key, || fetch_token_for(tenant, audience))
}

fn fetch_token_for(tenant: Option<&str>, audience: &str) -> Result<String, AuthError> {
    if EnvAuth::is_configured() {
        return fetch_service_principal_token(tenant, audience);
    }
    run_az_token(&az_token_args(tenant, audience), tenant)
}

/// Mint a token for the service principal the environment describes.
fn fetch_service_principal_token(
    tenant: Option<&str>,
    audience: &str,
) -> Result<String, AuthError> {
    let client_id = std::env::var("AZURE_CLIENT_ID")
        .map_err(|_| AuthError::MissingEnvVar("AZURE_CLIENT_ID".to_string()))?;
    let sp_tenant = std::env::var("AZURE_TENANT_ID")
        .map_err(|_| AuthError::MissingEnvVar("AZURE_TENANT_ID".to_string()))?;
    // An explicitly requested tenant wins over the service principal's
    // home tenant (multi-tenant environments name theirs in rigg.yaml).
    let tenant = tenant.unwrap_or(&sp_tenant);
    let credential = sp_credential_from_env()?;
    mint_service_principal_token(&login_endpoint(), tenant, &client_id, &credential, audience)
}

/// Run `az` with `args` and return the token it prints.
fn run_az_token(args: &[String], tenant: Option<&str>) -> Result<String, AuthError> {
    let output = Command::new("az").args(args).output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            AuthError::AzCliNotFound
        } else {
            AuthError::TokenError(e.to_string())
        }
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Only a sign-in failure is fixed by signing in: an unrelated `az`
        // error (a bad scope, a CLI crash) must not be labelled as one.
        let sign_in_failure = stderr.contains("not logged in")
            || stderr.contains("az login")
            || stderr.contains("AADSTS");
        if sign_in_failure {
            // A non-home tenant the operator has not signed into is the
            // common case; say exactly which `az login` fixes it.
            if let Some(t) = tenant {
                return Err(AuthError::TokenError(format!(
                    "{}\n  run: az login --tenant {t}",
                    token_error_detail(&stderr, output.status)
                )));
            }
            return Err(AuthError::NotLoggedIn);
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
            tenant_id: account_json
                .get("tenantId")
                .and_then(|t| t.as_str())
                .map(String::from),
        })
    }

    /// Get an access token for Azure Resource Manager (management.azure.com)
    pub fn get_arm_token() -> Result<String, AuthError> {
        token_for(None, arm_audience())
    }

    /// Get an ARM access token scoped to a specific tenant.
    /// `tenant: None` behaves exactly like [`Self::get_arm_token`] — same
    /// cache entry — so existing callers are unaffected.
    pub fn get_arm_token_for_tenant(tenant: Option<&str>) -> Result<String, AuthError> {
        token_for(tenant, arm_audience())
    }
}

impl Default for AzCliAuth {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthProvider for AzCliAuth {
    fn get_token(&self) -> Result<String, AuthError> {
        az_token_cache().get_or_fetch(self.resource_scope, || self.fetch_token())
    }

    fn method_name(&self) -> &'static str {
        "Azure CLI"
    }
}

impl AzCliAuth {
    fn fetch_token(&self) -> Result<String, AuthError> {
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
}

/// Environment variable authentication provider
#[derive(Debug)]
pub struct EnvAuth {
    client_id: String,
    credential: SpCredential,
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
        let tenant_id = std::env::var("AZURE_TENANT_ID")
            .map_err(|_| AuthError::MissingEnvVar("AZURE_TENANT_ID".to_string()))?;
        let credential = sp_credential_from_env()?;

        Ok(Self {
            client_id,
            credential,
            tenant_id,
            resource_scope: scope,
        })
    }

    /// Whether the environment describes a usable service principal: a client
    /// id, a tenant, and a credential — either a secret or the federated
    /// assertion file a workload-identity runner writes.
    pub fn is_configured() -> bool {
        let set = |name: &str| std::env::var(name).is_ok_and(|v| !v.is_empty());
        set("AZURE_CLIENT_ID")
            && set("AZURE_TENANT_ID")
            && (set("AZURE_CLIENT_SECRET") || set("AZURE_FEDERATED_TOKEN_FILE"))
    }
}

impl AuthProvider for EnvAuth {
    fn get_token(&self) -> Result<String, AuthError> {
        // Same client-credentials mint as `token_for`'s service-principal
        // branch, for this provider's own audience.
        mint_service_principal_token(
            &login_endpoint(),
            &self.tenant_id,
            &self.client_id,
            &self.credential,
            self.resource_scope,
        )
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
    pub tenant_id: Option<String>,
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
    if let Ok(token) = std::env::var("RIGG_ACCESS_TOKEN")
        && !token.is_empty()
    {
        return Ok(Box::new(StaticTokenAuth { token }));
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
            std::env::remove_var("AZURE_FEDERATED_TOKEN_FILE");
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
        assert!(
            matches!(&auth.credential, SpCredential::Secret(s) if s == "test-client-secret"),
            "{:?}",
            auth.credential
        );
        assert_eq!(auth.tenant_id, "test-tenant-id");

        unsafe { clear_azure_env_vars() };
    }

    #[test]
    fn env_auth_accepts_a_federated_token_file_instead_of_a_secret() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let file = std::env::temp_dir().join("rigg-federated-token-test");
        std::fs::write(&file, "  assertion-jwt\n").unwrap();
        unsafe {
            clear_azure_env_vars();
            std::env::set_var("AZURE_CLIENT_ID", "test-client-id");
            std::env::set_var("AZURE_TENANT_ID", "test-tenant-id");
            std::env::set_var("AZURE_FEDERATED_TOKEN_FILE", &file);
        }

        // GitHub OIDC sets no secret at all — that is still a configured
        // service principal.
        assert!(EnvAuth::is_configured());
        let auth = EnvAuth::from_env().unwrap();
        assert!(
            matches!(&auth.credential, SpCredential::FederatedAssertion(a) if a == "assertion-jwt"),
            "{:?}",
            auth.credential
        );

        unsafe { clear_azure_env_vars() };
        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn a_federated_assertion_wins_over_a_secret() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let file = std::env::temp_dir().join("rigg-federated-token-precedence");
        std::fs::write(&file, "assertion-jwt").unwrap();
        unsafe {
            set_azure_env_vars();
            std::env::set_var("AZURE_FEDERATED_TOKEN_FILE", &file);
        }

        assert!(matches!(
            sp_credential_from_env().unwrap(),
            SpCredential::FederatedAssertion(_)
        ));

        unsafe { clear_azure_env_vars() };
        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn a_credential_never_appears_in_its_debug_output() {
        let secret = SpCredential::Secret("s3cr3t".to_string());
        assert!(!format!("{secret:?}").contains("s3cr3t"));
        let federated = SpCredential::FederatedAssertion("assertion-jwt".to_string());
        assert!(!format!("{federated:?}").contains("assertion-jwt"));
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
            tenant_id: Some("11111111-1111-1111-1111-111111111111".to_string()),
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

#[cfg(test)]
mod token_for_tests {
    use super::*;

    #[test]
    fn cache_key_separates_tenants_and_audiences() {
        assert_eq!(
            token_cache_key(None, "https://management.azure.com"),
            "-|https://management.azure.com"
        );
        assert_eq!(
            token_cache_key(Some("t1"), "https://management.azure.com"),
            "t1|https://management.azure.com"
        );
        // A tenant can never collide with the home-tenant entry, and the
        // same tenant on two audiences gets two entries.
        assert_ne!(
            token_cache_key(Some("t1"), "https://management.azure.com"),
            token_cache_key(None, "https://management.azure.com")
        );
        assert_ne!(
            token_cache_key(Some("t1"), "https://search.azure.com"),
            token_cache_key(Some("t1"), "https://management.azure.com")
        );
    }

    #[test]
    fn arm_token_helpers_share_the_home_tenant_cache_entry() {
        // `get_arm_token` and `get_arm_token_for_tenant(None)` must key the
        // same entry, so existing callers keep hitting one cached token.
        assert_eq!(
            token_cache_key(None, arm_audience()),
            token_cache_key(None, arm_audience())
        );
        assert_eq!(arm_audience(), "https://management.azure.com");
    }

    #[test]
    fn az_args_use_the_scope_form_for_ordinary_audiences() {
        let args = az_token_args(None, "https://management.azure.com");
        assert_eq!(args[..2], ["account", "get-access-token"]);
        assert!(!args.contains(&"--tenant".to_string()));
        assert!(args.contains(&"--scope".to_string()));
        assert!(args.contains(&"https://management.azure.com/.default".to_string()));
        assert_eq!(
            args[args.len() - 4..],
            ["--query", "accessToken", "--output", "tsv"]
        );
    }

    #[test]
    fn az_args_pass_the_tenant_through() {
        let args = az_token_args(Some("tenant-1"), "https://search.azure.com");
        let at = args.iter().position(|a| a == "--tenant").unwrap();
        assert_eq!(args[at + 1], "tenant-1");
        assert!(args.contains(&"https://search.azure.com/.default".to_string()));
    }

    #[test]
    fn az_args_address_graph_by_resource_type() {
        let args = az_token_args(Some("tenant-1"), graph_audience());
        assert!(args.contains(&"--resource-type".to_string()));
        assert!(args.contains(&"ms-graph".to_string()));
        assert!(
            !args.contains(&"--scope".to_string()),
            "Graph is not addressed by scope: {args:?}"
        );
        assert_eq!(graph_audience(), "https://graph.microsoft.com");
    }

    #[test]
    fn az_args_never_carry_a_service_principal_username() {
        // `az account get-access-token` has no `--username`: it rejects the
        // flag outright. A service principal is minted from Entra ID instead.
        for audience in ["https://vault.azure.net", graph_audience()] {
            let args = az_token_args(Some("t"), audience);
            assert!(!args.iter().any(|a| a == "--username"), "{args:?}");
            assert!(!args.iter().any(|a| a.contains("secret")), "{args:?}");
        }
    }

    #[test]
    fn client_credentials_form_carries_the_secret_grant() {
        let form = client_credentials_form(
            "client-1",
            "https://management.azure.com",
            &SpCredential::Secret("s3cr3t".to_string()),
        );
        let field = |name: &str| {
            form.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(field("grant_type"), Some("client_credentials"));
        assert_eq!(field("client_id"), Some("client-1"));
        assert_eq!(
            field("scope"),
            Some("https://management.azure.com/.default")
        );
        assert_eq!(field("client_secret"), Some("s3cr3t"));
        assert_eq!(field("client_assertion"), None);
    }

    #[test]
    fn client_credentials_form_carries_the_federated_grant() {
        let form = client_credentials_form(
            "client-1",
            "https://graph.microsoft.com",
            &SpCredential::FederatedAssertion("assertion-jwt".to_string()),
        );
        let field = |name: &str| {
            form.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(field("scope"), Some("https://graph.microsoft.com/.default"));
        assert_eq!(
            field("client_assertion_type"),
            Some("urn:ietf:params:oauth:client-assertion-type:jwt-bearer")
        );
        assert_eq!(field("client_assertion"), Some("assertion-jwt"));
        assert_eq!(field("client_secret"), None);
    }

    #[test]
    fn login_endpoint_is_overridable() {
        assert_eq!(login_endpoint_from(None), DEFAULT_LOGIN_ENDPOINT);
        assert_eq!(login_endpoint_from(Some("")), DEFAULT_LOGIN_ENDPOINT);
        assert_eq!(
            login_endpoint_from(Some("http://127.0.0.1:1/")),
            "http://127.0.0.1:1"
        );
    }

    #[test]
    fn token_endpoint_error_reports_the_aadsts_code_only() {
        let detail = token_endpoint_error(
            401,
            r#"{"error":"invalid_client","error_description":"AADSTS7000215: Invalid client secret provided.\r\nTrace ID: x"}"#,
        );
        assert!(detail.contains("AADSTS7000215"), "{detail}");
        assert!(detail.contains("invalid_client"), "{detail}");
        assert!(detail.contains("401"), "{detail}");
        // A non-JSON body still says something.
        assert!(token_endpoint_error(500, "  boom  ").contains("boom"));
    }

    #[test]
    fn every_provider_audience_produces_usable_az_args() {
        for meta in registry::providers() {
            let args = az_token_args(None, meta.audience);
            assert!(
                args.contains(&"--scope".to_string()) || args.contains(&"ms-graph".to_string()),
                "{}: {args:?}",
                meta.label
            );
        }
    }
}

#[cfg(test)]
mod token_cache_tests {
    use super::*;
    use std::cell::Cell;
    use std::time::Duration;

    #[test]
    fn second_lookup_within_ttl_reuses_token_without_fetching() {
        let cache = TokenCache::new(Duration::from_secs(300));
        let fetches = Cell::new(0u32);
        let fetch = || {
            fetches.set(fetches.get() + 1);
            Ok("tok-1".to_string())
        };
        assert_eq!(cache.get_or_fetch("scope-a", fetch).unwrap(), "tok-1");
        assert_eq!(
            cache
                .get_or_fetch("scope-a", || panic!("must not fetch"))
                .unwrap(),
            "tok-1"
        );
        assert_eq!(fetches.get(), 1);
    }

    #[test]
    fn expired_entry_is_refetched() {
        let cache = TokenCache::new(Duration::ZERO);
        cache
            .get_or_fetch("scope-a", || Ok("old".to_string()))
            .unwrap();
        let got = cache
            .get_or_fetch("scope-a", || Ok("new".to_string()))
            .unwrap();
        assert_eq!(got, "new");
    }

    #[test]
    fn scopes_are_cached_independently() {
        let cache = TokenCache::new(Duration::from_secs(300));
        cache
            .get_or_fetch("scope-a", || Ok("tok-a".to_string()))
            .unwrap();
        let got = cache
            .get_or_fetch("scope-b", || Ok("tok-b".to_string()))
            .unwrap();
        assert_eq!(got, "tok-b");
    }

    #[test]
    fn fetch_errors_are_not_cached() {
        let cache = TokenCache::new(Duration::from_secs(300));
        let err = cache.get_or_fetch("scope-a", || Err(AuthError::TokenError("boom".to_string())));
        assert!(err.is_err());
        let got = cache
            .get_or_fetch("scope-a", || Ok("recovered".to_string()))
            .unwrap();
        assert_eq!(got, "recovered");
    }
}
