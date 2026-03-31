//! Human-readable text output for project description

use super::{
    AgentSummary, IndexSummary, KnowledgeBaseSummary, KnowledgeSourceSummary, ProjectSummary,
};

/// Print multi-line text with a consistent indent prefix on each line.
/// Blank lines are preserved but printed as just the prefix.
fn print_indented(text: &str, prefix: &str) {
    for line in text.lines() {
        if line.is_empty() {
            println!("{}", prefix.trim_end());
        } else {
            println!("{}{}", prefix, line);
        }
    }
}

pub(super) fn print_text(summary: &ProjectSummary) {
    println!("{}", summary.project_name);
    println!("{}", "=".repeat(summary.project_name.len()));
    println!();

    if !summary.search_services.is_empty() || !summary.foundry_services.is_empty() {
        println!("Services:");
        for svc in &summary.search_services {
            println!("  Azure AI Search: {}", svc);
        }
        for svc in &summary.foundry_services {
            println!("  Microsoft Foundry: {}", svc);
        }
        println!();
    }

    // -- Foundry Agents -------------------------------------------------------
    if !summary.agents.is_empty() {
        println!("Foundry Agents ({}):", summary.agents.len());
        println!();
        for agent in &summary.agents {
            let model_part = if agent.model.is_empty() {
                String::new()
            } else {
                format!(" ({})", agent.model)
            };
            println!("  {}{}", agent.name, model_part);

            // Tools summary
            if !agent.tools.is_empty() {
                let tool_labels: Vec<String> = agent
                    .tools
                    .iter()
                    .map(|t| match &t.knowledge_base_name {
                        Some(kb) => format!("{} -> {}", t.tool_type, kb),
                        None => t.tool_type.clone(),
                    })
                    .collect();
                println!("    Tools: {}", tool_labels.join(", "));
            }

            if !agent.instructions.is_empty() {
                let preview = agent.instructions.lines().next().unwrap_or("");
                println!("    Instructions: {}", preview);
            }
            println!();
        }

        // -- Agentic RAG Flows ------------------------------------------------
        print_rag_flows(summary);
    }

    // -- Search Resources -----------------------------------------------------
    print_search_resources(summary);

    // Dependencies
    if !summary.dependencies.is_empty() {
        println!("Dependencies:");
        // Group dependencies by source
        let mut grouped: std::collections::BTreeMap<&str, Vec<(&str, &str)>> =
            std::collections::BTreeMap::new();
        for dep in &summary.dependencies {
            grouped
                .entry(&dep.from)
                .or_default()
                .push((&dep.to, &dep.kind));
        }
        for (from, targets) in &grouped {
            let target_strs: Vec<String> = targets
                .iter()
                .map(|(to, kind)| format!("{} ({})", to, kind))
                .collect();
            println!("  {} -> {}", from, target_strs.join(", "));
        }
        println!();
    }
}

/// Render Agentic RAG flow trees (Agent -> KB -> KS -> Index)
fn print_rag_flows(summary: &ProjectSummary) {
    // Build lookup maps for cross-referencing
    let kb_map: std::collections::HashMap<&str, &KnowledgeBaseSummary> = summary
        .knowledge_bases
        .iter()
        .map(|kb| (kb.name.as_str(), kb))
        .collect();
    let ks_map: std::collections::HashMap<&str, &KnowledgeSourceSummary> = summary
        .knowledge_sources
        .iter()
        .map(|ks| (ks.name.as_str(), ks))
        .collect();
    let idx_map: std::collections::HashMap<&str, &IndexSummary> = summary
        .indexes
        .iter()
        .map(|idx| (idx.name.as_str(), idx))
        .collect();

    // Collect agents that have KB connections
    let agents_with_kbs: Vec<&AgentSummary> = summary
        .agents
        .iter()
        .filter(|a| a.tools.iter().any(|t| t.knowledge_base_name.is_some()))
        .collect();

    if agents_with_kbs.is_empty() {
        return;
    }

    println!("Agentic RAG Flows:");
    println!();

    // Track which KBs we've already fully described
    let mut described_kbs: std::collections::HashSet<&str> = std::collections::HashSet::new();

    for agent in &agents_with_kbs {
        let kb_names: Vec<&str> = agent
            .tools
            .iter()
            .filter_map(|t| t.knowledge_base_name.as_deref())
            .collect();

        println!("  {}", agent.name);

        for (i, kb_name) in kb_names.iter().enumerate() {
            let is_last_kb = i == kb_names.len() - 1;
            let branch = if is_last_kb { "└─" } else { "├─" };
            let cont = if is_last_kb { "   " } else { "│  " };

            if described_kbs.contains(kb_name) {
                println!("  {} Knowledge Base: {} (described above)", branch, kb_name);
                continue;
            }
            described_kbs.insert(kb_name);

            if let Some(kb) = kb_map.get(kb_name) {
                println!("  {} Knowledge Base: {}", branch, kb.name);
                let kb_indent = format!("  {}   ", cont);
                if let Some(ref desc) = kb.description {
                    println!("  {}   Description:", cont);
                    print_indented(desc, &kb_indent);
                }
                if let Some(ref mode) = kb.output_mode {
                    println!("  {}   Output: {}", cont, mode);
                }
                if let Some(ref retrieval) = kb.retrieval_instructions {
                    println!("  {}   Retrieval instructions:", cont);
                    print_indented(retrieval, &kb_indent);
                }

                // Knowledge sources under this KB
                for (j, ks_name) in kb.knowledge_sources.iter().enumerate() {
                    let is_last_ks = j == kb.knowledge_sources.len() - 1;
                    let ks_branch = if is_last_ks { "└─" } else { "├─" };
                    let ks_cont = if is_last_ks { "   " } else { "│  " };

                    if let Some(ks) = ks_map.get(ks_name.as_str()) {
                        let kind_part = ks
                            .kind
                            .as_ref()
                            .map(|k| format!(" ({})", k))
                            .unwrap_or_default();
                        println!(
                            "  {}   {} Knowledge Source: {}{}",
                            cont, ks_branch, ks.name, kind_part
                        );
                        if let Some(ref desc) = ks.description {
                            let ks_indent = format!("  {}   {}   ", cont, ks_cont);
                            print_indented(desc, &ks_indent);
                        }

                        // Index under this knowledge source
                        if let Some(ref idx_name) = ks.index_name {
                            if let Some(idx) = idx_map.get(idx_name.as_str()) {
                                let key_field = idx.fields.iter().find(|f| f.is_key);
                                let key_info = key_field
                                    .map(|f| format!(", key: {}", f.name))
                                    .unwrap_or_default();
                                println!(
                                    "  {}   {}   └─ Index: {} ({} fields{})",
                                    cont,
                                    ks_cont,
                                    idx.name,
                                    idx.fields.len(),
                                    key_info,
                                );
                                let mut caps = Vec::new();
                                if idx.vector_profile_count > 0 {
                                    caps.push(format!(
                                        "{} vector profile(s)",
                                        idx.vector_profile_count
                                    ));
                                }
                                if idx.has_semantic_config {
                                    caps.push("semantic search".to_string());
                                }
                                if !caps.is_empty() {
                                    println!("  {}   {}      {}", cont, ks_cont, caps.join(", "));
                                }
                            } else {
                                println!("  {}   {}   └─ Index: {}", cont, ks_cont, idx_name);
                            }
                        }
                    } else {
                        println!("  {}   {} Knowledge Source: {}", cont, ks_branch, ks_name);
                    }
                }
            } else {
                // KB not found locally (might be in a different service)
                println!(
                    "  {} Knowledge Base: {} (not in local config)",
                    branch, kb_name
                );
            }
        }
        println!();
    }
}

/// Render the flat listing of search resources (Indexes, Data Sources, etc.)
fn print_search_resources(summary: &ProjectSummary) {
    // Indexes
    if !summary.indexes.is_empty() {
        println!("Indexes ({}):", summary.indexes.len());
        for idx in &summary.indexes {
            let key_field = idx.fields.iter().find(|f| f.is_key);
            let key_info = key_field
                .map(|f| format!(", key: {}", f.name))
                .unwrap_or_default();
            println!("  {} ({} fields{})", idx.name, idx.fields.len(), key_info);

            // Field listing
            let field_strs: Vec<String> = idx
                .fields
                .iter()
                .map(|f| {
                    let mut s = format!("{} ({}", f.name, f.field_type);
                    if f.is_key {
                        s.push_str(", key");
                    }
                    if let Some(ref a) = f.analyzer {
                        s.push_str(&format!(", analyzer: {}", a));
                    }
                    s.push(')');
                    s
                })
                .collect();
            // Show up to 5 fields inline, then ellipsis
            if field_strs.len() <= 5 {
                println!("    Fields: {}", field_strs.join(", "));
            } else {
                let shown: Vec<&str> = field_strs.iter().take(5).map(|s| s.as_str()).collect();
                println!("    Fields: {}, ...", shown.join(", "));
            }

            if idx.vector_profile_count > 0 {
                println!("    Vector search: {} profile(s)", idx.vector_profile_count);
            }
            if idx.has_semantic_config {
                println!("    Semantic: default config");
            }
        }
        println!();
    }

    // Data Sources
    if !summary.data_sources.is_empty() {
        println!("Data Sources ({}):", summary.data_sources.len());
        for ds in &summary.data_sources {
            if ds.container.is_empty() {
                println!("  {} ({})", ds.name, ds.source_type);
            } else {
                println!("  {} ({} -> {})", ds.name, ds.source_type, ds.container);
            }
        }
        println!();
    }

    // Indexers
    if !summary.indexers.is_empty() {
        println!("Indexers ({}):", summary.indexers.len());
        for idxr in &summary.indexers {
            println!("  {}", idxr.name);
            let skillset_part = idxr
                .skillset
                .as_ref()
                .map(|s| format!(" | Skillset: {}", s))
                .unwrap_or_default();
            println!(
                "    Index: {} | Data Source: {}{}",
                idxr.target_index, idxr.data_source, skillset_part
            );
        }
        println!();
    }

    // Skillsets
    if !summary.skillsets.is_empty() {
        println!("Skillsets ({}):", summary.skillsets.len());
        for ss in &summary.skillsets {
            println!("  {} ({} skills)", ss.name, ss.skills.len());
            for skill in &ss.skills {
                match &skill.name {
                    Some(n) => println!("    - {} ({})", skill.odata_type, n),
                    None => println!("    - {}", skill.odata_type),
                }
            }
        }
        println!();
    }

    // Synonym Maps
    if !summary.synonym_maps.is_empty() {
        println!("Synonym Maps ({}):", summary.synonym_maps.len());
        for sm in &summary.synonym_maps {
            println!("  {} ({} format)", sm.name, sm.format);
        }
        println!();
    }

    // Aliases
    if !summary.aliases.is_empty() {
        println!("Aliases ({}):", summary.aliases.len());
        for alias in &summary.aliases {
            println!("  {} -> {}", alias.name, alias.indexes.join(", "));
        }
        println!();
    }

    // Knowledge Bases
    if !summary.knowledge_bases.is_empty() {
        println!("Knowledge Bases ({}):", summary.knowledge_bases.len());
        for kb in &summary.knowledge_bases {
            let sources_part = if kb.knowledge_sources.is_empty() {
                String::new()
            } else {
                format!(" -> {}", kb.knowledge_sources.join(", "))
            };
            println!("  {}{}", kb.name, sources_part);
            if let Some(ref desc) = kb.description {
                print_indented(desc, "    ");
            }
        }
        println!();
    }

    // Knowledge Sources
    if !summary.knowledge_sources.is_empty() {
        println!("Knowledge Sources ({}):", summary.knowledge_sources.len());
        for ks in &summary.knowledge_sources {
            let kind_part = ks
                .kind
                .as_ref()
                .map(|k| format!(" ({})", k))
                .unwrap_or_default();
            let mut targets = Vec::new();
            if let Some(ref idx) = ks.index_name {
                targets.push(format!("Index: {}", idx));
            }
            if let Some(ref kb) = ks.knowledge_base {
                targets.push(format!("KB: {}", kb));
            }
            if targets.is_empty() {
                println!("  {}{}", ks.name, kind_part);
            } else {
                println!("  {}{} -> {}", ks.name, kind_part, targets.join(", "));
            }
            if let Some(ref desc) = ks.description {
                print_indented(desc, "    ");
            }
        }
        println!();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_print_indented_captures_output() {
        // print_indented writes to stdout; we just verify it doesn't panic
        // and test it indirectly via the describe output
        print_indented("single line", "  ");
        print_indented("line one\nline two\n\nline four", "    ");
    }
}
