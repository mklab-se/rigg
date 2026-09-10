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
//! 3. show the rewiring preview — what points where after the translation,
//!    which sibling references were renamed, and what changes per resource;
//! 4. once the run proceeds, persist the answered bindings to `rigg.yaml`
//!    and write the merged documents through the target environment's
//!    `Store`.
//!
//! Answers reach `rigg.yaml` only in step 4: `--dry-run`, an abort and the
//! `needs-input` exit all leave the workspace file exactly as they found it.
//!
//! Nothing is deleted and resources that exist only in the target are never
//! touched. `--dry-run` stops after the preview.

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
    CommandError, GlobalContext, bindings, discovery, env as env_cmd, interactive, load_workspace,
    select_one_project,
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

    let project = select_one_project(&ws, args.project.as_deref())?;
    let store_to = Store::new(project, &args.to);
    let targets = Targets::of(&ws, &args);
    let checks = checks(&plan, &args, &settled);

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

    let pending_writes = plan
        .items
        .iter()
        .filter(|i| i.change() != Change::Unchanged)
        .count();

    if args.dry_run {
        if ctx.json() {
            println!("{}", preview.to_json(true));
        } else {
            println!();
            println!("(dry run — nothing written)");
        }
        return Ok(());
    }

    if pending_writes == 0 {
        // The run reached its end without aborting, so the answers are worth
        // keeping even though no document changed — otherwise the same
        // question comes back on every run.
        answered.persist()?;
        if ctx.json() {
            println!("{}", preview.to_json(false));
        } else {
            println!();
            println!(
                "nothing to promote — '{}' already matches '{}'",
                args.to, args.from
            );
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
    for item in &plan.items {
        match item.change() {
            Change::Unchanged => {}
            Change::Changed => {
                store_to.write_exact(
                    &ResourceRef::new(item.kind, item.target_name.clone()),
                    &item.merged,
                )?;
            }
            // A new resource lands at the SOURCE's stem: that is the logical
            // id the two trees correlate by.
            Change::New => {
                store_to.write_at_exact(&item.stem, item.kind, &item.merged)?;
            }
        }
    }

    if ctx.json() {
        println!("{}", preview.to_json(false));
    }
    say!(ctx);
    say!(
        ctx,
        "Promoted {pending_writes} resource(s) into '{}'.",
        args.to
    );
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
// checks
// ---------------------------------------------------------------------

struct Check {
    ok: bool,
    message: String,
}

/// Everything worth saying about the plan that is neither a rewiring, a
/// rename, nor a per-resource diff: unresolved bindings, declined questions,
/// and Web API auth carriers that did not cross.
fn checks(plan: &Plan, args: &PromoteArgs, settled: &Settled) -> Vec<Check> {
    let mut out = Vec::new();

    for item in &plan.items {
        for carrier in &item.auth {
            if let AuthCarrier::Stripped { path, .. } = carrier {
                out.push(Check {
                    ok: false,
                    message: format!(
                        "{} {path}: Web API auth carrier not carried over (it authorizes '{}') \
                         — resolved against '{}' on push",
                        item.label(),
                        args.from,
                        args.to
                    ),
                });
            }
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
            for check in self.checks {
                let mark = if check.ok {
                    "✓".green()
                } else {
                    "!".yellow()
                };
                println!("  {mark} {}", check.message);
            }
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
