//! Show differences between local and remote resources.

mod ai;
mod compare;
mod cross_env;
mod format;
mod foundry;
mod search;

use anyhow::Result;

use hoist_core::resources::ResourceKind;
use hoist_core::service::ServiceDomain;

use crate::cli::{DiffFormat, ResourceTypeFlags};
use crate::commands::common::resolve_resource_selection_from_flags;
use crate::commands::load_config_and_env;

/// A diff result with full resource context for enhanced JSON output.
struct ResourceDiff {
    kind: ResourceKind,
    resource_name: String,
    display_id: String,
    result: hoist_diff::DiffResult,
    /// Full local content for AI narrative (YAML for agents, JSON for search)
    local_content: Option<String>,
    /// Full remote content for AI narrative
    remote_content: Option<String>,
}

pub async fn run(
    flags: &ResourceTypeFlags,
    format: DiffFormat,
    exit_code: bool,
    env_override: Option<&str>,
    compare_env: Option<&str>,
    no_explain: bool,
    explain_flag: bool,
) -> Result<()> {
    let (project_root, config, env) = load_config_and_env(env_override)?;
    let files_root = config.files_root(&project_root);

    // AI explanations: on by default when ai: is configured, unless --no-explain
    let use_explain = if no_explain {
        false
    } else if explain_flag {
        true
    } else {
        crate::commands::ai::is_ai_active()
    };

    // Cross-environment diff: compare two remotes directly
    if let Some(right_env_name) = compare_env {
        let right_env = config
            .resolve_env(Some(right_env_name))
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        return cross_env::run_cross_env_diff(flags, format, exit_code, &env, &right_env).await;
    }

    // Determine which resource types to diff
    let selection = resolve_resource_selection_from_flags(flags, env.sync.include_preview, true);

    if selection.is_empty() {
        println!("No resource types specified. Use --all or specify types (e.g., --indexes)");
        return Ok(());
    }

    let kinds = selection.kinds();

    // Split kinds by service domain
    let search_kinds: Vec<ResourceKind> = kinds
        .iter()
        .filter(|k| k.domain() == ServiceDomain::Search)
        .copied()
        .collect();
    let foundry_kinds: Vec<ResourceKind> = kinds
        .iter()
        .filter(|k| k.domain() == ServiceDomain::Foundry)
        .copied()
        .collect();

    let primary_search_svc = env.primary_search_service();

    let mut all_diffs: Vec<ResourceDiff> = Vec::new();
    let mut has_changes = false;

    // --- Search resources ---
    if let (false, Some(search_svc)) = (search_kinds.is_empty(), primary_search_svc) {
        let service_dir = env.search_service_dir(&files_root, search_svc);
        search::diff_search_resources(
            &search_kinds,
            search_svc,
            &service_dir,
            &selection,
            &mut all_diffs,
            &mut has_changes,
        )
        .await?;
    }

    // --- Foundry agents ---
    if !foundry_kinds.is_empty() && env.has_foundry() {
        foundry::diff_foundry_agents(
            &env,
            &files_root,
            &selection,
            &mut all_diffs,
            &mut has_changes,
        )
        .await?;
    }

    // Format output
    let labels = Some(("locally", "on the server"));
    match format {
        DiffFormat::Text => {
            if use_explain && has_changes {
                // AI narrative mode: single narrative replaces per-change descriptions
                let unchanged_count = all_diffs.iter().filter(|d| d.result.is_equal).count();
                match ai::generate_ai_narrative(&all_diffs, "diff", unchanged_count).await {
                    Some(narrative) => {
                        println!("{}", narrative);
                    }
                    None => {
                        // Fallback to non-AI output on error
                        format::format_diff_text(
                            &all_diffs,
                            labels,
                            &std::collections::HashMap::new(),
                        );
                    }
                }
            } else {
                format::format_diff_text(&all_diffs, labels, &std::collections::HashMap::new());
            }
        }
        DiffFormat::Json => {
            // JSON output: per-resource AI summaries for MCP/structured consumers
            let ai_summaries = if use_explain && has_changes {
                ai::generate_ai_summaries(&all_diffs).await
            } else {
                std::collections::HashMap::new()
            };
            let json = format::format_diff_json(
                &mut all_diffs,
                ("locally", "on the server"),
                &ai_summaries,
            );
            print!("{}", json);
        }
    }

    // Exit code handling
    if exit_code && has_changes {
        std::process::exit(5); // 5 = drift detected
    }

    Ok(())
}
