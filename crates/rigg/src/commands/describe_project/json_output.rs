//! JSON output for project description

use serde_json::{Value, json};

use super::ProjectSummary;

pub(super) fn print_json(summary: &ProjectSummary) {
    let indexes: Vec<Value> = summary
        .indexes
        .iter()
        .map(|idx| {
            let fields: Vec<Value> = idx
                .fields
                .iter()
                .map(|f| {
                    let mut field = json!({
                        "name": f.name,
                        "type": f.field_type,
                        "key": f.is_key,
                    });
                    if let Some(ref a) = f.analyzer {
                        field["analyzer"] = json!(a);
                    }
                    field
                })
                .collect();
            json!({
                "name": idx.name,
                "file_path": idx.file_path,
                "field_count": idx.fields.len(),
                "key_field": idx.fields.iter().find(|f| f.is_key).map(|f| &f.name),
                "fields": fields,
                "vector_profile_count": idx.vector_profile_count,
                "has_semantic_config": idx.has_semantic_config,
            })
        })
        .collect();

    let data_sources: Vec<Value> = summary
        .data_sources
        .iter()
        .map(|ds| {
            json!({
                "name": ds.name,
                "file_path": ds.file_path,
                "type": ds.source_type,
                "container": ds.container,
            })
        })
        .collect();

    let indexers: Vec<Value> = summary
        .indexers
        .iter()
        .map(|idxr| {
            json!({
                "name": idxr.name,
                "file_path": idxr.file_path,
                "target_index": idxr.target_index,
                "data_source": idxr.data_source,
                "skillset": idxr.skillset,
            })
        })
        .collect();

    let skillsets: Vec<Value> = summary
        .skillsets
        .iter()
        .map(|ss| {
            let skills: Vec<Value> = ss
                .skills
                .iter()
                .map(|s| {
                    json!({
                        "type": s.odata_type,
                        "name": s.name,
                    })
                })
                .collect();
            json!({
                "name": ss.name,
                "file_path": ss.file_path,
                "skill_count": ss.skills.len(),
                "skills": skills,
            })
        })
        .collect();

    let synonym_maps: Vec<Value> = summary
        .synonym_maps
        .iter()
        .map(|sm| {
            json!({
                "name": sm.name,
                "file_path": sm.file_path,
                "format": sm.format,
            })
        })
        .collect();

    let aliases: Vec<Value> = summary
        .aliases
        .iter()
        .map(|a| {
            json!({
                "name": a.name,
                "file_path": a.file_path,
                "indexes": a.indexes,
            })
        })
        .collect();

    let knowledge_bases: Vec<Value> = summary
        .knowledge_bases
        .iter()
        .map(|kb| {
            json!({
                "name": kb.name,
                "file_path": kb.file_path,
                "description": kb.description,
                "retrieval_instructions": kb.retrieval_instructions,
                "output_mode": kb.output_mode,
                "knowledge_sources": kb.knowledge_sources,
            })
        })
        .collect();

    let knowledge_sources: Vec<Value> = summary
        .knowledge_sources
        .iter()
        .map(|ks| {
            json!({
                "name": ks.name,
                "file_path": ks.file_path,
                "description": ks.description,
                "kind": ks.kind,
                "index_name": ks.index_name,
                "knowledge_base": ks.knowledge_base,
            })
        })
        .collect();

    let agents: Vec<Value> = summary
        .agents
        .iter()
        .map(|a| {
            let tools: Vec<Value> = a
                .tools
                .iter()
                .map(|t| {
                    json!({
                        "type": t.tool_type,
                        "knowledge_base": t.knowledge_base_name,
                    })
                })
                .collect();
            json!({
                "name": a.name,
                "file_path": a.file_path,
                "model": a.model,
                "tool_count": a.tool_count,
                "tools": tools,
                "instructions": a.instructions,
            })
        })
        .collect();

    let deps: Vec<Value> = summary
        .dependencies
        .iter()
        .map(|d| {
            json!({
                "from": d.from,
                "to": d.to,
                "kind": d.kind,
            })
        })
        .collect();

    let output = json!({
        "project_name": summary.project_name,
        "search_services": summary.search_services,
        "foundry_services": summary.foundry_services,
        "indexes": indexes,
        "data_sources": data_sources,
        "indexers": indexers,
        "skillsets": skillsets,
        "synonym_maps": synonym_maps,
        "aliases": aliases,
        "knowledge_bases": knowledge_bases,
        "knowledge_sources": knowledge_sources,
        "agents": agents,
        "dependencies": deps,
    });

    println!("{}", serde_json::to_string_pretty(&output).unwrap());
}
