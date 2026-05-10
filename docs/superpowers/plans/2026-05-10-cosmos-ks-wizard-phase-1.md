# Cosmos DB → Knowledge Source: Phase 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make rigg able to scaffold a complete Cosmos DB data source and Cosmos-backed Knowledge Source via `rigg new`, plus a sampling library (`rigg-client::cosmos`) ready for Phase 2's `rigg analyze` command. After this phase the user can manually wire a Cosmos → KS configuration and push it to Azure with `rigg push`.

**Architecture:** Three changes across three crates: (1) `rigg-client::cosmos` — new module with AAD/connection-string auth and document sampling; (2) `rigg-core::scaffold` — Cosmos-aware data source template + a typed KS scaffold function; (3) `rigg/commands/scaffold` and `rigg/cli` — wire `--type cosmosdb` through `rigg new knowledgesource`. Lint rules updated to validate Cosmos data sources. No new top-level commands.

**Tech Stack:** Rust 1.85 (edition 2024). reqwest (existing workspace dep). serde_json with `preserve_order` (existing). Add `hmac`, `sha2`, `base64` to `rigg-client` for Cosmos master-key signing. `chrono` (existing workspace dep) for RFC1123 dates.

**Source spec:** `docs/superpowers/specs/2026-05-08-cosmos-ks-wizard-design.md` (Phase 1 section).

**Out of scope (Phase 2+):** `rigg analyze cosmos` CLI command, `rigg suggest`, `rigg ai skill --emit` restructure, `rigg wiz` state machine.

---

## File map

**Create:**
- `crates/rigg-client/src/cosmos.rs` — Cosmos REST sampling: `CosmosAuth` enum, connection-string parser, master-key HMAC signer, `sample_documents()` async fn.

**Modify:**
- `crates/rigg-client/Cargo.toml` — add `hmac`, `sha2`, `base64` deps.
- `crates/rigg-client/src/lib.rs` — declare and re-export the `cosmos` module.
- `crates/rigg-client/src/auth.rs` — add `AzCliAuth::for_cosmos()` factory.
- `crates/rigg-core/src/scaffold.rs` — extend `scaffold_datasource` for `cosmosdb` type; add `scaffold_knowledge_source_typed`.
- `crates/rigg/src/cli.rs` — extend `NewCommands::KnowledgeSource` with `--type` and `--container` flags.
- `crates/rigg/src/commands/scaffold.rs` — pass `--type` and `--container` through to the new typed KS scaffold.
- `crates/rigg/src/commands/validate/lint.rs` — add Cosmos-specific lints to `lint_datasource`.

**Tests:** inline `#[cfg(test)] mod tests` blocks in each modified file (existing rigg convention).

---

## Pre-Push Verification

After every commit in this plan, you SHOULD run:
```bash
cargo fmt --all -- --check
cargo clippy --workspace -- -D warnings
cargo test --workspace
```
At minimum, run all three before declaring the plan complete. (CLAUDE.md mandates this for any push.)

---

## Task 1: Add Cosmos auth scope to `AzCliAuth`

**Files:**
- Modify: `crates/rigg-client/src/auth.rs:32-62`

- [ ] **Step 1: Write the failing test**

Add at the end of the existing `mod tests` block in `auth.rs` (or create one if it doesn't exist — search for `#[cfg(test)]` first):

```rust
#[test]
fn test_for_cosmos_uses_cosmos_scope() {
    let auth = AzCliAuth::for_cosmos();
    assert_eq!(auth.resource_scope, "https://cosmos.azure.com");
}
```

If `resource_scope` is private, also add a `#[cfg(test)] pub(crate)` test-only accessor or relax the field visibility within the impl block — pick whichever matches the existing test pattern in `auth.rs`.

- [ ] **Step 2: Run the test and verify it fails**

```bash
cargo test -p rigg-client test_for_cosmos_uses_cosmos_scope -- --nocapture
```

Expected: FAIL with `cannot find function 'for_cosmos' in 'AzCliAuth'`.

- [ ] **Step 3: Implement `for_cosmos`**

In `crates/rigg-client/src/auth.rs`, add a new factory method to `impl AzCliAuth` (next to the existing `for_search`, `for_foundry`, `for_cognitive_services`):

```rust
/// Create an auth provider for Azure Cosmos DB
pub fn for_cosmos() -> Self {
    Self {
        resource_scope: "https://cosmos.azure.com",
    }
}
```

- [ ] **Step 4: Run the test and verify it passes**

```bash
cargo test -p rigg-client test_for_cosmos_uses_cosmos_scope -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rigg-client/src/auth.rs
git commit -m "rigg-client: add Cosmos DB auth scope (AzCliAuth::for_cosmos)"
```

---

## Task 2: Add HMAC/SHA2/base64 dependencies to `rigg-client`

**Files:**
- Modify: `crates/rigg-client/Cargo.toml`

- [ ] **Step 1: Add dependencies**

In `crates/rigg-client/Cargo.toml`, under `[dependencies]`, add:

```toml
hmac = "0.12"
sha2 = "0.10"
base64 = "0.22"
```

- [ ] **Step 2: Verify the build still compiles**

```bash
cargo build -p rigg-client
```

Expected: build succeeds (no warnings about unused deps yet — they'll be used in Task 4).

- [ ] **Step 3: Commit**

```bash
git add crates/rigg-client/Cargo.toml Cargo.lock
git commit -m "rigg-client: add hmac/sha2/base64 deps for Cosmos signing"
```

---

## Task 3: Cosmos connection string parser

**Files:**
- Create: `crates/rigg-client/src/cosmos.rs`
- Modify: `crates/rigg-client/src/lib.rs`

- [ ] **Step 1: Create `cosmos.rs` with module skeleton and the failing test**

Create `crates/rigg-client/src/cosmos.rs`:

```rust
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
```

Then in `crates/rigg-client/src/lib.rs`, add the module declaration after `pub mod auth;`:

```rust
pub mod cosmos;
```

- [ ] **Step 2: Run the tests and verify they pass**

```bash
cargo test -p rigg-client cosmos:: -- --nocapture
```

Expected: 5 tests PASS (`parse_connection_string_basic`, `parse_connection_string_no_trailing_semicolon`, `parse_connection_string_ignores_extra_fields`, `parse_connection_string_missing_endpoint`, `parse_connection_string_missing_key`).

- [ ] **Step 3: Commit**

```bash
git add crates/rigg-client/src/cosmos.rs crates/rigg-client/src/lib.rs
git commit -m "rigg-client: cosmos connection-string parser"
```

---

## Task 4: Cosmos master-key HMAC signer

**Files:**
- Modify: `crates/rigg-client/src/cosmos.rs`

- [ ] **Step 1: Write the failing test**

Append to the `mod tests` block in `cosmos.rs`. The test uses a known input/output pair — the master key, date, and resource link below produce a deterministic signature; the expected sig was computed by hand following the [Cosmos REST signing algorithm](https://learn.microsoft.com/en-us/rest/api/cosmos-db/access-control-on-cosmosdb-resources):

```rust
#[test]
fn build_master_key_authorization_token_is_deterministic() {
    // Hand-derived golden input/output pair.
    // Master key: base64("0123456789012345678901234567890123456789012345==") — 32 bytes after decode.
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

    // Token must start with the canonical prefix and be URL-encoded.
    assert!(token.starts_with("type%3Dmaster%26ver%3D1.0%26sig%3D"));
    // The %3D and %26 escapes are the URL-encoded forms of '=' and '&'.

    // Determinism: signing twice produces the same token.
    let token2 = build_master_key_authorization_token(
        "POST",
        "docs",
        "dbs/mydb/colls/mycoll",
        date,
        master_key_b64,
    )
    .unwrap();
    assert_eq!(token, token2);

    // Different verb produces a different signature.
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
```

- [ ] **Step 2: Run the test and verify it fails**

```bash
cargo test -p rigg-client cosmos::tests::build_master_key_authorization_token_is_deterministic -- --nocapture
```

Expected: FAIL with `cannot find function 'build_master_key_authorization_token'`.

- [ ] **Step 3: Implement the signer**

Add to `cosmos.rs` (above the `mod tests` block):

```rust
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

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
    let key_bytes = B64
        .decode(master_key_b64)
        .map_err(|e| CosmosError::InvalidConnectionString(format!("invalid base64 master key: {e}")))?;

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
```

- [ ] **Step 4: Run the test and verify it passes**

```bash
cargo test -p rigg-client cosmos::tests::build_master_key_authorization_token_is_deterministic -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Add a parametric edge-case test for invalid base64**

Append to the `mod tests` block:

```rust
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
```

- [ ] **Step 6: Run, verify pass**

```bash
cargo test -p rigg-client cosmos::tests -- --nocapture
```

Expected: all `cosmos::tests::*` PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/rigg-client/src/cosmos.rs
git commit -m "rigg-client: cosmos master-key HMAC-SHA256 signer"
```

---

## Task 5: `CosmosAuth` enum and `sample_documents` request builder

This task introduces the public auth abstraction and a pure (non-network) request builder. The `sample_documents` async function calling `reqwest` follows in Task 6.

**Files:**
- Modify: `crates/rigg-client/src/cosmos.rs`

- [ ] **Step 1: Write failing tests for the request builder**

Append to the `mod tests` block in `cosmos.rs`:

```rust
#[test]
fn build_query_request_master_key_sets_required_headers() {
    let req = build_query_request(
        "https://acct.documents.azure.com:443/",
        "mydb",
        "mycoll",
        &CosmosAuth::MasterKey("MDEyMzQ1Njc4OTAxMjM0NTY3ODkwMTIzNDU2Nzg5MDEyMzQ1Ng==".into()),
        20,
        "Fri, 10 May 2026 12:00:00 GMT",
    )
    .unwrap();

    assert_eq!(req.url, "https://acct.documents.azure.com:443/dbs/mydb/colls/mycoll/docs");
    assert_eq!(req.method, "POST");
    assert!(req.headers.iter().any(|(k, _)| k == "x-ms-version"));
    assert!(req.headers.iter().any(|(k, _)| k == "x-ms-documentdb-isquery"));
    assert!(req.headers.iter().any(|(k, _)| k == "x-ms-documentdb-query-enablecrosspartition"));
    assert!(req.headers.iter().any(|(k, _)| k == "x-ms-date"));
    assert!(req.headers.iter().any(|(k, v)| k == "Authorization" && v.starts_with("type%3Dmaster")));
    assert_eq!(req.body, r#"{"query":"SELECT TOP @n * FROM c","parameters":[{"name":"@n","value":20}]}"#);
}

#[test]
fn build_query_request_bearer_uses_aad_header() {
    let req = build_query_request(
        "https://acct.documents.azure.com:443/",
        "mydb",
        "mycoll",
        &CosmosAuth::Bearer("eyFAKE.TOKEN".into()),
        5,
        "Fri, 10 May 2026 12:00:00 GMT",
    )
    .unwrap();

    let auth_value = req
        .headers
        .iter()
        .find(|(k, _)| k == "Authorization")
        .map(|(_, v)| v.as_str())
        .unwrap();
    assert_eq!(auth_value, "Bearer eyFAKE.TOKEN");
    assert!(req.body.contains("\"value\":5"));
}

#[test]
fn build_query_request_handles_endpoint_without_trailing_slash() {
    let req = build_query_request(
        "https://acct.documents.azure.com:443",
        "mydb",
        "mycoll",
        &CosmosAuth::Bearer("t".into()),
        1,
        "Fri, 10 May 2026 12:00:00 GMT",
    )
    .unwrap();
    assert_eq!(req.url, "https://acct.documents.azure.com:443/dbs/mydb/colls/mycoll/docs");
}
```

- [ ] **Step 2: Run, verify they fail**

```bash
cargo test -p rigg-client cosmos::tests::build_query_request -- --nocapture
```

Expected: FAIL with `cannot find type 'CosmosAuth'` and `cannot find function 'build_query_request'`.

- [ ] **Step 3: Implement `CosmosAuth` and `build_query_request`**

Add to `cosmos.rs` (above the `mod tests` block):

```rust
/// Authentication mode for Cosmos REST calls.
#[derive(Debug, Clone)]
pub enum CosmosAuth {
    /// AAD bearer token (acquired via `AzCliAuth::for_cosmos()`).
    Bearer(String),
    /// Cosmos master key (base64-encoded). Use with care; secret material.
    MasterKey(String),
}

/// A built (but not sent) HTTP request to Cosmos. Used by `sample_documents`
/// and exposed for unit testing the construction logic without hitting the network.
#[derive(Debug)]
pub struct CosmosRequest {
    pub method: &'static str,
    pub url: String,
    pub headers: Vec<(&'static str, String)>,
    pub body: String,
}

/// Build a Cosmos `query documents` REST request. Pure: no network I/O.
pub fn build_query_request(
    endpoint: &str,
    database: &str,
    container: &str,
    auth: &CosmosAuth,
    sample_size: u32,
    rfc1123_date: &str,
) -> Result<CosmosRequest, CosmosError> {
    let endpoint = endpoint.trim_end_matches('/');
    let url = format!("{endpoint}/dbs/{database}/colls/{container}/docs");

    let resource_link = format!("dbs/{database}/colls/{container}");
    let auth_value = match auth {
        CosmosAuth::Bearer(token) => format!("Bearer {token}"),
        CosmosAuth::MasterKey(key) => {
            build_master_key_authorization_token("POST", "docs", &resource_link, rfc1123_date, key)?
        }
    };

    let body = format!(
        r#"{{"query":"SELECT TOP @n * FROM c","parameters":[{{"name":"@n","value":{sample_size}}}]}}"#
    );

    let headers = vec![
        ("Authorization", auth_value),
        ("x-ms-version", "2018-12-31".to_string()),
        ("x-ms-date", rfc1123_date.to_string()),
        ("x-ms-documentdb-isquery", "True".to_string()),
        (
            "x-ms-documentdb-query-enablecrosspartition",
            "True".to_string(),
        ),
        ("Content-Type", "application/query+json".to_string()),
        ("Accept", "application/json".to_string()),
    ];

    Ok(CosmosRequest {
        method: "POST",
        url,
        headers,
        body,
    })
}
```

- [ ] **Step 4: Run, verify all three new tests pass**

```bash
cargo test -p rigg-client cosmos::tests::build_query_request -- --nocapture
```

Expected: 3 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rigg-client/src/cosmos.rs
git commit -m "rigg-client: cosmos request builder + CosmosAuth enum"
```

---

## Task 6: `sample_documents` async network function

**Files:**
- Modify: `crates/rigg-client/src/cosmos.rs`

There is no automated test for the network call in this plan. The deterministic `build_query_request` is covered in Task 5; the network execution is exercised manually in Task 12 against a real Cosmos account. Errors surface through the existing `CosmosError` enum; the new `Network` variant is added in Step 1 below.

- [ ] **Step 1: Extend `CosmosError` with a `Network` variant**

Replace the `CosmosError` enum near the top of `cosmos.rs` (added in Task 3) with the expanded version below:

```rust
#[derive(Debug, Error)]
pub enum CosmosError {
    #[error("Invalid Cosmos connection string: {0}")]
    InvalidConnectionString(String),
    #[error("Cosmos REST network error: {0}")]
    Network(String),
}
```

- [ ] **Step 2: Implement `sample_documents`**

Append to `cosmos.rs` (after the existing functions, before `mod tests`):

```rust
use chrono::Utc;
use serde_json::Value;

/// Sample up to `sample_size` documents from a Cosmos container.
///
/// AAD via `CosmosAuth::Bearer` is preferred; `CosmosAuth::MasterKey` is the
/// connection-string fallback path. Caller is responsible for choosing.
pub async fn sample_documents(
    endpoint: &str,
    database: &str,
    container: &str,
    auth: &CosmosAuth,
    sample_size: u32,
) -> Result<Vec<Value>, CosmosError> {
    // RFC1123 in GMT, e.g., "Fri, 10 May 2026 12:00:00 GMT".
    let date = Utc::now().format("%a, %d %b %Y %H:%M:%S GMT").to_string();
    let req = build_query_request(endpoint, database, container, auth, sample_size, &date)?;

    let client = reqwest::Client::new();
    let mut request_builder = client
        .request(reqwest::Method::POST, &req.url)
        .body(req.body);
    for (k, v) in &req.headers {
        request_builder = request_builder.header(*k, v);
    }

    let resp = request_builder
        .send()
        .await
        .map_err(|e| CosmosError::Network(format!("request failed: {e}")))?;
    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| CosmosError::Network(format!("read body: {e}")))?;

    if !status.is_success() {
        return Err(CosmosError::Network(format!(
            "Cosmos returned {status}: {body}"
        )));
    }

    let parsed: Value = serde_json::from_str(&body)
        .map_err(|e| CosmosError::Network(format!("invalid JSON response: {e}")))?;

    let docs = parsed
        .get("Documents")
        .and_then(|d| d.as_array())
        .cloned()
        .unwrap_or_default();

    Ok(docs)
}
```

- [ ] **Step 3: Verify the crate still compiles**

```bash
cargo build -p rigg-client
```

Expected: build succeeds.

- [ ] **Step 4: Run all existing tests to confirm no regression**

```bash
cargo test -p rigg-client
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rigg-client/src/cosmos.rs
git commit -m "rigg-client: sample_documents async REST call"
```

---

## Task 7: Cosmos-aware `scaffold_datasource`

The existing `scaffold_datasource(name, ds_type, container)` is generic. We extend it so that when `ds_type == "cosmosdb"` it includes a default query and the `_ts` change-detection policy. Other types unchanged.

**Files:**
- Modify: `crates/rigg-core/src/scaffold.rs:79-90` (function body) and the `mod tests` block.

- [ ] **Step 1: Write the failing test**

Append to the `mod tests` block in `scaffold.rs`:

```rust
#[test]
fn test_scaffold_datasource_cosmosdb_includes_query_and_change_detection() {
    let ds = scaffold_datasource("my-cosmos", "cosmosdb", "my-container");
    assert_eq!(ds["name"], "my-cosmos");
    assert_eq!(ds["type"], "cosmosdb");
    assert_eq!(ds["container"]["name"], "my-container");
    assert_eq!(ds["container"]["query"], "SELECT * FROM c");
    assert_eq!(
        ds["dataChangeDetectionPolicy"]["@odata.type"],
        "#Microsoft.Azure.Search.HighWaterMarkChangeDetectionPolicy"
    );
    assert_eq!(
        ds["dataChangeDetectionPolicy"]["highWaterMarkColumnName"],
        "_ts"
    );
}

#[test]
fn test_scaffold_datasource_azureblob_unchanged() {
    let ds = scaffold_datasource("my-blob", "azureblob", "documents");
    assert_eq!(ds["name"], "my-blob");
    assert_eq!(ds["type"], "azureblob");
    assert_eq!(ds["container"]["name"], "documents");
    assert!(ds.get("dataChangeDetectionPolicy").is_none());
    // The container block should NOT have a 'query' for blob
    assert!(ds["container"].get("query").is_none());
}
```

- [ ] **Step 2: Run, verify the new tests fail**

```bash
cargo test -p rigg-core test_scaffold_datasource_cosmosdb_includes_query_and_change_detection -- --nocapture
cargo test -p rigg-core test_scaffold_datasource_azureblob_unchanged -- --nocapture
```

Expected: the cosmosdb test FAILs with "left=null right='SELECT * FROM c'"; the azureblob test PASSes already.

- [ ] **Step 3: Implement Cosmos-specific defaults in `scaffold_datasource`**

Replace the body of `scaffold_datasource` in `crates/rigg-core/src/scaffold.rs` with:

```rust
pub fn scaffold_datasource(name: &str, ds_type: &str, container: &str) -> Value {
    let mut container_block = json!({ "name": container });
    if ds_type == "cosmosdb" {
        container_block["query"] = json!("SELECT * FROM c");
    }

    let mut ds = json!({
        "name": name,
        "type": ds_type,
        "credentials": {
            "connectionString": ""
        },
        "container": container_block
    });

    if ds_type == "cosmosdb" {
        ds["dataChangeDetectionPolicy"] = json!({
            "@odata.type": "#Microsoft.Azure.Search.HighWaterMarkChangeDetectionPolicy",
            "highWaterMarkColumnName": "_ts"
        });
    }

    ds
}
```

- [ ] **Step 4: Run, verify all `scaffold` tests pass**

```bash
cargo test -p rigg-core scaffold:: -- --nocapture
```

Expected: all `scaffold::tests::*` PASS, including the existing `test_scaffold_datasource` and `test_scaffold_datasource_types` tests (the latter loops over types and asserts only `type` matches — unchanged behavior).

- [ ] **Step 5: Commit**

```bash
git add crates/rigg-core/src/scaffold.rs
git commit -m "rigg-core: scaffold_datasource emits Cosmos query + change detection"
```

---

## Task 8: Add `scaffold_knowledge_source_typed` for Cosmos KS

The existing `scaffold_knowledge_source(name, index, knowledge_base)` produces a minimal KS. Cosmos-backed KSes need `kind: "azureCosmosDB"` plus a `azureCosmosDBParameters` block (matching the pattern in `scaffold_agentic_rag` at `scaffold.rs:218-227`). We add a new function that takes an explicit data-source type.

**Files:**
- Modify: `crates/rigg-core/src/scaffold.rs:152-164` (after the existing `scaffold_knowledge_source`) and the `mod tests` block.

- [ ] **Step 1: Write the failing tests**

Append to the `mod tests` block in `scaffold.rs`:

```rust
#[test]
fn test_scaffold_knowledge_source_typed_cosmosdb() {
    let ks = scaffold_knowledge_source_typed(
        "my-ks",
        "my-ks-index",
        None,
        "azureCosmosDB",
        Some("my-container"),
    );
    assert_eq!(ks["name"], "my-ks");
    assert_eq!(ks["indexName"], "my-ks-index");
    assert_eq!(ks["kind"], "azureCosmosDB");
    assert_eq!(ks["azureCosmosDBParameters"]["containerName"], "my-container");
    assert!(ks.get("knowledgeBaseName").is_none());
}

#[test]
fn test_scaffold_knowledge_source_typed_with_kb() {
    let ks = scaffold_knowledge_source_typed(
        "my-ks",
        "my-ks-index",
        Some("my-kb"),
        "azureCosmosDB",
        Some("docs"),
    );
    assert_eq!(ks["knowledgeBaseName"], "my-kb");
    assert_eq!(ks["azureCosmosDBParameters"]["containerName"], "docs");
}

#[test]
fn test_scaffold_knowledge_source_typed_no_container_omits_parameters() {
    // Defensive: if no container is supplied, no parameters block is emitted —
    // the user can fill it in manually.
    let ks = scaffold_knowledge_source_typed("my-ks", "my-ks-index", None, "azureCosmosDB", None);
    assert!(ks.get("azureCosmosDBParameters").is_none());
    assert_eq!(ks["kind"], "azureCosmosDB");
}
```

- [ ] **Step 2: Run, verify they fail**

```bash
cargo test -p rigg-core test_scaffold_knowledge_source_typed -- --nocapture
```

Expected: 3 FAILs with "cannot find function `scaffold_knowledge_source_typed`".

- [ ] **Step 3: Implement `scaffold_knowledge_source_typed`**

Add to `crates/rigg-core/src/scaffold.rs` directly below the existing `scaffold_knowledge_source` function (around line 164):

```rust
/// Scaffold a Knowledge Source with an explicit data-source `kind` (e.g.,
/// `"azureBlob"`, `"azureCosmosDB"`).
///
/// When `container` is `Some`, a `<kind>Parameters` block is emitted with
/// `containerName`. The `kind` value is used verbatim — pass the same casing
/// Azure expects (`azureBlob`, `azureCosmosDB`, etc.).
pub fn scaffold_knowledge_source_typed(
    name: &str,
    index: &str,
    knowledge_base: Option<&str>,
    kind: &str,
    container: Option<&str>,
) -> Value {
    let mut ks = json!({
        "name": name,
        "indexName": index,
        "kind": kind,
    });

    if let Some(kb) = knowledge_base {
        ks["knowledgeBaseName"] = json!(kb);
    }

    if let Some(c) = container {
        let params_key = format!("{kind}Parameters");
        ks[params_key] = json!({ "containerName": c });
    }

    ks
}
```

- [ ] **Step 4: Run, verify all 3 new tests pass**

```bash
cargo test -p rigg-core test_scaffold_knowledge_source_typed -- --nocapture
```

Expected: 3 PASS.

- [ ] **Step 5: Run all `scaffold` tests to confirm no regressions**

```bash
cargo test -p rigg-core scaffold:: -- --nocapture
```

Expected: all PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/rigg-core/src/scaffold.rs
git commit -m "rigg-core: scaffold_knowledge_source_typed for Cosmos KS"
```

---

## Task 9: Add `--type` and `--container` flags to `rigg new knowledgesource`

**Files:**
- Modify: `crates/rigg/src/cli.rs:617-629` (the `NewCommands::KnowledgeSource` variant)

- [ ] **Step 1: Add the flags**

Replace the existing `KnowledgeSource` variant in `crates/rigg/src/cli.rs` (around line 617) with:

```rust
/// Create a new knowledge source definition
KnowledgeSource {
    /// Resource name
    name: String,

    /// Target index name
    #[arg(long)]
    index: String,

    /// Optional knowledge base name
    #[arg(long)]
    knowledge_base: Option<String>,

    /// Data source kind (e.g., "azureBlob", "azureCosmosDB").
    /// When omitted, produces a minimal KS the user must complete manually.
    #[arg(long)]
    r#type: Option<String>,

    /// Container or collection name (required when `--type` is set)
    #[arg(long)]
    container: Option<String>,
},
```

- [ ] **Step 2: Confirm the build still compiles (the command handler will be updated in Task 10)**

```bash
cargo build -p rigg
```

Expected: compile FAILS in `commands/scaffold.rs` because the destructuring pattern for `KnowledgeSource` no longer matches. **This is expected** — Task 10 fixes it.

- [ ] **Step 3: Do not commit yet**

Tasks 9 and 10 must land together (the CLI definition and the command wiring change in lockstep). Continue to Task 10 before committing.

---

## Task 10: Wire `--type` and `--container` through the KS scaffold command

**Files:**
- Modify: `crates/rigg/src/commands/scaffold.rs:131-154` (the `KnowledgeSource` arm)

- [ ] **Step 1: Update the destructuring pattern and call the typed scaffold when `--type` is provided**

Replace the `NewCommands::KnowledgeSource` arm in `crates/rigg/src/commands/scaffold.rs` (around line 131) with:

```rust
NewCommands::KnowledgeSource {
    name,
    index,
    knowledge_base,
    r#type,
    container,
} => {
    validate_resource_name(&name)?;

    if r#type.is_none() && container.is_some() {
        bail!("--container requires --type to be set");
    }

    let value = match r#type.as_deref() {
        Some(kind) => scaffold::scaffold_knowledge_source_typed(
            &name,
            &index,
            knowledge_base.as_deref(),
            kind,
            container.as_deref(),
        ),
        None => scaffold::scaffold_knowledge_source(&name, &index, knowledge_base.as_deref()),
    };
    let path = write_knowledge_source(&env, &files_root, &name, &value)?;
    print_created("Knowledge Source", &name, &path);
    println!();
    if let Some(kind) = &r#type {
        println!("  Kind: {}", kind);
    }
    println!("  After pushing, Azure will auto-provision managed sub-resources:");
    println!("    {}-index         (search index)", name);
    println!("    {}-indexer       (indexer)", name);
    println!("    {}-datasource    (data source)", name);
    println!("    {}-skillset      (skillset)", name);
    println!();
    println!("  Next: {}", "rigg push --knowledgesources".bold());
    println!(
        "  Then: {}  (to sync managed resources back)",
        "rigg pull --knowledgesources".bold()
    );
    Ok(())
}
```

- [ ] **Step 2: Verify the build now succeeds**

```bash
cargo build -p rigg
```

Expected: PASS.

- [ ] **Step 3: Run the rigg crate tests**

```bash
cargo test -p rigg
```

Expected: all tests PASS (no new tests added, but pattern-match tests should still pass).

- [ ] **Step 4: Manual smoke check**

In a scratch directory **outside** the rigg repo (e.g., `/tmp/rigg-test`), set up a minimal `rigg.yaml` and verify the command runs end-to-end. Skip this step if you don't have a valid project handy — the next task adds an integration test against the existing `test-projects/` machinery.

```bash
cd /tmp && mkdir -p rigg-test && cd rigg-test
# Create a minimal rigg.yaml with at least one search service if needed,
# OR just observe the error message — it should mention 'No search service'
# and not panic.
cargo run --manifest-path /Users/kristofer/repos/rigg/Cargo.toml --bin rigg -- \
  new knowledgesource demo --index demo-idx --type azureCosmosDB --container demo-container
```

Expected (no project): clean error message about missing config, NOT a panic.

- [ ] **Step 5: Commit (CLI + command wiring together)**

```bash
git add crates/rigg/src/cli.rs crates/rigg/src/commands/scaffold.rs
git commit -m "rigg: --type and --container flags for 'rigg new knowledgesource'"
```

---

## Task 11: Cosmos-specific lint rules in `rigg validate`

**Files:**
- Modify: `crates/rigg/src/commands/validate/lint.rs` (extend `lint_datasource`)

- [ ] **Step 1: Locate the existing `lint_datasource` test pattern**

Open `crates/rigg/src/commands/validate/lint.rs`. Read the existing `mod tests` (search for `#[cfg(test)] mod tests` near the end of the file). New tests follow the same shape as the existing data-source lint tests.

- [ ] **Step 2: Write the failing tests**

Append to the `mod tests` block:

```rust
#[test]
fn test_lint_cosmosdb_warns_when_missing_change_detection() {
    let ds = serde_json::json!({
        "name": "my-cosmos",
        "type": "cosmosdb",
        "credentials": { "connectionString": "..." },
        "container": { "name": "my-container", "query": "SELECT * FROM c" }
        // dataChangeDetectionPolicy intentionally omitted
    });
    let mut warnings = Vec::new();
    lint_datasource("my-cosmos", &ds, &mut warnings);
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("my-cosmos") && w.contains("dataChangeDetectionPolicy")),
        "expected change-detection warning, got: {warnings:?}"
    );
}

#[test]
fn test_lint_cosmosdb_warns_when_missing_query() {
    let ds = serde_json::json!({
        "name": "my-cosmos",
        "type": "cosmosdb",
        "credentials": { "connectionString": "..." },
        "container": { "name": "my-container" }
        // query intentionally omitted
    });
    let mut warnings = Vec::new();
    lint_datasource("my-cosmos", &ds, &mut warnings);
    assert!(
        warnings.iter().any(|w| w.contains("query")),
        "expected query warning, got: {warnings:?}"
    );
}

#[test]
fn test_lint_cosmosdb_no_warning_when_complete() {
    let ds = serde_json::json!({
        "name": "my-cosmos",
        "type": "cosmosdb",
        "credentials": { "connectionString": "..." },
        "container": { "name": "my-container", "query": "SELECT * FROM c" },
        "dataChangeDetectionPolicy": {
            "@odata.type": "#Microsoft.Azure.Search.HighWaterMarkChangeDetectionPolicy",
            "highWaterMarkColumnName": "_ts"
        }
    });
    let mut warnings = Vec::new();
    lint_datasource("my-cosmos", &ds, &mut warnings);
    let cosmos_warnings: Vec<_> = warnings
        .iter()
        .filter(|w| w.contains("query") || w.contains("dataChangeDetectionPolicy"))
        .collect();
    assert!(
        cosmos_warnings.is_empty(),
        "expected no Cosmos warnings, got: {cosmos_warnings:?}"
    );
}

#[test]
fn test_lint_azureblob_unchanged_by_cosmos_rules() {
    let ds = serde_json::json!({
        "name": "my-blob",
        "type": "azureblob",
        "credentials": { "connectionString": "..." },
        "container": { "name": "documents" }
    });
    let mut warnings = Vec::new();
    lint_datasource("my-blob", &ds, &mut warnings);
    // Cosmos-specific warnings should not fire for blob data sources
    assert!(
        !warnings.iter().any(|w| w.contains("dataChangeDetectionPolicy")),
        "blob data source got Cosmos warning: {warnings:?}"
    );
}
```

- [ ] **Step 3: Run, verify the new tests fail**

```bash
cargo test -p rigg test_lint_cosmosdb -- --nocapture
```

Expected: 2 FAILs (`test_lint_cosmosdb_warns_when_missing_change_detection`, `test_lint_cosmosdb_warns_when_missing_query`); 1 PASS (`test_lint_cosmosdb_no_warning_when_complete`); 1 PASS (`test_lint_azureblob_unchanged_by_cosmos_rules`) — these latter two pass because no Cosmos lint exists yet to misbehave.

- [ ] **Step 4: Implement the lint extension**

The current `lint_datasource` (in `crates/rigg/src/commands/validate/lint.rs`, ~line 73) is:

```rust
fn lint_datasource(name: &str, value: &serde_json::Value, warnings: &mut Vec<String>) {
    // Check for empty or missing container name
    let container_name = value
        .get("container")
        .and_then(|c| c.get("name"))
        .and_then(|n| n.as_str())
        .unwrap_or("");

    if container_name.is_empty() {
        warnings.push(format!(
            "data-sources/{}.json: container name is empty or missing",
            name
        ));
    }
}
```

Add the new Cosmos-specific block at the end of the function, **before its closing `}`**, leaving the existing container-name check unchanged. Note the directory in warning messages is `data-sources/` (with hyphen) to match the existing style:

```rust
    // Cosmos-specific lints
    if value.get("type").and_then(|t| t.as_str()) == Some("cosmosdb") {
        let has_change_detection = value.get("dataChangeDetectionPolicy").is_some();
        if !has_change_detection {
            warnings.push(format!(
                "data-sources/{name}.json: cosmosdb data source has no \"dataChangeDetectionPolicy\" — \
                 incremental indexing will not work; consider adding a HighWaterMark policy on \"_ts\""
            ));
        }
        let has_query = value
            .get("container")
            .and_then(|c| c.get("query"))
            .and_then(|q| q.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false);
        if !has_query {
            warnings.push(format!(
                "data-sources/{name}.json: cosmosdb data source has no \"container.query\" — \
                 a query (e.g., \"SELECT * FROM c\") is recommended"
            ));
        }
    }
```

- [ ] **Step 5: Run, verify all four tests pass**

```bash
cargo test -p rigg test_lint_cosmosdb -- --nocapture
cargo test -p rigg test_lint_azureblob_unchanged_by_cosmos_rules -- --nocapture
```

Expected: all 4 PASS.

- [ ] **Step 6: Run all rigg tests to confirm no regressions**

```bash
cargo test -p rigg
```

Expected: all PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/rigg/src/commands/validate/lint.rs
git commit -m "rigg: lint Cosmos data sources for missing query and change-detection"
```

---

## Task 12: Manual end-to-end verification against a real Cosmos account

This task is a manual procedure — no code, no automated test. It validates that the scaffolds and the existing `rigg push` actually work against Azure with a Cosmos-backed KS.

**Files:** none (manual procedure).

**Prerequisite:** an Azure subscription with a Cosmos DB account, a database, and a container with at least one document; an Azure AI Search service the user has access to; user is signed in via `az login`.

- [ ] **Step 1: Create a scratch test project**

```bash
mkdir -p /tmp/rigg-cosmos-test && cd /tmp/rigg-cosmos-test
cargo run --manifest-path /Users/kristofer/repos/rigg/Cargo.toml --bin rigg -- init --template agentic
```

Follow the interactive prompts to point at your Cosmos-adjacent search service. Confirm `rigg.yaml` is written.

- [ ] **Step 2: Scaffold the data source**

```bash
cargo run --manifest-path /Users/kristofer/repos/rigg/Cargo.toml --bin rigg -- \
  new datasource cosmos-test --type cosmosdb --container <your-container>
```

- [ ] **Step 3: Edit the generated data source file**

Open the generated `search/<service>/search-management/data-sources/cosmos-test.json` and fill in `credentials.connectionString` with your Cosmos connection string.

Verify the file already contains:
- `"type": "cosmosdb"`
- `"container": { "name": "<your-container>", "query": "SELECT * FROM c" }`
- `dataChangeDetectionPolicy` block with `"_ts"`

- [ ] **Step 4: Scaffold the knowledge source**

```bash
cargo run --manifest-path /Users/kristofer/repos/rigg/Cargo.toml --bin rigg -- \
  new knowledgesource cosmos-test-ks \
    --index cosmos-test-idx \
    --type azureCosmosDB \
    --container <your-container>
```

Verify the generated KS file has:
- `"kind": "azureCosmosDB"`
- `"azureCosmosDBParameters": { "containerName": "<your-container>" }`

- [ ] **Step 5: Run validate**

```bash
cargo run --manifest-path /Users/kristofer/repos/rigg/Cargo.toml --bin rigg -- \
  validate --strict --check-references
```

Expected: no errors. May show warnings about empty connection strings if you forgot Step 3.

- [ ] **Step 6: Push**

```bash
cargo run --manifest-path /Users/kristofer/repos/rigg/Cargo.toml --bin rigg -- push --all
```

Confirm preview, accept, and watch it push. If the Azure preview API rejects the KS payload, capture the error message — the spec's "Open implementation questions" section anticipates discovering schema details here.

- [ ] **Step 7: Verify the KS exists in Azure**

```bash
cargo run --manifest-path /Users/kristofer/repos/rigg/Cargo.toml --bin rigg -- \
  pull --knowledgesources --filter cosmos-test-ks
```

Expected: pulls back the KS and any auto-provisioned managed sub-resources.

- [ ] **Step 8: Document any preview-API surprises**

If Step 6 surfaces required fields the scaffold doesn't emit, file a follow-up task in the Phase 2 plan or this plan's epilogue. Update `scaffold_knowledge_source_typed` accordingly. Do this in a separate commit titled `rigg-core: <specific-thing> required by Cosmos KS preview API`.

- [ ] **Step 9: Tear down the scratch project**

```bash
cd ~ && rm -rf /tmp/rigg-cosmos-test
# Optionally also delete the Azure resources via:
#   rigg delete --knowledgesource cosmos-test-ks --target remote
#   rigg delete --datasource cosmos-test --target remote
```

---

## Task 13: Pre-push verification

**Files:** none.

- [ ] **Step 1: Run all CI checks**

```bash
cargo fmt --all -- --check
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

All three must pass cleanly. (CLAUDE.md: required before declaring the task complete.)

- [ ] **Step 2: Confirm no untracked files were left behind**

```bash
git status
```

Expected: clean tree (or only the manual `test-projects/` directory if it exists per CLAUDE.md, which is gitignored).

---

## Done

End of phase 1. The user can now manually configure a Cosmos DB → Knowledge Source pipeline using `rigg new` + an editor + `rigg push`. The `rigg-client::cosmos` library is ready for Phase 2's `rigg analyze cosmos` command.

Phase 2 plan (to be written next): standalone `rigg analyze` and `rigg suggest` commands.
