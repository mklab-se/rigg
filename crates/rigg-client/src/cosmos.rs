//! Azure Cosmos DB REST sampling.
//!
//! Phase 1 of the Cosmos KS wizard: library-only. Used by Phase 2's
//! `rigg analyze cosmos` CLI command.

use thiserror::Error;

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
        let (k, v) = part
            .split_once('=')
            .ok_or_else(|| CosmosError::InvalidConnectionString(format!("missing '=' in part '{part}'")))?;
        match k {
            "AccountEndpoint" => endpoint = Some(v.to_string()),
            "AccountKey" => key = Some(v.to_string()),
            _ => {} // ignore unknown keys
        }
    }

    let endpoint =
        endpoint.ok_or_else(|| CosmosError::InvalidConnectionString("missing AccountEndpoint".into()))?;
    let key = key.ok_or_else(|| CosmosError::InvalidConnectionString("missing AccountKey".into()))?;
    Ok((endpoint, key))
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
        let s = "AccountEndpoint=https://x.documents.azure.com:443/;AccountKey=AAAA;DatabaseName=mydb;";
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
}
