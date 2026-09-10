//! Environment management commands.

use std::collections::BTreeMap;

use anyhow::{Result, anyhow, bail};
use colored::Colorize;
use serde_json::json;
use serde_yaml::Value as Yaml;

use rigg_client::arm::ArmClient;
use rigg_core::binding::{Binding, BindingCache, EnvBindings, TargetKind, validate_binding_name};
use rigg_core::workspace::{Environment, Workspace};

use crate::cli::EnvCommands;
use crate::commands::bindings::{edit_workspace_yaml, envs_mut, other_env_bindings, shared_with};
use crate::commands::{CommandError, GlobalContext, bindings, discovery, load_workspace};
use crate::say;

pub async fn run(ctx: &GlobalContext, cmd: EnvCommands) -> Result<()> {
    match cmd {
        EnvCommands::List => list(ctx),
        EnvCommands::Show { name, refresh } => show(ctx, name.as_deref(), refresh).await,
        EnvCommands::SetDefault { name } => set_default(&name),
        EnvCommands::Add {
            name,
            tenant,
            subscription,
            search_service,
            foundry_account,
            foundry_project,
            protected,
            bind,
            like,
            same,
            skip,
        } => {
            add(
                ctx,
                AddOptions {
                    name,
                    tenant,
                    subscription,
                    search_service,
                    foundry_account,
                    foundry_project,
                    protected,
                    bind,
                    like,
                    same,
                    skip,
                },
            )
            .await
        }
        EnvCommands::Remove { name } => remove(&name),
        EnvCommands::Bind {
            env,
            name,
            value,
            learn,
        } => bind(ctx, &env, name, value, learn),
        EnvCommands::Unbind { env, name } => unbind(ctx, &env, &name),
    }
}

fn list(ctx: &GlobalContext) -> Result<()> {
    let ws = load_workspace()?;
    if ctx.json() {
        let entries: Vec<serde_json::Value> = ws
            .config
            .environments
            .iter()
            .map(|(name, env)| env_json(&ws, name, env, &BTreeMap::new()))
            .collect();
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }
    for (name, env) in &ws.config.environments {
        let marker = if env.default { " (default)" } else { "" };
        println!("{}{}", name.bold(), marker.dimmed());
        print_env(&ws, name, env, "  ", &BTreeMap::new());
    }
    Ok(())
}

async fn show(ctx: &GlobalContext, name: Option<&str>, refresh: bool) -> Result<()> {
    let ws = load_workspace()?;
    let resolved = ws.resolve_env(name.or(ctx.env.as_deref()))?;
    let mut errors: BTreeMap<String, String> = BTreeMap::new();
    if refresh {
        errors = refresh_bindings(ctx, &ws, &resolved.name, &resolved.env).await?;
    }
    if ctx.json() {
        let value = env_json(&ws, &resolved.name, &resolved.env, &errors);
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    println!("{}", resolved.name.bold());
    print_env(&ws, &resolved.name, &resolved.env, "  ", &errors);
    Ok(())
}

/// Re-resolve every binding of `env` against ARM and save the cache — the
/// declared `dependencies` and the implicit `search`/`foundry` targets,
/// which are cached under those reserved names. A binding that cannot be
/// resolved is reported (and dropped from the cache) rather than failing the
/// command — `env show` must still print everything else it knows.
async fn refresh_bindings(
    ctx: &GlobalContext,
    ws: &Workspace,
    env_name: &str,
    env: &Environment,
) -> Result<BTreeMap<String, String>> {
    let mut errors = BTreeMap::new();
    // The implicit `search`/`foundry` targets are bindings too (they are
    // what most references resolve against), so they are refreshed even when
    // an environment declares no dependencies at all.
    let mut targets: Vec<(String, TargetKind, String)> = Vec::new();
    if let Some(search) = &env.search {
        targets.push((
            "search".to_string(),
            TargetKind::Search,
            search.service.clone(),
        ));
    }
    if let Some(foundry) = &env.foundry {
        targets.push((
            "foundry".to_string(),
            TargetKind::Foundry,
            foundry.account.clone(),
        ));
    }
    for (name, binding) in &env.dependencies {
        targets.push((
            name.clone(),
            TargetKind::Binding(binding.kind),
            binding.value.clone(),
        ));
    }
    if targets.is_empty() {
        return Ok(errors);
    }
    let arm = match ArmClient::for_tenant(env.tenant.as_deref()) {
        Ok(arm) => arm,
        Err(e) => {
            let e = anyhow::Error::from(e);
            say!(ctx, "  (could not reach Azure Resource Manager: {e:#})");
            return Ok(errors);
        }
    };
    let mut cache = BindingCache::load(ws, env_name);
    for (name, kind, value) in &targets {
        match arm
            .resolve_target(*kind, value, env.subscription.as_deref())
            .await
        {
            Ok(mut resolved) => {
                // ARM knows the resource's name; the binding name is ours.
                resolved.name = name.clone();
                cache.bindings.insert(name.clone(), resolved);
            }
            Err(e) => {
                cache.bindings.remove(name);
                errors.insert(name.clone(), format!("{e}"));
            }
        }
    }
    cache.save(ws, env_name)?;
    Ok(errors)
}

fn print_env(
    ws: &Workspace,
    env_name: &str,
    env: &Environment,
    indent: &str,
    errors: &BTreeMap<String, String>,
) {
    println!("{indent}protected: {}", env.policy.protected);
    if let Some(tenant) = &env.tenant {
        println!("{indent}tenant: {tenant}");
    }
    if let Some(subscription) = &env.subscription {
        println!("{indent}subscription: {subscription}");
    }
    let cache = BindingCache::load(ws, env_name);
    if let Some(s) = &env.search {
        let label = s.name.as_deref().unwrap_or("search");
        println!(
            "{indent}{label}: {} → {} (Azure AI Search){}",
            s.service,
            s.url(),
            resolution_suffix(&cache, errors, "search")
        );
    }
    if let Some(f) = &env.foundry {
        let label = f.name.as_deref().unwrap_or("foundry");
        println!(
            "{indent}{label}: {}/{} → {} (Microsoft Foundry){}",
            f.account,
            f.project,
            f.url(),
            resolution_suffix(&cache, errors, "foundry")
        );
    }
    if env.dependencies.is_empty() {
        return;
    }
    let others = other_env_bindings(ws, env_name);
    println!("{indent}dependencies:");
    for (name, binding) in &env.dependencies {
        let mut row = format!("{indent}  {name}  {}  {}", binding.kind, binding.value);
        row.push_str(&resolution_suffix(&cache, errors, name));
        let shared = shared_with(&others, binding);
        if !shared.is_empty() {
            row.push_str(&format!(" (shared with: {})", shared.join(", ")));
        }
        println!("{row}");
    }
}

/// What a binding resolved to (ARM id, endpoint, or physical name), or why it
/// could not be resolved — empty when it has never been refreshed.
fn resolution_suffix(
    cache: &BindingCache,
    errors: &BTreeMap<String, String>,
    name: &str,
) -> String {
    if let Some(err) = errors.get(name) {
        return format!("  {} {err}", "?".yellow());
    }
    match cache.get(name) {
        Some(resolved) => {
            let id = resolved
                .arm_id
                .as_deref()
                .or(resolved.endpoint.as_deref())
                .unwrap_or(&resolved.physical_name);
            format!(" → {}", id.dimmed())
        }
        None => String::new(),
    }
}

fn env_json(
    ws: &Workspace,
    name: &str,
    env: &Environment,
    errors: &BTreeMap<String, String>,
) -> serde_json::Value {
    let cache = BindingCache::load(ws, name);
    let others = other_env_bindings(ws, name);
    json!({
        "name": name,
        "default": env.default,
        "protected": env.policy.protected,
        "tenant": env.tenant,
        "subscription": env.subscription,
        "search": env.search.as_ref().map(|s| &s.service),
        "search_arm_id": cache.get("search").and_then(|r| r.arm_id.clone()),
        "foundry": env.foundry.as_ref().map(|f| format!("{}/{}", f.account, f.project)),
        "foundry_arm_id": cache.get("foundry").and_then(|r| r.arm_id.clone()),
        "dependencies": env.dependencies.iter().map(|(bname, binding)| json!({
            "name": bname,
            "type": binding.kind.to_string(),
            "value": binding.value,
            "physical_name": binding.physical_name(),
            "arm_id": cache.get(bname).and_then(|r| r.arm_id.clone()),
            "location": cache.get(bname).and_then(|r| r.location.clone()),
            "shared_with": shared_with(&others, binding),
            "error": errors.get(bname),
        })).collect::<Vec<_>>(),
    })
}

fn set_default(name: &str) -> Result<()> {
    edit_workspace_yaml(|doc| {
        let envs = envs_mut(doc)?;
        if !envs.contains_key(name) {
            bail!("unknown environment '{name}'");
        }
        let keys: Vec<Yaml> = envs.keys().cloned().collect();
        for key in keys {
            let is_target = key.as_str() == Some(name);
            if let Some(env) = envs.get_mut(&key).and_then(|e| e.as_mapping_mut()) {
                if is_target {
                    env.insert("default".into(), Yaml::Bool(true));
                } else {
                    env.remove("default");
                }
            }
        }
        Ok(())
    })?;
    println!("Default environment set to '{name}'.");
    Ok(())
}

/// Declare (or replace) one binding, or learn a whole set from the files.
fn bind(
    ctx: &GlobalContext,
    env_name: &str,
    name: Option<String>,
    value: Option<String>,
    learn: bool,
) -> Result<()> {
    if learn {
        if name.is_some() || value.is_some() {
            return Err(anyhow!(CommandError::Usage(
                "`--learn` proposes bindings from the files; don't also pass a name and value"
                    .to_string()
            )));
        }
        return learn_bindings(ctx, env_name);
    }
    let (Some(name), Some(spec)) = (name, value) else {
        return Err(anyhow!(CommandError::Usage(
            "usage: rigg env bind <env> <name> <type>:<value> (or `rigg env bind <env> --learn`)"
                .to_string()
        )));
    };
    validate_binding_name(&name).map_err(|e| anyhow!(CommandError::Usage(e)))?;
    let binding = bindings::parse_binding(&spec)?;
    bindings::write_binding(env_name, &name, &binding)?;
    say!(
        ctx,
        "Bound '{name}' in environment '{env_name}': {} {}",
        binding.kind,
        binding.value
    );
    Ok(())
}

fn unbind(ctx: &GlobalContext, env_name: &str, name: &str) -> Result<()> {
    bindings::remove_binding(env_name, name)?;
    say!(
        ctx,
        "Removed binding '{name}' from environment '{env_name}'."
    );
    Ok(())
}

fn learn_bindings(ctx: &GlobalContext, env_name: &str) -> Result<()> {
    let ws = load_workspace()?;
    let env = ws.config.environments.get(env_name).ok_or_else(|| {
        anyhow!(CommandError::Usage(format!(
            "unknown environment '{env_name}'"
        )))
    })?;
    let cache = BindingCache::load(&ws, env_name);
    let table = EnvBindings::of_env(env_name, env, Some(&cache));
    let proposals = bindings::learn(&ws, env_name, &table)?;
    if proposals.is_empty() {
        say!(
            ctx,
            "No unbound infrastructure references found in '{env_name}'."
        );
        return Ok(());
    }
    say!(
        ctx,
        "Found {} unbound infrastructure reference(s) in '{env_name}':",
        proposals.len()
    );
    for p in &proposals {
        say!(
            ctx,
            "  {}  {}  {}  ({} reference(s), e.g. {})",
            p.name,
            p.kind,
            p.value,
            p.sources.len(),
            p.sources
                .first()
                .map(|(file, path)| format!("{file}:{path}"))
                .unwrap_or_default()
        );
    }

    let to_write = if ctx.yes {
        proposals
            .iter()
            .map(|p| {
                (
                    p.name.clone(),
                    Binding {
                        kind: p.kind,
                        value: p.value.clone(),
                    },
                )
            })
            .collect()
    } else {
        let mut asker = ctx.asker("env bind --learn", json!({"env": env_name}));
        bindings::resolve_proposals(asker.as_mut(), env_name, &proposals)?
    };

    if to_write.is_empty() {
        say!(ctx, "Nothing bound.");
        return Ok(());
    }
    bindings::write_bindings(env_name, &to_write)?;
    for (name, binding) in &to_write {
        say!(
            ctx,
            "Bound '{name}' in environment '{env_name}': {} {}",
            binding.kind,
            binding.value
        );
    }
    Ok(())
}

struct AddOptions {
    name: String,
    tenant: Option<String>,
    subscription: Option<String>,
    search_service: Option<String>,
    foundry_account: Option<String>,
    foundry_project: Option<String>,
    protected: bool,
    bind: Vec<String>,
    like: Option<String>,
    same: Vec<String>,
    skip: Vec<String>,
}

async fn add(ctx: &GlobalContext, opts: AddOptions) -> Result<()> {
    let AddOptions {
        name,
        tenant,
        subscription,
        search_service,
        foundry_account,
        foundry_project,
        protected,
        bind,
        like,
        same,
        skip,
    } = opts;

    if foundry_account.is_some() != foundry_project.is_some() {
        bail!("--foundry-account and --foundry-project must be given together");
    }
    let overrides: Vec<(String, Binding)> = bind
        .iter()
        .map(|flag| bindings::parse_bind_flag(flag))
        .collect::<Result<_>>()?;

    let ws = load_workspace()?;
    if ws.config.environments.contains_key(&name) {
        return Err(anyhow!(CommandError::Usage(format!(
            "environment '{name}' already exists"
        ))));
    }

    // Validate `--like`/`--same`/`--skip` and build the (unasked) copy set
    // up front — cheap, and needed below to tell a deliberately target-less
    // environment (bindings given) from one with nothing to add at all.
    // `--same` is the default for everything, and is accepted as an
    // explicit statement of intent (it must name a real binding).
    let like_copy: Option<BTreeMap<String, Binding>> = match &like {
        Some(source_name) => {
            let source = ws.config.environments.get(source_name).ok_or_else(|| {
                anyhow!(CommandError::Usage(format!(
                    "unknown environment '{source_name}' (--like)"
                )))
            })?;
            for flag in same.iter().chain(skip.iter()) {
                if !source.dependencies.contains_key(flag) {
                    return Err(anyhow!(CommandError::Usage(format!(
                        "environment '{source_name}' has no binding '{flag}'"
                    ))));
                }
            }
            if let Some(both) = same.iter().find(|s| skip.contains(s)) {
                return Err(anyhow!(CommandError::Usage(format!(
                    "'{both}' is named by both --same and --skip"
                ))));
            }
            Some(
                source
                    .dependencies
                    .iter()
                    .filter(|(bname, _)| !skip.contains(bname))
                    .map(|(bname, b)| (bname.clone(), b.clone()))
                    .collect(),
            )
        }
        None => None,
    };
    let has_something_to_bind =
        !overrides.is_empty() || like_copy.as_ref().is_some_and(|c| !c.is_empty());

    // Targets before dependencies before `protected` (spec §3 order).
    // Explicit target flags skip the wizard entirely (non-interactive-
    // friendly, scriptable). With neither flag: a TTY runs the interactive
    // wizard (ARM discovery, same as `rigg init`); anything else is a usage
    // error that points at the wizard — unless bindings were given, which
    // makes a target-less environment a deliberate choice.
    let has_targets = search_service.is_some() || foundry_account.is_some();
    let (search, foundry) = if has_targets {
        (search_service, foundry_account.zip(foundry_project))
    } else if ctx.interactive() {
        discovery::discover_interactive(ctx.no_color).await?
    } else if has_something_to_bind {
        (None, None)
    } else if like.is_some() {
        return Err(anyhow!(CommandError::Usage(
            "nothing to add: pass --search-service/--foundry-account or use --like with an \
             environment that has bindings"
                .to_string()
        )));
    } else {
        return Err(anyhow!(CommandError::Usage(
            "in non-interactive mode pass --search-service and/or \
             --foundry-account/--foundry-project (or run `rigg env add <name>` on a terminal \
             for the interactive wizard)"
                .to_string()
        )));
    };

    // Dependencies: copied from `--like` (minus `--skip`, and minus `--same`
    // — those are a stated intent, kept as-is with no question asked), then
    // asked about interactively for the rest, then overridden by `--bind`.
    let mut deps: BTreeMap<String, Binding> = BTreeMap::new();
    if let (Some(source_name), Some(copy)) = (&like, like_copy) {
        if ctx.interactive() {
            deps = ask_like_bindings(
                ctx,
                copy,
                LikeAskContext {
                    new_env: &name,
                    source_env: source_name,
                    overrides: &overrides,
                    same: &same,
                    tenant: tenant.as_deref(),
                    subscription: subscription.as_deref(),
                },
            )
            .await?;
        } else {
            deps = copy;
        }
    }
    for (bname, binding) in overrides {
        deps.insert(bname, binding);
    }

    let protected = if protected {
        true
    } else if ctx.interactive() {
        let mut asker = ctx.asker("env add", json!({"env": name}));
        asker
            .ask(&crate::commands::ask::Question::confirm(
                format!("env.{name}.protected"),
                "Protect this environment (require typed confirmation for cloud changes)?",
                false,
            ))?
            .as_bool()
            .unwrap_or(false)
    } else {
        false
    };

    // `--like` copies bindings, not placement: a second environment usually
    // lives in another subscription (often another tenant), and guessing it
    // from the source would send every by-name resolution to the wrong
    // place. Pass --tenant/--subscription when they really are shared.

    let deps_for_print = deps.clone();
    edit_workspace_yaml(|doc| {
        let envs = envs_mut(doc)?;
        if envs.contains_key(&name) {
            bail!("environment '{name}' already exists");
        }
        let mut env = serde_yaml::Mapping::new();
        if envs.is_empty() {
            env.insert("default".into(), Yaml::Bool(true));
        }
        if let Some(tenant) = &tenant {
            env.insert("tenant".into(), Yaml::String(tenant.clone()));
        }
        if let Some(subscription) = &subscription {
            env.insert("subscription".into(), Yaml::String(subscription.clone()));
        }
        if let Some(service) = &search {
            let mut s = serde_yaml::Mapping::new();
            s.insert("service".into(), Yaml::String(service.clone()));
            env.insert("search".into(), Yaml::Mapping(s));
        }
        if let Some((account, project)) = &foundry {
            let mut f = serde_yaml::Mapping::new();
            f.insert("account".into(), Yaml::String(account.clone()));
            f.insert("project".into(), Yaml::String(project.clone()));
            env.insert("foundry".into(), Yaml::Mapping(f));
        }
        if protected {
            let mut p = serde_yaml::Mapping::new();
            p.insert("protected".into(), Yaml::Bool(true));
            env.insert("policy".into(), Yaml::Mapping(p));
        }
        if !deps.is_empty() {
            let mut d = serde_yaml::Mapping::new();
            for (bname, binding) in &deps {
                d.insert(Yaml::String(bname.clone()), bindings::binding_yaml(binding));
            }
            env.insert("dependencies".into(), Yaml::Mapping(d));
        }
        envs.insert(name.clone().into(), Yaml::Mapping(env));
        Ok(())
    })?;

    println!("Environment '{name}' added.");
    if let Some(s) = &search {
        println!("  search:   {s}");
    }
    if let Some((a, p)) = &foundry {
        println!("  foundry:  {a}/{p}");
    }
    if protected {
        println!("  protected: true");
    }
    for (bname, binding) in &deps_for_print {
        println!("  {bname}:  {} {}", binding.kind, binding.value);
    }
    println!("Set as default with: rigg env set-default {name}");
    Ok(())
}

/// Fixed context for [`ask_like_bindings`] — grouped to keep the function's
/// argument count sane.
struct LikeAskContext<'a> {
    new_env: &'a str,
    source_env: &'a str,
    overrides: &'a [(String, Binding)],
    same: &'a [String],
    tenant: Option<&'a str>,
    subscription: Option<&'a str>,
}

/// One question per copied binding: keep the source's value, pick another
/// resource of the same type from ARM, or skip it. Bindings already named
/// by `--bind` are not asked about, and neither are ones named by `--same`
/// — that flag is itself the answer ("keep the source's value"), stated
/// up front, so it's honored silently rather than asked about again.
async fn ask_like_bindings(
    ctx: &GlobalContext,
    copy: BTreeMap<String, Binding>,
    args: LikeAskContext<'_>,
) -> Result<BTreeMap<String, Binding>> {
    use crate::commands::ask::{Candidate, Question};
    let LikeAskContext {
        new_env,
        source_env,
        overrides,
        same,
        tenant,
        subscription,
    } = args;

    let mut out: BTreeMap<String, Binding> = BTreeMap::new();
    let asked: Vec<(String, Binding)> = copy
        .into_iter()
        .filter(|(name, binding)| {
            if overrides.iter().any(|(o, _)| o == name) {
                return false;
            }
            if same.contains(name) {
                out.insert(name.clone(), binding.clone());
                return false;
            }
            true
        })
        .collect();
    if asked.is_empty() {
        return Ok(out);
    }

    let mut questions = Vec::with_capacity(asked.len());
    for (name, binding) in &asked {
        let mut candidates = vec![Candidate {
            value: "same".into(),
            label: format!("same as {source_env} ({})", binding.value),
        }];
        for found in discovery::binding_candidates(binding.kind, tenant, subscription).await {
            if found.eq_ignore_ascii_case(&binding.value) {
                continue;
            }
            candidates.push(Candidate {
                value: found.clone(),
                label: found,
            });
        }
        candidates.push(Candidate {
            value: bindings::SKIP_ANSWER.into(),
            label: "skip (leave unbound)".into(),
        });
        questions.push(
            Question::choice(
                format!("binding.{new_env}.{name}"),
                format!("{name} ({}) in '{new_env}':", binding.kind),
                candidates,
            )
            .allow_other(),
        );
    }

    let mut asker = ctx.asker("env add", json!({"env": new_env, "like": source_env}));
    let answers = asker.ask_all(&questions)?;
    for ((name, binding), answer) in asked.into_iter().zip(answers) {
        match answer.as_str().unwrap_or_default() {
            "same" => {
                out.insert(name, binding);
            }
            v if v.eq_ignore_ascii_case(bindings::SKIP_ANSWER) || v.is_empty() => {}
            v => {
                out.insert(
                    name,
                    Binding {
                        kind: binding.kind,
                        value: v.to_string(),
                    },
                );
            }
        }
    }
    Ok(out)
}

fn remove(name: &str) -> Result<()> {
    edit_workspace_yaml(|doc| {
        let envs = envs_mut(doc)?;
        if envs.remove(name).is_none() {
            bail!("unknown environment '{name}'");
        }
        Ok(())
    })?;
    println!("Environment '{name}' removed.");
    Ok(())
}
