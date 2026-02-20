//! `hoist new` — scaffold new resource files from templates
//!
//! Generates clean, valid resource files with sensible defaults.
//! No Azure connection required.

use anyhow::{Result, bail};
use colored::Colorize;

use hoist_core::normalize::format_json;
use hoist_core::resources::ResourceKind;
use hoist_core::resources::agent::agent_to_yaml;
use hoist_core::resources::traits::validate_resource_name;
use hoist_core::scaffold;
use hoist_core::service::ServiceDomain;

use crate::cli::NewCommands;
use crate::commands::load_config_and_env;

pub fn run(cmd: NewCommands, env_override: Option<&str>) -> Result<()> {
    let (project_root, config, env) = load_config_and_env(env_override)?;
    let files_root = config.files_root(&project_root);

    match cmd {
        NewCommands::Index {
            name,
            vector,
            semantic,
        } => {
            validate_resource_name(&name)?;
            let value = scaffold::scaffold_index(&name, vector, semantic);
            let path =
                write_search_resource(&env, &files_root, ResourceKind::Index, &name, &value)?;
            print_created("Index", &name, &path);
            println!("  Next: {}", "hoist push --indexes".bold());
            Ok(())
        }
        NewCommands::Datasource {
            name,
            r#type,
            container,
        } => {
            validate_resource_name(&name)?;
            let value = scaffold::scaffold_datasource(&name, &r#type, &container);
            let path =
                write_search_resource(&env, &files_root, ResourceKind::DataSource, &name, &value)?;
            print_created("Data Source", &name, &path);
            println!(
                "  {}",
                "Note: Set the connection string in Azure portal or via 'az' CLI before pushing."
                    .dimmed()
            );
            println!("  Next: {}", "hoist push --datasources".bold());
            Ok(())
        }
        NewCommands::Indexer {
            name,
            datasource,
            index,
            skillset,
            schedule,
        } => {
            validate_resource_name(&name)?;
            let value = scaffold::scaffold_indexer(
                &name,
                &datasource,
                &index,
                skillset.as_deref(),
                &schedule,
            );
            let path =
                write_search_resource(&env, &files_root, ResourceKind::Indexer, &name, &value)?;
            print_created("Indexer", &name, &path);
            let mut refs = format!("datasource '{}', index '{}'", datasource, index);
            if let Some(ss) = &skillset {
                refs.push_str(&format!(", skillset '{}'", ss));
            }
            println!("  References: {}", refs);
            println!("  Next: {}", "hoist push --indexers".bold());
            Ok(())
        }
        NewCommands::Skillset { name } => {
            validate_resource_name(&name)?;
            let value = scaffold::scaffold_skillset(&name);
            let path =
                write_search_resource(&env, &files_root, ResourceKind::Skillset, &name, &value)?;
            print_created("Skillset", &name, &path);
            println!(
                "  {}",
                "Add skills to the skills array, then push.".dimmed()
            );
            println!("  Next: {}", "hoist push --skillsets".bold());
            Ok(())
        }
        NewCommands::SynonymMap { name } => {
            validate_resource_name(&name)?;
            let value = scaffold::scaffold_synonym_map(&name);
            let path =
                write_search_resource(&env, &files_root, ResourceKind::SynonymMap, &name, &value)?;
            print_created("Synonym Map", &name, &path);
            println!("  Next: {}", "hoist push --synonymmaps".bold());
            Ok(())
        }
        NewCommands::Alias { name, index } => {
            validate_resource_name(&name)?;
            let value = scaffold::scaffold_alias(&name, &index);
            let path =
                write_search_resource(&env, &files_root, ResourceKind::Alias, &name, &value)?;
            print_created("Alias", &name, &path);
            println!("  Points to index '{}'", index);
            println!("  Next: {}", "hoist push --aliases".bold());
            Ok(())
        }
        NewCommands::KnowledgeBase { name } => {
            validate_resource_name(&name)?;
            let value = scaffold::scaffold_knowledge_base(&name);
            let path = write_search_resource(
                &env,
                &files_root,
                ResourceKind::KnowledgeBase,
                &name,
                &value,
            )?;
            print_created("Knowledge Base", &name, &path);
            println!(
                "  {}",
                "Add retrieval instructions and knowledge sources, then push.".dimmed()
            );
            println!("  Next: {}", "hoist push --knowledgebases".bold());
            Ok(())
        }
        NewCommands::KnowledgeSource {
            name,
            index,
            knowledge_base,
        } => {
            validate_resource_name(&name)?;
            let value =
                scaffold::scaffold_knowledge_source(&name, &index, knowledge_base.as_deref());
            let path = write_knowledge_source(&env, &files_root, &name, &value)?;
            print_created("Knowledge Source", &name, &path);
            println!();
            println!("  After pushing, Azure will auto-provision managed sub-resources:");
            println!("    {}-index         (search index)", name);
            println!("    {}-indexer       (indexer)", name);
            println!("    {}-datasource    (data source)", name);
            println!("    {}-skillset      (skillset)", name);
            println!();
            println!("  Next: {}", "hoist push --knowledgesources".bold());
            println!(
                "  Then: {}  (to sync managed resources back)",
                "hoist pull --knowledgesources".bold()
            );
            Ok(())
        }
        NewCommands::Agent { name, model } => {
            validate_resource_name(&name)?;
            let value = scaffold::scaffold_agent(&name, &model);
            let path = write_agent(&env, &files_root, &name, &value)?;
            print_created("Agent", &name, &path);
            println!();
            println!("  To connect this agent to a knowledge base, add a tool:");
            println!("    {}", "tools:".dimmed());
            println!("      {}", "- type: mcp".dimmed());
            println!("        {}", "server_label: <knowledge-base-name>".dimmed());
            println!(
                "        {}",
                "server_url: https://<search-service>.search.windows.net/knowledgebases/<kb-name>/mcp".dimmed()
            );
            println!();
            println!("  Next: {}", "hoist push --agents".bold());
            Ok(())
        }
        NewCommands::AgenticRag {
            name,
            model,
            datasource_type,
            container,
        } => {
            validate_resource_name(&name)?;

            let search_svc = env.primary_search_service().ok_or_else(|| {
                anyhow::anyhow!("No search service configured in this environment")
            })?;
            let search_service_name = &search_svc.name;

            let rag = scaffold::scaffold_agentic_rag(
                &name,
                &model,
                search_service_name,
                &datasource_type,
                &container,
            );

            // Write knowledge base
            let kb_path = write_search_resource(
                &env,
                &files_root,
                ResourceKind::KnowledgeBase,
                &rag.knowledge_base_name,
                &rag.knowledge_base,
            )?;
            print_created("Knowledge Base", &rag.knowledge_base_name, &kb_path);

            // Write knowledge source
            let ks_path = write_knowledge_source(
                &env,
                &files_root,
                &rag.knowledge_source_name,
                &rag.knowledge_source,
            )?;
            print_created("Knowledge Source", &rag.knowledge_source_name, &ks_path);

            // Write agent
            let agent_path = write_agent(&env, &files_root, &rag.agent_name, &rag.agent)?;
            print_created("Agent", &rag.agent_name, &agent_path);

            println!();
            println!(
                "  {} All resources are pre-wired:",
                "Agentic RAG system scaffolded!".green().bold()
            );
            println!(
                "    Agent '{}' -> Knowledge Base '{}' -> Knowledge Source '{}'",
                rag.agent_name, rag.knowledge_base_name, rag.knowledge_source_name
            );
            println!();
            println!("  After pushing, Azure will auto-provision managed sub-resources for the");
            println!("  knowledge source (index, indexer, data source, skillset).");
            println!();
            println!("  Next: {}", "hoist push --all".bold());
            println!(
                "  Then: {}  (to sync managed resources back)",
                "hoist pull --all".bold()
            );
            Ok(())
        }
    }
}

/// Write a search resource JSON file to the appropriate directory.
/// Returns the path of the created file.
fn write_search_resource(
    env: &hoist_core::config::ResolvedEnvironment,
    files_root: &std::path::Path,
    kind: ResourceKind,
    name: &str,
    value: &serde_json::Value,
) -> Result<std::path::PathBuf> {
    assert_eq!(kind.domain(), ServiceDomain::Search);

    let search_svc = env
        .primary_search_service()
        .ok_or_else(|| anyhow::anyhow!("No search service configured in this environment"))?;
    let service_dir = env.search_service_dir(files_root, search_svc);
    let resource_dir = service_dir.join(kind.directory_name());
    std::fs::create_dir_all(&resource_dir)?;

    let file_path = resource_dir.join(format!("{}.json", name));
    if file_path.exists() {
        bail!(
            "{} '{}' already exists at {}",
            kind.display_name(),
            name,
            file_path.display()
        );
    }

    std::fs::write(&file_path, format_json(value))?;
    Ok(file_path)
}

/// Write a knowledge source to its subdirectory (matching managed resources layout).
/// Returns the path of the created file.
fn write_knowledge_source(
    env: &hoist_core::config::ResolvedEnvironment,
    files_root: &std::path::Path,
    name: &str,
    value: &serde_json::Value,
) -> Result<std::path::PathBuf> {
    let search_svc = env
        .primary_search_service()
        .ok_or_else(|| anyhow::anyhow!("No search service configured in this environment"))?;
    let service_dir = env.search_service_dir(files_root, search_svc);
    let ks_dir = service_dir
        .join(ResourceKind::KnowledgeSource.directory_name())
        .join(name);

    if ks_dir.exists() {
        bail!(
            "Knowledge Source '{}' already exists at {}",
            name,
            ks_dir.display()
        );
    }

    std::fs::create_dir_all(&ks_dir)?;
    let file_path = ks_dir.join(format!("{}.json", name));
    std::fs::write(&file_path, format_json(value))?;
    Ok(file_path)
}

/// Write a Foundry agent YAML file.
/// Returns the path of the created file.
fn write_agent(
    env: &hoist_core::config::ResolvedEnvironment,
    files_root: &std::path::Path,
    name: &str,
    value: &serde_json::Value,
) -> Result<std::path::PathBuf> {
    let foundry_svc = env
        .foundry
        .first()
        .ok_or_else(|| anyhow::anyhow!("No Foundry service configured in this environment"))?;
    let service_dir = env.foundry_service_dir(files_root, foundry_svc);
    let agents_dir = service_dir.join("agents");
    std::fs::create_dir_all(&agents_dir)?;

    let file_path = agents_dir.join(format!("{}.yaml", name));
    if file_path.exists() {
        bail!("Agent '{}' already exists at {}", name, file_path.display());
    }

    let yaml = agent_to_yaml(value);
    std::fs::write(&file_path, yaml)?;
    Ok(file_path)
}

fn print_created(kind_name: &str, name: &str, path: &std::path::Path) {
    println!("  {} {} '{}'", "+".green(), kind_name, name);
    println!("    {}", path.display());
}
