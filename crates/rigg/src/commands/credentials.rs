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
