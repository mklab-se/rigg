//! `rigg migrate knowledge-source` — convert an indexed knowledge source
//! (azureBlob, azureSql, ...) into the explicit `searchIndex` shape,
//! materializing its Azure-generated pipeline as first-class project files.
//!
//! Local-only: this command never mutates Azure. The next `rigg push`
//! applies the change — for an in-place migration that is a REPLACE
//! (delete + recreate, index rebuild) gated behind --allow-replace.

use std::collections::BTreeMap;

use anyhow::{Result, anyhow};
use colored::Colorize;
use serde_json::Value;

use rigg_core::migrate as core_migrate;
use rigg_core::normalize::normalize_for_disk;
use rigg_core::resources::{ResourceKind, ResourceRef, validate_resource_name};
use rigg_core::store::{ProjectState, Store, assert_exclusive_ownership};

use crate::cli::{MigrateCommands, MigrateKsArgs};
use crate::commands::credentials;
use crate::commands::remote::{Remote, ensure_any_connection};
use crate::commands::{
    CommandError, GlobalContext, interactive, load_workspace, resolve_env, select_projects,
};

pub async fn run(ctx: &GlobalContext, command: MigrateCommands) -> Result<()> {
    match command {
        MigrateCommands::KnowledgeSource(args) => knowledge_source(ctx, args).await,
    }
}

enum Mode {
    InPlace,
    SideBySide(String),
}

async fn knowledge_source(ctx: &GlobalContext, args: MigrateKsArgs) -> Result<()> {
    let ws = load_workspace()?;
    let env = resolve_env(&ws, ctx)?;
    assert_exclusive_ownership(&ws, &env.name)?;
    let project = select_projects(&ws, args.project.as_deref(), false)?[0];
    let store = Store::new(project, &env.name);
    let mut state = ProjectState::load(&ws, &env.name, &project.name);
    let remote = Remote::for_project(&env, project);
    ensure_any_connection(&remote, project)?;

    let ks_ref = ResourceRef::new(ResourceKind::KnowledgeSource, args.name.clone());
    let Some(remote_ks) = remote.get(&ks_ref).await? else {
        return Err(anyhow!(
            "knowledge source '{}' not found on the remote service (env: {})",
            args.name,
            env.name
        ));
    };
    let kind = remote_ks.get("kind").and_then(Value::as_str).unwrap_or("");
    if kind == "searchIndex" {
        println!(
            "Knowledge source '{}' is already kind searchIndex — nothing to migrate.",
            args.name
        );
        return Ok(());
    }
    if !core_migrate::is_indexed_with_created(&remote_ks) {
        return Err(anyhow!(
            "knowledge source '{}' (kind: {kind}) has no Azure-generated pipeline to migrate \
             (remote knowledge sources connect to external content — there is no index to take over)",
            args.name
        ));
    }
    if store.locate(&ks_ref)?.is_none() && !state.has_baseline(&ks_ref) {
        return Err(anyhow!(
            "knowledge source '{}' is not managed by project '{}' — adopt it first: \
             rigg adopt {} knowledge-sources/{}",
            args.name,
            project.name,
            project.name,
            args.name
        ));
    }

    let created = core_migrate::created_resources(&remote_ks);
    let Some(index_name) = created.get(&ResourceKind::Index).cloned() else {
        return Err(anyhow!(
            "knowledge source '{}' names no generated index in createdResources — cannot migrate",
            args.name
        ));
    };

    // Fetch the generated sub-resource definitions (skip any deleted manually).
    let mut sub_docs: BTreeMap<ResourceKind, Value> = BTreeMap::new();
    for (kind, name) in &created {
        let r = ResourceRef::new(*kind, name.clone());
        match remote.get(&r).await? {
            Some(doc) => {
                sub_docs.insert(*kind, doc);
            }
            None => println!(
                "  {} generated {} '{}' no longer exists remotely — skipped",
                "!".yellow(),
                kind.directory_name(),
                name
            ),
        }
    }

    let mode = resolve_mode(ctx, &args)?;

    match mode {
        Mode::InPlace => {
            in_place(
                ctx,
                &store,
                &mut state,
                &ks_ref,
                &remote_ks,
                &index_name,
                &created,
                &sub_docs,
            )
            .await?;
            state.save(&ws, &env.name, &project.name)?;
        }
        Mode::SideBySide(new_name) => {
            side_by_side(
                ctx, &store, &remote, &ks_ref, &remote_ks, &created, &sub_docs, &new_name,
            )
            .await?;
        }
    }
    Ok(())
}

fn resolve_mode(ctx: &GlobalContext, args: &MigrateKsArgs) -> Result<Mode> {
    if args.in_place {
        return Ok(Mode::InPlace);
    }
    if let Some(new_name) = &args.rename {
        validate_resource_name(new_name)
            .map_err(|e| anyhow!(CommandError::Usage(format!("invalid --rename name: {e}"))))?;
        if new_name == &args.name {
            return Err(anyhow!(CommandError::Usage(
                "--rename must differ from the current name (use --in-place to keep names)"
                    .to_string()
            )));
        }
        return Ok(Mode::SideBySide(new_name.clone()));
    }
    if !ctx.interactive() {
        return Err(anyhow!(CommandError::Usage(
            "pass --in-place or --rename <new-name> (non-interactive run)".to_string()
        )));
    }
    const IN_PLACE: &str =
        "in-place — same names; next push REPLACES the knowledge source and REBUILDS the index";
    const SIDE: &str =
        "side-by-side — new names; old knowledge source keeps serving until you cut over";
    let choice = interactive::select(
        "Migration mode:",
        vec![IN_PLACE.to_string(), SIDE.to_string()],
        ctx.no_color,
    )?;
    if choice == IN_PLACE {
        Ok(Mode::InPlace)
    } else {
        let new_name = interactive::text("New knowledge source name:", ctx.no_color)?;
        validate_resource_name(&new_name).map_err(|e| anyhow!("invalid name: {e}"))?;
        Ok(Mode::SideBySide(new_name))
    }
}

#[allow(clippy::too_many_arguments)]
async fn in_place(
    ctx: &GlobalContext,
    store: &Store<'_>,
    state: &mut ProjectState,
    ks_ref: &ResourceRef,
    remote_ks: &Value,
    index_name: &str,
    created: &BTreeMap<ResourceKind, String>,
    sub_docs: &BTreeMap<ResourceKind, Value>,
) -> Result<()> {
    println!(
        "{} '{}' in place (kind: {} → searchIndex)",
        "Migrate".bold(),
        ks_ref.name,
        remote_ks.get("kind").and_then(Value::as_str).unwrap_or("?")
    );
    // Materialize the generated definitions as explicit project files. They
    // exist remotely with exactly this content, so baselines are seeded too
    // (like adopt) — push replaces them as part of the knowledge-source
    // replace bundle.
    for (kind, doc) in sub_docs {
        let r = ResourceRef::new(*kind, created[kind].clone());
        let disk = normalize_for_disk(*kind, doc);
        store.write(&r, &disk)?;
        state.set_baseline(&r, &disk);
        println!("  {} {}", "wrote".cyan(), r);
    }
    // Rewrite the knowledge source itself. Its baseline is deliberately left
    // at the remote (azureBlob) shape so status/push see the kind change.
    let new_ks = core_migrate::to_search_index_ks(remote_ks, index_name);
    store.write(ks_ref, &new_ks)?;
    println!("  {} {} (kind: searchIndex)", "wrote".cyan(), ks_ref);

    check_datasource_credentials(ctx, store, created).await?;

    println!();
    println!(
        "{} the next `rigg push` will {} this knowledge source: it deletes the",
        "⚠".yellow().bold(),
        "REPLACE".red().bold()
    );
    println!("  old one (Azure cascades away the generated pipeline) and recreates it");
    println!("  explicitly. The index is REBUILT from source data — this takes time,");
    println!("  costs ingestion/embeddings, and the source is unavailable to knowledge");
    println!("  bases until the indexer repopulates it. Push requires --allow-replace");
    println!("  non-interactively.");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn side_by_side(
    ctx: &GlobalContext,
    store: &Store<'_>,
    remote: &Remote,
    old_ks_ref: &ResourceRef,
    remote_ks: &Value,
    created: &BTreeMap<ResourceKind, String>,
    sub_docs: &BTreeMap<ResourceKind, Value>,
    new_name: &str,
) -> Result<()> {
    println!(
        "{} '{}' side-by-side as '{}'",
        "Migrate".bold(),
        old_ks_ref.name,
        new_name
    );
    let mut names = core_migrate::derive_names(&old_ks_ref.name, new_name, created);
    // Interactive: let the user adjust each derived sub-resource name.
    if ctx.interactive() {
        for (kind, name) in names.clone() {
            let edited = interactive::text_with_default(
                &format!("Name for the new {}:", kind.directory_name()),
                &name,
                ctx.no_color,
            )?;
            validate_resource_name(&edited).map_err(|e| anyhow!("invalid name: {e}"))?;
            names.insert(kind, edited);
        }
    }

    // Collision checks: the new names must be unused locally and remotely.
    let mut new_refs: Vec<ResourceRef> = vec![ResourceRef::new(
        ResourceKind::KnowledgeSource,
        new_name.to_string(),
    )];
    for (kind, name) in &names {
        if sub_docs.contains_key(kind) {
            new_refs.push(ResourceRef::new(*kind, name.clone()));
        }
    }
    for r in &new_refs {
        if store.locate(r)?.is_some() {
            return Err(anyhow!(
                "{} already exists in this project — pick another name",
                r
            ));
        }
        if remote.get(r).await?.is_some() {
            return Err(anyhow!("{} already exists remotely — pick another name", r));
        }
    }

    // Write the new pipeline: renamed docs with the indexer rewired.
    for (kind, doc) in sub_docs {
        let name = names[kind].clone();
        let mut disk = normalize_for_disk(*kind, doc);
        disk["name"] = Value::String(name.clone());
        if *kind == ResourceKind::Indexer {
            for (field, to_kind) in [
                ("dataSourceName", ResourceKind::DataSource),
                ("targetIndexName", ResourceKind::Index),
                ("skillsetName", ResourceKind::Skillset),
            ] {
                if disk.get(field).and_then(Value::as_str).is_some() {
                    if let Some(new) = names.get(&to_kind) {
                        disk[field] = Value::String(new.clone());
                    }
                }
            }
        }
        let r = ResourceRef::new(*kind, name);
        store.write(&r, &disk)?;
        println!("  {} {}", "wrote".cyan(), r);
    }
    let index_name = names
        .get(&ResourceKind::Index)
        .cloned()
        .unwrap_or_else(|| format!("{new_name}-index"));
    let mut new_ks = core_migrate::to_search_index_ks(remote_ks, &index_name);
    new_ks["name"] = Value::String(new_name.to_string());
    let new_ks_ref = ResourceRef::new(ResourceKind::KnowledgeSource, new_name.to_string());
    store.write(&new_ks_ref, &new_ks)?;
    println!("  {} {} (kind: searchIndex)", "wrote".cyan(), new_ks_ref);

    check_datasource_credentials_named(ctx, store, names.get(&ResourceKind::DataSource)).await?;

    println!();
    println!("Next steps:");
    println!("  1. rigg push               — builds the new pipeline (fresh index ingestion)");
    println!("  2. verify retrieval quality against '{new_name}'");
    println!(
        "  3. point your knowledge base(s) at '{new_name}' instead of '{}'",
        old_ks_ref.name
    );
    println!(
        "  4. delete {} and its files, then `rigg push --prune` to remove the old pipeline",
        old_ks_ref
    );
    Ok(())
}

/// The generated data source's credentials never leave Azure (write-only) —
/// the copied file has none. Discover the storage account by container via
/// ARM and set an identity-based connection; otherwise warn.
async fn check_datasource_credentials(
    ctx: &GlobalContext,
    store: &Store<'_>,
    created: &BTreeMap<ResourceKind, String>,
) -> Result<()> {
    check_datasource_credentials_named(ctx, store, created.get(&ResourceKind::DataSource)).await
}

async fn check_datasource_credentials_named(
    ctx: &GlobalContext,
    store: &Store<'_>,
    ds_name: Option<&String>,
) -> Result<()> {
    let Some(ds_name) = ds_name else {
        return Ok(());
    };
    let r = ResourceRef::new(ResourceKind::DataSource, ds_name.clone());
    if store.locate(&r)?.is_none() {
        return Ok(());
    }
    let mut doc = store.read(&r)?;
    let conn = doc
        .pointer("/credentials/connectionString")
        .and_then(Value::as_str)
        .unwrap_or("");
    if conn.starts_with("ResourceId=") {
        return Ok(());
    }
    if ctx.interactive() {
        println!();
        println!("The data source's credentials are not stored in Azure's GET responses, so the");
        println!("copied file has none. Identity-based access is recommended (no keys on disk).");
        let container = credentials::container_name(&doc).map(str::to_string);
        if let Some(conn) = credentials::discover_connection_interactive(
            &r.to_string(),
            container.as_deref(),
            ctx.no_color,
        )
        .await?
        {
            credentials::set_connection(&mut doc, &conn);
            store.write(&r, &doc)?;
            println!(
                "  {} {} connection set (identity-based, no key on disk)",
                "✓".green(),
                r
            );
            if let Some(account) = conn.strip_prefix("ResourceId=") {
                credentials::print_rbac_hint(account.trim_end_matches(';'));
            }
            return Ok(());
        }
    }
    println!(
        "  {} {} has no usable credentials — set credentials.connectionString \
         (identity-based `ResourceId=...`) before pushing, or run `rigg push` \
         interactively to auto-discover",
        "!".yellow(),
        r
    );
    Ok(())
}
