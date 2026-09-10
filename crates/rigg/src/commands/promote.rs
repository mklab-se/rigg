//! `rigg promote` — translate one environment's project tree into another.
//!
//! The translation itself is [`rigg_core::promote::translate`], which is
//! pure: it takes both environments' binding tables and documents and
//! returns the document the target should have for every logical resource,
//! plus what it could not decide. This module is the I/O around it:
//!
//! 1. load both environments (bindings + documents, correlated by LOGICAL
//!    id — the file stem — never by physical name);
//! 2. turn everything the engine left [`Pending`] into a question, apply the
//!    answers as bindings IN MEMORY, and re-translate (up to [`MAX_ROUNDS`]
//!    times, so a question that does not settle cannot loop);
//! 3. run the [`online_phase`] (unless `--offline`), which asks the TARGET's
//!    Azure what only it can answer (a Web API skill's auth carrier, a
//!    deployment's model availability and quota), folds the answers into the
//!    documents, and appends its decisions to the Checks — they are part of
//!    the plan, not a side note after it;
//! 4. show the preview — what points where after the translation, which
//!    sibling references were renamed, what changes per resource, and the
//!    Checks (now including the online decisions). `--dry-run` stops here:
//!    it still ran the online phase, so it may itself exit 6 on an
//!    unanswered deployment question — a question is part of the plan, and
//!    this happens before anything is written;
//! 5. once the run proceeds, persist the answered bindings to `rigg.yaml`
//!    and write the merged documents through the target environment's
//!    `Store`.
//!
//! Answers reach `rigg.yaml` only in step 5: `--dry-run`, an abort and the
//! `needs-input` exit all leave the workspace file exactly as they found it.
//!
//! Nothing is deleted and resources that exist only in the target are never
//! touched. `--dry-run` is preview only, but it is not offline: it runs the
//! online phase like any other promote (pass `--offline` too for a
//! network-free preview) and stops after the preview is printed.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, anyhow, bail};
use colored::Colorize;
use serde_json::{Value, json};

use rigg_core::binding::{Binding, BindingCache, BindingType, EnvBindings, validate_binding_name};
use rigg_core::infra;
use rigg_core::promote::{
    AuthCarrier, Change, Doc, EnvDocs, Item, Pending, Plan, rewiring_table, translate,
};
use rigg_core::resources::{ResourceKind, ResourceRef};
use rigg_core::store::Store;
use rigg_core::workspace::{Environment, Workspace};
use rigg_diff::output::SideLabels;

use crate::cli::PromoteArgs;
use crate::commands::ask::{Answer, Candidate, Question};
use crate::commands::{
    CommandError, GlobalContext, bindings, credentials, discovery, env as env_cmd, interactive,
    load_workspace, select_one_project,
};
use crate::say;

/// How many times the question loop may re-translate. Each round applies the
/// answers it got as bindings, so a well-formed answer settles its question;
/// the cap only catches an answer that keeps the same question open (e.g. a
/// binding value that still matches no resource).
const MAX_ROUNDS: usize = 5;

/// The answer that declines a question.
const SKIP: &str = "skip";
/// The answer that gives the target environment the source's own value.
const SAME: &str = "same";

pub async fn run(ctx: &GlobalContext, args: PromoteArgs) -> Result<()> {
    let mut ws = load_workspace()?;
    if !ws.config.environments.contains_key(&args.from) {
        return Err(anyhow!(CommandError::Usage(format!(
            "unknown environment '{}' (see `rigg env list`)",
            args.from
        ))));
    }
    if args.from == args.to {
        return Err(anyhow!(CommandError::Usage(
            "--from and --to must name different environments".to_string()
        )));
    }
    if !ws.config.environments.contains_key(&args.to) {
        if !ctx.interactive() {
            return Err(anyhow!(CommandError::Usage(missing_env_message(
                &ws, &args.from, &args.to
            ))));
        }
        // The environment-creation questions live in `env add`, not here.
        env_cmd::add_like_inline(ctx, &ws, &args.to, &args.from).await?;
        ws = load_workspace()?;
        if !ws.config.environments.contains_key(&args.to) {
            bail!("environment '{}' was not created", args.to);
        }
    }
    let project_name = select_one_project(&ws, args.project.as_deref())?
        .name
        .clone();

    // --- translate, asking about anything it cannot decide ---------------
    let mut settled = Settled::default();
    let mut answered = AnsweredBindings::default();
    let mut round = 0usize;
    let plan = loop {
        let (source, target) = load_sides(&ws, &project_name, &args)?;
        let plan = translate(&source, &target);
        let asks = build_asks(&args, &ws, &plan, &source, &settled).await;
        if asks.is_empty() {
            break plan;
        }
        round += 1;
        if round > MAX_ROUNDS {
            bail!(
                "promote still has open questions after {MAX_ROUNDS} rounds — \
                 record the bindings it needs with `rigg env bind` and re-run"
            );
        }
        let questions: Vec<Question> = asks.iter().map(|a| a.question.clone()).collect();
        let mut asker = ctx.asker(
            "promote",
            json!({"project": project_name, "from": args.from, "to": args.to}),
        );
        // An unanswered question leaves here as `NeedsInput` (exit 6) —
        // nothing has been written to `rigg.yaml` at this point, and
        // nothing will be.
        let answers = asker.ask_all(&questions)?;
        apply_answers(&args, &asks, &answers, &mut settled, &mut answered)?;
        answered.apply_to(&mut ws.config);
    };

    let mut plan = plan;
    let project = select_one_project(&ws, args.project.as_deref())?;
    let store_to = Store::new(project, &args.to);
    let targets = Targets::of(&ws, &args);
    let mut checks = checks(&plan, &args, &settled);
    let offered = pending_writes(&plan);

    // Everything that needs the TARGET's Azure to decide, folded into the
    // Checks BEFORE the preview is built: the online decisions are part of
    // the plan, not a side note after it, so `--dry-run` previews them too
    // (and may itself exit 6 on an unanswered deployment question — a
    // question is part of the plan, and this happens before anything is
    // written). A document whose ONLY difference is a missing auth carrier
    // is `Unchanged` until this phase derives one, and skipping it would
    // leave that to `rigg push`'s auth gate forever. The phase itself
    // returns before it builds an ARM client when there is nothing to
    // check, so an ordinary no-op promote still touches no network.
    if !args.offline {
        online_phase(ctx, &ws, &args, &project_name, &mut plan, &mut checks).await?;
    }

    // Scoped: nothing below this point needs the plan or checks by
    // reference any more before they are consumed by the write loop.
    {
        let preview = Preview {
            project: &project_name,
            args: &args,
            plan: &plan,
            checks: &checks,
            targets: &targets,
        };
        if !ctx.json() {
            preview.print();
        }

        if args.dry_run {
            if ctx.json() {
                println!("{}", preview.to_json(true));
            } else {
                println!();
                println!("(dry run — nothing written)");
            }
            return Ok(());
        }
    }

    // Recomputed: the online phase both adds changes (a derived auth
    // carrier) and takes items away (a declined deployment).
    if pending_writes(&plan) == 0 {
        // The run reached its end without aborting, so the answers are worth
        // keeping even though no document changed — otherwise the same
        // question comes back on every run.
        answered.persist()?;
        if ctx.json() {
            let preview = Preview {
                project: &project_name,
                args: &args,
                plan: &plan,
                checks: &checks,
                targets: &targets,
            };
            println!("{}", preview.to_json(false));
        } else {
            println!();
            if offered > 0 {
                // The preview offered changes; the online phase declined
                // every one of them. That is not "already matches".
                println!("Nothing written into '{}'.", args.to);
            } else {
                println!(
                    "nothing to promote — '{}' already matches '{}'",
                    args.to, args.from
                );
            }
        }
        return Ok(());
    }

    if ctx.interactive() {
        if !interactive::confirm_default_yes("Proceed?", ctx.no_color)? {
            println!("aborted");
            return Ok(());
        }
    } else if !ctx.yes {
        return Err(anyhow!(CommandError::Usage(
            "non-interactive promote requires --yes".to_string()
        )));
    }

    // The bindings the questions produced belong to the workspace before its
    // files start referring to them.
    answered.persist()?;

    // `write_exact`, not `write`: the merged document is already the finished
    // target file — it carries the target's own pins and annotations
    // deliberately, and its write-only fields (a data source's translated
    // `credentials.connectionString`) are exactly what promote rewired.
    // Carrying anything over from the file being replaced would silently
    // undo the translation.
    let mut written = 0usize;
    for item in &plan.items {
        match item.change() {
            Change::Unchanged => {}
            Change::Changed => {
                store_to.write_exact(
                    &ResourceRef::new(item.kind, item.target_name.clone()),
                    &item.merged,
                )?;
                written += 1;
            }
            // A new resource lands at the SOURCE's stem: that is the logical
            // id the two trees correlate by.
            Change::New => {
                store_to.write_at_exact(&item.stem, item.kind, &item.merged)?;
                written += 1;
            }
        }
    }

    if ctx.json() {
        let preview = Preview {
            project: &project_name,
            args: &args,
            plan: &plan,
            checks: &checks,
            targets: &targets,
        };
        println!("{}", preview.to_json(false));
    }
    say!(ctx);
    say!(ctx, "Promoted {written} resource(s) into '{}'.", args.to);
    say!(ctx, "hint: rigg validate {project_name}");
    say!(ctx, "      rigg auth doctor -e {}", args.to);
    say!(
        ctx,
        "      rigg push {project_name} -e {} --dry-run",
        args.to
    );
    say!(ctx, "      rigg push {project_name} -e {}", args.to);
    Ok(())
}

/// How many of the plan's documents this run would actually write.
fn pending_writes(plan: &Plan) -> usize {
    plan.items
        .iter()
        .filter(|i| i.change() != Change::Unchanged)
        .count()
}

/// The exact command that creates the missing target environment, filled in
/// from the source environment's own targets.
fn missing_env_message(ws: &Workspace, from: &str, to: &str) -> String {
    let mut command = format!("rigg env add {to} --like {from}");
    if let Some(env) = ws.config.environments.get(from) {
        if let Some(search) = &env.search {
            command.push_str(&format!(" --search-service {}", search.service));
        }
        if let Some(foundry) = &env.foundry {
            command.push_str(&format!(
                " --foundry-account {} --foundry-project {}",
                foundry.account, foundry.project
            ));
        }
    }
    format!("environment '{to}' does not exist — create it first: {command}")
}

// ---------------------------------------------------------------------
// loading both sides
// ---------------------------------------------------------------------

fn load_sides(
    ws: &Workspace,
    project_name: &str,
    args: &PromoteArgs,
) -> Result<(EnvDocs, EnvDocs)> {
    let project = ws.project(project_name)?;
    let source = env_docs(ws, project, &args.from)?;
    let target = env_docs(ws, project, &args.to)?;
    Ok((source, target))
}

fn env_docs(
    ws: &Workspace,
    project: &rigg_core::workspace::Project,
    env_name: &str,
) -> Result<EnvDocs> {
    let env = ws
        .config
        .environments
        .get(env_name)
        .cloned()
        .unwrap_or_default();
    let cache = BindingCache::load(ws, env_name);
    let bindings = EnvBindings::of_env(env_name, &env, Some(&cache));
    let store = Store::new(project, env_name);
    let mut docs = Vec::new();
    for (r, path) in store.list()? {
        let body = store.read_path(&path)?;
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        docs.push(Doc {
            kind: r.kind,
            stem,
            physical: r.name,
            body,
        });
    }
    Ok(EnvDocs {
        env: env_name.to_string(),
        bindings,
        docs,
    })
}

// ---------------------------------------------------------------------
// questions
// ---------------------------------------------------------------------

/// What the user already told us, carried across question rounds so nothing
/// is asked twice (and so the preview can report what was declined).
#[derive(Default)]
struct Settled {
    /// Physical resources the user declined to bind in the source.
    unbound_skipped: BTreeSet<String>,
    /// Binding names the user declined to give the target.
    binding_skipped: BTreeSet<String>,
    /// External hosts the user confirmed keeping verbatim.
    external_kept: BTreeSet<String>,
}

/// The bindings the questions produced, held in memory until the run
/// actually proceeds.
///
/// Each round applies them to the loaded [`Workspace`]'s config so the next
/// translation sees them, and only [`AnsweredBindings::persist`] — called
/// after the preview, once the user has said yes — writes them to
/// `rigg.yaml`. That is what keeps `--dry-run`, an aborted confirmation and
/// the `needs-input` (exit 6) path from editing the workspace file. An
/// aborted interactive run loses its answers; that is the trade.
#[derive(Default)]
struct AnsweredBindings(Vec<(String, String, Binding)>);

impl AnsweredBindings {
    fn record(&mut self, env: &str, name: &str, binding: Binding) {
        self.0.push((env.to_string(), name.to_string(), binding));
    }

    /// Make the answers visible to the next round's translation.
    fn apply_to(&self, config: &mut rigg_core::workspace::WorkspaceConfig) {
        for (env, name, binding) in &self.0 {
            if let Some(environment) = config.environments.get_mut(env) {
                environment
                    .dependencies
                    .insert(name.clone(), binding.clone());
            }
        }
    }

    /// Write them to `rigg.yaml`, one edit per environment.
    fn persist(&self) -> Result<()> {
        let mut per_env: BTreeMap<&str, Vec<(String, Binding)>> = BTreeMap::new();
        for (env, name, binding) in &self.0 {
            per_env
                .entry(env.as_str())
                .or_default()
                .push((name.clone(), binding.clone()));
        }
        for (env, bindings) in per_env {
            bindings::write_bindings(env, &bindings)?;
        }
        Ok(())
    }
}

/// One question, plus what to do with its answer.
struct Ask {
    question: Question,
    action: Action,
}

enum Action {
    /// Record a binding for the physical resource in the SOURCE environment.
    Bind {
        physical: String,
        kind: BindingType,
        value: String,
    },
    /// Give the TARGET environment a binding the source has.
    Missing {
        binding: String,
        kind: BindingType,
        source_value: String,
    },
    /// Keep an unbound external endpoint verbatim.
    External { host: String },
}

async fn build_asks(
    args: &PromoteArgs,
    ws: &Workspace,
    plan: &Plan,
    source: &EnvDocs,
    settled: &Settled,
) -> Vec<Ask> {
    let mut asks: Vec<Ask> = Vec::new();
    let mut asked: BTreeSet<String> = BTreeSet::new();
    for pending in &plan.pending {
        let ask = match pending {
            Pending::UnboundInSource {
                kind,
                stem,
                path,
                target,
                physical,
                proposed_name,
            } => {
                if settled.unbound_skipped.contains(physical) {
                    continue;
                }
                let Some(binding_type) = infra::binding_type_for(*target) else {
                    continue; // a search service is an env target, not a binding
                };
                let value = reference_value(source, *kind, stem, path, binding_type)
                    .unwrap_or_else(|| physical.clone());
                Ask {
                    question: Question::text(
                        format!("promote.bind.{}.{physical}", args.from),
                        format!(
                            "{}/{stem} at {path} uses {target} '{physical}', which is not bound \
                             in '{}'. Bind it as (or '{SKIP}'):",
                            kind.directory_name(),
                            args.from,
                        ),
                    )
                    .with_default(proposed_name.clone()),
                    action: Action::Bind {
                        physical: physical.clone(),
                        kind: binding_type,
                        value,
                    },
                }
            }
            Pending::MissingInTarget {
                binding,
                binding_type,
                source_physical,
                used_by,
            } => {
                if settled.binding_skipped.contains(binding) {
                    continue;
                }
                // An implicit `search`/`foundry` target cannot be declared as
                // a dependency binding — it is reported, not asked about.
                let Some(binding_type) = *binding_type else {
                    continue;
                };
                let source_value = declared_value(ws, &args.from, binding)
                    .unwrap_or_else(|| source_physical.clone());
                let mut candidates = vec![Candidate {
                    value: SAME.to_string(),
                    label: format!("same as {}: {source_value} (shared)", args.from),
                }];
                candidates.extend(
                    arm_candidates(args, ws, binding_type, &source_value)
                        .await
                        .into_iter()
                        .map(|name| Candidate {
                            value: name.clone(),
                            label: name,
                        }),
                );
                candidates.push(Candidate {
                    value: SKIP.to_string(),
                    label: format!("skip (keep the value '{}' has)", args.from),
                });
                Ask {
                    question: Question::choice(
                        format!("binding.{}.{binding}", args.to),
                        format!(
                            "'{}' has no binding '{binding}' ({binding_type}), used by {} \
                             reference(s). Use:",
                            args.to,
                            used_by.len()
                        ),
                        candidates,
                    )
                    .allow_other(),
                    action: Action::Missing {
                        binding: binding.clone(),
                        kind: binding_type,
                        source_value,
                    },
                }
            }
            Pending::External { host, used_by } => {
                if settled.external_kept.contains(host) {
                    continue;
                }
                Ask {
                    question: Question::confirm(
                        format!("promote.external.{host}"),
                        format!(
                            "'{host}' is an external API, bound in neither environment (used by \
                             {} reference(s)). Keep it verbatim in '{}'?",
                            used_by.len(),
                            args.to
                        ),
                        true,
                    ),
                    action: Action::External { host: host.clone() },
                }
            }
            // Never a question: the binding exists, it just isn't resolved
            // well enough to rewrite this value's shape.
            Pending::UnresolvedTarget { .. } => continue,
        };
        if asked.insert(ask.question.id.clone()) {
            asks.push(ask);
        }
    }
    asks
}

/// The value to record for a reference the source does not bind: the ARM id
/// the file already carries when it has one, else the physical name — the
/// same rule `rigg env bind --learn` applies.
fn reference_value(
    source: &EnvDocs,
    kind: ResourceKind,
    stem: &str,
    path: &str,
    binding_type: BindingType,
) -> Option<String> {
    let doc = source
        .docs
        .iter()
        .find(|d| d.kind == kind && d.stem == stem)?;
    let found = infra::extract(kind, &doc.body)
        .into_iter()
        .find(|f| f.path == path)?;
    Some(bindings::proposed_value(binding_type, &found))
}

/// The value `env` declares for dependency binding `name`.
fn declared_value(ws: &Workspace, env: &str, name: &str) -> Option<String> {
    ws.config
        .environments
        .get(env)?
        .dependencies
        .get(name)
        .map(|b| b.value.clone())
}

/// Resources of `kind` visible in the target environment's subscription, as
/// extra candidates. Gated on `--offline` alone: a scripted caller reading
/// the `needs-input` document deserves the same pick-list a person gets, and
/// `--offline` is the one flag that says "do not talk to Azure". Best-effort
/// — [`discovery::binding_candidates`] returns an empty list when ARM is not
/// reachable.
async fn arm_candidates(
    args: &PromoteArgs,
    ws: &Workspace,
    kind: BindingType,
    source_value: &str,
) -> Vec<String> {
    if args.offline {
        return Vec::new();
    }
    let Some(env) = ws.config.environments.get(&args.to) else {
        return Vec::new();
    };
    let source_physical = Binding {
        kind,
        value: source_value.to_string(),
    }
    .physical_name();
    discovery::binding_candidates(kind, env.tenant.as_deref(), env.subscription.as_deref())
        .await
        .into_iter()
        .filter(|found| !found.eq_ignore_ascii_case(&source_physical))
        .collect()
}

fn apply_answers(
    args: &PromoteArgs,
    asks: &[Ask],
    answers: &[Answer],
    settled: &mut Settled,
    answered: &mut AnsweredBindings,
) -> Result<()> {
    for (ask, answer) in asks.iter().zip(answers) {
        match &ask.action {
            Action::Bind {
                physical,
                kind,
                value,
            } => {
                let raw = answer.as_str().unwrap_or_default().trim().to_string();
                // `skip` is therefore not a name a binding can be given here
                // — declining always wins over naming.
                if raw.is_empty() || raw.eq_ignore_ascii_case(SKIP) {
                    settled.unbound_skipped.insert(physical.clone());
                    continue;
                }
                // Checked here rather than at `persist` time: a bad name must
                // not reach the in-memory binding table the next round
                // translates with.
                validate_binding_name(&raw).map_err(|e| anyhow!(CommandError::Usage(e)))?;
                answered.record(
                    &args.from,
                    &raw,
                    Binding {
                        kind: *kind,
                        value: value.clone(),
                    },
                );
            }
            Action::Missing {
                binding,
                kind,
                source_value,
            } => {
                let raw = answer.as_str().unwrap_or_default().trim().to_string();
                if raw.is_empty() || raw.eq_ignore_ascii_case(SKIP) {
                    settled.binding_skipped.insert(binding.clone());
                    continue;
                }
                let value = if raw == SAME {
                    source_value.clone()
                } else {
                    raw
                };
                answered.record(&args.to, binding, Binding { kind: *kind, value });
            }
            Action::External { host } => {
                if answer.as_bool().unwrap_or(false) {
                    settled.external_kept.insert(host.clone());
                } else {
                    return Err(anyhow!(CommandError::Usage(format!(
                        "'{host}' is not bound in either environment — bind it before promoting: \
                         `rigg env bind {} <name> api:https://{host}` (and the same in '{}')",
                        args.from, args.to
                    ))));
                }
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------
// the online phase
// ---------------------------------------------------------------------

/// Everything that needs the TARGET environment's Azure to decide: a Web API
/// skill's auth carrier (re-derived from the target function app's Easy Auth
/// state) and a deployment's model availability and quota in the target
/// account's region.
///
/// It mutates the merged documents in place and appends what it decided to
/// `checks`, so the run's final Checks — text and JSON alike — say what
/// happened. **Not reaching Azure is never an error**: promote's product is
/// files, and every skipped decision is reported and left to `rigg push`.
/// The one thing that does stop the run is an unanswered question (exit 6),
/// which by construction happens before anything is written.
///
/// It runs before the preview is built — and so before both the `--dry-run`
/// stop and the "nothing to promote" exit — because a document whose only
/// difference from the target's is a missing auth carrier is `Unchanged`
/// until this phase supplies one, and its Checks belong in the one preview
/// the run shows. To keep an ordinary no-op promote off the network, it
/// returns here — before building an ARM client — whenever there is nothing
/// to check.
async fn online_phase(
    ctx: &GlobalContext,
    ws: &Workspace,
    args: &PromoteArgs,
    project_name: &str,
    plan: &mut Plan,
    checks: &mut Vec<Check>,
) -> Result<()> {
    // (item index, `skills[i]` path, the source authenticated with a key)
    let carriers: Vec<(usize, String, bool)> = plan
        .items
        .iter()
        .enumerate()
        .flat_map(|(i, item)| {
            item.auth.iter().filter_map(move |carrier| match carrier {
                AuthCarrier::Stripped { path, used_key } if !target_kept_carrier(item, path) => {
                    Some((i, path.clone(), *used_key))
                }
                _ => None,
            })
        })
        .collect();
    let deployments: Vec<usize> = plan
        .items
        .iter()
        .enumerate()
        .filter(|(_, item)| needs_capacity_check(item))
        .map(|(i, _)| i)
        .collect();
    if carriers.is_empty() && deployments.is_empty() {
        return Ok(());
    }

    let env = ws
        .config
        .environments
        .get(&args.to)
        .cloned()
        .unwrap_or_default();
    let arm = match rigg_client::arm::ArmClient::for_tenant(env.tenant.as_deref()) {
        Ok(arm) => arm,
        Err(e) => {
            for (item, path, _) in &carriers {
                checks.push(Check {
                    ok: false,
                    message: format!(
                        "{} {path}: ARM unavailable ({e}): resolved on push",
                        plan.items[*item].label()
                    ),
                });
            }
            for item in &deployments {
                checks.push(Check {
                    ok: false,
                    message: format!(
                        "{}: ARM unavailable ({e}): availability and quota not checked",
                        plan.items[*item].label()
                    ),
                });
            }
            return Ok(());
        }
    };

    rederive_auth(&arm, args, plan, checks, &carriers).await;
    check_deployments(
        ctx,
        &arm,
        ws,
        args,
        project_name,
        plan,
        checks,
        &deployments,
    )
    .await
}

/// A deployment the target does not have yet, or one whose `sku.capacity`
/// this promote raises: the only two cases that ask the target region for
/// anything it is not already giving.
fn needs_capacity_check(item: &Item) -> bool {
    item.kind == ResourceKind::Deployment
        && (item.is_new || capacity_of(&item.merged) > item.before.as_ref().and_then(capacity_of))
}

/// The target's own carrier for this skill was kept ([`AuthCarrier::Kept`]),
/// so there is nothing to re-derive — the file already says how the target
/// authenticates.
fn target_kept_carrier(item: &Item, path: &str) -> bool {
    item.auth
        .iter()
        .any(|c| matches!(c, AuthCarrier::Kept { path: kept } if kept == path))
}

/// `skills[3]` → `3`.
fn skill_index(path: &str) -> Option<usize> {
    path.strip_prefix("skills[")?
        .strip_suffix(']')?
        .parse()
        .ok()
}

/// Give every stripped Web API auth carrier the shape the TARGET's function
/// app needs: Entra ID when the app has Easy Auth, else the push-time key
/// annotation when the source used a key, else anonymous.
async fn rederive_auth(
    arm: &rigg_client::arm::ArmClient,
    args: &PromoteArgs,
    plan: &mut Plan,
    checks: &mut Vec<Check>,
    carriers: &[(usize, String, bool)],
) {
    for (item, path, used_key) in carriers {
        let label = plan.items[*item].label();
        let mut report = |ok: bool, message: String| {
            checks.push(Check {
                ok,
                message: format!("{label} {path}: {message}"),
            })
        };
        let Some(index) = skill_index(path) else {
            report(
                false,
                "auth carrier unresolved — unrecognized skill path: resolved on push".to_string(),
            );
            continue;
        };
        let pointer = format!("/skills/{index}");
        let uri = plan.items[*item]
            .merged
            .pointer(&format!("{pointer}/uri"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let Some((site, _)) = credentials::parse_function_uri(&uri) else {
            report(
                false,
                format!("auth carrier unresolved — '{uri}' is not an Azure Functions endpoint"),
            );
            continue;
        };
        let site_id = match arm.find_web_site_id(&site).await {
            Ok(id) => id,
            Err(e) => {
                report(
                    false,
                    format!(
                        "auth carrier unresolved — function app '{site}' ({e}): resolved on push"
                    ),
                );
                continue;
            }
        };
        // An ARM failure here is NOT evidence: a caller who may read the site
        // but not its `authsettingsV2` would otherwise be told the app is
        // anonymous. Only a document ARM actually returned decides anything.
        let audience = match arm.site_auth_settings(&site_id).await {
            Ok(settings) => credentials::easy_auth_audience_of(&settings),
            Err(e) => {
                report(
                    false,
                    format!("auth carrier unresolved — '{site}' ({e}): resolved on push"),
                );
                continue;
            }
        };
        let Some(skill) = plan.items[*item].merged.pointer_mut(&pointer) else {
            report(
                false,
                format!("auth carrier unresolved — no '{path}' in the target document"),
            );
            continue;
        };
        match audience {
            Some(audience) => {
                skill["authResourceId"] = Value::String(audience.clone());
                skill["uri"] = Value::String(credentials::strip_code_param(&uri));
                credentials::remove_function_key_header(skill);
                if let Some(map) = skill.as_object_mut() {
                    map.remove(credentials::X_RIGG_AUTH);
                }
                report(true, format!("Entra ID ({audience}) on '{site}'"));
            }
            // No Entra auth on the target app: keep the shape the source
            // authorized with, so push (or `--refresh-credentials`) can
            // inject the TARGET app's key. The file only ever holds the
            // placeholder.
            None if *used_key => {
                skill[credentials::X_RIGG_AUTH] =
                    Value::String(credentials::X_RIGG_AUTH_FUNCTION_KEY.to_string());
                skill["uri"] = Value::String(credentials::set_code_param(&uri, "<redacted>"));
                report(
                    false,
                    format!("function key resolved at push time ('{site}' has no Entra auth)"),
                );
            }
            None => report(
                false,
                format!(
                    "anonymous ('{site}' has no Entra auth and '{}' used no key)",
                    args.from
                ),
            ),
        }
    }
}

/// One deployment question, and the item it decides.
struct DeploymentAsk {
    item: usize,
    reason: String,
    question: Question,
}

/// The answer that promotes a deployment despite the check.
const CONTINUE: &str = "continue";
/// Answer prefix that overrides the deployment's capacity: `capacity:20`.
const CAPACITY: &str = "capacity:";

/// What one answer to a `promote.deployment.<stem>` question says.
enum DeploymentAnswer {
    /// Promote the deployment as it is.
    Continue,
    /// Leave it out of this promote.
    Skip,
    /// Promote it with this `sku.capacity` instead.
    Capacity(i64),
}

/// Parse one answer. All three forms are matched case-insensitively — the
/// answer may come from a person typing or from a script echoing a candidate
/// back — and a capacity must be a whole number of units, at least one.
/// `None` is a usage error, never a silent fallback.
fn parse_deployment_answer(raw: &str) -> Option<DeploymentAnswer> {
    let raw = raw.trim();
    if raw.eq_ignore_ascii_case(SKIP) {
        return Some(DeploymentAnswer::Skip);
    }
    if raw.eq_ignore_ascii_case(CONTINUE) {
        return Some(DeploymentAnswer::Continue);
    }
    let (prefix, rest) = raw.split_at_checked(CAPACITY.len())?;
    if !prefix.eq_ignore_ascii_case(CAPACITY) {
        return None;
    }
    rest.trim()
        .parse::<i64>()
        .ok()
        .filter(|n| *n >= 1)
        .map(DeploymentAnswer::Capacity)
}

/// Check every new (or capacity-raising) deployment against what the TARGET
/// account's region actually offers, and ask about the ones that do not fit.
#[allow(clippy::too_many_arguments)] // one online step, one signature
async fn check_deployments(
    ctx: &GlobalContext,
    arm: &rigg_client::arm::ArmClient,
    ws: &Workspace,
    args: &PromoteArgs,
    project_name: &str,
    plan: &mut Plan,
    checks: &mut Vec<Check>,
    deployments: &[usize],
) -> Result<()> {
    if deployments.is_empty() {
        return Ok(());
    }
    let skip_all = |checks: &mut Vec<Check>, why: String| {
        for item in deployments {
            checks.push(Check {
                ok: false,
                message: format!("{}: {why}", plan.items[*item].label()),
            });
        }
    };

    let env = ws
        .config
        .environments
        .get(&args.to)
        .cloned()
        .unwrap_or_default();
    let Some(foundry) = env.foundry.as_ref() else {
        skip_all(
            checks,
            format!(
                "'{}' has no Foundry account — availability not checked",
                args.to
            ),
        );
        return Ok(());
    };
    // The resolved binding cache knows the account's region without a
    // round-trip; ARM answers when it does not.
    let cached = BindingCache::load(ws, &args.to);
    let cached = cached
        .get("foundry")
        .and_then(|b| b.location.clone().map(|l| (l, b.subscription.clone())));
    let (location, subscription) = match cached {
        Some(found) => found,
        None => match arm.find_cognitive_account(&foundry.account).await {
            Ok(account) => (account.location.clone(), subscription_of(&account.id)),
            Err(e) => {
                skip_all(
                    checks,
                    format!(
                        "Foundry account '{}' not resolved ({e}) — availability not checked",
                        foundry.account
                    ),
                );
                return Ok(());
            }
        },
    };
    let Some(subscription) = subscription.or_else(|| env.subscription.clone()) else {
        skip_all(
            checks,
            format!(
                "subscription of '{}' unknown — run `rigg env show {} --refresh`",
                foundry.account, args.to
            ),
        );
        return Ok(());
    };
    let models = match arm.list_location_models(&subscription, &location).await {
        Ok(models) => models,
        Err(e) => {
            skip_all(
                checks,
                format!("models of {location} not listed ({e}) — availability not checked"),
            );
            return Ok(());
        }
    };
    let usages = match arm.list_location_usages(&subscription, &location).await {
        Ok(usages) => usages,
        Err(e) => {
            checks.push(Check {
                ok: false,
                message: format!("quota in {location} not read ({e}) — capacity not checked"),
            });
            Vec::new()
        }
    };

    let mut asks: Vec<DeploymentAsk> = Vec::new();
    for item in deployments {
        let deployment = &plan.items[*item];
        match verdict(
            &deployment.merged,
            deployment.before.as_ref(),
            &models,
            &usages,
            &location,
        ) {
            Ok(message) => checks.push(Check {
                ok: true,
                message: format!("{}: {message}", deployment.label()),
            }),
            Err(reason) => asks.push(DeploymentAsk {
                item: *item,
                reason: reason.clone(),
                question: Question::choice(
                    format!("promote.deployment.{}", deployment.stem),
                    format!(
                        "{}: {reason}. Promote it anyway, skip it, or set a capacity \
                         ('{CAPACITY}<n>'):",
                        deployment.label()
                    ),
                    vec![
                        Candidate {
                            value: CONTINUE.to_string(),
                            label: "continue — promote it as it is".to_string(),
                        },
                        Candidate {
                            value: SKIP.to_string(),
                            label: format!(
                                "skip — leave it out of this promote into '{}'",
                                args.to
                            ),
                        },
                    ],
                )
                .allow_other(),
            }),
        }
    }
    if asks.is_empty() {
        return Ok(());
    }

    let questions: Vec<Question> = asks.iter().map(|a| a.question.clone()).collect();
    let mut asker = ctx.asker(
        "promote",
        json!({"project": project_name, "from": args.from, "to": args.to}),
    );
    // Unanswered, this leaves as `NeedsInput` (exit 6) — before `persist`
    // and before the first file write, so the workspace is untouched.
    let answers = asker.ask_all(&questions)?;

    let mut skipped: Vec<usize> = Vec::new();
    for (ask, answer) in asks.iter().zip(answers) {
        let raw = answer.as_str().unwrap_or_default().trim().to_string();
        let label = plan.items[ask.item].label();
        let reason = &ask.reason;
        let Some(parsed) = parse_deployment_answer(&raw) else {
            return Err(anyhow!(CommandError::Usage(format!(
                "invalid answer for 'promote.deployment.{}': '{raw}' — expected \
                 '{CONTINUE}', '{SKIP}' or '{CAPACITY}<number>' (a whole number of units, \
                 at least 1)",
                plan.items[ask.item].stem
            ))));
        };
        match parsed {
            DeploymentAnswer::Skip => {
                skipped.push(ask.item);
                checks.push(Check {
                    ok: false,
                    message: format!("{label}: skipped ({reason})"),
                });
            }
            DeploymentAnswer::Continue => checks.push(Check {
                ok: false,
                message: format!("{label}: promoted anyway ({reason})"),
            }),
            DeploymentAnswer::Capacity(capacity) => {
                set_capacity(&mut plan.items[ask.item].merged, capacity);
                checks.push(Check {
                    ok: true,
                    message: format!("{label}: capacity set to {capacity} ({reason})"),
                });
            }
        }
    }
    // A skipped deployment leaves the write set entirely: it is not promoted,
    // and the JSON preview must not claim it was.
    skipped.sort_unstable();
    skipped.dedup();
    for item in skipped.into_iter().rev() {
        plan.items.remove(item);
    }
    Ok(())
}

/// `/subscriptions/<id>/resourceGroups/...` → `<id>`.
fn subscription_of(arm_id: &str) -> Option<String> {
    let mut parts = arm_id.split('/');
    parts.find(|p| p.eq_ignore_ascii_case("subscriptions"))?;
    parts.next().filter(|s| !s.is_empty()).map(str::to_string)
}

fn capacity_of(doc: &Value) -> Option<f64> {
    doc.pointer("/sku/capacity").and_then(Value::as_f64)
}

fn set_capacity(doc: &mut Value, capacity: i64) {
    match doc.get_mut("sku").and_then(Value::as_object_mut) {
        Some(sku) => {
            sku.insert("capacity".to_string(), json!(capacity));
        }
        None => {
            if let Some(map) = doc.as_object_mut() {
                map.insert("sku".to_string(), json!({ "capacity": capacity }));
            }
        }
    }
}

/// What the target region says about one deployment: `Ok` is a line for the
/// Checks section, `Err` is the reason to ask about it.
///
/// `before` is the target's current document, when it has one: raising a
/// deployment from 50 to 55 units asks the region's quota for the five it
/// does not already hold, not for all 55.
fn verdict(
    doc: &Value,
    before: Option<&Value>,
    models: &[Value],
    usages: &[Value],
    location: &str,
) -> Result<String, String> {
    let model = doc
        .pointer("/properties/model/name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if model.is_empty() {
        return Ok(format!("no model named — nothing to check in {location}"));
    }
    let version = doc
        .pointer("/properties/model/version")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let named = if version.is_empty() {
        format!("model '{model}'")
    } else {
        format!("model '{model}' version {version}")
    };
    let Some(offered) = models.iter().find(|m| model_matches(m, model, version)) else {
        return Err(format!("{named} is not available in {location}"));
    };
    let sku = doc
        .pointer("/sku/name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let (Some(usage_name), Some(capacity)) = (usage_name_of(offered, sku), capacity_of(doc)) else {
        return Ok(format!(
            "{named} available in {location} (no quota to check)"
        ));
    };
    // Only the increase is new demand; what the target already runs is
    // already counted in `currentValue`.
    let demand = (capacity - before.and_then(capacity_of).unwrap_or(0.0)).max(0.0);
    let Some(usage) = usages
        .iter()
        .find(|u| u.pointer("/name/value").and_then(Value::as_str) == Some(usage_name.as_str()))
    else {
        return Ok(format!(
            "{named} available in {location} (quota '{usage_name}' not reported)"
        ));
    };
    let limit = usage
        .get("limit")
        .and_then(Value::as_f64)
        .unwrap_or_default();
    let free = limit
        - usage
            .get("currentValue")
            .and_then(Value::as_f64)
            .unwrap_or_default();
    if free >= demand {
        Ok(format!(
            "{named} available in {location}, quota ok ({free} of {limit} free, asks for {demand})"
        ))
    } else {
        Err(format!(
            "quota '{usage_name}' in {location} has {free} of {limit} free and the deployment asks \
             for {demand}"
        ))
    }
}

/// One `locations/{l}/models` entry offers this model. The version matches
/// when both name it the same, or when either side does not say.
fn model_matches(offered: &Value, name: &str, version: &str) -> bool {
    let model = offered.get("model").unwrap_or(offered);
    let named = model
        .get("name")
        .and_then(Value::as_str)
        .is_some_and(|n| n.eq_ignore_ascii_case(name));
    let offered_version = model
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or_default();
    named && (version.is_empty() || offered_version.is_empty() || version == offered_version)
}

/// The quota (`usageName`) the model's `sku` counts against.
fn usage_name_of(offered: &Value, sku: &str) -> Option<String> {
    offered
        .pointer("/model/skus")
        .or_else(|| offered.get("skus"))?
        .as_array()?
        .iter()
        .find(|s| {
            s.get("name")
                .and_then(Value::as_str)
                .is_some_and(|n| n.eq_ignore_ascii_case(sku))
        })?
        .get("usageName")
        .and_then(Value::as_str)
        .map(str::to_string)
}

// ---------------------------------------------------------------------
// checks
// ---------------------------------------------------------------------

struct Check {
    ok: bool,
    message: String,
}

fn print_checks(checks: &[Check]) {
    for check in checks {
        let mark = if check.ok {
            "✓".green()
        } else {
            "!".yellow()
        };
        println!("  {mark} {}", check.message);
    }
}

/// Everything worth saying about the plan that is neither a rewiring, a
/// rename, nor a per-resource diff: unresolved bindings, declined questions,
/// and Web API auth carriers that did not cross.
fn checks(plan: &Plan, args: &PromoteArgs, settled: &Settled) -> Vec<Check> {
    let mut out = Vec::new();

    // Only `--offline` reports carriers and deployments here: online, the
    // phase that actually decides them reports each exactly once, and a
    // placeholder line above its verdict would say the same thing twice.
    if args.offline {
        for item in &plan.items {
            for carrier in &item.auth {
                // A carrier the target already had is kept, not re-derived —
                // the file itself says how '{to}' authenticates.
                let AuthCarrier::Stripped { path, .. } = carrier else {
                    continue;
                };
                if target_kept_carrier(item, path) {
                    continue;
                }
                out.push(Check {
                    ok: false,
                    message: format!(
                        "{} {path}: Web API auth carrier unresolved (it authorizes '{}') — \
                         resolved against '{}' on push",
                        item.label(),
                        args.from,
                        args.to
                    ),
                });
            }
        }
        for item in plan.items.iter().filter(|i| needs_capacity_check(i)) {
            out.push(Check {
                ok: false,
                message: format!(
                    "{}: availability and quota not checked (--offline)",
                    item.label()
                ),
            });
        }
    }

    for pending in &plan.pending {
        match pending {
            Pending::UnresolvedTarget {
                binding, used_by, ..
            } => {
                out.push(Check {
                    ok: false,
                    message: format!(
                        "binding '{binding}' in '{}' is declared by name only — run `rigg env \
                         show {} --refresh` (or declare the full ARM id)",
                        args.to, args.to
                    ),
                });
                // Say which files it left pointing at the source, the same
                // way a skipped binding does.
                for (kind, stem, path) in used_by {
                    out.push(Check {
                        ok: false,
                        message: format!(
                            "{}/{stem} {path}: kept from '{}' (binding '{binding}' unresolved in \
                             '{}')",
                            kind.directory_name(),
                            args.from,
                            args.to
                        ),
                    });
                }
            }
            Pending::MissingInTarget {
                binding,
                binding_type,
                used_by,
                ..
            } => {
                let declinable = binding_type.is_some();
                if declinable && !settled.binding_skipped.contains(binding) {
                    continue;
                }
                let why = if declinable {
                    format!("binding '{binding}' skipped")
                } else {
                    format!("'{}' has no '{binding}' target", args.to)
                };
                for (kind, stem, path) in used_by {
                    out.push(Check {
                        ok: false,
                        message: format!(
                            "{}/{stem} {path}: kept from '{}' ({why})",
                            kind.directory_name(),
                            args.from
                        ),
                    });
                }
            }
            Pending::UnboundInSource {
                kind,
                stem,
                path,
                target,
                physical,
                ..
            } => {
                let bindable = infra::binding_type_for(*target).is_some();
                if bindable && !settled.unbound_skipped.contains(physical) {
                    continue;
                }
                out.push(Check {
                    ok: false,
                    message: format!(
                        "{}/{stem} {path}: {target} '{physical}' is not bound in '{}' — kept as is",
                        kind.directory_name(),
                        args.from
                    ),
                });
            }
            Pending::External { host, used_by } => {
                if !settled.external_kept.contains(host) {
                    continue;
                }
                for (kind, stem, path) in used_by {
                    out.push(Check {
                        ok: true,
                        message: format!(
                            "{}/{stem} {path}: external API '{host}' kept verbatim",
                            kind.directory_name()
                        ),
                    });
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------
// preview
// ---------------------------------------------------------------------

/// The two environments' service targets, for the preview's header line.
#[derive(Default)]
struct Targets {
    search: Option<(String, String)>,
    foundry: Option<(String, String)>,
}

impl Targets {
    fn of(ws: &Workspace, args: &PromoteArgs) -> Targets {
        let from = ws.config.environments.get(&args.from);
        let to = ws.config.environments.get(&args.to);
        let search = match (from.and_then(search_of), to.and_then(search_of)) {
            (Some(a), Some(b)) => Some((a, b)),
            _ => None,
        };
        let foundry = match (from.and_then(foundry_of), to.and_then(foundry_of)) {
            (Some(a), Some(b)) => Some((a, b)),
            _ => None,
        };
        Targets { search, foundry }
    }
}

fn search_of(env: &Environment) -> Option<String> {
    env.search.as_ref().map(|s| s.service.clone())
}

fn foundry_of(env: &Environment) -> Option<String> {
    env.foundry
        .as_ref()
        .map(|f| format!("{}/{}", f.account, f.project))
}

/// One row of the `Renamed siblings` table.
struct RenamedRow {
    label: String,
    from: String,
    to: String,
    references: usize,
}

fn renamed_rows(plan: &Plan) -> Vec<RenamedRow> {
    let mut rows: BTreeMap<(ResourceKind, String), (String, String, usize)> = BTreeMap::new();
    for renamed in plan.items.iter().flat_map(|i| &i.renamed) {
        rows.entry((renamed.kind, renamed.stem.clone()))
            .and_modify(|row| row.2 += 1)
            .or_insert((renamed.from.clone(), renamed.to.clone(), 1));
    }
    rows.into_iter()
        .map(|((kind, stem), (from, to, references))| RenamedRow {
            label: format!("{}/{stem}", kind.directory_name()),
            from,
            to,
            references,
        })
        .collect()
}

fn items_of(plan: &Plan, change: Change) -> Vec<&Item> {
    plan.items.iter().filter(|i| i.change() == change).collect()
}

struct Preview<'a> {
    project: &'a str,
    args: &'a PromoteArgs,
    plan: &'a Plan,
    checks: &'a [Check],
    targets: &'a Targets,
}

impl Preview<'_> {
    fn print(&self) {
        let (from, to) = (&self.args.from, &self.args.to);
        println!(
            "{} project '{}': {} {} {}",
            "Promote".bold(),
            self.project,
            from,
            "→".dimmed(),
            to
        );
        let mut targets: Vec<String> = Vec::new();
        if let Some((a, b)) = &self.targets.search {
            targets.push(format!("Search {a} → {b}"));
        }
        if let Some((a, b)) = &self.targets.foundry {
            targets.push(format!("Foundry {a} → {b}"));
        }
        if !targets.is_empty() {
            println!("  Targets: {}", targets.join(", "));
        }

        let rewiring = rewiring_table(self.plan);
        if !rewiring.is_empty() {
            println!();
            println!("{}", "Rewiring (bindings)".bold());
            for (binding, target, from_name, to_name, shared, references) in &rewiring {
                let arrow = if *shared { "=" } else { "→" };
                let mut row = format!(
                    "  {binding:<14} {:<14} {from_name:<26} {arrow} {to_name:<26}",
                    target.to_string()
                );
                if *shared {
                    row.push_str(" shared");
                }
                if *references > 1 {
                    row.push_str(&format!(" ({references} references)"));
                }
                println!("{}", row.trim_end());
            }
        }

        let renamed = renamed_rows(self.plan);
        if !renamed.is_empty() {
            println!();
            println!("{}", "Renamed siblings".bold());
            for row in &renamed {
                println!(
                    "  {:<24} {} → {}  ({} reference(s) rewritten)",
                    row.label, row.from, row.to, row.references
                );
            }
        }

        let changed = items_of(self.plan, Change::Changed);
        let new = items_of(self.plan, Change::New);
        let unchanged = items_of(self.plan, Change::Unchanged);
        println!();
        println!("{}", "Resources".bold());
        println!(
            "  {} changed, {} new, {} unchanged, {} kept (only in '{to}')",
            changed.len(),
            new.len(),
            unchanged.len(),
            self.plan.kept_only_in_to.len(),
        );
        if !changed.is_empty() {
            println!();
            let labels = SideLabels {
                new_side: format!("{from} (incoming)"),
                old_side: to.clone(),
            };
            for item in &changed {
                let diff = rigg_diff::semantic::diff(
                    item.before.as_ref().unwrap_or(&Value::Null),
                    &item.merged,
                    "name",
                );
                print!(
                    "{}",
                    rigg_diff::output::format_text(&diff, &item.label(), &labels)
                );
            }
        }
        if !new.is_empty() {
            println!();
            println!("new (will be created in '{to}'):");
            for item in &new {
                println!("  {}", item.label());
            }
        }
        if !self.plan.kept_only_in_to.is_empty() {
            println!();
            println!("kept (only in '{to}' — never touched by promote):");
            for (kind, stem) in &self.plan.kept_only_in_to {
                println!("  {}/{stem}", kind.directory_name());
            }
        }

        if !self.checks.is_empty() {
            println!();
            println!("{}", "Checks".bold());
            print_checks(self.checks);
        }
    }

    fn to_json(&self, dry_run: bool) -> String {
        let labels =
            |items: Vec<&Item>| -> Vec<String> { items.iter().map(|i| i.label()).collect() };
        let mut targets = serde_json::Map::new();
        if let Some((from, to)) = &self.targets.search {
            targets.insert("search".to_string(), json!({"from": from, "to": to}));
        }
        if let Some((from, to)) = &self.targets.foundry {
            targets.insert("foundry".to_string(), json!({"from": from, "to": to}));
        }
        let value = json!({
            "project": self.project,
            "from": self.args.from,
            "to": self.args.to,
            "targets": Value::Object(targets),
            "rewiring": rewiring_table(self.plan)
                .into_iter()
                .map(|(binding, target, from, to, shared, references)| json!({
                    "binding": binding,
                    "type": target.to_string(),
                    "from": from,
                    "to": to,
                    "shared": shared,
                    "references": references,
                }))
                .collect::<Vec<_>>(),
            "renamed": renamed_rows(self.plan)
                .into_iter()
                .map(|row| json!({
                    "resource": row.label,
                    "from": row.from,
                    "to": row.to,
                    "references": row.references,
                }))
                .collect::<Vec<_>>(),
            "resources": {
                "changed": labels(items_of(self.plan, Change::Changed)),
                "new": labels(items_of(self.plan, Change::New)),
                "unchanged": labels(items_of(self.plan, Change::Unchanged)),
                "kept_only_in_to": self.plan
                    .kept_only_in_to
                    .iter()
                    .map(|(kind, stem)| format!("{}/{stem}", kind.directory_name()))
                    .collect::<Vec<_>>(),
            },
            "checks": self.checks
                .iter()
                .map(|c| json!({"ok": c.ok, "message": c.message}))
                .collect::<Vec<_>>(),
            // Everything the engine could not decide has been answered by the
            // time a preview exists; the key stays for shape stability.
            "questions": Vec::<Value>::new(),
            "dry_run": dry_run,
        });
        serde_json::to_string_pretty(&value).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One `locations/{l}/models` entry, in the shape ARM returns.
    fn offered(name: &str, version: &str, usage: &str) -> Value {
        json!({
            "kind": "OpenAI",
            "model": {
                "format": "OpenAI",
                "name": name,
                "version": version,
                "skus": [{"name": "GlobalStandard", "usageName": usage}]
            }
        })
    }

    fn deployment(model: &str, version: &str, capacity: i64) -> Value {
        json!({
            "name": model,
            "sku": {"name": "GlobalStandard", "capacity": capacity},
            "properties": {"model": {"format": "OpenAI", "name": model, "version": version}}
        })
    }

    fn usage(name: &str, current: f64, limit: f64) -> Value {
        json!({"name": {"value": name}, "currentValue": current, "limit": limit})
    }

    #[test]
    fn skill_index_and_subscription_are_parsed_from_their_paths() {
        assert_eq!(skill_index("skills[3]"), Some(3));
        assert_eq!(skill_index("skills[]"), None);
        assert_eq!(skill_index("skills.3"), None);
        assert_eq!(
            subscription_of(
                "/subscriptions/sub-a/resourceGroups/rg/providers/Microsoft.CognitiveServices/accounts/a"
            )
            .as_deref(),
            Some("sub-a")
        );
        assert_eq!(subscription_of("/resourceGroups/rg"), None);
    }

    #[test]
    fn a_model_the_region_does_not_offer_is_a_question() {
        let models = vec![offered(
            "gpt-5-mini",
            "2026-01-01",
            "OpenAI.GlobalStandard.gpt-5-mini",
        )];
        let reason = verdict(
            &deployment("gpt-5-nano", "2026-01-01", 10),
            None,
            &models,
            &[],
            "swedencentral",
        )
        .unwrap_err();
        assert!(
            reason.contains("not available in swedencentral"),
            "{reason}"
        );

        // Same model, a version the region does not have.
        let reason = verdict(
            &deployment("gpt-5-mini", "2025-01-01", 10),
            None,
            &models,
            &[],
            "swedencentral",
        )
        .unwrap_err();
        assert!(reason.contains("version 2025-01-01"), "{reason}");

        // A deployment that does not pin a version takes what is offered.
        assert!(
            verdict(
                &deployment("gpt-5-mini", "", 10),
                None,
                &models,
                &[],
                "swedencentral"
            )
            .is_ok()
        );
    }

    #[test]
    fn quota_is_short_when_the_free_headroom_is_below_the_capacity() {
        let models = vec![offered(
            "gpt-5-mini",
            "1",
            "OpenAI.GlobalStandard.gpt-5-mini",
        )];
        let usages = vec![usage("OpenAI.GlobalStandard.gpt-5-mini", 90.0, 100.0)];
        let reason = verdict(
            &deployment("gpt-5-mini", "1", 50),
            None,
            &models,
            &usages,
            "swe",
        )
        .unwrap_err();
        assert!(
            reason.contains("10 of 100 free") && reason.contains("asks for 50"),
            "{reason}"
        );
        // Exactly the headroom fits.
        let ok = verdict(
            &deployment("gpt-5-mini", "1", 10),
            None,
            &models,
            &usages,
            "swe",
        )
        .unwrap();
        assert!(ok.contains("quota ok"), "{ok}");
        // A quota the region does not report is not a question.
        let ok = verdict(
            &deployment("gpt-5-mini", "1", 10),
            None,
            &models,
            &[],
            "swe",
        )
        .unwrap();
        assert!(ok.contains("not reported"), "{ok}");
    }

    #[test]
    fn the_usage_name_comes_from_the_sku_the_deployment_asks_for() {
        let offered = json!({
            "model": {
                "name": "m",
                "version": "1",
                "skus": [
                    {"name": "Standard", "usageName": "OpenAI.Standard.m"},
                    {"name": "GlobalStandard", "usageName": "OpenAI.GlobalStandard.m"}
                ]
            }
        });
        assert_eq!(
            usage_name_of(&offered, "globalstandard").as_deref(),
            Some("OpenAI.GlobalStandard.m")
        );
        assert_eq!(usage_name_of(&offered, "ProvisionedManaged"), None);
    }

    #[test]
    fn a_capacity_increase_only_demands_the_delta_over_what_is_deployed() {
        let models = vec![offered(
            "gpt-5-mini",
            "1",
            "OpenAI.GlobalStandard.gpt-5-mini",
        )];
        let usages = vec![usage("OpenAI.GlobalStandard.gpt-5-mini", 90.0, 100.0)];
        // 50 → 55 is five more units, not fifty: the target already holds 50.
        let before = deployment("gpt-5-mini", "1", 50);
        let ok = verdict(
            &deployment("gpt-5-mini", "1", 55),
            Some(&before),
            &models,
            &usages,
            "swe",
        )
        .unwrap();
        assert!(ok.contains("asks for 5"), "{ok}");
        // The same document as a NEW deployment demands all of it.
        let reason = verdict(
            &deployment("gpt-5-mini", "1", 55),
            None,
            &models,
            &usages,
            "swe",
        )
        .unwrap_err();
        assert!(reason.contains("asks for 55"), "{reason}");
    }

    #[test]
    fn deployment_answers_are_case_insensitive_and_capacity_must_be_positive() {
        assert!(matches!(
            parse_deployment_answer("Continue"),
            Some(DeploymentAnswer::Continue)
        ));
        assert!(matches!(
            parse_deployment_answer("SKIP"),
            Some(DeploymentAnswer::Skip)
        ));
        assert!(matches!(
            parse_deployment_answer("Capacity: 20"),
            Some(DeploymentAnswer::Capacity(20))
        ));
        assert!(parse_deployment_answer("capacity:0").is_none());
        assert!(parse_deployment_answer("capacity:-3").is_none());
        assert!(parse_deployment_answer("capacity:2.5").is_none());
        assert!(parse_deployment_answer("capacity:").is_none());
        assert!(parse_deployment_answer("maybe").is_none());
    }

    #[test]
    fn a_capacity_answer_replaces_the_deployments_own() {
        let mut doc = deployment("m", "1", 50);
        set_capacity(&mut doc, 5);
        assert_eq!(doc["sku"]["capacity"], 5);
        assert_eq!(doc["sku"]["name"], "GlobalStandard", "the sku itself stays");
        let mut without = json!({"name": "m"});
        set_capacity(&mut without, 5);
        assert_eq!(without["sku"]["capacity"], 5);
    }

    fn args(from: &str, to: &str) -> PromoteArgs {
        PromoteArgs {
            project: None,
            from: from.to_string(),
            to: to.to_string(),
            dry_run: false,
            offline: true,
        }
    }

    #[test]
    fn missing_env_message_names_the_source_environments_targets() {
        let ws: rigg_core::workspace::WorkspaceConfig = serde_yaml::from_str(
            "environments:\n  dev:\n    search: { service: s-dev }\n    foundry: { account: a, project: p }\n",
        )
        .unwrap();
        let ws = Workspace {
            root: std::path::PathBuf::from("."),
            config: ws,
            projects: Vec::new(),
        };
        let message = missing_env_message(&ws, "dev", "staging");
        assert!(
            message.contains(
                "rigg env add staging --like dev --search-service s-dev --foundry-account a \
                 --foundry-project p"
            ),
            "{message}"
        );
    }

    #[test]
    fn declined_external_endpoint_is_a_usage_error_naming_env_bind() {
        let asks = vec![Ask {
            question: Question::confirm("promote.external.api.partner.example", "keep?", true),
            action: Action::External {
                host: "api.partner.example".to_string(),
            },
        }];
        let err = apply_answers(
            &args("dev", "prod"),
            &asks,
            &[Answer::Confirm(false)],
            &mut Settled::default(),
            &mut AnsweredBindings::default(),
        )
        .unwrap_err();
        let message = format!("{err}");
        assert!(
            message.contains("rigg env bind dev <name> api:https://api.partner.example"),
            "the decline must name the command that binds it: {message}"
        );
    }

    #[test]
    fn skip_answers_are_remembered_instead_of_written() {
        let mut settled = Settled::default();
        let mut answered = AnsweredBindings::default();
        let asks = vec![
            Ask {
                question: Question::text("promote.bind.dev.acct", "?"),
                action: Action::Bind {
                    physical: "acct".to_string(),
                    kind: BindingType::Storage,
                    value: "acct".to_string(),
                },
            },
            Ask {
                question: Question::choice("binding.prod.docs", "?", Vec::new()),
                action: Action::Missing {
                    binding: "docs".to_string(),
                    kind: BindingType::Storage,
                    source_value: "devacct".to_string(),
                },
            },
        ];
        apply_answers(
            &args("dev", "prod"),
            &asks,
            &[
                Answer::Text(SKIP.to_string()),
                Answer::Choice(SKIP.to_string()),
            ],
            &mut settled,
            &mut answered,
        )
        .expect("skipping writes nothing, so no workspace is touched");
        assert!(settled.unbound_skipped.contains("acct"));
        assert!(settled.binding_skipped.contains("docs"));
        assert!(
            answered.0.is_empty(),
            "a declined question records no binding"
        );
    }

    #[test]
    fn answers_are_recorded_in_memory_and_batched_per_environment_on_persist() {
        let mut answered = AnsweredBindings::default();
        let asks = vec![
            Ask {
                question: Question::text("promote.bind.dev.acct", "?"),
                action: Action::Bind {
                    physical: "acct".to_string(),
                    kind: BindingType::Storage,
                    value: "acct".to_string(),
                },
            },
            Ask {
                question: Question::choice("binding.prod.docs", "?", Vec::new()),
                action: Action::Missing {
                    binding: "docs".to_string(),
                    kind: BindingType::Storage,
                    source_value: "devacct".to_string(),
                },
            },
        ];
        apply_answers(
            &args("dev", "prod"),
            &asks,
            &[
                Answer::Text("blobs".to_string()),
                Answer::Choice(SAME.to_string()),
            ],
            &mut Settled::default(),
            &mut answered,
        )
        .expect("nothing is written until the run proceeds");
        assert_eq!(
            answered
                .0
                .iter()
                .map(|(env, name, b)| (env.as_str(), name.as_str(), b.value.as_str()))
                .collect::<Vec<_>>(),
            vec![("dev", "blobs", "acct"), ("prod", "docs", "devacct")],
        );

        // They are visible to the next round's translation without touching
        // `rigg.yaml`.
        let mut config: rigg_core::workspace::WorkspaceConfig =
            serde_yaml::from_str("environments:\n  dev: {}\n  prod: {}\n").unwrap();
        answered.apply_to(&mut config);
        assert_eq!(
            config.environments["dev"].dependencies["blobs"].value,
            "acct"
        );
        assert_eq!(
            config.environments["prod"].dependencies["docs"].value,
            "devacct"
        );
    }
}
