//! Key Vault secrets data plane — the `key-vault:<secret>@<binding>` key
//! source (spec `2026-09-09-identity-and-auth-design.md` §6).
//!
//! The operator's own token is used (Key Vault Secrets User,
//! `4633458b-17de-408a-b874-0445c86b69e6`); the secret's value is returned to
//! the caller for injection and is **never logged** — not at any level, not
//! in an error.

use std::time::Duration;

use reqwest::Client;
use serde_json::Value;
use tracing::debug;

use rigg_core::registry::{self, KEYVAULT_SECRETS_API_VERSION, Provider};

use crate::auth::token_for;
use crate::error::ClientError;

/// The vault base URL to use: `RIGG_KEYVAULT_ENDPOINT` when set (it replaces
/// the vault host entirely, which is how tests point at a fake), else the
/// vault URI itself. A pure function so it is testable without mutating
/// process env vars.
pub(crate) fn base_url_from(vault_uri: &str, env: Option<&str>) -> String {
    match env {
        Some(v) if !v.is_empty() => v.trim_end_matches('/').to_string(),
        _ => vault_uri.trim_end_matches('/').to_string(),
    }
}

/// Key Vault secrets client, scoped to one vault.
pub struct KeyVaultClient {
    http: Client,
    token: String,
    base_url: String,
}

impl KeyVaultClient {
    /// Build a client for one vault, authenticating in `tenant`
    /// (`None` = the CLI's current tenant).
    pub fn for_tenant(tenant: Option<&str>, vault_uri: &str) -> Result<Self, ClientError> {
        let token = token_for(tenant, registry::provider(Provider::KeyVaultData).audience)?;
        Ok(Self::with_token_and_base(
            token,
            base_url_from(
                vault_uri,
                std::env::var("RIGG_KEYVAULT_ENDPOINT").ok().as_deref(),
            ),
        ))
    }

    /// Build a client from an already-obtained token and an explicit base
    /// URL — for tests, so Key Vault tests never touch the process env var.
    pub fn with_token_and_base(token: String, base_url: String) -> Self {
        Self {
            http: Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("reqwest client builds"),
            token,
            base_url,
        }
    }

    /// Read the current version of a secret.
    ///
    /// Only the URL is traced — never the response body.
    pub async fn get_secret(&self, name: &str) -> Result<String, ClientError> {
        let url = format!(
            "{}/secrets/{}?api-version={}",
            self.base_url,
            urlencoding::encode(name),
            KEYVAULT_SECRETS_API_VERSION
        );
        debug!("Key Vault GET {url}");
        let response = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            // The body of a Key Vault error never contains the secret, but
            // it does name the vault and the caller — enough to act on.
            let body = response.text().await?;
            return Err(ClientError::from_response(status.as_u16(), &body));
        }
        let value: Value = response.json().await?;
        value
            .get("value")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                ClientError::InvalidResponse(format!(
                    "Key Vault returned no value for secret '{name}'"
                ))
            })
    }
}

/// Read one secret from one vault — the shape the push-time key injection
/// uses. The value is returned to the caller and never logged.
pub async fn get_secret(
    tenant: Option<&str>,
    vault_uri: &str,
    name: &str,
) -> Result<String, ClientError> {
    KeyVaultClient::for_tenant(tenant, vault_uri)?
        .get_secret(name)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_prefers_the_test_override_over_the_vault_host() {
        assert_eq!(
            base_url_from("https://kv.vault.azure.net/", Some("http://127.0.0.1:1")),
            "http://127.0.0.1:1"
        );
        assert_eq!(
            base_url_from("https://kv.vault.azure.net/", Some("http://127.0.0.1:1/")),
            "http://127.0.0.1:1"
        );
        assert_eq!(
            base_url_from("https://kv.vault.azure.net/", None),
            "https://kv.vault.azure.net"
        );
        assert_eq!(
            base_url_from("https://kv.vault.azure.net", Some("")),
            "https://kv.vault.azure.net"
        );
    }

    #[test]
    fn the_key_vault_audience_comes_from_the_registry() {
        assert_eq!(
            registry::provider(Provider::KeyVaultData).audience,
            "https://vault.azure.net"
        );
    }
}
