//! Translating one environment's resource documents into another's —
//! the engine behind `rigg promote` (spec
//! `docs/superpowers/specs/2026-09-09-promote-v2-design.md` §2).
//!
//! [`translate`] takes both environments as [`EnvDocs`] (bindings + the
//! project's documents, correlated by LOGICAL id — the file stem — never by
//! physical name) and produces a [`Plan`]: for every source document, the
//! document the target environment should have. Five rules, in order:
//!
//! 1. **Local annotations and auth carriers are stripped** — `x-rigg-pin`
//!    belongs to the target's file, and a WebApiSkill's key/`authResourceId`/
//!    `x-rigg-auth` authorizes the SOURCE environment's function app and must
//!    never cross ([`AuthCarrier::Stripped`]).
//! 2. **Infrastructure translation** — every `registry::InfraRef` value is
//!    parsed to a physical resource, mapped to the binding name it has in the
//!    source, and re-rendered from the target's binding of the same name
//!    ([`Rewire`]). Same physical value on both sides = `shared`: no change,
//!    still reported.
//! 3. **Sibling translation** — registry reference fields, `x-rigg-ref`
//!    annotations and the knowledge-base name inside an MCP URL follow a
//!    sibling that is physically named differently in the target
//!    ([`Renamed`]).
//! 4. **Kept from the target** — its `name`, every path its `x-rigg-pin`
//!    lists (with the array semantics of [`registry::restore_path`]) and the
//!    annotation itself; plus its own Web API auth carriers
//!    ([`AuthCarrier::Kept`]).
//! 5. **Everything else comes from the source** — that is the promotion.
//!
//! Anything translation cannot decide becomes a [`Pending`] question instead
//! of a guess; the value is then left exactly as the source had it.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::binding::{
    BindingEntry, BindingKind, BindingType, BindingValue, EnvBindings, RESERVED_BINDING_NAMES,
};
use crate::infra::{self, RenderTarget, Target};
use crate::registry::{self, X_RIGG_AUTH, X_RIGG_AUTH_FUNCTION_KEY, X_RIGG_PIN, X_RIGG_REF};
use crate::resources::ResourceKind;

/// The `x-functions-key` header a WebApiSkill uses to carry a function key.
const FUNCTION_KEY_HEADER: &str = "x-functions-key";
/// The placeholder Azure itself returns in place of a redacted key — and
/// what a file records for a key that lives in ARM, never on disk.
const REDACTED_KEY: &str = "<redacted>";

/// Where one infrastructure reference or binding is used:
/// `(kind, file stem, path within the document)`.
pub type Usage = (ResourceKind, String, String);

/// One environment's side of a promotion: its bindings and the project's
/// documents in it.
#[derive(Debug, Clone)]
pub struct EnvDocs {
    pub env: String,
    pub bindings: EnvBindings,
    pub docs: Vec<Doc>,
}

/// One resource document, identified logically by `stem` (its file name) and
/// physically by `physical` (its `name` field, when it has one).
#[derive(Debug, Clone)]
pub struct Doc {
    pub kind: ResourceKind,
    pub stem: String,
    pub physical: String,
    pub body: Value,
}

/// One infrastructure reference re-pointed at the target environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rewire {
    /// Concrete path in the document, e.g. `skills[0].uri`.
    pub path: String,
    /// The binding name both environments know this resource by.
    pub binding: String,
    pub target: Target,
    /// The source binding's physical name.
    pub from: String,
    /// The target binding's physical name.
    pub to: String,
    /// Both environments point at the same physical resource.
    pub shared: bool,
}

/// One reference rewritten to follow a sibling that is physically named
/// differently in the target environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Renamed {
    /// The reference field's path (registry syntax), `x-rigg-ref`, or the
    /// concrete path of the MCP URL carrying a knowledge-base name.
    pub path: String,
    /// The kind of the sibling being referenced.
    pub kind: ResourceKind,
    /// The sibling's logical id (file stem).
    pub stem: String,
    pub from: String,
    pub to: String,
}

/// What happened to one skill's Web API auth carrier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthCarrier {
    /// The target's existing carrier was kept, on the merged document's
    /// `skills[i]` named by `path`.
    Kept { path: String },
    /// The source's carrier was removed (it authorizes the source
    /// environment's function app). `used_key` records that the source
    /// authenticated with a function key, so the target's carrier can be
    /// re-derived in the same shape.
    Stripped { path: String, used_key: bool },
}

/// How an [`Item`] relates to what the target environment has today.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    New,
    Changed,
    Unchanged,
}

/// One logical resource's translation.
#[derive(Debug, Clone)]
pub struct Item {
    pub kind: ResourceKind,
    pub stem: String,
    /// The physical name the document will have in the target environment.
    pub target_name: String,
    pub is_new: bool,
    /// The target's current document, when it has one.
    pub before: Option<Value>,
    pub merged: Value,
    pub rewired: Vec<Rewire>,
    pub renamed: Vec<Renamed>,
    pub auth: Vec<AuthCarrier>,
    /// The paths the target's own `x-rigg-pin` asked to keep, restored from
    /// its document (sorted, de-duplicated). The target's `name` and the
    /// annotation itself are always kept and are not listed here — this is
    /// the user's pin list, not rigg's.
    pub pinned: Vec<String>,
}

impl Item {
    pub fn label(&self) -> String {
        format!("{}/{}", self.kind.directory_name(), self.stem)
    }

    /// New, changed, or unchanged relative to the target's current document
    /// (semantic comparison — key order and nulls do not count).
    pub fn change(&self) -> Change {
        match &self.before {
            None => Change::New,
            Some(before) => {
                if rigg_diff::semantic::diff(before, &self.merged, "name").is_equal {
                    Change::Unchanged
                } else {
                    Change::Changed
                }
            }
        }
    }
}

/// Something translation cannot decide on its own. Each is a question in the
/// interaction model's protocol (spec §3); nothing is guessed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pending {
    /// A value in the source names a physical resource no source binding
    /// covers — there is nothing to map it through.
    UnboundInSource {
        kind: ResourceKind,
        stem: String,
        path: String,
        target: Target,
        physical: String,
        proposed_name: String,
    },
    /// The source binding has no counterpart in the target environment.
    /// `binding_type` is `None` for the implicit `search`/`foundry` targets.
    MissingInTarget {
        binding: String,
        binding_type: Option<BindingType>,
        source_physical: String,
        used_by: Vec<Usage>,
    },
    /// The target binding exists but is declared by name only (or otherwise
    /// lacks what the value's shape needs, e.g. a full ARM id or a base URL):
    /// `rigg env show <to> --refresh`, or declare the ARM id.
    /// `binding_type` is `None` for the implicit `search`/`foundry` targets.
    UnresolvedTarget {
        binding: String,
        binding_type: Option<BindingType>,
        used_by: Vec<Usage>,
    },
    /// An external endpoint bound in neither environment.
    External { host: String, used_by: Vec<Usage> },
}

impl Pending {
    fn sort_key(&self) -> (u8, String, String) {
        match self {
            Pending::UnboundInSource {
                kind, stem, path, ..
            } => (0, format!("{}/{stem}", kind.directory_name()), path.clone()),
            Pending::MissingInTarget { binding, .. } => (1, binding.clone(), String::new()),
            Pending::UnresolvedTarget { binding, .. } => (2, binding.clone(), String::new()),
            Pending::External { host, .. } => (3, host.clone(), String::new()),
        }
    }
}

/// The whole translation of one project from `from` to `to`.
#[derive(Debug, Clone)]
pub struct Plan {
    pub from: String,
    pub to: String,
    /// Every source resource, in (kind, stem) order — new, changed and
    /// unchanged alike; ask [`Item::change`] which.
    pub items: Vec<Item>,
    /// Resources only the target has. Never touched by promote.
    pub kept_only_in_to: Vec<(ResourceKind, String)>,
    pub pending: Vec<Pending>,
}

/// Translate every document of `source` into the document `target` should
/// have. Pure and offline: no Azure calls, no file I/O.
pub fn translate(source: &EnvDocs, target: &EnvDocs) -> Plan {
    let target_docs: BTreeMap<(ResourceKind, &str), &Doc> = target
        .docs
        .iter()
        .map(|d| ((d.kind, d.stem.as_str()), d))
        .collect();
    let renames = Renames::build(source, target);
    let mut pending = PendingSet::default();

    let mut ordered: Vec<&Doc> = source.docs.iter().collect();
    ordered.sort_by(|a, b| {
        kind_order(a.kind)
            .cmp(&kind_order(b.kind))
            .then_with(|| a.stem.cmp(&b.stem))
    });

    let mut items = Vec::with_capacity(ordered.len());
    for doc in ordered {
        let target_doc = target_docs.get(&(doc.kind, doc.stem.as_str())).copied();
        let before = target_doc.map(|d| &d.body);
        let mut merged = doc.body.clone();
        // The annotation lives in the TARGET's file; a source-side copy must
        // not leak. The target's own is restored by `keep_from_target`.
        if let Some(map) = merged.as_object_mut() {
            map.remove(X_RIGG_PIN);
        }
        let mut auth = strip_source_auth(&mut merged);
        let (rewired, mut renamed) =
            rewire_infra(doc, &mut merged, source, target, &renames, &mut pending);
        renamed.extend(rename_siblings(doc.kind, &mut merged, &renames));
        let mut pinned = Vec::new();
        if let Some(target_doc) = target_doc {
            pinned = keep_from_target(&mut merged, target_doc);
            auth.extend(keep_target_auth(&mut merged, &target_doc.body));
        }
        // The target's physical identity always wins; only a brand-new
        // resource is named by the source (or, failing that, by its stem).
        let target_name = match target_doc {
            Some(target_doc) => target_doc.physical.clone(),
            None => merged
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(&doc.stem)
                .to_string(),
        };
        items.push(Item {
            kind: doc.kind,
            stem: doc.stem.clone(),
            target_name,
            is_new: target_doc.is_none(),
            before: before.cloned(),
            merged,
            rewired,
            renamed,
            auth,
            pinned,
        });
    }

    let source_ids: BTreeSet<(ResourceKind, &str)> = source
        .docs
        .iter()
        .map(|d| (d.kind, d.stem.as_str()))
        .collect();
    let mut kept_only_in_to: Vec<(ResourceKind, String)> = target
        .docs
        .iter()
        .filter(|d| !source_ids.contains(&(d.kind, d.stem.as_str())))
        .map(|d| (d.kind, d.stem.clone()))
        .collect();
    kept_only_in_to.sort();
    kept_only_in_to.dedup();

    Plan {
        from: source.env.clone(),
        to: target.env.clone(),
        items,
        kept_only_in_to,
        pending: pending.into_sorted(),
    }
}

/// One row per binding the plan rewires: `(binding, target, from, to,
/// shared, reference count)`, ordered by binding name. The binding's
/// [`Target`] is the first one seen — an `ai-services` binding can serve
/// both `ModelHost` and `AiServices` references, and the preview shows one
/// row per binding.
pub fn rewiring_table(plan: &Plan) -> Vec<(String, Target, String, String, bool, usize)> {
    let mut rows: BTreeMap<String, (Target, String, String, bool, usize)> = BTreeMap::new();
    for rewire in plan.items.iter().flat_map(|i| &i.rewired) {
        rows.entry(rewire.binding.clone())
            .and_modify(|row| row.4 += 1)
            .or_insert((
                rewire.target,
                rewire.from.clone(),
                rewire.to.clone(),
                rewire.shared,
                1,
            ));
    }
    rows.into_iter()
        .map(|(binding, (target, from, to, shared, refs))| {
            (binding, target, from, to, shared, refs)
        })
        .collect()
}

/// A binding name proposed for an unbound physical resource: lower-kebab of
/// the name (same rule as `rigg env learn`), never a reserved name.
pub fn proposed_binding_name(physical: &str) -> String {
    let mut name = String::with_capacity(physical.len());
    for c in physical.chars() {
        if c.is_ascii_alphanumeric() {
            name.push(c.to_ascii_lowercase());
        } else if !name.ends_with('-') {
            name.push('-');
        }
    }
    let mut name = name.trim_matches('-').to_string();
    if name.is_empty() {
        name = "binding".to_string();
    }
    if RESERVED_BINDING_NAMES.contains(&name.as_str()) {
        name.push_str("-1");
    }
    name
}

// ---------------------------------------------------------------------
// step 1 — strip what belongs to the source environment only
// ---------------------------------------------------------------------

/// Remove every skill's Web API auth carrier: the key in the URI, the
/// `x-functions-key` header, `authResourceId` and the `x-rigg-auth`
/// annotation. Each authorizes the SOURCE environment's function app; the
/// target's carrier is kept (or re-derived) instead.
fn strip_source_auth(merged: &mut Value) -> Vec<AuthCarrier> {
    let mut out = Vec::new();
    let Some(skills) = merged.get_mut("skills").and_then(Value::as_array_mut) else {
        return out;
    };
    for (i, skill) in skills.iter_mut().enumerate() {
        let Some(map) = skill.as_object_mut() else {
            continue;
        };
        let uri = map
            .get("uri")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let key_in_uri = has_code_param(&uri);
        let key_header = map
            .get("httpHeaders")
            .and_then(|h| h.get(FUNCTION_KEY_HEADER))
            .is_some();
        let carrier = key_in_uri
            || key_header
            || map.contains_key("authResourceId")
            || map.contains_key(X_RIGG_AUTH);
        if !carrier {
            continue;
        }
        if key_in_uri {
            map.insert("uri".to_string(), Value::String(strip_code_param(&uri)));
        }
        if key_header && let Some(Value::Object(headers)) = map.get_mut("httpHeaders") {
            headers.remove(FUNCTION_KEY_HEADER);
            if headers.is_empty() {
                map.remove("httpHeaders");
            }
        }
        map.remove("authResourceId");
        map.remove(X_RIGG_AUTH);
        out.push(AuthCarrier::Stripped {
            path: format!("skills[{i}]"),
            used_key: key_in_uri || key_header,
        });
    }
    out
}

fn has_code_param(uri: &str) -> bool {
    uri.split_once('?')
        .is_some_and(|(_, query)| query.split('&').any(is_code_param))
}

/// Put `key` into the uri's `code` query parameter, replacing any existing
/// one. The inverse of [`strip_code_param`]; shared with the CLI's
/// credential plumbing so a key lands the same way everywhere.
pub fn set_code_param(uri: &str, key: &str) -> String {
    let (base, query) = match uri.split_once('?') {
        Some((b, q)) => (b, q),
        None => (uri, ""),
    };
    let mut params: Vec<String> = query
        .split('&')
        .filter(|p| !p.is_empty() && !is_code_param(p))
        .map(str::to_string)
        .collect();
    params.push(format!("code={key}"));
    format!("{base}?{}", params.join("&"))
}

fn strip_code_param(uri: &str) -> String {
    let Some((base, query)) = uri.split_once('?') else {
        return uri.to_string();
    };
    let kept: Vec<&str> = query.split('&').filter(|p| !is_code_param(p)).collect();
    if kept.is_empty() {
        base.to_string()
    } else {
        format!("{base}?{}", kept.join("&"))
    }
}

fn is_code_param(param: &str) -> bool {
    param
        .split_once('=')
        .is_some_and(|(k, _)| k.eq_ignore_ascii_case("code"))
}

// ---------------------------------------------------------------------
// step 2 — infrastructure translation
// ---------------------------------------------------------------------

fn rewire_infra(
    doc: &Doc,
    merged: &mut Value,
    source: &EnvDocs,
    target: &EnvDocs,
    renames: &Renames,
    pending: &mut PendingSet,
) -> (Vec<Rewire>, Vec<Renamed>) {
    let mut rewired = Vec::new();
    let mut renamed = Vec::new();

    for found in infra::extract(doc.kind, merged) {
        let kind_of_target = found.physical.target;
        let physical = found.physical.physical.clone();
        let usage: Usage = (doc.kind, doc.stem.clone(), found.path.clone());

        // `Api` references are matched by URL prefix, exactly as
        // `infra::classify` matches them — never by host equality, or a
        // binding scoped to `https://api.x/v1` would claim `…/v2/enrich`.
        let src_binding = if kind_of_target == Target::Api {
            let ref_url = found.physical.original.as_str().unwrap_or_default();
            infra::find_api_binding(&source.bindings, ref_url)
        } else {
            source
                .bindings
                .find_physical(infra::wanted_for(kind_of_target), &physical)
        };
        let Some(src_entry) = src_binding else {
            if kind_of_target == Target::Api {
                pending.note_external(&physical, usage);
            } else {
                pending.note_unbound(Pending::UnboundInSource {
                    kind: doc.kind,
                    stem: doc.stem.clone(),
                    path: found.path.clone(),
                    target: kind_of_target,
                    physical: physical.clone(),
                    proposed_name: proposed_binding_name(&physical),
                });
            }
            continue;
        };
        let Some(tgt_entry) = target.bindings.get(&src_entry.name) else {
            pending.note_missing(
                &src_entry.name,
                declared_type(src_entry.kind),
                &src_entry.physical_name,
                usage,
            );
            continue;
        };

        // A knowledge-base name inside an MCP URL is a sibling reference:
        // translate it before rendering the URL around it.
        let kb_rename = found.physical.kb_name.as_deref().and_then(|kb| {
            renames
                .get(ResourceKind::KnowledgeBase, kb)
                .map(|(new, stem)| (kb.to_string(), new.to_string(), stem.to_string()))
        });
        let kb_name = match (&kb_rename, &found.physical.kb_name) {
            (Some((_, new, _)), _) => Some(new.clone()),
            (None, kb) => kb.clone(),
        };

        let render_target = RenderTarget {
            physical: tgt_entry.physical_name.clone(),
            arm_id: arm_id_of(tgt_entry),
            base_url: base_url_of(tgt_entry),
            kb_name,
            source_base_url: base_url_of(src_entry),
        };
        let Ok(value) = infra::render(found.form, &found.physical.original, &render_target) else {
            // The target binding is known, but not well enough to rewrite
            // this value's shape (no ARM id, no base URL).
            pending.note_unresolved(
                &tgt_entry.name,
                infra::binding_type_for(kind_of_target),
                usage,
            );
            continue;
        };
        if !set_path(merged, &found.path, value) {
            // Nothing was written — never claim a rewiring that did not
            // happen.
            continue;
        }
        rewired.push(Rewire {
            path: found.path.clone(),
            binding: src_entry.name.clone(),
            target: kind_of_target,
            from: src_entry.physical_name.clone(),
            to: tgt_entry.physical_name.clone(),
            shared: src_entry.physical_name == tgt_entry.physical_name,
        });
        if let Some((old, new, stem)) = kb_rename {
            renamed.push(Renamed {
                path: found.path,
                kind: ResourceKind::KnowledgeBase,
                stem,
                from: old,
                to: new,
            });
        }
    }
    (rewired, renamed)
}

fn declared_type(kind: BindingKind) -> Option<BindingType> {
    match kind {
        BindingKind::Declared(t) => Some(t),
        BindingKind::ImplicitSearch | BindingKind::ImplicitFoundry => None,
    }
}

fn arm_id_of(entry: &BindingEntry) -> Option<String> {
    entry
        .resolved
        .as_ref()
        .and_then(|r| r.arm_id.clone())
        .or_else(|| {
            entry
                .declared
                .as_ref()
                .and_then(|b| b.arm_id().map(str::to_string))
        })
}

fn base_url_of(entry: &BindingEntry) -> Option<String> {
    if let Some(declared) = &entry.declared
        && let BindingValue::Url(url) = declared.value()
    {
        return Some(url);
    }
    entry.resolved.as_ref().and_then(|r| r.endpoint.clone())
}

/// Set `value` at a CONCRETE path (`a.b[2].c`) — the shape
/// [`infra::extract`] reports, so every segment already exists. Returns
/// whether the write actually happened.
fn set_path(root: &mut Value, path: &str, value: Value) -> bool {
    fn walk(v: &mut Value, segments: &[(&str, Option<usize>)], value: Value) -> bool {
        let Some(((key, index), rest)) = segments.split_first() else {
            *v = value;
            return true;
        };
        let Some(next) = v.get_mut(key) else {
            return false;
        };
        let next = match index {
            Some(i) => match next.get_mut(*i) {
                Some(item) => item,
                None => return false,
            },
            None => next,
        };
        walk(next, rest, value)
    }
    let segments: Vec<(&str, Option<usize>)> = path.split('.').map(split_index).collect();
    walk(root, &segments, value)
}

/// `skills[2]` → `("skills", Some(2))`; `uri` → `("uri", None)`.
fn split_index(segment: &str) -> (&str, Option<usize>) {
    match segment.split_once('[') {
        Some((key, rest)) => (key, rest.trim_end_matches(']').parse().ok()),
        None => (segment, None),
    }
}

// ---------------------------------------------------------------------
// step 3 — sibling translation
// ---------------------------------------------------------------------

/// Which siblings are physically named differently in the target:
/// `(kind, source physical name)` → `(target physical name, stem)`.
struct Renames {
    map: BTreeMap<(ResourceKind, String), (String, String)>,
}

impl Renames {
    fn build(source: &EnvDocs, target: &EnvDocs) -> Renames {
        let target_physical: BTreeMap<(ResourceKind, &str), &str> = target
            .docs
            .iter()
            .map(|d| ((d.kind, d.stem.as_str()), d.physical.as_str()))
            .collect();
        let mut map = BTreeMap::new();
        for doc in &source.docs {
            if let Some(physical) = target_physical.get(&(doc.kind, doc.stem.as_str()))
                && *physical != doc.physical.as_str()
            {
                map.insert(
                    (doc.kind, doc.physical.clone()),
                    ((*physical).to_string(), doc.stem.clone()),
                );
            }
        }
        Renames { map }
    }

    fn get(&self, kind: ResourceKind, physical: &str) -> Option<(&str, &str)> {
        self.map
            .get(&(kind, physical.to_string()))
            .map(|(new, stem)| (new.as_str(), stem.as_str()))
    }
}

fn rename_siblings(kind: ResourceKind, merged: &mut Value, renames: &Renames) -> Vec<Renamed> {
    let mut out = Vec::new();

    // Collect every reference value at a CONCRETE path first, across all of
    // the kind's reference fields, then write each one back at its own path.
    // Every value is mapped by what it was BEFORE any rewrite: a whole-
    // document rename pass would corrupt a swap (dev `a` = `ks-1`, `b` =
    // `ks-2`; prod the other way round) by rewriting `ks-1` to `ks-2` and
    // then that same value back to `ks-1`.
    let mut hits: Vec<(String, ResourceKind, String)> = Vec::new();
    for field in registry::meta(kind).reference_fields {
        collect_concrete(merged, field.path, &mut |path, value| {
            if let Some(name) = value.as_str()
                && !name.is_empty()
            {
                hits.push((path.to_string(), field.to, name.to_string()));
            }
        });
    }
    // One record per rewritten value, at the path it was rewritten at.
    for (path, to, old) in hits {
        let Some((new, stem)) = renames.get(to, &old) else {
            continue;
        };
        let (new, stem) = (new.to_string(), stem.to_string());
        if !set_path(merged, &path, Value::String(new.clone())) {
            continue;
        }
        out.push(Renamed {
            path,
            kind: to,
            stem,
            from: old,
            to: new,
        });
    }

    // `x-rigg-ref` annotations follow the same rule, at their own concrete
    // paths — one record per annotation, not one per distinct value.
    let mut annotations: Vec<(String, String)> = Vec::new();
    collect_x_rigg_refs(merged, "", &mut annotations);
    for (path, value) in annotations {
        let Some((dir, old)) = value.split_once('/') else {
            continue;
        };
        let Some(referenced) = ResourceKind::from_directory_name(dir) else {
            continue;
        };
        let Some((new, stem)) = renames.get(referenced, old) else {
            continue;
        };
        let (new, stem) = (new.to_string(), stem.to_string());
        if !set_path(merged, &path, Value::String(format!("{dir}/{new}"))) {
            continue;
        }
        out.push(Renamed {
            path,
            kind: referenced,
            stem,
            from: old.to_string(),
            to: new,
        });
    }
    out
}

/// Visit every value at a registry `path` (`key[]` descends into arrays)
/// with its CONCRETE path — `indexProjections.selectors[1].targetIndexName`.
/// The read-only counterpart of the paths [`set_path`] understands.
fn collect_concrete(root: &Value, path: &str, f: &mut dyn FnMut(&str, &Value)) {
    fn walk(v: &Value, segments: &[&str], prefix: String, f: &mut dyn FnMut(&str, &Value)) {
        let Some((head, rest)) = segments.split_first() else {
            f(&prefix, v);
            return;
        };
        if let Some(key) = head.strip_suffix("[]") {
            let target = if key.is_empty() { Some(v) } else { v.get(key) };
            if let Some(Value::Array(items)) = target {
                for (i, item) in items.iter().enumerate() {
                    let next = if prefix.is_empty() {
                        format!("{key}[{i}]")
                    } else {
                        format!("{prefix}.{key}[{i}]")
                    };
                    walk(item, rest, next, f);
                }
            }
        } else if let Some(next) = v.get(*head) {
            let path = if prefix.is_empty() {
                (*head).to_string()
            } else {
                format!("{prefix}.{head}")
            };
            walk(next, rest, path, f);
        }
    }
    let segments: Vec<&str> = path.split('.').collect();
    walk(root, &segments, String::new(), f);
}

/// Every `x-rigg-ref` annotation in `v`, as `(concrete path, value)` —
/// `tools[0].x-rigg-ref`, the shape [`set_path`] understands.
fn collect_x_rigg_refs(v: &Value, prefix: &str, out: &mut Vec<(String, String)>) {
    match v {
        Value::Object(map) => {
            for (key, value) in map {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                if key == X_RIGG_REF {
                    if let Some(s) = value.as_str() {
                        out.push((path, s.to_string()));
                    }
                } else {
                    collect_x_rigg_refs(value, &path, out);
                }
            }
        }
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                collect_x_rigg_refs(item, &format!("{prefix}[{i}]"), out);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------
// step 4 — what the target keeps
// ---------------------------------------------------------------------

/// Restore the target's physical identity and pinned paths into `merged`,
/// returning the paths its `x-rigg-pin` asked for.
///
/// Identity is unconditional: the promoted document is named
/// [`Doc::physical`] — never the source's name. When the target's file
/// carries no `name` key at all (its identity is the file stem) the key is
/// removed instead, so the target keeps its shape as well as its name.
fn keep_from_target(merged: &mut Value, target: &Doc) -> Vec<String> {
    if let Some(map) = merged.as_object_mut() {
        if target.body.get("name").is_none() && target.physical == target.stem {
            map.remove("name");
        } else {
            map.insert("name".to_string(), Value::String(target.physical.clone()));
        }
    }

    let mut pinned = Vec::new();
    if let Some(paths) = target.body.get(X_RIGG_PIN).and_then(Value::as_array) {
        for path in paths.iter().filter_map(Value::as_str) {
            registry::restore_path(merged, &target.body, path);
            pinned.push(path.to_string());
        }
    }
    if target.body.get(X_RIGG_PIN).is_some() {
        registry::restore_path(merged, &target.body, X_RIGG_PIN);
    }
    pinned.sort();
    pinned.dedup();
    pinned
}

/// Re-apply the target's own Web API auth carriers. A carrier authorizes ONE
/// skill's endpoint, so the target's skill is matched to a merged skill by
/// `name`, then by (already translated) `uri`, and only then — when the two
/// skill lists have the same length, so position still means something — by
/// index. Matched by nothing: the carrier is not re-applied and nothing is
/// recorded, rather than landing on a skill it does not authorize.
fn keep_target_auth(merged: &mut Value, target: &Value) -> Vec<AuthCarrier> {
    let mut out = Vec::new();
    let (Some(target_skills), Some(merged_skills)) = (
        target.get("skills").and_then(Value::as_array),
        merged.get("skills").and_then(Value::as_array),
    ) else {
        return out;
    };
    let same_length = target_skills.len() == merged_skills.len();

    // Read every carrier out of the target first: applying them needs a
    // mutable borrow of the same document `merged_skills` is read from.
    let mut carriers: Vec<Carrier> = Vec::new();
    for (j, target_skill) in target_skills.iter().enumerate() {
        let carrier = Carrier {
            skill: 0,
            auth_resource_id: target_skill.get("authResourceId").cloned(),
            annotation: target_skill.get(X_RIGG_AUTH).cloned(),
            key_header: target_skill
                .pointer(&format!("/httpHeaders/{FUNCTION_KEY_HEADER}"))
                .cloned(),
        };
        if carrier.is_empty() {
            continue;
        }
        let Some(skill) = match_skill(merged_skills, target_skill, j, same_length) else {
            continue;
        };
        carriers.push(Carrier { skill, ..carrier });
    }

    for carrier in carriers {
        let Some(skill) = merged
            .get_mut("skills")
            .and_then(Value::as_array_mut)
            .and_then(|skills| skills.get_mut(carrier.skill))
            .and_then(Value::as_object_mut)
        else {
            continue;
        };
        if let Some(value) = carrier.auth_resource_id {
            skill.insert("authResourceId".to_string(), value);
        }
        if let Some(value) = carrier.annotation {
            let function_key = value.as_str() == Some(X_RIGG_AUTH_FUNCTION_KEY);
            skill.insert(X_RIGG_AUTH.to_string(), value);
            // The target authenticates with a key in the uri, but the
            // translated uri carries none (the source's was stripped): leave
            // the placeholder `rigg push`'s auth gate reads, or the skill
            // looks like an anonymous endpoint and the key is never resolved.
            if function_key {
                let uri = skill.get("uri").and_then(Value::as_str).unwrap_or_default();
                let header = skill
                    .get("httpHeaders")
                    .and_then(|h| h.get(FUNCTION_KEY_HEADER))
                    .is_some();
                if !uri.is_empty() && !header && !has_code_param(uri) {
                    let uri = set_code_param(uri, REDACTED_KEY);
                    skill.insert("uri".to_string(), Value::String(uri));
                }
            }
        }
        if let Some(value) = carrier.key_header {
            let headers = skill
                .entry("httpHeaders".to_string())
                .or_insert_with(|| Value::Object(serde_json::Map::new()));
            if let Some(map) = headers.as_object_mut() {
                map.insert(FUNCTION_KEY_HEADER.to_string(), value);
            }
        }
        out.push(AuthCarrier::Kept {
            path: format!("skills[{}]", carrier.skill),
        });
    }
    out
}

/// One target skill's Web API auth carrier, and the merged `skills[i]` it
/// belongs to.
struct Carrier {
    skill: usize,
    auth_resource_id: Option<Value>,
    annotation: Option<Value>,
    key_header: Option<Value>,
}

impl Carrier {
    fn is_empty(&self) -> bool {
        self.auth_resource_id.is_none() && self.annotation.is_none() && self.key_header.is_none()
    }
}

/// Which merged skill `target_skill` is: by `name`, else by `uri`, else by
/// position when both lists have the same length.
fn match_skill(
    merged_skills: &[Value],
    target_skill: &Value,
    index: usize,
    same_length: bool,
) -> Option<usize> {
    let by = |key: &str| {
        target_skill
            .get(key)
            .and_then(Value::as_str)
            .and_then(|wanted| {
                merged_skills
                    .iter()
                    .position(|s| s.get(key).and_then(Value::as_str) == Some(wanted))
            })
    };
    by("name")
        .or_else(|| by("uri"))
        .or_else(|| (same_length && index < merged_skills.len()).then_some(index))
}

// ---------------------------------------------------------------------
// pending questions
// ---------------------------------------------------------------------

#[derive(Default)]
struct PendingSet {
    unbound: Vec<Pending>,
    missing: BTreeMap<String, Pending>,
    unresolved: BTreeMap<String, Pending>,
    external: BTreeMap<String, Pending>,
}

impl PendingSet {
    fn note_unbound(&mut self, question: Pending) {
        if !self.unbound.contains(&question) {
            self.unbound.push(question);
        }
    }

    fn note_missing(
        &mut self,
        binding: &str,
        binding_type: Option<BindingType>,
        source_physical: &str,
        usage: Usage,
    ) {
        let entry =
            self.missing
                .entry(binding.to_string())
                .or_insert_with(|| Pending::MissingInTarget {
                    binding: binding.to_string(),
                    binding_type,
                    source_physical: source_physical.to_string(),
                    used_by: Vec::new(),
                });
        if let Pending::MissingInTarget { used_by, .. } = entry {
            push_usage(used_by, usage);
        }
    }

    fn note_unresolved(&mut self, binding: &str, binding_type: Option<BindingType>, usage: Usage) {
        let entry = self
            .unresolved
            .entry(binding.to_string())
            .or_insert_with(|| Pending::UnresolvedTarget {
                binding: binding.to_string(),
                binding_type,
                used_by: Vec::new(),
            });
        if let Pending::UnresolvedTarget { used_by, .. } = entry {
            push_usage(used_by, usage);
        }
    }

    fn note_external(&mut self, host: &str, usage: Usage) {
        let entry = self
            .external
            .entry(host.to_string())
            .or_insert_with(|| Pending::External {
                host: host.to_string(),
                used_by: Vec::new(),
            });
        if let Pending::External { used_by, .. } = entry {
            push_usage(used_by, usage);
        }
    }

    fn into_sorted(self) -> Vec<Pending> {
        let mut out = self.unbound;
        out.extend(self.missing.into_values());
        out.extend(self.unresolved.into_values());
        out.extend(self.external.into_values());
        out.sort_by_key(Pending::sort_key);
        out.dedup();
        out
    }
}

fn push_usage(used_by: &mut Vec<Usage>, usage: Usage) {
    if !used_by.contains(&usage) {
        used_by.push(usage);
    }
}

/// Position of `kind` in [`ResourceKind::all`] — the registry's push-friendly
/// declaration order, which is also the order a plan lists resources in.
fn kind_order(kind: ResourceKind) -> usize {
    ResourceKind::all()
        .iter()
        .position(|k| *k == kind)
        .unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::{Binding, BindingType};
    use crate::registry::{SEARCH_PREVIEW_API_VERSION, X_RIGG_PIN};
    use crate::resources::ResourceKind;
    use crate::workspace::{Environment, FoundryConnection, SearchConnection};
    use serde_json::{Value, json};

    const DEV_ACCT: &str =
        "/subscriptions/D/resourceGroups/rg/providers/Microsoft.Storage/storageAccounts/devacct";
    const PROD_ACCT: &str =
        "/subscriptions/P/resourceGroups/prg/providers/Microsoft.Storage/storageAccounts/prodacct";
    const SHARED_ACCT: &str =
        "/subscriptions/S/resourceGroups/rg/providers/Microsoft.Storage/storageAccounts/sharedacct";

    fn env(
        name: &str,
        search: &str,
        foundry: &str,
        deps: &[(&str, BindingType, &str)],
    ) -> EnvBindings {
        let environment = Environment {
            search: Some(SearchConnection {
                service: search.to_string(),
                ..Default::default()
            }),
            foundry: Some(FoundryConnection {
                account: foundry.to_string(),
                project: "p".to_string(),
                ..Default::default()
            }),
            dependencies: deps
                .iter()
                .map(|(n, kind, value)| {
                    (
                        n.to_string(),
                        Binding {
                            kind: *kind,
                            value: value.to_string(),
                        },
                    )
                })
                .collect(),
            ..Default::default()
        };
        EnvBindings::of_env(name, &environment, None)
    }

    fn doc(kind: ResourceKind, stem: &str, body: Value) -> Doc {
        Doc {
            kind,
            stem: stem.to_string(),
            physical: body["name"].as_str().unwrap_or(stem).to_string(),
            body,
        }
    }

    fn conn_string(arm_id: &str) -> String {
        format!("ResourceId={arm_id};")
    }

    fn data_source(stem: &str, name: &str, arm_id: &str) -> Doc {
        doc(
            ResourceKind::DataSource,
            stem,
            json!({
                "name": name,
                "type": "azureblob",
                "credentials": {"connectionString": conn_string(arm_id)},
                "container": {"name": "c"}
            }),
        )
    }

    fn item<'a>(plan: &'a Plan, kind: ResourceKind, stem: &str) -> &'a Item {
        plan.items
            .iter()
            .find(|i| i.kind == kind && i.stem == stem)
            .unwrap_or_else(|| panic!("no item {kind:?}/{stem} in {:?}", plan.items))
    }

    #[test]
    fn new_in_target_data_source_is_rewired_to_the_target_storage_binding() {
        let src = EnvDocs {
            env: "dev".into(),
            bindings: env(
                "dev",
                "s-dev",
                "f-dev",
                &[("docs", BindingType::Storage, DEV_ACCT)],
            ),
            docs: vec![data_source("ds", "ds", DEV_ACCT)],
        };
        let tgt = EnvDocs {
            env: "prod".into(),
            bindings: env(
                "prod",
                "s-prod",
                "f-prod",
                &[("docs", BindingType::Storage, PROD_ACCT)],
            ),
            docs: vec![],
        };

        let plan = translate(&src, &tgt);
        assert!(plan.pending.is_empty(), "{:?}", plan.pending);
        assert_eq!(plan.from, "dev");
        assert_eq!(plan.to, "prod");
        let item = &plan.items[0];
        assert!(item.is_new);
        assert!(item.before.is_none());
        assert_eq!(item.change(), Change::New);
        assert_eq!(
            item.merged["credentials"]["connectionString"],
            json!(conn_string(PROD_ACCT))
        );
        assert_eq!(item.rewired.len(), 1);
        assert_eq!(item.rewired[0].binding, "docs");
        assert_eq!(item.rewired[0].path, "credentials.connectionString");
        assert_eq!(item.rewired[0].target, Target::Storage);
        assert_eq!(item.rewired[0].from, "devacct");
        assert_eq!(item.rewired[0].to, "prodacct");
        assert!(!item.rewired[0].shared);
        assert_eq!(item.target_name, "ds");
        assert!(item.pinned.is_empty(), "nothing to pin from: {item:?}");
    }

    #[test]
    fn shared_binding_is_reported_shared_and_unchanged() {
        let src = EnvDocs {
            env: "dev".into(),
            bindings: env(
                "dev",
                "s-dev",
                "f-dev",
                &[("docs", BindingType::Storage, SHARED_ACCT)],
            ),
            docs: vec![data_source("ds", "ds", SHARED_ACCT)],
        };
        let tgt = EnvDocs {
            env: "prod".into(),
            bindings: env(
                "prod",
                "s-prod",
                "f-prod",
                &[("docs", BindingType::Storage, SHARED_ACCT)],
            ),
            docs: vec![],
        };

        let plan = translate(&src, &tgt);
        assert!(plan.pending.is_empty(), "{:?}", plan.pending);
        let item = &plan.items[0];
        assert_eq!(
            item.merged["credentials"]["connectionString"],
            json!(conn_string(SHARED_ACCT)),
            "a shared binding leaves the value untouched"
        );
        assert!(item.rewired[0].shared);
        assert_eq!(item.rewired[0].from, item.rewired[0].to);

        let table = rewiring_table(&plan);
        assert_eq!(
            table,
            vec![(
                "docs".to_string(),
                Target::Storage,
                "sharedacct".to_string(),
                "sharedacct".to_string(),
                true,
                1
            )]
        );
    }

    #[test]
    fn model_host_rewires_through_implicit_foundry() {
        let index = |name: &str, host: &str| {
            doc(
                ResourceKind::Index,
                "docs-index",
                json!({
                    "name": name,
                    "fields": [],
                    "vectorSearch": {"vectorizers": [
                        {"name": "v", "azureOpenAIParameters": {"resourceUri": format!("https://{host}.openai.azure.com")}}
                    ]}
                }),
            )
        };
        let src = EnvDocs {
            env: "dev".into(),
            bindings: env("dev", "s-dev", "f-dev", &[]),
            docs: vec![index("docs-index", "f-dev")],
        };
        let tgt = EnvDocs {
            env: "prod".into(),
            bindings: env("prod", "s-prod", "f-prod", &[]),
            docs: vec![],
        };

        let plan = translate(&src, &tgt);
        assert!(plan.pending.is_empty(), "{:?}", plan.pending);
        let item = &plan.items[0];
        assert_eq!(
            item.merged["vectorSearch"]["vectorizers"][0]["azureOpenAIParameters"]["resourceUri"],
            json!("https://f-prod.openai.azure.com")
        );
        assert_eq!(item.rewired[0].binding, "foundry");
        assert_eq!(item.rewired[0].target, Target::ModelHost);
        assert!(!item.rewired[0].shared);
    }

    #[test]
    fn renamed_sibling_index_is_followed_by_the_indexer_and_by_kb_mcp_urls() {
        let mcp = |svc: &str, kb: &str| {
            format!(
                "https://{svc}.search.windows.net/knowledgebases/{kb}/mcp?api-version={SEARCH_PREVIEW_API_VERSION}"
            )
        };
        let src = EnvDocs {
            env: "dev".into(),
            bindings: env("dev", "s-dev", "f-dev", &[]),
            docs: vec![
                doc(
                    ResourceKind::Index,
                    "docs-index",
                    json!({"name": "docs-index-dev", "fields": []}),
                ),
                doc(
                    ResourceKind::Indexer,
                    "ix",
                    json!({"name": "ix", "dataSourceName": "ds", "targetIndexName": "docs-index-dev", "skillsetName": "sk"}),
                ),
                doc(
                    ResourceKind::Skillset,
                    "sk",
                    json!({
                        "name": "sk",
                        "skills": [],
                        "indexProjections": {"selectors": [{"targetIndexName": "docs-index-dev", "parentKeyFieldName": "p"}]}
                    }),
                ),
                doc(ResourceKind::KnowledgeBase, "kb", json!({"name": "kb-dev"})),
                doc(
                    ResourceKind::Agent,
                    "regulus",
                    json!({
                        "name": "Regulus",
                        "model": "gpt-5-mini",
                        "tools": [{
                            "type": "mcp",
                            "server_url": mcp("s-dev", "kb-dev"),
                            "x-rigg-ref": "knowledge-bases/kb-dev"
                        }]
                    }),
                ),
            ],
        };
        let tgt = EnvDocs {
            env: "prod".into(),
            bindings: env("prod", "s-prod", "f-prod", &[]),
            docs: vec![
                doc(
                    ResourceKind::Index,
                    "docs-index",
                    json!({"name": "docs-index", "fields": []}),
                ),
                doc(ResourceKind::KnowledgeBase, "kb", json!({"name": "kb"})),
            ],
        };

        let plan = translate(&src, &tgt);
        assert!(plan.pending.is_empty(), "{:?}", plan.pending);

        let indexer = item(&plan, ResourceKind::Indexer, "ix");
        assert_eq!(indexer.merged["targetIndexName"], json!("docs-index"));
        assert_eq!(
            indexer.merged["dataSourceName"],
            json!("ds"),
            "unrenamed siblings are left alone"
        );
        assert_eq!(indexer.renamed.len(), 1);
        assert_eq!(indexer.renamed[0].kind, ResourceKind::Index);
        assert_eq!(indexer.renamed[0].stem, "docs-index");
        assert_eq!(indexer.renamed[0].from, "docs-index-dev");
        assert_eq!(indexer.renamed[0].to, "docs-index");
        assert_eq!(indexer.renamed[0].path, "targetIndexName");

        let skillset = item(&plan, ResourceKind::Skillset, "sk");
        assert_eq!(
            skillset.merged["indexProjections"]["selectors"][0]["targetIndexName"],
            json!("docs-index")
        );

        let agent = item(&plan, ResourceKind::Agent, "regulus");
        assert_eq!(
            agent.merged["tools"][0]["server_url"],
            json!(mcp("s-prod", "kb")),
            "the MCP URL follows both the search service and the renamed knowledge base"
        );
        assert_eq!(
            agent.merged["tools"][0]["x-rigg-ref"],
            json!("knowledge-bases/kb")
        );
        assert!(
            agent
                .renamed
                .iter()
                .any(|r| r.path == "tools[0].x-rigg-ref" && r.from == "kb-dev" && r.to == "kb"),
            "{:?}",
            agent.renamed
        );
        assert_eq!(agent.rewired[0].binding, "search");
        assert_eq!(agent.rewired[0].target, Target::SearchService);

        let index = item(&plan, ResourceKind::Index, "docs-index");
        assert_eq!(
            index.merged["name"],
            json!("docs-index"),
            "the target keeps its own physical name"
        );
        assert_eq!(index.change(), Change::Unchanged);
    }

    #[test]
    fn unbound_source_reference_becomes_a_pending_question_and_is_left_untouched() {
        let other = "/subscriptions/D/resourceGroups/rg/providers/Microsoft.Storage/storageAccounts/otheracct";
        let src = EnvDocs {
            env: "dev".into(),
            bindings: env(
                "dev",
                "s-dev",
                "f-dev",
                &[("docs", BindingType::Storage, DEV_ACCT)],
            ),
            docs: vec![data_source("ds", "ds", other)],
        };
        let tgt = EnvDocs {
            env: "prod".into(),
            bindings: env(
                "prod",
                "s-prod",
                "f-prod",
                &[("docs", BindingType::Storage, PROD_ACCT)],
            ),
            docs: vec![],
        };

        let plan = translate(&src, &tgt);
        assert_eq!(plan.pending.len(), 1, "{:?}", plan.pending);
        match &plan.pending[0] {
            Pending::UnboundInSource {
                kind,
                stem,
                path,
                target,
                physical,
                proposed_name,
            } => {
                assert_eq!(*kind, ResourceKind::DataSource);
                assert_eq!(stem, "ds");
                assert_eq!(path, "credentials.connectionString");
                assert_eq!(*target, Target::Storage);
                assert_eq!(physical, "otheracct");
                assert_eq!(proposed_name, "otheracct");
            }
            other => panic!("expected UnboundInSource, got {other:?}"),
        }
        assert_eq!(
            plan.items[0].merged["credentials"]["connectionString"],
            json!(conn_string(other)),
            "an undecidable reference is left exactly as it was"
        );
        assert!(plan.items[0].rewired.is_empty());
    }

    #[test]
    fn missing_target_binding_is_pending_once_with_all_users() {
        let src = EnvDocs {
            env: "dev".into(),
            bindings: env(
                "dev",
                "s-dev",
                "f-dev",
                &[("docs", BindingType::Storage, DEV_ACCT)],
            ),
            docs: vec![
                data_source("ds1", "ds1", DEV_ACCT),
                data_source("ds2", "ds2", DEV_ACCT),
            ],
        };
        let tgt = EnvDocs {
            env: "prod".into(),
            bindings: env("prod", "s-prod", "f-prod", &[]),
            docs: vec![],
        };

        let plan = translate(&src, &tgt);
        assert_eq!(plan.pending.len(), 1, "{:?}", plan.pending);
        match &plan.pending[0] {
            Pending::MissingInTarget {
                binding,
                binding_type,
                source_physical,
                used_by,
            } => {
                assert_eq!(binding, "docs");
                assert_eq!(*binding_type, Some(BindingType::Storage));
                assert_eq!(source_physical, "devacct");
                assert_eq!(used_by.len(), 2, "{used_by:?}");
                assert_eq!(
                    used_by[0],
                    (
                        ResourceKind::DataSource,
                        "ds1".to_string(),
                        "credentials.connectionString".to_string()
                    )
                );
                assert_eq!(used_by[1].1, "ds2");
            }
            other => panic!("expected MissingInTarget, got {other:?}"),
        }
        for i in &plan.items {
            assert!(i.rewired.is_empty());
            assert_eq!(
                i.merged["credentials"]["connectionString"],
                json!(conn_string(DEV_ACCT))
            );
        }
    }

    #[test]
    fn target_binding_known_only_by_name_is_an_unresolved_question() {
        let src = EnvDocs {
            env: "dev".into(),
            bindings: env(
                "dev",
                "s-dev",
                "f-dev",
                &[("docs", BindingType::Storage, DEV_ACCT)],
            ),
            docs: vec![data_source("ds", "ds", DEV_ACCT)],
        };
        let tgt = EnvDocs {
            env: "prod".into(),
            bindings: env(
                "prod",
                "s-prod",
                "f-prod",
                &[("docs", BindingType::Storage, "prodacct")],
            ),
            docs: vec![],
        };

        let plan = translate(&src, &tgt);
        assert_eq!(plan.pending.len(), 1, "{:?}", plan.pending);
        match &plan.pending[0] {
            Pending::UnresolvedTarget {
                binding,
                binding_type,
                used_by,
            } => {
                assert_eq!(binding, "docs");
                assert_eq!(*binding_type, Some(BindingType::Storage));
                assert_eq!(used_by.len(), 1);
                assert_eq!(used_by[0].1, "ds");
            }
            other => panic!("expected UnresolvedTarget, got {other:?}"),
        }
        assert_eq!(
            plan.items[0].merged["credentials"]["connectionString"],
            json!(conn_string(DEV_ACCT)),
            "an unresolvable target leaves the value alone"
        );
        assert!(plan.items[0].rewired.is_empty());
    }

    #[test]
    fn target_keeps_name_and_x_rigg_pin_paths_with_array_semantics() {
        let src = EnvDocs {
            env: "dev".into(),
            bindings: env("dev", "s-dev", "f-dev", &[]),
            docs: vec![doc(
                ResourceKind::Agent,
                "agent",
                json!({
                    "name": "agent-dev",
                    "model": "gpt-5-mini",
                    "tools": [{"type": "mcp", "server_url": "https://dev.example/x"}],
                    "x-rigg-pin": ["should-not-leak"]
                }),
            )],
        };
        let tgt = EnvDocs {
            env: "prod".into(),
            bindings: env("prod", "s-prod", "f-prod", &[]),
            docs: vec![doc(
                ResourceKind::Agent,
                "agent",
                json!({
                    "name": "agent",
                    "model": "gpt-4o-old",
                    "tools": [
                        {"type": "mcp", "server_url": "https://prod.example/x"},
                        {"type": "file_search", "vector_store_ids": ["vs-prod"]},
                        {"type": "mcp", "server_url": "https://prod.example/y",
                         "project_connection_id": "conn-prod-2"}
                    ],
                    "x-rigg-pin": ["tools[].server_url"]
                }),
            )],
        };

        let plan = translate(&src, &tgt);
        let item = &plan.items[0];
        assert!(!item.is_new);
        assert_eq!(item.change(), Change::Changed);
        assert_eq!(item.merged["name"], json!("agent"), "target keeps its name");
        assert_eq!(item.target_name, "agent");
        assert_eq!(
            item.merged["model"],
            json!("gpt-5-mini"),
            "non-pinned promoted"
        );

        let tools = item.merged["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 3, "target-only tools survive: {tools:?}");
        assert_eq!(tools[0]["server_url"], json!("https://prod.example/x"));
        assert_eq!(
            tools[1],
            json!({"type": "file_search", "vector_store_ids": ["vs-prod"]})
        );
        assert_eq!(tools[2]["project_connection_id"], json!("conn-prod-2"));

        assert_eq!(
            item.merged[X_RIGG_PIN],
            json!(["tools[].server_url"]),
            "the target's annotation travels, the source's is stripped"
        );
        assert_eq!(
            item.pinned,
            vec!["tools[].server_url".to_string()],
            "`pinned` is the user's pin list — not `name`, not the annotation"
        );

        // No target at all: the source's own annotation still never leaks.
        let empty = EnvDocs {
            env: "prod".into(),
            bindings: env("prod", "s-prod", "f-prod", &[]),
            docs: vec![],
        };
        let fresh = translate(&src, &empty);
        assert!(fresh.items[0].merged.get(X_RIGG_PIN).is_none());
        assert!(fresh.items[0].pinned.is_empty());
    }

    #[test]
    fn web_api_auth_carriers_never_cross_but_target_carriers_are_kept() {
        let source_skillset = doc(
            ResourceKind::Skillset,
            "sk",
            json!({
                "name": "sk",
                "skills": [{
                    "@odata.type": "#Microsoft.Skills.Custom.WebApiSkill",
                    "name": "enrich",
                    "uri": "https://fn-dev.azurewebsites.net/api/enrich?code=<redacted>",
                    "x-rigg-auth": "function-key",
                    "inputs": [],
                    "outputs": []
                }]
            }),
        );
        let src = EnvDocs {
            env: "dev".into(),
            bindings: env(
                "dev",
                "s-dev",
                "f-dev",
                &[("enrich-fn", BindingType::FunctionApp, "fn-dev")],
            ),
            docs: vec![source_skillset],
        };
        let bindings_prod = || {
            env(
                "prod",
                "s-prod",
                "f-prod",
                &[("enrich-fn", BindingType::FunctionApp, "fn-prod")],
            )
        };

        // New in the target: the source's carrier is stripped, never copied.
        let plan = translate(
            &src,
            &EnvDocs {
                env: "prod".into(),
                bindings: bindings_prod(),
                docs: vec![],
            },
        );
        assert!(plan.pending.is_empty(), "{:?}", plan.pending);
        let item = &plan.items[0];
        assert_eq!(
            item.merged["skills"][0]["uri"],
            json!("https://fn-prod.azurewebsites.net/api/enrich"),
            "URI translated to the target function app, key dropped"
        );
        assert!(item.merged["skills"][0].get("x-rigg-auth").is_none());
        assert_eq!(
            item.auth,
            vec![AuthCarrier::Stripped {
                path: "skills[0]".to_string(),
                used_key: true
            }]
        );
        assert_eq!(item.rewired[0].binding, "enrich-fn");
        assert_eq!(item.rewired[0].target, Target::FunctionApp);

        // The target already has its own carrier: it is kept.
        let plan = translate(
            &src,
            &EnvDocs {
                env: "prod".into(),
                bindings: bindings_prod(),
                docs: vec![doc(
                    ResourceKind::Skillset,
                    "sk",
                    json!({
                        "name": "sk",
                        "skills": [{
                            "@odata.type": "#Microsoft.Skills.Custom.WebApiSkill",
                            "name": "enrich",
                            "uri": "https://fn-prod.azurewebsites.net/api/enrich",
                            "authResourceId": "api://prod-app",
                            "inputs": [],
                            "outputs": []
                        }]
                    }),
                )],
            },
        );
        let item = &plan.items[0];
        assert_eq!(
            item.merged["skills"][0]["authResourceId"],
            json!("api://prod-app"),
            "the target's own auth carrier survives the promote"
        );
        assert!(item.merged["skills"][0].get("x-rigg-auth").is_none());
        assert!(
            item.auth.contains(&AuthCarrier::Kept {
                path: "skills[0]".to_string()
            }),
            "{:?}",
            item.auth
        );
        assert!(
            item.auth
                .iter()
                .any(|a| matches!(a, AuthCarrier::Stripped { used_key: true, .. })),
            "{:?}",
            item.auth
        );
        assert_eq!(item.change(), Change::Unchanged, "{:?}", item.merged);
    }

    #[test]
    fn kept_only_in_target_and_unchanged_classification() {
        let src = EnvDocs {
            env: "dev".into(),
            bindings: env("dev", "s-dev", "f-dev", &[]),
            docs: vec![doc(
                ResourceKind::Index,
                "idx",
                json!({"name": "idx", "fields": []}),
            )],
        };
        let tgt = EnvDocs {
            env: "prod".into(),
            bindings: env("prod", "s-prod", "f-prod", &[]),
            docs: vec![
                doc(
                    ResourceKind::Index,
                    "idx",
                    json!({"name": "idx", "fields": []}),
                ),
                doc(
                    ResourceKind::SynonymMap,
                    "syn",
                    json!({"name": "syn", "format": "solr", "synonyms": "a,b"}),
                ),
            ],
        };

        let plan = translate(&src, &tgt);
        assert_eq!(plan.items.len(), 1);
        assert_eq!(plan.items[0].change(), Change::Unchanged);
        assert!(!plan.items[0].is_new);
        assert_eq!(
            plan.kept_only_in_to,
            vec![(ResourceKind::SynonymMap, "syn".to_string())]
        );
    }

    #[test]
    fn a_target_document_without_a_name_key_never_takes_the_source_identity() {
        let src = EnvDocs {
            env: "dev".into(),
            bindings: env("dev", "s-dev", "f-dev", &[]),
            docs: vec![doc(
                ResourceKind::Agent,
                "regulus",
                json!({"name": "Regulus-dev", "model": "gpt-5-mini"}),
            )],
        };
        // The target's file has no `name` at all — its identity is the stem.
        let tgt = EnvDocs {
            env: "prod".into(),
            bindings: env("prod", "s-prod", "f-prod", &[]),
            docs: vec![doc(
                ResourceKind::Agent,
                "regulus",
                json!({"model": "gpt-4o-old"}),
            )],
        };

        let plan = translate(&src, &tgt);
        let item = &plan.items[0];
        assert!(
            item.merged.get("name").is_none(),
            "the target's shape is kept, and `Regulus-dev` never crosses: {:?}",
            item.merged
        );
        assert_eq!(item.target_name, "regulus");
        assert_eq!(item.merged["model"], json!("gpt-5-mini"));
    }

    #[test]
    fn a_target_documents_own_name_always_wins_over_the_sources() {
        let src = EnvDocs {
            env: "dev".into(),
            bindings: env("dev", "s-dev", "f-dev", &[]),
            docs: vec![doc(
                ResourceKind::Index,
                "docs-index",
                json!({"name": "docs-index-dev", "fields": []}),
            )],
        };
        let tgt = EnvDocs {
            env: "prod".into(),
            bindings: env("prod", "s-prod", "f-prod", &[]),
            docs: vec![doc(
                ResourceKind::Index,
                "docs-index",
                json!({"name": "docs-index-prod", "fields": []}),
            )],
        };

        let plan = translate(&src, &tgt);
        assert_eq!(plan.items[0].merged["name"], json!("docs-index-prod"));
        assert_eq!(plan.items[0].target_name, "docs-index-prod");
    }

    #[test]
    fn a_kept_auth_carrier_follows_the_named_skill_not_the_position() {
        // The source inserts a skill BEFORE the WebApiSkill, so the target's
        // `skills[0]` and the merged document's `skills[0]` are different
        // skills: matching by position would authorize the wrong one.
        let src = EnvDocs {
            env: "dev".into(),
            bindings: env(
                "dev",
                "s-dev",
                "f-dev",
                &[("enrich-fn", BindingType::FunctionApp, "fn-dev")],
            ),
            docs: vec![doc(
                ResourceKind::Skillset,
                "sk",
                json!({
                    "name": "sk",
                    "skills": [
                        {"@odata.type": "#Microsoft.Skills.Text.SplitSkill", "name": "split",
                         "inputs": [], "outputs": []},
                        {"@odata.type": "#Microsoft.Skills.Custom.WebApiSkill", "name": "enrich",
                         "uri": "https://fn-dev.azurewebsites.net/api/enrich",
                         "inputs": [], "outputs": []}
                    ]
                }),
            )],
        };
        let tgt = EnvDocs {
            env: "prod".into(),
            bindings: env(
                "prod",
                "s-prod",
                "f-prod",
                &[("enrich-fn", BindingType::FunctionApp, "fn-prod")],
            ),
            docs: vec![doc(
                ResourceKind::Skillset,
                "sk",
                json!({
                    "name": "sk",
                    "skills": [
                        {"@odata.type": "#Microsoft.Skills.Custom.WebApiSkill", "name": "enrich",
                         "uri": "https://fn-prod.azurewebsites.net/api/enrich",
                         "authResourceId": "api://prod-app",
                         "inputs": [], "outputs": []}
                    ]
                }),
            )],
        };

        let plan = translate(&src, &tgt);
        let item = &plan.items[0];
        assert!(
            item.merged["skills"][0].get("authResourceId").is_none(),
            "the split skill is not the one the carrier authorizes: {:?}",
            item.merged["skills"][0]
        );
        assert_eq!(
            item.merged["skills"][1]["authResourceId"],
            json!("api://prod-app")
        );
        assert_eq!(
            item.auth,
            vec![AuthCarrier::Kept {
                path: "skills[1]".to_string()
            }]
        );
    }

    #[test]
    fn an_api_reference_outside_the_bindings_prefix_is_external() {
        let skillset = |uri: &str| {
            doc(
                ResourceKind::Skillset,
                "sk",
                json!({
                    "name": "sk",
                    "skills": [{
                        "@odata.type": "#Microsoft.Skills.Custom.WebApiSkill",
                        "name": "enrich", "uri": uri, "inputs": [], "outputs": []
                    }]
                }),
            )
        };
        let dev = |docs: Vec<Doc>| EnvDocs {
            env: "dev".into(),
            bindings: env(
                "dev",
                "s-dev",
                "f-dev",
                &[("partner", BindingType::Api, "https://api.x/v1")],
            ),
            docs,
        };
        let prod = EnvDocs {
            env: "prod".into(),
            bindings: env(
                "prod",
                "s-prod",
                "f-prod",
                &[("partner", BindingType::Api, "https://api.y/v2")],
            ),
            docs: vec![],
        };

        // Same host, different path scope: the binding does not cover it.
        let plan = translate(&dev(vec![skillset("https://api.x/v2/enrich")]), &prod);
        assert_eq!(plan.pending.len(), 1, "{:?}", plan.pending);
        match &plan.pending[0] {
            Pending::External { host, used_by } => {
                assert_eq!(host, "api.x");
                assert_eq!(used_by.len(), 1);
                assert_eq!(used_by[0].2, "skills[0].uri");
            }
            other => panic!("expected External, got {other:?}"),
        }
        assert!(plan.items[0].rewired.is_empty());
        assert_eq!(
            plan.items[0].merged["skills"][0]["uri"],
            json!("https://api.x/v2/enrich"),
            "an unbound reference is left exactly as it was"
        );

        // Inside the binding's prefix: rewired onto the target's base URL.
        let plan = translate(&dev(vec![skillset("https://api.x/v1/enrich")]), &prod);
        assert!(plan.pending.is_empty(), "{:?}", plan.pending);
        assert_eq!(
            plan.items[0].merged["skills"][0]["uri"],
            json!("https://api.y/v2/enrich")
        );
        assert_eq!(plan.items[0].rewired[0].binding, "partner");
        assert_eq!(plan.items[0].rewired[0].target, Target::Api);
    }

    #[test]
    fn every_rewritten_reference_value_is_recorded_at_its_own_concrete_path() {
        let src = EnvDocs {
            env: "dev".into(),
            bindings: env("dev", "s-dev", "f-dev", &[]),
            docs: vec![
                doc(
                    ResourceKind::Index,
                    "docs-index",
                    json!({"name": "docs-index-dev", "fields": []}),
                ),
                doc(
                    ResourceKind::Skillset,
                    "sk",
                    json!({
                        "name": "sk",
                        "skills": [],
                        "indexProjections": {"selectors": [
                            {"targetIndexName": "docs-index-dev", "parentKeyFieldName": "p"},
                            {"targetIndexName": "docs-index-dev", "parentKeyFieldName": "q"}
                        ]}
                    }),
                ),
            ],
        };
        let tgt = EnvDocs {
            env: "prod".into(),
            bindings: env("prod", "s-prod", "f-prod", &[]),
            docs: vec![doc(
                ResourceKind::Index,
                "docs-index",
                json!({"name": "docs-index", "fields": []}),
            )],
        };

        let plan = translate(&src, &tgt);
        let skillset = item(&plan, ResourceKind::Skillset, "sk");
        let paths: Vec<&str> = skillset.renamed.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                "indexProjections.selectors[0].targetIndexName",
                "indexProjections.selectors[1].targetIndexName"
            ],
            "one record per value rewritten, at its concrete path"
        );
        for selector in skillset.merged["indexProjections"]["selectors"]
            .as_array()
            .unwrap()
        {
            assert_eq!(selector["targetIndexName"], json!("docs-index"));
        }
    }

    #[test]
    fn knowledge_source_storage_and_embedding_host_are_rewired() {
        let ks = |acct: &str, host: &str| {
            doc(
                ResourceKind::KnowledgeSource,
                "docs-ks",
                json!({
                    "name": "docs-ks",
                    "kind": "azureBlob",
                    "azureBlobParameters": {
                        "connectionString": conn_string(acct),
                        "containerName": "c",
                        "ingestionParameters": {
                            "embeddingModel": {"azureOpenAIParameters": {
                                "resourceUri": format!("https://{host}.openai.azure.com"),
                                "deploymentId": "embed"
                            }}
                        }
                    }
                }),
            )
        };
        let src = EnvDocs {
            env: "dev".into(),
            bindings: env(
                "dev",
                "s-dev",
                "f-dev",
                &[("docs", BindingType::Storage, DEV_ACCT)],
            ),
            docs: vec![ks(DEV_ACCT, "f-dev")],
        };
        let tgt = EnvDocs {
            env: "prod".into(),
            bindings: env(
                "prod",
                "s-prod",
                "f-prod",
                &[("docs", BindingType::Storage, PROD_ACCT)],
            ),
            docs: vec![],
        };

        let plan = translate(&src, &tgt);
        assert!(plan.pending.is_empty(), "{:?}", plan.pending);
        let item = &plan.items[0];
        assert_eq!(
            item.merged["azureBlobParameters"]["connectionString"],
            json!(conn_string(PROD_ACCT))
        );
        assert_eq!(
            item.merged["azureBlobParameters"]["ingestionParameters"]["embeddingModel"]["azureOpenAIParameters"]
                ["resourceUri"],
            json!("https://f-prod.openai.azure.com")
        );
        let bindings: Vec<&str> = item.rewired.iter().map(|r| r.binding.as_str()).collect();
        assert_eq!(bindings, vec!["docs", "foundry"], "{:?}", item.rewired);
    }

    #[test]
    fn knowledge_base_models_are_rewired_and_its_sources_follow_the_rename() {
        let src = EnvDocs {
            env: "dev".into(),
            bindings: env("dev", "s-dev", "f-dev", &[]),
            docs: vec![
                doc(
                    ResourceKind::KnowledgeSource,
                    "docs-ks",
                    json!({"name": "docs-ks-dev", "kind": "searchIndex"}),
                ),
                doc(
                    ResourceKind::KnowledgeBase,
                    "kb",
                    json!({
                        "name": "kb",
                        "knowledgeSources": [{"name": "docs-ks-dev", "kind": "searchIndex"}],
                        "models": [{"azureOpenAIParameters": {
                            "resourceUri": "https://f-dev.openai.azure.com",
                            "deploymentId": "chat"
                        }}]
                    }),
                ),
            ],
        };
        let tgt = EnvDocs {
            env: "prod".into(),
            bindings: env("prod", "s-prod", "f-prod", &[]),
            docs: vec![doc(
                ResourceKind::KnowledgeSource,
                "docs-ks",
                json!({"name": "docs-ks", "kind": "searchIndex"}),
            )],
        };

        let plan = translate(&src, &tgt);
        assert!(plan.pending.is_empty(), "{:?}", plan.pending);
        let kb = item(&plan, ResourceKind::KnowledgeBase, "kb");
        assert_eq!(
            kb.merged["models"][0]["azureOpenAIParameters"]["resourceUri"],
            json!("https://f-prod.openai.azure.com")
        );
        assert_eq!(kb.rewired[0].binding, "foundry");
        assert_eq!(kb.rewired[0].target, Target::ModelHost);
        assert_eq!(kb.merged["knowledgeSources"][0]["name"], json!("docs-ks"));
        assert_eq!(kb.renamed.len(), 1, "{:?}", kb.renamed);
        assert_eq!(kb.renamed[0].path, "knowledgeSources[0].name");
        assert_eq!(kb.renamed[0].kind, ResourceKind::KnowledgeSource);
        assert_eq!(kb.renamed[0].from, "docs-ks-dev");
        assert_eq!(kb.renamed[0].to, "docs-ks");
    }

    #[test]
    fn connection_target_url_is_rewired_to_the_target_search_service() {
        let mcp = |svc: &str, kb: &str| {
            format!(
                "https://{svc}.search.windows.net/knowledgebases/{kb}/mcp?api-version={SEARCH_PREVIEW_API_VERSION}"
            )
        };
        let src = EnvDocs {
            env: "dev".into(),
            bindings: env("dev", "s-dev", "f-dev", &[]),
            docs: vec![
                doc(ResourceKind::KnowledgeBase, "kb", json!({"name": "kb-dev"})),
                doc(
                    ResourceKind::Connection,
                    "kb-conn",
                    json!({
                        "name": "kb-conn",
                        "properties": {
                            "category": "CustomKeys",
                            "authType": "AAD",
                            "target": mcp("s-dev", "kb-dev")
                        }
                    }),
                ),
            ],
        };
        let tgt = EnvDocs {
            env: "prod".into(),
            bindings: env("prod", "s-prod", "f-prod", &[]),
            docs: vec![doc(
                ResourceKind::KnowledgeBase,
                "kb",
                json!({"name": "kb"}),
            )],
        };

        let plan = translate(&src, &tgt);
        assert!(plan.pending.is_empty(), "{:?}", plan.pending);
        let conn = item(&plan, ResourceKind::Connection, "kb-conn");
        assert_eq!(
            conn.merged["properties"]["target"],
            json!(mcp("s-prod", "kb")),
            "both the search service and the renamed knowledge base follow"
        );
        assert_eq!(conn.rewired.len(), 1);
        assert_eq!(conn.rewired[0].path, "properties.target");
        assert_eq!(conn.rewired[0].binding, "search");
        assert_eq!(conn.rewired[0].target, Target::SearchService);
        assert_eq!(conn.rewired[0].to, "s-prod");
    }

    #[test]
    fn agent_model_and_connection_references_follow_the_target_names() {
        let src = EnvDocs {
            env: "dev".into(),
            bindings: env("dev", "s-dev", "f-dev", &[]),
            docs: vec![
                doc(
                    ResourceKind::Agent,
                    "regulus",
                    json!({
                        "name": "Regulus",
                        "model": "gpt-5-mini-dev",
                        "tools": [{"type": "azure_ai_search", "project_connection_id": "aoai-dev"}]
                    }),
                ),
                doc(
                    ResourceKind::Deployment,
                    "chat",
                    json!({"name": "gpt-5-mini-dev", "properties": {"model": {"name": "gpt-5-mini"}}}),
                ),
                doc(
                    ResourceKind::Connection,
                    "aoai",
                    json!({"name": "aoai-dev", "properties": {"category": "AzureOpenAI"}}),
                ),
            ],
        };
        let tgt = EnvDocs {
            env: "prod".into(),
            bindings: env("prod", "s-prod", "f-prod", &[]),
            docs: vec![
                doc(
                    ResourceKind::Deployment,
                    "chat",
                    json!({"name": "gpt-5-mini", "properties": {"model": {"name": "gpt-5-mini"}}}),
                ),
                doc(
                    ResourceKind::Connection,
                    "aoai",
                    json!({"name": "aoai", "properties": {"category": "AzureOpenAI"}}),
                ),
            ],
        };

        let plan = translate(&src, &tgt);
        assert!(plan.pending.is_empty(), "{:?}", plan.pending);
        let agent = item(&plan, ResourceKind::Agent, "regulus");
        assert_eq!(agent.merged["model"], json!("gpt-5-mini"));
        assert_eq!(
            agent.merged["tools"][0]["project_connection_id"],
            json!("aoai")
        );
        let renamed: Vec<(&str, &str, &str)> = agent
            .renamed
            .iter()
            .map(|r| (r.path.as_str(), r.from.as_str(), r.to.as_str()))
            .collect();
        assert_eq!(
            renamed,
            vec![
                ("model", "gpt-5-mini-dev", "gpt-5-mini"),
                ("tools[0].project_connection_id", "aoai-dev", "aoai")
            ]
        );
        assert_eq!(
            agent.merged["name"],
            json!("Regulus"),
            "a new-in-target agent keeps the source's name"
        );
    }

    #[test]
    fn sibling_names_swapped_between_environments_are_not_collapsed() {
        // dev `a` = `ks-1`, `b` = `ks-2`; prod has them the other way round.
        // Each reference must be mapped by the value it had BEFORE any
        // rewrite — a whole-document rename pass would rewrite `ks-1` to
        // `ks-2` and then that same value back to `ks-1`.
        let ks = |stem: &str, name: &str| {
            doc(
                ResourceKind::KnowledgeSource,
                stem,
                json!({"name": name, "kind": "searchIndex"}),
            )
        };
        let kb = |first: &str, second: &str| {
            doc(
                ResourceKind::KnowledgeBase,
                "kb",
                json!({
                    "name": "kb",
                    "knowledgeSources": [{"name": first}, {"name": second}]
                }),
            )
        };
        let src = EnvDocs {
            env: "dev".into(),
            bindings: env("dev", "s-dev", "f-dev", &[]),
            docs: vec![ks("a", "ks-1"), ks("b", "ks-2"), kb("ks-1", "ks-2")],
        };
        let tgt = EnvDocs {
            env: "prod".into(),
            bindings: env("prod", "s-prod", "f-prod", &[]),
            docs: vec![ks("a", "ks-2"), ks("b", "ks-1"), kb("ks-2", "ks-1")],
        };

        let plan = translate(&src, &tgt);
        assert!(plan.pending.is_empty(), "{:?}", plan.pending);
        let kb = item(&plan, ResourceKind::KnowledgeBase, "kb");
        assert_eq!(
            kb.merged["knowledgeSources"],
            json!([{"name": "ks-2"}, {"name": "ks-1"}]),
            "each reference follows its OWN sibling across the swap"
        );
        let renamed: Vec<(&str, &str, &str, &str)> = kb
            .renamed
            .iter()
            .map(|r| {
                (
                    r.path.as_str(),
                    r.stem.as_str(),
                    r.from.as_str(),
                    r.to.as_str(),
                )
            })
            .collect();
        assert_eq!(
            renamed,
            vec![
                ("knowledgeSources[0].name", "a", "ks-1", "ks-2"),
                ("knowledgeSources[1].name", "b", "ks-2", "ks-1"),
            ]
        );
        assert_eq!(kb.change(), Change::Unchanged);
    }

    #[test]
    fn a_chain_of_sibling_renames_maps_each_value_by_its_own_source_name() {
        // dev x/y/z → prod y/z/w: applied as whole-document passes, `x`
        // would be rewritten to `y`, then to `z`, then to `w`.
        let ks = |stem: &str, name: &str| {
            doc(
                ResourceKind::KnowledgeSource,
                stem,
                json!({"name": name, "kind": "searchIndex"}),
            )
        };
        let src = EnvDocs {
            env: "dev".into(),
            bindings: env("dev", "s-dev", "f-dev", &[]),
            docs: vec![
                ks("a", "x"),
                ks("b", "y"),
                ks("c", "z"),
                doc(
                    ResourceKind::KnowledgeBase,
                    "kb",
                    json!({
                        "name": "kb",
                        "knowledgeSources": [{"name": "x"}, {"name": "y"}, {"name": "z"}]
                    }),
                ),
            ],
        };
        let tgt = EnvDocs {
            env: "prod".into(),
            bindings: env("prod", "s-prod", "f-prod", &[]),
            docs: vec![ks("a", "y"), ks("b", "z"), ks("c", "w")],
        };

        let plan = translate(&src, &tgt);
        let kb = item(&plan, ResourceKind::KnowledgeBase, "kb");
        assert_eq!(
            kb.merged["knowledgeSources"],
            json!([{"name": "y"}, {"name": "z"}, {"name": "w"}])
        );
    }

    #[test]
    fn a_kept_function_key_carrier_leaves_a_redacted_code_placeholder() {
        // The target authenticates with a key in the URI: promote keeps the
        // annotation, and the translated URI must carry the placeholder the
        // auth gate looks for — not read as an anonymous endpoint.
        let src = EnvDocs {
            env: "dev".into(),
            bindings: env(
                "dev",
                "s-dev",
                "f-dev",
                &[("enrich-fn", BindingType::FunctionApp, "fn-dev")],
            ),
            docs: vec![doc(
                ResourceKind::Skillset,
                "sk",
                json!({
                    "name": "sk",
                    "skills": [{
                        "@odata.type": "#Microsoft.Skills.Custom.WebApiSkill",
                        "name": "enrich",
                        "uri": "https://fn-dev.azurewebsites.net/api/enrich?code=<redacted>",
                        "x-rigg-auth": "function-key",
                        "inputs": [],
                        "outputs": []
                    }]
                }),
            )],
        };
        let tgt = EnvDocs {
            env: "prod".into(),
            bindings: env(
                "prod",
                "s-prod",
                "f-prod",
                &[("enrich-fn", BindingType::FunctionApp, "fn-prod")],
            ),
            docs: vec![doc(
                ResourceKind::Skillset,
                "sk",
                json!({
                    "name": "sk",
                    "skills": [{
                        "@odata.type": "#Microsoft.Skills.Custom.WebApiSkill",
                        "name": "enrich",
                        "uri": "https://fn-prod.azurewebsites.net/api/enrich?code=<redacted>",
                        "x-rigg-auth": "function-key",
                        "inputs": [],
                        "outputs": []
                    }]
                }),
            )],
        };

        let plan = translate(&src, &tgt);
        let item = &plan.items[0];
        assert_eq!(
            item.merged["skills"][0]["x-rigg-auth"],
            json!("function-key")
        );
        assert_eq!(
            item.merged["skills"][0]["uri"],
            json!("https://fn-prod.azurewebsites.net/api/enrich?code=<redacted>"),
            "the kept key carrier leaves the placeholder the auth gate reads"
        );
        assert_eq!(item.change(), Change::Unchanged, "{:?}", item.merged);
    }

    #[test]
    fn connection_targets_that_are_not_kb_mcp_urls_are_rewired_too() {
        let src = EnvDocs {
            env: "dev".into(),
            bindings: env("dev", "s-dev", "f-dev", &[]),
            docs: vec![
                doc(
                    ResourceKind::Connection,
                    "search",
                    json!({
                        "name": "search",
                        "properties": {
                            "category": "CognitiveSearch",
                            "authType": "AAD",
                            "target": "https://s-dev.search.windows.net"
                        }
                    }),
                ),
                doc(
                    ResourceKind::Connection,
                    "aoai",
                    json!({
                        "name": "aoai",
                        "properties": {
                            "category": "AzureOpenAI",
                            "authType": "AAD",
                            "target": "https://f-dev.openai.azure.com/"
                        }
                    }),
                ),
            ],
        };
        let tgt = EnvDocs {
            env: "prod".into(),
            bindings: env("prod", "s-prod", "f-prod", &[]),
            docs: vec![],
        };

        let plan = translate(&src, &tgt);
        assert!(plan.pending.is_empty(), "{:?}", plan.pending);

        let search = item(&plan, ResourceKind::Connection, "search");
        assert_eq!(
            search.merged["properties"]["target"],
            json!("https://s-prod.search.windows.net")
        );
        assert_eq!(search.rewired[0].binding, "search");
        assert_eq!(search.rewired[0].target, Target::SearchService);
        assert_eq!(search.rewired[0].to, "s-prod");

        let aoai = item(&plan, ResourceKind::Connection, "aoai");
        assert_eq!(
            aoai.merged["properties"]["target"],
            json!("https://f-prod.openai.azure.com/"),
            "a model-host target rewires through the implicit foundry binding"
        );
        assert_eq!(aoai.rewired[0].binding, "foundry");
        assert_eq!(aoai.rewired[0].target, Target::ModelHost);
    }

    #[test]
    fn an_agent_tool_server_url_that_is_a_function_endpoint_is_rewired() {
        let src = EnvDocs {
            env: "dev".into(),
            bindings: env(
                "dev",
                "s-dev",
                "f-dev",
                &[("tools-fn", BindingType::FunctionApp, "fn-dev")],
            ),
            docs: vec![doc(
                ResourceKind::Agent,
                "regulus",
                json!({
                    "name": "Regulus",
                    "model": "gpt-5-mini",
                    "tools": [{"type": "mcp", "server_url": "https://fn-dev.azurewebsites.net/runtime/webhooks/mcp"}]
                }),
            )],
        };
        let tgt = EnvDocs {
            env: "prod".into(),
            bindings: env(
                "prod",
                "s-prod",
                "f-prod",
                &[("tools-fn", BindingType::FunctionApp, "fn-prod")],
            ),
            docs: vec![],
        };

        let plan = translate(&src, &tgt);
        assert!(plan.pending.is_empty(), "{:?}", plan.pending);
        let agent = item(&plan, ResourceKind::Agent, "regulus");
        assert_eq!(
            agent.merged["tools"][0]["server_url"],
            json!("https://fn-prod.azurewebsites.net/runtime/webhooks/mcp")
        );
        assert_eq!(agent.rewired[0].binding, "tools-fn");
        assert_eq!(agent.rewired[0].target, Target::FunctionApp);
    }
}
