//! Dependency bindings: editing `rigg.yaml`'s `dependencies` maps, and
//! *learning* bindings from the infrastructure references already present in
//! a project's files.
//!
//! `rigg env bind/unbind`, `rigg env add --like` and (workstream 2)
//! `rigg promote` all go through the helpers here so there is exactly one
//! place that writes a binding.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, anyhow, bail};
use serde_yaml::Value as Yaml;

use rigg_core::binding::{
    Binding, BindingCache, BindingType, EnvBindings, Wanted, validate_binding_name,
};
use rigg_core::infra::{self, Class, Target};
use rigg_core::store::Store;
use rigg_core::workspace::{ResolvedEnv, WORKSPACE_FILE, Workspace};

use super::ask::{Answer, Asker, Question};
use super::{CommandError, GlobalContext, load_workspace};

/// The answer that means "don't bind this" in a `learn` question.
pub const SKIP_ANSWER: &str = "skip";

/// A binding `learn` suggests for an environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    /// Proposed binding name (lower-kebab of the physical name).
    pub name: String,
    pub kind: BindingType,
    /// The value to record: the physical name, an ARM id when the file
    /// carries a bare one, or an origin (`https://host`) for `api`.
    pub value: String,
    /// Where the reference was found: `(file, json path)`.
    pub sources: Vec<(String, String)>,
}

/// Edit `rigg.yaml` in place. The file is re-serialized from the parsed
/// document, so **no** comment in it survives — the header `rigg init`
/// writes is regenerated here so at least that one is never lost.
pub fn edit_workspace_yaml(edit: impl FnOnce(&mut Yaml) -> Result<()>) -> Result<()> {
    let ws = load_workspace()?;
    let path = ws.root.join(WORKSPACE_FILE);
    let text = std::fs::read_to_string(&path)?;
    let mut doc: Yaml = serde_yaml::from_str(&text)?;
    edit(&mut doc)?;
    let body = serde_yaml::to_string(&doc)?;
    let root = doc.get("root").and_then(Yaml::as_str);
    std::fs::write(&path, format!("{}{body}", workspace_yaml_header(root)))?;
    Ok(())
}

/// The header comment `rigg init` puts at the top of `rigg.yaml` (kept in
/// step with `init.rs`), for `<root>` when the workspace has a `root:`.
fn workspace_yaml_header(root: Option<&str>) -> String {
    let where_ = match root {
        Some(sub) => format!("{sub}/projects/<name>/"),
        None => "projects/<name>/".to_string(),
    };
    format!(
        "# Rigg workspace configuration.\n\
         # Resource definitions live in {where_} — see `rigg new project`.\n"
    )
}

/// The `environments:` mapping, created when missing.
pub fn envs_mut(doc: &mut Yaml) -> Result<&mut serde_yaml::Mapping> {
    let map = doc
        .as_mapping_mut()
        .ok_or_else(|| anyhow!("invalid rigg.yaml"))?;
    let envs = map
        .entry("environments".into())
        .or_insert_with(|| Yaml::Mapping(Default::default()));
    envs.as_mapping_mut()
        .ok_or_else(|| anyhow!("`environments` must be a mapping"))
}

/// The YAML shape of one binding: `{ "<type>": "<value>" }`.
pub fn binding_yaml(binding: &Binding) -> Yaml {
    let mut map = serde_yaml::Mapping::new();
    map.insert(
        Yaml::String(binding.kind.to_string()),
        Yaml::String(binding.value.clone()),
    );
    Yaml::Mapping(map)
}

/// Add (or replace) `env`'s `<name>` binding in `rigg.yaml`.
pub fn write_binding(env: &str, name: &str, binding: &Binding) -> Result<()> {
    write_bindings(env, &[(name.to_string(), binding.clone())])
}

/// Add (or replace) several bindings in one `rigg.yaml` edit.
pub fn write_bindings(env: &str, bindings: &[(String, Binding)]) -> Result<()> {
    for (name, _) in bindings {
        validate_binding_name(name).map_err(|e| anyhow!(CommandError::Usage(e)))?;
    }
    edit_workspace_yaml(|doc| {
        let envs = envs_mut(doc)?;
        let env_map = envs
            .get_mut(env)
            .ok_or_else(|| anyhow!(CommandError::Usage(format!("unknown environment '{env}'"))))?
            .as_mapping_mut()
            .ok_or_else(|| anyhow!("environment '{env}' is not a mapping"))?;
        let deps = env_map
            .entry("dependencies".into())
            .or_insert_with(|| Yaml::Mapping(Default::default()))
            .as_mapping_mut()
            .ok_or_else(|| anyhow!("`dependencies` must be a mapping"))?;
        for (name, binding) in bindings {
            deps.insert(Yaml::String(name.clone()), binding_yaml(binding));
        }
        Ok(())
    })
}

/// Remove `env`'s `<name>` binding, dropping an empty `dependencies:` key.
pub fn remove_binding(env: &str, name: &str) -> Result<()> {
    edit_workspace_yaml(|doc| {
        let envs = envs_mut(doc)?;
        let env_map = envs
            .get_mut(env)
            .ok_or_else(|| anyhow!(CommandError::Usage(format!("unknown environment '{env}'"))))?
            .as_mapping_mut()
            .ok_or_else(|| anyhow!("environment '{env}' is not a mapping"))?;
        let empty = {
            let deps = env_map
                .get_mut("dependencies")
                .and_then(Yaml::as_mapping_mut)
                .ok_or_else(|| {
                    anyhow!(CommandError::Usage(format!(
                        "environment '{env}' has no binding '{name}'"
                    )))
                })?;
            if deps.remove(name).is_none() {
                bail!(CommandError::Usage(format!(
                    "environment '{env}' has no binding '{name}'"
                )));
            }
            deps.is_empty()
        };
        if empty {
            env_map.remove("dependencies");
        }
        Ok(())
    })
}

/// Parse a `<type>:<value>` binding argument.
pub fn parse_binding(spec: &str) -> Result<Binding> {
    let (kind, value) = spec.split_once(':').ok_or_else(|| {
        anyhow!(CommandError::Usage(format!(
            "invalid binding '{spec}': expected <type>:<value>, e.g. storage:mklabstorageacc"
        )))
    })?;
    let kind: BindingType = kind
        .trim()
        .parse()
        .map_err(|e: String| anyhow!(CommandError::Usage(e)))?;
    let value = value.trim();
    if value.is_empty() {
        bail!(CommandError::Usage(format!(
            "invalid binding '{spec}': the value is empty"
        )));
    }
    Ok(Binding {
        kind,
        value: value.to_string(),
    })
}

/// Parse a `--bind <name>=<type>:<value>` flag.
pub fn parse_bind_flag(flag: &str) -> Result<(String, Binding)> {
    let (name, spec) = flag.split_once('=').ok_or_else(|| {
        anyhow!(CommandError::Usage(format!(
            "invalid --bind '{flag}': expected <name>=<type>:<value>"
        )))
    })?;
    validate_binding_name(name).map_err(|e| anyhow!(CommandError::Usage(e)))?;
    Ok((name.to_string(), parse_binding(spec)?))
}

/// Lower-kebab a physical name or host: everything outside `[a-z0-9]`
/// becomes `-`, repeats collapse, leading/trailing `-` are trimmed.
fn kebab(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// The origin (`scheme://host[:port]`) of a URL, for `api` proposals.
fn origin(url: &str) -> String {
    match url.split_once("://") {
        Some((scheme, rest)) => {
            let host = rest.split('/').next().unwrap_or(rest);
            format!("{scheme}://{host}")
        }
        None => url.to_string(),
    }
}

/// Propose bindings for every infrastructure reference in `env`'s files
/// that no binding of this environment already covers.
///
/// Names are the physical name lower-kebabed (for `api`, the host's first
/// label); a name that would collide with an existing binding of another
/// type or value — or with an earlier proposal — gets a `-2`, `-3`, … suffix.
pub fn learn(ws: &Workspace, env_name: &str, env_bindings: &EnvBindings) -> Result<Vec<Proposal>> {
    let mut grouped: BTreeMap<(BindingType, String), Proposal> = BTreeMap::new();

    for project in &ws.projects {
        if !Store::envs_of(project).contains(&env_name.to_string()) {
            continue;
        }
        let store = Store::new(project, env_name);
        let Ok(list) = store.list() else { continue };
        for (r, path) in list {
            let Ok(value) = store.read(&r) else { continue };
            let refs = infra::extract(r.kind, &value);
            if refs.is_empty() {
                continue;
            }
            for classified in infra::classify(env_bindings, &[], refs) {
                // Bound/Shared: already covered here. Leak can't occur —
                // no other environments are passed in.
                if !matches!(classified.class, Class::Unbound | Class::External) {
                    continue;
                }
                let found = classified.found;
                let Some(kind) = infra::binding_type_for(found.physical.target) else {
                    continue;
                };
                let value = proposed_value(kind, &found);
                let source = (path.display().to_string(), found.path.clone());
                grouped
                    .entry((kind, value.to_lowercase()))
                    .or_insert_with(|| Proposal {
                        name: String::new(),
                        kind,
                        value,
                        sources: Vec::new(),
                    })
                    .sources
                    .push(source);
            }
        }
    }

    let mut taken: Vec<String> = env_bindings.iter().map(|e| e.name.clone()).collect();
    let mut proposals: Vec<Proposal> = Vec::new();
    for ((kind, _), mut proposal) in grouped {
        let base = match kind {
            // `api` values are origins; name after the host's first label.
            BindingType::Api => kebab(
                origin(&proposal.value)
                    .split_once("://")
                    .map(|(_, host)| host)
                    .unwrap_or(&proposal.value)
                    .split('.')
                    .next()
                    .unwrap_or_default(),
            ),
            _ => kebab(&physical_of(&proposal)),
        };
        let base = if base.is_empty() {
            kind.to_string()
        } else {
            base
        };
        let mut name = base.clone();
        let mut n = 1;
        while validate_binding_name(&name).is_err() || taken.contains(&name) {
            n += 1;
            name = format!("{base}-{n}");
        }
        taken.push(name.clone());
        proposal.name = name;
        proposals.push(proposal);
    }
    Ok(proposals)
}

/// The physical name a proposal points at (its value, or the ARM id's last
/// segment when the value is an id).
fn physical_of(proposal: &Proposal) -> String {
    Binding {
        kind: proposal.kind,
        value: proposal.value.clone(),
    }
    .physical_name()
}

/// What to record as a proposal's value: an origin for `api`, otherwise the
/// full ARM id when the file carries one — spliced out of a storage
/// connection string's `ResourceId=` or read from an identity's
/// `userAssignedIdentity` — and the bare physical name only when it does
/// not. Keeping the id means the binding needs no ARM by-name lookup (and
/// no guess about which subscription the resource lives in).
fn proposed_value(kind: BindingType, found: &infra::FoundRef) -> String {
    if kind == BindingType::Api {
        return origin(found.physical.original.as_str().unwrap_or_default());
    }
    let original = &found.physical.original;
    let arm_id = match found.physical.target {
        Target::Storage => original.as_str().and_then(storage_resource_id),
        Target::Identity => original
            .get("userAssignedIdentity")
            .and_then(serde_json::Value::as_str)
            .map(|s| s.trim_end_matches('/').to_string()),
        _ => original
            .as_str()
            .filter(|s| s.starts_with("/subscriptions/"))
            .map(|s| s.trim_end_matches('/').to_string()),
    };
    arm_id
        .filter(|id| id.starts_with("/subscriptions/"))
        .unwrap_or_else(|| found.physical.physical.clone())
}

/// The ARM id inside a storage connection string's `ResourceId=` (located
/// case-insensitively anywhere, like `infra::parse`), without its trailing
/// `/` and without any `;`-separated tail.
fn storage_resource_id(conn: &str) -> Option<String> {
    let lower = conn.to_ascii_lowercase();
    let start = lower.find("resourceid=")? + "resourceid=".len();
    let rest = &conn[start..];
    let id = rest.split(';').next().unwrap_or(rest).trim_end_matches('/');
    (!id.is_empty()).then(|| id.to_string())
}

/// One `learn.<env>.<name>` question per proposal: a text answer naming the
/// binding (defaulting to the proposed name), or `skip`.
pub fn proposals_to_questions(env: &str, proposals: &[Proposal]) -> Vec<Question> {
    proposals
        .iter()
        .map(|p| {
            Question::text(
                format!("learn.{env}.{}", p.name),
                format!(
                    "Name for the {} '{}' (or '{SKIP_ANSWER}'):",
                    p.kind, p.value
                ),
            )
            .with_default(p.name.clone())
        })
        .collect()
}

/// Turn the answers to [`proposals_to_questions`] into the bindings to
/// write, dropping every proposal answered `skip` (or with an empty name).
pub fn answers_to_bindings(
    proposals: &[Proposal],
    answers: &[Answer],
) -> Result<Vec<(String, Binding)>> {
    if proposals.len() != answers.len() {
        return Err(anyhow!(CommandError::Usage(format!(
            "got {} answer(s) for {} binding proposal(s) — every proposal needs exactly one \
             answer (use '{SKIP_ANSWER}' to decline)",
            answers.len(),
            proposals.len()
        ))));
    }
    let mut out = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (p, answer) in proposals.iter().zip(answers) {
        let raw = answer.as_str().unwrap_or_default().trim();
        if raw.is_empty() || raw.eq_ignore_ascii_case(SKIP_ANSWER) {
            continue;
        }
        validate_binding_name(raw).map_err(|e| anyhow!(CommandError::Usage(e)))?;
        if !seen.insert(raw.to_string()) {
            return Err(anyhow!(CommandError::Usage(format!(
                "two proposals were both renamed to '{raw}' — bindings need distinct names"
            ))));
        }
        out.push((
            raw.to_string(),
            Binding {
                kind: p.kind,
                value: p.value.clone(),
            },
        ));
    }
    Ok(out)
}

/// Ask `asker` about every proposal and return the bindings to write.
pub fn resolve_proposals(
    asker: &mut dyn Asker,
    env: &str,
    proposals: &[Proposal],
) -> Result<Vec<(String, Binding)>> {
    if proposals.is_empty() {
        return Ok(Vec::new());
    }
    let questions = proposals_to_questions(env, proposals);
    let answers = asker.ask_all(&questions)?;
    answers_to_bindings(proposals, &answers)
}

/// Every environment except `env_name`, as binding tables — the "shared
/// with" column's input.
pub fn other_env_bindings(ws: &Workspace, env_name: &str) -> Vec<EnvBindings> {
    ws.config
        .environments
        .iter()
        .filter(|(name, _)| name.as_str() != env_name)
        .map(|(name, env)| EnvBindings::of_env(name, env, None))
        .collect()
}

/// The environments that bind the same *physical* resource as `binding`.
///
/// Sharing is one thing everywhere: the same physical value competing for the
/// same [`Wanted`] that `infra::classify` uses — so an `ai-services` binding
/// and another environment's implicit `foundry` target do count as shared,
/// while the binding *names* need not match.
pub fn shared_with(others: &[EnvBindings], binding: &Binding) -> Vec<String> {
    others
        .iter()
        .filter(|o| {
            o.find_physical(Wanted::for_binding(binding.kind), &binding.physical_name())
                .is_some()
        })
        .map(|o| o.env.clone())
        .collect()
}

/// Offer to record any bindings [`learn`] finds newly learnable in `env`,
/// e.g. after `rigg adopt` or `rigg pull` write new project files.
///
/// Interactively: print the proposal table and ask whether to record them
/// (default yes); a "no" leaves `rigg.yaml` untouched. Non-interactively
/// (including `--output json`, where stdout must stay clean of prose): a
/// one-line hint on stderr pointing at `rigg env bind <env> --learn`.
pub fn offer_to_learn(ctx: &GlobalContext, ws: &Workspace, env: &ResolvedEnv) -> Result<()> {
    let cache = BindingCache::load(ws, &env.name);
    let table = EnvBindings::of_env(&env.name, &env.env, Some(&cache));
    let proposals = learn(ws, &env.name, &table)?;
    if proposals.is_empty() {
        return Ok(());
    }

    if ctx.interactive() {
        println!();
        println!("Infrastructure references not yet bound in '{}':", env.name);
        println!("  {:<20} {:<12} value", "name", "type");
        for p in &proposals {
            let source = p
                .sources
                .first()
                .map(|(file, path)| format!("  (from {file}:{path})"))
                .unwrap_or_default();
            println!("  {:<20} {:<12} {}{source}", p.name, p.kind, p.value);
        }
        let mut asker = ctx.asker("learn", serde_json::json!({"env": env.name}));
        let record = asker
            .ask(&Question::confirm(
                format!("learn.{}.record", env.name),
                "Record these bindings in rigg.yaml?",
                true,
            ))?
            .as_bool()
            .unwrap_or(false);
        if record {
            let to_write: Vec<(String, Binding)> = proposals
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
                .collect();
            write_bindings(&env.name, &to_write)?;
        }
    } else {
        eprintln!(
            "hint: {} infrastructure reference(s) are not bound in '{}' — run `rigg env bind {} --learn` to record them",
            proposals.len(),
            env.name,
            env.name
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kebab_and_origin_shape_proposal_names() {
        assert_eq!(kebab("MKLab_Storage.01"), "mklab-storage-01");
        assert_eq!(kebab("--weird--"), "weird");
        assert_eq!(
            origin("https://api.partner.example/v1/x"),
            "https://api.partner.example"
        );
        assert_eq!(origin("nonsense"), "nonsense");
    }

    #[test]
    fn parse_binding_accepts_urls_and_rejects_unknown_types() {
        let b = parse_binding("api:https://api.partner.example/v1").unwrap();
        assert_eq!(b.kind, BindingType::Api);
        assert_eq!(b.value, "https://api.partner.example/v1");
        assert!(parse_binding("cosmos:x").is_err());
        assert!(parse_binding("storage:").is_err());
        assert!(parse_binding("storage").is_err());
        let (name, b) = parse_bind_flag("docs=storage:acct").unwrap();
        assert_eq!(name, "docs");
        assert_eq!(b.value, "acct");
        assert!(parse_bind_flag("search=storage:acct").is_err());
    }

    #[test]
    fn proposed_value_keeps_the_arm_id_a_file_already_carries() {
        // storage: the id is spliced out of the connection string, whatever
        // sits before or after it.
        let conn = serde_json::json!(
            "AccountName=x;ResourceId=/subscriptions/S/resourceGroups/RG/providers/Microsoft.Storage/storageAccounts/acct/;Database=d"
        );
        let found = infra::parse(rigg_core::registry::InfraForm::StorageResourceId, &conn)
            .map(|physical| infra::FoundRef {
                path: "credentials.connectionString".into(),
                form: rigg_core::registry::InfraForm::StorageResourceId,
                physical,
            })
            .unwrap();
        assert_eq!(
            proposed_value(BindingType::Storage, &found),
            "/subscriptions/S/resourceGroups/RG/providers/Microsoft.Storage/storageAccounts/acct"
        );

        // identity: the id is the object's `userAssignedIdentity`.
        let id = serde_json::json!({
            "@odata.type": "#Microsoft.Azure.Search.DataUserAssignedIdentity",
            "userAssignedIdentity": "/subscriptions/S/resourcegroups/RG/providers/Microsoft.ManagedIdentity/userAssignedIdentities/rigg-dev"
        });
        let found = infra::parse(rigg_core::registry::InfraForm::UserAssignedIdentity, &id)
            .map(|physical| infra::FoundRef {
                path: "identity".into(),
                form: rigg_core::registry::InfraForm::UserAssignedIdentity,
                physical,
            })
            .unwrap();
        assert_eq!(
            proposed_value(BindingType::Identity, &found),
            "/subscriptions/S/resourcegroups/RG/providers/Microsoft.ManagedIdentity/userAssignedIdentities/rigg-dev"
        );

        // no id in the file → the bare physical name, as before.
        let host = serde_json::json!("https://devaisrvc.openai.azure.com");
        let found = infra::parse(rigg_core::registry::InfraForm::OpenAiEndpoint, &host)
            .map(|physical| infra::FoundRef {
                path: "resourceUri".into(),
                form: rigg_core::registry::InfraForm::OpenAiEndpoint,
                physical,
            })
            .unwrap();
        assert_eq!(proposed_value(BindingType::AiServices, &found), "devaisrvc");
    }

    #[test]
    fn storage_resource_id_is_located_anywhere_and_trimmed() {
        assert_eq!(
            storage_resource_id("resourceid=/subscriptions/s/x/;Database=d").as_deref(),
            Some("/subscriptions/s/x")
        );
        assert_eq!(storage_resource_id("AccountName=x"), None);
    }

    #[test]
    fn workspace_yaml_header_matches_init() {
        assert_eq!(
            workspace_yaml_header(None),
            "# Rigg workspace configuration.\n# Resource definitions live in projects/<name>/ — see `rigg new project`.\n"
        );
        assert!(
            workspace_yaml_header(Some("cfg"))
                .contains("live in cfg/projects/<name>/ — see `rigg new project`.")
        );
    }

    #[test]
    fn answers_must_match_the_proposals_one_for_one() {
        let proposals = vec![Proposal {
            name: "acct".into(),
            kind: BindingType::Storage,
            value: "acct".into(),
            sources: vec![],
        }];
        let err = answers_to_bindings(&proposals, &[]).unwrap_err();
        assert!(err.to_string().contains("0 answer(s) for 1"), "{err}");
    }

    #[test]
    fn answers_skip_and_rename_proposals() {
        let proposals = vec![
            Proposal {
                name: "acct".into(),
                kind: BindingType::Storage,
                value: "acct".into(),
                sources: vec![],
            },
            Proposal {
                name: "other".into(),
                kind: BindingType::Storage,
                value: "other".into(),
                sources: vec![],
            },
        ];
        let qs = proposals_to_questions("dev", &proposals);
        assert_eq!(qs[0].id, "learn.dev.acct");
        assert_eq!(qs[0].default.as_deref(), Some("acct"));
        let out = answers_to_bindings(
            &proposals,
            &[
                Answer::Text("docs".into()),
                Answer::Text(SKIP_ANSWER.into()),
            ],
        )
        .unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, "docs");
        assert_eq!(out[0].1.value, "acct");
    }

    #[test]
    fn answers_rejects_two_proposals_renamed_to_the_same_name() {
        let proposals = vec![
            Proposal {
                name: "acct".into(),
                kind: BindingType::Storage,
                value: "acct".into(),
                sources: vec![],
            },
            Proposal {
                name: "other".into(),
                kind: BindingType::AiServices,
                value: "other".into(),
                sources: vec![],
            },
        ];
        let err = answers_to_bindings(
            &proposals,
            &[Answer::Text("docs".into()), Answer::Text("docs".into())],
        )
        .unwrap_err();
        assert!(err.to_string().contains("docs"), "{err}");
    }
}
