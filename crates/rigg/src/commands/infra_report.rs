//! One voice for classified infrastructure references.
//!
//! `rigg validate` and `rigg push`'s binding preflight run the *same*
//! classification ([`rigg_core::infra::classify`]) over the same documents,
//! so they must also say the same thing about what they found — this module
//! owns those message texts and their severity.

use rigg_core::binding::RESERVED_BINDING_NAMES;
use rigg_core::infra::{self, Class, Classified, Target};

/// How severely a classified reference should be reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Printed, but the command still succeeds.
    Warning,
    /// Fails the command (exit 3).
    Error,
}

/// What to report about one classified reference in `env`, or `None` when
/// there is nothing to report (`Bound`/`Shared` are healthy).
///
/// `display` names where the reference was found — a workspace-relative file
/// path in `validate`, a resource reference in `push`. `strict` is the
/// environment's `policy.strict-bindings` (which defaults to `protected`):
/// it promotes `Unbound`/`External` from warnings to errors. A `Leak` is an
/// error either way — it means the file points at another environment's
/// infrastructure.
pub fn classified_finding(
    c: &Classified,
    env: &str,
    display: &str,
    strict: bool,
) -> Option<(Level, String)> {
    let path = &c.found.path;
    let target = c.found.physical.target;
    let physical = &c.found.physical.physical;

    match &c.class {
        Class::Bound(_) | Class::Shared(..) => None,
        Class::Leak { binding, envs } => {
            let other_env = envs.first().map(String::as_str).unwrap_or("?");
            Some((
                Level::Error,
                format!(
                    "[{display}] {path} references {target} '{physical}', which is bound in \
                     environment '{other_env}' as '{binding}' but not in '{env}' — {}",
                    leak_hint(env, binding, target, physical)
                ),
            ))
        }
        Class::Unbound => Some((
            level(strict),
            format!(
                "[{display}] {path} references {target} '{physical}', which no environment \
                 binds — run `rigg env bind {env} --learn` to record it"
            ),
        )),
        Class::External => {
            let raw = c.found.physical.original.as_str().unwrap_or(physical);
            let origin = url_origin(raw);
            Some((
                level(strict),
                format!(
                    "[{display}] {path} calls external API '{origin}' — bind it as an api \
                     dependency to track it across environments"
                ),
            ))
        }
    }
}

fn level(strict: bool) -> Level {
    if strict { Level::Error } else { Level::Warning }
}

/// The fix suggested after a leak. An `env bind` suggestion is only useful
/// when the leaked resource *can* be a dependency binding: a match on the
/// other environment's implicit `search`/`foundry` target (or any reference
/// to a Search service) means the file names another environment's own
/// service, which is fixed by changing the target or the file — not by
/// declaring a dependency. Otherwise the suggestion must use a real binding
/// type keyword (`ai-services`, not the `model host` display word).
fn leak_hint(env: &str, binding: &str, target: Target, physical: &str) -> String {
    let implicit = RESERVED_BINDING_NAMES.contains(&binding) || target == Target::SearchService;
    if implicit {
        let noun = if binding == "foundry" {
            "Foundry account"
        } else {
            "Search service"
        };
        return format!(
            "the file points at another environment's {noun} — change this environment's target \
             or fix the file"
        );
    }
    match infra::binding_type_for(target) {
        Some(kind) => {
            format!("bind it (rigg env bind {env} {binding} {kind}:{physical}) or fix the file")
        }
        None => "change this environment's target or fix the file".to_string(),
    }
}

/// The scheme + host of a URL, dropping any path/query — `https://host` from
/// `https://host/path?query`.
pub fn url_origin(url: &str) -> String {
    match url.split_once("://") {
        Some((scheme, rest)) => {
            let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
            format!("{scheme}://{host}")
        }
        None => url.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rigg_core::infra::{FoundRef, PhysicalRef};
    use rigg_core::registry::InfraForm;
    use serde_json::json;

    fn classified(target: Target, physical: &str, class: Class) -> Classified {
        Classified {
            found: FoundRef {
                path: "skills[0].resourceUri".to_string(),
                form: InfraForm::OpenAiEndpoint,
                physical: PhysicalRef {
                    target,
                    physical: physical.to_string(),
                    original: json!(format!("https://{physical}.openai.azure.com")),
                    kb_name: None,
                },
            },
            class,
        }
    }

    #[test]
    fn model_host_leak_suggests_the_ai_services_keyword() {
        let c = classified(
            Target::ModelHost,
            "prodaifndr",
            Class::Leak {
                binding: "enrichment".to_string(),
                envs: vec!["prod".to_string()],
            },
        );
        let (level, msg) = classified_finding(&c, "dev", "f.json", false).unwrap();
        assert_eq!(level, Level::Error);
        assert!(
            msg.contains("rigg env bind dev enrichment ai-services:prodaifndr"),
            "{msg}"
        );
    }

    #[test]
    fn leak_onto_an_implicit_target_suggests_changing_the_target_not_a_binding() {
        for (binding, noun) in [("foundry", "Foundry account"), ("search", "Search service")] {
            let c = classified(
                Target::ModelHost,
                "prodaifndr",
                Class::Leak {
                    binding: binding.to_string(),
                    envs: vec!["prod".to_string()],
                },
            );
            let (_, msg) = classified_finding(&c, "dev", "f.json", false).unwrap();
            assert!(msg.contains(noun), "{msg}");
            assert!(!msg.contains("env bind"), "{msg}");
        }
    }

    #[test]
    fn unbound_names_the_validated_environment_and_follows_strictness() {
        let c = classified(Target::ModelHost, "orphan", Class::Unbound);
        let (level, msg) = classified_finding(&c, "prod", "f.json", false).unwrap();
        assert_eq!(level, Level::Warning);
        assert!(msg.contains("rigg env bind prod --learn"), "{msg}");
        let (level, _) = classified_finding(&c, "prod", "f.json", true).unwrap();
        assert_eq!(level, Level::Error);
    }

    #[test]
    fn bound_and_shared_report_nothing() {
        assert!(
            classified_finding(
                &classified(Target::ModelHost, "x", Class::Bound("foundry".into())),
                "dev",
                "f.json",
                true
            )
            .is_none()
        );
        assert!(
            classified_finding(
                &classified(
                    Target::ModelHost,
                    "x",
                    Class::Shared("foundry".into(), vec!["prod".into()])
                ),
                "dev",
                "f.json",
                true
            )
            .is_none()
        );
    }
}
