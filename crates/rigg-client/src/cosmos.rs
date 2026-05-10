//! Azure Cosmos DB REST sampling.
//!
//! Phase 1 of the Cosmos KS wizard: library-only. Used by Phase 2's
//! `rigg analyze cosmos` CLI command.

use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use thiserror::Error;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Error)]
pub enum CosmosError {
    #[error("Invalid Cosmos connection string: {0}")]
    InvalidConnectionString(String),
}

/// Parse an Azure Cosmos DB connection string.
///
/// Expected format:
/// `AccountEndpoint=https://acct.documents.azure.com:443/;AccountKey=<base64-key>;`
///
/// Returns `(endpoint, master_key)`. Trailing semicolon is optional.
/// Additional fields (e.g., `DatabaseName=`) are ignored.
pub fn parse_connection_string(s: &str) -> Result<(String, String), CosmosError> {
    let mut endpoint: Option<String> = None;
    let mut key: Option<String> = None;

    for part in s.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (k, v) = part.split_once('=').ok_or_else(|| {
            CosmosError::InvalidConnectionString(format!("missing '=' in part '{part}'"))
        })?;
        match k {
            "AccountEndpoint" => endpoint = Some(v.to_string()),
            "AccountKey" => key = Some(v.to_string()),
            _ => {} // ignore unknown keys
        }
    }

    let endpoint = endpoint
        .ok_or_else(|| CosmosError::InvalidConnectionString("missing AccountEndpoint".into()))?;
    let key =
        key.ok_or_else(|| CosmosError::InvalidConnectionString("missing AccountKey".into()))?;
    Ok((endpoint, key))
}

/// Build a Cosmos REST `Authorization` header value using a master key.
///
/// The returned string is already URL-encoded (`%3D` for `=`, `%26` for `&`).
///
/// `verb`: HTTP method (e.g., `"POST"`).
/// `resource_type`: resource type segment (e.g., `"docs"`, `"colls"`, `"dbs"`).
/// `resource_link`: lowercase-sensitive resource path (e.g., `"dbs/mydb/colls/mycoll"`).
/// `date`: RFC1123-formatted date string (the same one sent in the `x-ms-date` header).
/// `master_key_b64`: base64-encoded master key from the connection string.
///
/// See https://learn.microsoft.com/en-us/rest/api/cosmos-db/access-control-on-cosmosdb-resources
pub fn build_master_key_authorization_token(
    verb: &str,
    resource_type: &str,
    resource_link: &str,
    date: &str,
    master_key_b64: &str,
) -> Result<String, CosmosError> {
    let key_bytes = B64.decode(master_key_b64).map_err(|e| {
        CosmosError::InvalidConnectionString(format!("invalid base64 master key: {e}"))
    })?;

    let string_to_sign = format!(
        "{}\n{}\n{}\n{}\n\n",
        verb.to_lowercase(),
        resource_type.to_lowercase(),
        resource_link, // case-sensitive — DO NOT lowercase
        date.to_lowercase(),
    );

    let mut mac = HmacSha256::new_from_slice(&key_bytes)
        .map_err(|e| CosmosError::InvalidConnectionString(format!("hmac key error: {e}")))?;
    mac.update(string_to_sign.as_bytes());
    let signature_b64 = B64.encode(mac.finalize().into_bytes());

    let token = format!("type=master&ver=1.0&sig={signature_b64}");
    Ok(url_encode_token(&token))
}

/// URL-encode the small set of characters that appear in a Cosmos auth token
/// (`=` and `&`). Cosmos does not require general percent-encoding here.
fn url_encode_token(s: &str) -> String {
    s.replace('=', "%3D").replace('&', "%26")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_connection_string_basic() {
        let s = "AccountEndpoint=https://acct.documents.azure.com:443/;AccountKey=ZmFrZWtleQ==;";
        let (endpoint, key) = parse_connection_string(s).unwrap();
        assert_eq!(endpoint, "https://acct.documents.azure.com:443/");
        assert_eq!(key, "ZmFrZWtleQ==");
    }

    #[test]
    fn parse_connection_string_no_trailing_semicolon() {
        let s = "AccountEndpoint=https://x.documents.azure.com:443/;AccountKey=AAAA";
        let (endpoint, key) = parse_connection_string(s).unwrap();
        assert_eq!(endpoint, "https://x.documents.azure.com:443/");
        assert_eq!(key, "AAAA");
    }

    #[test]
    fn parse_connection_string_ignores_extra_fields() {
        let s =
            "AccountEndpoint=https://x.documents.azure.com:443/;AccountKey=AAAA;DatabaseName=mydb;";
        let (endpoint, key) = parse_connection_string(s).unwrap();
        assert_eq!(endpoint, "https://x.documents.azure.com:443/");
        assert_eq!(key, "AAAA");
    }

    #[test]
    fn parse_connection_string_missing_endpoint() {
        let s = "AccountKey=AAAA;";
        let err = parse_connection_string(s).unwrap_err();
        assert!(format!("{err}").contains("AccountEndpoint"));
    }

    #[test]
    fn parse_connection_string_missing_key() {
        let s = "AccountEndpoint=https://x.documents.azure.com:443/;";
        let err = parse_connection_string(s).unwrap_err();
        assert!(format!("{err}").contains("AccountKey"));
    }

    #[test]
    fn build_master_key_authorization_token_is_deterministic() {
        let master_key_b64 = "MDEyMzQ1Njc4OTAxMjM0NTY3ODkwMTIzNDU2Nzg5MDEyMzQ1Ng==";
        let date = "Fri, 10 May 2026 12:00:00 GMT";
        let token = build_master_key_authorization_token(
            "POST",
            "docs",
            "dbs/mydb/colls/mycoll",
            date,
            master_key_b64,
        )
        .unwrap();

        assert!(token.starts_with("type%3Dmaster%26ver%3D1.0%26sig%3D"));

        let token2 = build_master_key_authorization_token(
            "POST",
            "docs",
            "dbs/mydb/colls/mycoll",
            date,
            master_key_b64,
        )
        .unwrap();
        assert_eq!(token, token2);

        let token3 = build_master_key_authorization_token(
            "GET",
            "docs",
            "dbs/mydb/colls/mycoll",
            date,
            master_key_b64,
        )
        .unwrap();
        assert_ne!(token, token3);
    }

    #[test]
    fn build_master_key_authorization_token_rejects_invalid_base64() {
        let err = build_master_key_authorization_token(
            "POST",
            "docs",
            "dbs/mydb/colls/mycoll",
            "Fri, 10 May 2026 12:00:00 GMT",
            "!!!not-base64!!!",
        )
        .unwrap_err();
        assert!(format!("{err}").contains("base64"));
    }
}
