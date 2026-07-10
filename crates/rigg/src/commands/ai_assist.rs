//! Ailloy-powered assistance: explanations, conflict-merge proposals, doctor
//! advice (spec §10). Everything here is gated on the user having enabled AI
//! for rigg (`rigg ai enable`) and not passing `--no-ai`; anything that
//! mutates state is proposed only — the user always confirms.

use anyhow::{Result, anyhow};
use serde_json::Value;

use crate::commands::GlobalContext;

/// Is AI assistance active for this invocation?
pub fn ai_on(ctx: &GlobalContext) -> bool {
    !ctx.no_ai && ailloy::config_tui::is_ai_active("rigg")
}

/// Plain-language summary of a diff report.
pub async fn explain_diff(report: &str) -> Result<String> {
    let system = "You explain configuration differences between a developer's LOCAL files and \
                  what is currently in AZURE (Azure AI Search / Microsoft Foundry). The report \
                  labels each side — attribute every value to the correct side and NEVER assume \
                  the user intends to push or pull. Structure your answer as: one or two lines on \
                  what differs (interpret, don't restate every field); then 'If you pull:' — what \
                  the local files would become; then 'If you push:' — what would change in Azure, \
                  flagging risks under that direction only (deletions, immutable index fields, \
                  SKU/capacity/billing). Max 150 words.";
    let user = format!("Diff report:\n{report}");
    rigg_client::ai::generate_text(system, &user).await
}

/// Propose a merged document for a conflicted resource. Returns parsed JSON.
pub async fn propose_merge(resource: &str, local: &Value, remote: &Value) -> Result<Value> {
    let system = "You merge two conflicting versions of an Azure resource definition (JSON). \
                  Preserve the intent of BOTH sides: keep additions from each unless they contradict; \
                  on direct contradictions prefer the local version and keep the remote value only when \
                  the local one is clearly a placeholder. NEVER invent fields, NEVER include secrets. \
                  Respond with ONLY the merged JSON document — no prose, no code fences.";
    let user = format!(
        "Resource: {resource}\n\nLOCAL version (edited by the user):\n{}\n\nREMOTE version (currently in Azure):\n{}",
        serde_json::to_string_pretty(local)?,
        serde_json::to_string_pretty(remote)?
    );
    let response = rigg_client::ai::generate_text_with_limit(system, &user, 8000).await?;
    let text = response.trim();
    let text = text
        .strip_prefix("```json")
        .or_else(|| text.strip_prefix("```"))
        .unwrap_or(text)
        .trim_end_matches("```")
        .trim();
    serde_json::from_str(text).map_err(|e| anyhow!("AI returned invalid JSON: {e}"))
}

/// Remediation advice for auth doctor failures.
pub async fn explain_doctor(failures: &[String]) -> Result<String> {
    let system = "You advise on Azure managed-identity and RBAC problems for an Azure AI Search + \
                  Microsoft Foundry stack. For each problem give the likely cause and the concrete fix \
                  (az CLI or portal step). Max 150 words total.";
    let user = format!("Problems found:\n- {}", failures.join("\n- "));
    rigg_client::ai::generate_text(system, &user).await
}
