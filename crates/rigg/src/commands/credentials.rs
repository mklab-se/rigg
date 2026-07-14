//! Identity-based data-source connection helpers.
//!
//! Rigg never stores keys in files; data sources use keyless
//! `ResourceId=<storage account ARM id>;` references and the search
//! service's managed identity. Azure's GET responses never return
//! credentials, so copied/migrated definitions arrive without one — these
//! helpers detect that and, since the user is already logged in via Azure
//! CLI, DISCOVER the right storage account through ARM (by the container
//! the data source reads) instead of asking the user to hand-type an id.

use anyhow::Result;
use colored::Colorize;
use serde_json::Value;

use rigg_client::arm::ArmClient;

use crate::commands::interactive;

/// A data source with no usable connection (missing/null/empty
/// `credentials.connectionString`).
pub fn missing_credentials(doc: &Value) -> bool {
    doc.pointer("/credentials/connectionString")
        .and_then(Value::as_str)
        .map(|s| s.trim().is_empty())
        .unwrap_or(true)
}

/// The blob container a data source reads from, when declared.
pub fn container_name(doc: &Value) -> Option<&str> {
    doc.pointer("/container/name").and_then(Value::as_str)
}

/// Set `credentials.connectionString` on a data-source document.
pub fn set_connection(doc: &mut Value, connection: &str) {
    doc["credentials"] = serde_json::json!({ "connectionString": connection });
}

/// After an identity-based connection is chosen the search service's
/// managed identity still needs data-plane RBAC — point at the doctor.
pub fn print_rbac_hint(account: &str) {
    println!(
        "  hint: the search service's managed identity needs 'Storage Blob Data Reader' on {account} — run `rigg auth doctor --fix` to verify/grant"
    );
}

/// Interactively resolve an identity-based connection for a data source:
/// discover which storage account(s) hold its container via ARM (Azure CLI
/// login), confirm/select, with a manual entry fallback. Returns the chosen
/// `ResourceId=...;` string, or None when the user skips.
pub async fn discover_connection_interactive(
    ds_display: &str,
    container: Option<&str>,
    plain: bool,
) -> Result<Option<String>> {
    let Some(container) = container else {
        println!(
            "  {} {ds_display} declares no container — cannot auto-discover its storage account",
            "!".yellow()
        );
        return manual_entry(plain);
    };
    println!(
        "  looking up which storage account holds container '{container}' (ARM, via your az login)..."
    );
    let arm = match ArmClient::new() {
        Ok(arm) => arm,
        Err(e) => {
            println!(
                "  {} ARM access unavailable ({e}) — enter the connection manually",
                "!".yellow()
            );
            return manual_entry(plain);
        }
    };
    let matches = match arm.find_storage_accounts_with_container(container).await {
        Ok(m) => m,
        Err(e) => {
            println!("  {} discovery failed ({e})", "!".yellow());
            return manual_entry(plain);
        }
    };
    match matches.as_slice() {
        [] => {
            println!(
                "  {} no storage account with a container '{container}' is visible to your login",
                "!".yellow()
            );
            manual_entry(plain)
        }
        [account] => {
            println!("  found {} — {}", account.name.bold(), account.id);
            if interactive::confirm_default_yes(
                &format!(
                    "Use identity-based access to '{}' for {ds_display}?",
                    account.name
                ),
                plain,
            )? {
                Ok(Some(format!("ResourceId={};", account.id)))
            } else {
                manual_entry(plain)
            }
        }
        many => {
            const MANUAL: &str = "enter manually";
            let mut options: Vec<String> = many
                .iter()
                .map(|a| format!("{} — {}", a.name, a.id))
                .collect();
            options.push(MANUAL.to_string());
            let choice = interactive::select(
                &format!(
                    "Several storage accounts hold a container '{container}' — which one does {ds_display} read?"
                ),
                options,
                plain,
            )?;
            if choice == MANUAL {
                return manual_entry(plain);
            }
            let account = many
                .iter()
                .find(|a| choice.starts_with(&a.name))
                .expect("choice derived from list");
            Ok(Some(format!("ResourceId={};", account.id)))
        }
    }
}

fn manual_entry(plain: bool) -> Result<Option<String>> {
    let entered = interactive::text_with_default(
        "Storage connection (ResourceId=/subscriptions/.../storageAccounts/<name>;) — empty to skip:",
        "",
        plain,
    )?;
    let entered = entered.trim().to_string();
    Ok((!entered.is_empty()).then_some(entered))
}

/// A skillset whose `cognitiveServices` connection is key-based but carries
/// no usable key (Azure never returns keys on GET, so copied definitions
/// arrive with a null or `<redacted>` placeholder). Returns the subdomain
/// URL when one is declared — the ingredient needed for the identity-based
/// rewrite.
pub fn skillset_missing_ai_services_key(doc: &Value) -> Option<Option<String>> {
    let cs = doc.get("cognitiveServices")?.as_object()?;
    let odata = cs.get("@odata.type").and_then(Value::as_str).unwrap_or("");
    if !odata.ends_with("ByKey") {
        return None;
    }
    let key = cs.get("key").and_then(Value::as_str).unwrap_or("");
    if !key.trim().is_empty() && key != "<redacted>" {
        return None; // a real key — validate rejects it elsewhere
    }
    Some(
        cs.get("subdomainUrl")
            .and_then(Value::as_str)
            .map(str::to_string),
    )
}

/// Rewrite a skillset's AI services connection to the keyless
/// identity-based form (the search service's system-assigned managed
/// identity authenticates; nothing secret on disk).
pub fn set_ai_services_identity(doc: &mut Value, subdomain_url: &str) {
    doc["cognitiveServices"] = serde_json::json!({
        "@odata.type": "#Microsoft.Azure.Search.AIServicesByIdentity",
        "subdomainUrl": subdomain_url,
        "identity": null
    });
}

/// The account name in an AI services subdomain URL
/// (`https://<name>.cognitiveservices.azure.com/` → `<name>`).
pub fn ai_services_account_name(subdomain_url: &str) -> Option<&str> {
    subdomain_url
        .strip_prefix("https://")
        .and_then(|rest| rest.split('.').next())
        .filter(|s| !s.is_empty())
}

/// RBAC pointer for the identity-based AI services connection.
pub fn print_ai_services_rbac_hint(account: &str) {
    println!(
        "  hint: the search service's managed identity needs 'Cognitive Services User' on AI services account '{account}' — run `rigg auth doctor --fix` to verify/grant"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn detects_key_based_ai_services_with_placeholder() {
        let doc = json!({"name": "s", "cognitiveServices": {
            "@odata.type": "#Microsoft.Azure.Search.AIServicesByKey",
            "key": "<redacted>",
            "subdomainUrl": "https://acc.cognitiveservices.azure.com/"
        }});
        assert_eq!(
            skillset_missing_ai_services_key(&doc),
            Some(Some("https://acc.cognitiveservices.azure.com/".to_string()))
        );
    }

    #[test]
    fn ignores_identity_based_and_real_keys() {
        let identity = json!({"name": "s", "cognitiveServices": {
            "@odata.type": "#Microsoft.Azure.Search.AIServicesByIdentity",
            "subdomainUrl": "https://acc.cognitiveservices.azure.com/"
        }});
        assert_eq!(skillset_missing_ai_services_key(&identity), None);
        let real = json!({"name": "s", "cognitiveServices": {
            "@odata.type": "#Microsoft.Azure.Search.AIServicesByKey",
            "key": "abc123",
            "subdomainUrl": "https://acc.cognitiveservices.azure.com/"
        }});
        assert_eq!(skillset_missing_ai_services_key(&real), None);
        let none = json!({"name": "s", "skills": []});
        assert_eq!(skillset_missing_ai_services_key(&none), None);
    }

    #[test]
    fn rewrite_sets_identity_form() {
        let mut doc = json!({"name": "s", "cognitiveServices": {
            "@odata.type": "#Microsoft.Azure.Search.AIServicesByKey",
            "key": null,
            "subdomainUrl": "https://acc.cognitiveservices.azure.com/"
        }});
        set_ai_services_identity(&mut doc, "https://acc.cognitiveservices.azure.com/");
        assert_eq!(
            doc["cognitiveServices"]["@odata.type"],
            "#Microsoft.Azure.Search.AIServicesByIdentity"
        );
        assert!(doc["cognitiveServices"].get("key").is_none());
    }

    #[test]
    fn account_name_from_subdomain() {
        assert_eq!(
            ai_services_account_name("https://mklabaisrvc.cognitiveservices.azure.com/"),
            Some("mklabaisrvc")
        );
        assert_eq!(ai_services_account_name("nonsense"), None);
    }

    #[test]
    fn missing_credentials_detection() {
        assert!(missing_credentials(
            &json!({"credentials": {"connectionString": null}})
        ));
        assert!(missing_credentials(&json!({"name": "x"})));
        assert!(!missing_credentials(
            &json!({"credentials": {"connectionString": "ResourceId=/x;"}})
        ));
    }
}
