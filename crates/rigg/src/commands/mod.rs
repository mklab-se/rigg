//! Command implementations and shared context.

pub mod adopt;
pub mod ai;
pub mod ai_assist;
pub mod ask;
pub mod auth;
pub mod az;
pub mod ci;
pub mod completion;
pub mod concepts;
pub mod copy;
pub mod credentials;
pub mod delete;
pub mod describe;
pub mod dev;
pub mod dev_spec;
pub mod diff;
pub mod discovery;
pub mod doctor;
pub mod env;
pub mod init;
pub mod interactive;
pub mod mcp_cmd;
pub mod migrate;
pub mod new;
pub mod promote;
pub mod pull;
pub mod push;
pub mod remote;
pub mod skill;
pub mod status;
pub mod validate;
pub mod version;

use std::io::IsTerminal;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use rigg_core::workspace::{Project, ResolvedEnv, Workspace};

use crate::cli::{Cli, OutputFormat};

/// Human-readable prose: stdout in text mode, stderr when `--output json`
/// (stdout must then carry only JSON documents). Every "explain, then act"
/// narration line — plan items, progress, notes — goes through this so a
/// scripted `--output json` caller sees nothing on stdout but the JSON
/// documents it asked for, while a human still sees the same narration (on
/// stderr) as the command runs.
#[macro_export]
macro_rules! say {
    ($ctx:expr) => {
        if $ctx.json() { eprintln!() } else { println!() }
    };
    ($ctx:expr, $($arg:tt)*) => {
        if $ctx.json() { eprintln!($($arg)*) } else { println!($($arg)*) }
    };
}

/// Standardized process exit codes (documented, stable, scriptable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    Success = 0,
    Error = 1,
    Usage = 2,
    ValidationFailed = 3,
    AuthDenied = 4,
    DriftOrConflict = 5,
    NeedsInput = 6,
}

impl From<ExitCode> for std::process::ExitCode {
    fn from(code: ExitCode) -> Self {
        std::process::ExitCode::from(code as u8)
    }
}

/// Typed command failure that maps to a specific exit code.
#[derive(Debug, thiserror::Error)]
#[allow(dead_code)] // AuthDenied is mapped from client errors today; kept for doctor (0.19)
pub enum CommandError {
    #[error("{0}")]
    Validation(String),
    #[error("{0}")]
    AuthDenied(String),
    #[error("{0}")]
    DriftOrConflict(String),
    #[error("{0}")]
    Usage(String),
    #[error("{0}")]
    NeedsInput(ask::NeedsInput),
}

/// Map a command result to the process exit code, printing errors to
/// stderr. `CommandError::NeedsInput` (or a bare `ask::NeedsInput`,
/// produced directly by a `ScriptedAsker`) is special-cased: the
/// `needs-input` protocol document goes to **stdout** so a scripted caller
/// can parse it, and the human-readable summary goes to stderr only in text
/// mode.
pub fn exit_code_for(result: Result<()>, output: OutputFormat) -> ExitCode {
    match result {
        Ok(()) => ExitCode::Success,
        Err(err) => {
            let needs_input = match err.downcast_ref::<CommandError>() {
                Some(CommandError::NeedsInput(ni)) => Some(ni),
                _ => err.downcast_ref::<ask::NeedsInput>(),
            };
            if let Some(ni) = needs_input {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&ni.to_json()).unwrap_or_default()
                );
                if output == OutputFormat::Text {
                    eprintln!(
                        "{ni}\nAnswer with --answer <id>=<value> (or --answers-file) and re-run."
                    );
                }
                return ExitCode::NeedsInput;
            }
            let code = match err.downcast_ref::<CommandError>() {
                Some(CommandError::Validation(_)) => ExitCode::ValidationFailed,
                Some(CommandError::AuthDenied(_)) => ExitCode::AuthDenied,
                Some(CommandError::DriftOrConflict(_)) => ExitCode::DriftOrConflict,
                Some(CommandError::Usage(_)) => ExitCode::Usage,
                Some(CommandError::NeedsInput(_)) => ExitCode::NeedsInput,
                None => match err.downcast_ref::<rigg_client::error::ClientError>() {
                    Some(ce) if is_auth_error(ce) => ExitCode::AuthDenied,
                    _ => ExitCode::Error,
                },
            };
            eprintln!("Error: {err:#}");
            code
        }
    }
}

pub fn is_auth_error(err: &rigg_client::error::ClientError) -> bool {
    matches!(
        err,
        rigg_client::error::ClientError::Api {
            status: 401 | 403,
            ..
        } | rigg_client::error::ClientError::Auth(_)
            | rigg_client::error::ClientError::Forbidden { .. }
    )
}

/// Global flags resolved once per invocation.
#[derive(Debug, Clone)]
pub struct GlobalContext {
    pub env: Option<String>,
    pub output: OutputFormat,
    pub yes: bool,
    pub non_interactive: bool,
    pub no_color: bool,
    #[allow(dead_code)] // reserved for quiet-mode output tuning
    pub quiet: bool,
    pub no_ai: bool,
    /// Answers supplied via `--answer id=value` / `--answers-file`, keyed
    /// by question id. Validated against `ask::KNOWN_ID_PREFIXES` in
    /// `from_cli`.
    pub answers: std::collections::BTreeMap<String, String>,
}

impl GlobalContext {
    pub fn from_cli(cli: &Cli) -> Result<Self> {
        let answers = ask::load_answers(&cli.answer, cli.answers_file.as_deref())
            .map_err(|e| anyhow!(CommandError::Usage(e.to_string())))?;
        for id in answers.keys() {
            if !ask::KNOWN_ID_PREFIXES.iter().any(|p| id.starts_with(p)) {
                return Err(anyhow!(CommandError::Usage(format!(
                    "unknown answer id '{id}': no question with this id is known"
                ))));
            }
        }
        Ok(GlobalContext {
            env: cli.env.clone(),
            output: cli.output,
            yes: cli.yes,
            non_interactive: cli.non_interactive
                || std::env::var("RIGG_NON_INTERACTIVE").is_ok()
                || !std::io::stdin().is_terminal()
                || !std::io::stdout().is_terminal(),
            no_color: cli.no_color,
            quiet: cli.quiet,
            no_ai: cli.no_ai,
            answers,
        })
    }

    pub fn json(&self) -> bool {
        self.output == OutputFormat::Json
    }

    /// May we prompt the user interactively?
    pub fn interactive(&self) -> bool {
        !self.non_interactive && !self.yes && !self.json()
    }

    /// A `ScriptedAsker` pre-loaded with this invocation's `--answer` /
    /// `--answers-file` answers, for a command that needs to ask questions.
    pub fn scripted_asker(&self, command: &str, context: serde_json::Value) -> ask::ScriptedAsker {
        ask::ScriptedAsker::new(self.answers.clone(), command, context)
    }

    /// The `Asker` a command should use to ask `Question`s: interactive when
    /// this invocation may prompt ([`Self::interactive`]), otherwise scripted
    /// from `--answer` / `--answers-file`, returning `NeedsInput` (exit 6)
    /// for anything still missing.
    pub fn asker(&self, command: &str, context: serde_json::Value) -> Box<dyn ask::Asker> {
        if self.interactive() {
            Box::new(ask::InteractiveAsker::new(
                self.answers.clone(),
                self.no_color,
            ))
        } else {
            Box::new(self.scripted_asker(command, context))
        }
    }
}

/// Load the workspace from the current directory (walking up).
pub fn load_workspace() -> Result<Workspace> {
    load_workspace_from(Path::new("."))
}

pub fn load_workspace_from(start: &Path) -> Result<Workspace> {
    Workspace::discover(start).context(
        "not inside a rigg workspace (run `rigg init` to create one, or cd into a workspace)",
    )
}

/// Text-mode hint printed when the workspace has no projects yet.
pub fn print_no_projects_hint() {
    println!(
        "No projects yet. A project groups the resources you manage together —\n\
         see `rigg concepts`, then `rigg new project <name>`."
    );
}

/// Resolve which projects a command operates on from `[PROJECT]` / `--all`.
pub fn select_projects<'w>(
    ws: &'w Workspace,
    project: Option<&str>,
    all: bool,
) -> Result<Vec<&'w Project>> {
    match (project, all) {
        (Some(_), true) => Err(anyhow!(CommandError::Usage(
            "pass either a project name or --all, not both".to_string()
        ))),
        (Some(name), false) => Ok(vec![ws.project(name)?]),
        (None, true) => {
            if ws.projects.is_empty() {
                bail!("workspace has no projects (create one with `rigg new project <name>`)");
            }
            Ok(ws.projects.iter().collect())
        }
        (None, false) => match ws.projects.len() {
            0 => bail!("workspace has no projects (create one with `rigg new project <name>`)"),
            1 => Ok(vec![&ws.projects[0]]),
            n => Err(anyhow!(CommandError::Usage(format!(
                "workspace has {n} projects; name one or pass --all"
            )))),
        },
    }
}

/// Resolve exactly one project: the named one, or the workspace's only one
/// when the name is omitted — the CLI-wide single-project convention.
pub fn select_one_project<'w>(ws: &'w Workspace, project: Option<&str>) -> Result<&'w Project> {
    match project {
        Some(name) => Ok(ws.project(name)?),
        None => match ws.projects.len() {
            0 => bail!("workspace has no projects (create one with `rigg new project <name>`)"),
            1 => Ok(&ws.projects[0]),
            n => Err(anyhow!(CommandError::Usage(format!(
                "workspace has {n} projects; name one"
            )))),
        },
    }
}

/// Resolve the environment for this invocation.
pub fn resolve_env(ws: &Workspace, ctx: &GlobalContext) -> Result<ResolvedEnv> {
    Ok(ws.resolve_env(ctx.env.as_deref())?)
}

/// Resolve the environment for commands where silently acting on the wrong
/// one is costly (adopt): an explicit selection (`--env` / `RIGG_ENV`) always
/// wins and a lone configured environment is used as-is, but with several
/// environments the user must choose — interactively when possible, otherwise
/// a usage error naming the candidates. The `default: true` marker is
/// deliberately NOT enough here.
pub fn resolve_env_or_choose(
    ws: &Workspace,
    ctx: &GlobalContext,
    verb: &str,
) -> Result<ResolvedEnv> {
    if ctx.env.is_some() || std::env::var("RIGG_ENV").is_ok() {
        return resolve_env(ws, ctx);
    }
    let names: Vec<String> = ws.config.environments.keys().cloned().collect();
    match names.as_slice() {
        [] => resolve_env(ws, ctx), // standard "no environment" error
        [only] => Ok(ws.resolve_env(Some(only))?),
        _ if ctx.interactive() => {
            let default = ws.default_env_name();
            let labels: Vec<String> = names
                .iter()
                .map(|n| {
                    if Some(n.as_str()) == default {
                        format!("{n} (default)")
                    } else {
                        n.clone()
                    }
                })
                .collect();
            let picked = interactive::select(
                &format!("{verb} from which environment?"),
                labels,
                ctx.no_color,
            )?;
            let name = picked.trim_end_matches(" (default)");
            Ok(ws.resolve_env(Some(name))?)
        }
        _ => Err(anyhow!(CommandError::Usage(format!(
            "multiple environments configured ({}); pass --env <name> (or set RIGG_ENV) to say which one to {} from",
            names.join(", "),
            verb.to_lowercase(),
        )))),
    }
}

/// Gate a cloud-mutating operation (`push` apply/`--prune`, `delete
/// --remote`) against an environment's `policy.protected` flag, speaking the
/// question protocol: the gate is an `ask::Question::confirm_env` put to
/// `ctx.asker()`, so it composes with `--answer` / `--answers-file` and
/// `--yes` like any other question.
///
/// Returns `Ok(true)` when the operation may proceed, `Ok(false)` when the
/// user declined via an interactive typed-name mismatch — callers should
/// print "Aborted." and return `Ok(())`, matching every other decline in the
/// CLI (e.g. answering `n` to "Apply N change(s)?").
///
/// No-op (`Ok(true)`) when the environment is unprotected. Otherwise:
/// - `--confirm-env <name>` is pre-supplied to the asker as the answer to
///   `confirm.protected.<env>`, so it matches the environment name exactly
///   the same way a typed confirmation or `--answer` would; a mismatch here
///   is a usage error (`Err(CommandError::Usage)`, exit 2) naming the
///   expected environment, not a silent decline.
/// - Interactive session (no `--confirm-env`) → prompts the user to type the
///   environment name; a mismatch → `Ok(false)`.
/// - Non-interactive session (incl. `--yes`) with no answer → the scripted
///   asker returns `NeedsInput`, which propagates as `Err` and maps to exit
///   code 6 — the caller answers with `--confirm-env` / `--answer` and
///   re-runs.
///
/// `ctx.yes` (`--yes`) is deliberately **not** consulted: `--yes` exists to
/// skip the routine "apply N changes?" prompt, and scripts/agents reach for
/// it reflexively. If it also satisfied this gate, a protected environment
/// would be no safer than an unprotected one the moment someone habitually
/// pipes `-y` into their commands. Protection must be opted into explicitly,
/// per invocation, via a typed name (interactive) or `--confirm-env` /
/// `--answer` (non-interactive) — never implied by a blanket "yes to
/// everything" flag. `--yes` makes the session non-interactive
/// ([`GlobalContext::interactive`]), so the scripted asker runs regardless.
pub fn confirm_protected_env(
    ctx: &GlobalContext,
    env: &ResolvedEnv,
    confirm_env: Option<&str>,
    operation: &str,
) -> Result<bool> {
    if !env.protected() {
        return Ok(true);
    }
    let question = ask::Question::confirm_env(&env.name, operation);
    let mut scoped = ctx.clone();
    if let Some(name) = confirm_env {
        scoped.answers.insert(question.id.clone(), name.to_string());
    }
    let mut asker = scoped.asker(operation, serde_json::json!({"env": env.name}));
    match asker.ask(&question) {
        Ok(answer) => Ok(answer.as_bool().unwrap_or(false)),
        // A pre-supplied `--confirm-env` that fails coercion (wrong name) is
        // a usage error (exit 2), not a generic failure (exit 1) — it names
        // the environment `--confirm-env` must match exactly. An answer that
        // came from an interactive prompt instead (confirm_env is None here)
        // is left as-is so NeedsInput (exit 6) still propagates untouched.
        Err(_) if confirm_env.is_some() => Err(anyhow!(CommandError::Usage(format!(
            "--confirm-env must equal the environment name '{}' exactly",
            env.name
        )))),
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_are_stable() {
        assert_eq!(ExitCode::Success as u8, 0);
        assert_eq!(ExitCode::Error as u8, 1);
        assert_eq!(ExitCode::Usage as u8, 2);
        assert_eq!(ExitCode::ValidationFailed as u8, 3);
        assert_eq!(ExitCode::AuthDenied as u8, 4);
        assert_eq!(ExitCode::DriftOrConflict as u8, 5);
        assert_eq!(ExitCode::NeedsInput as u8, 6);
    }

    #[test]
    fn command_errors_map_to_codes() {
        let v: Result<()> = Err(anyhow!(CommandError::Validation("bad".into())));
        assert_eq!(
            exit_code_for(v, OutputFormat::Text),
            ExitCode::ValidationFailed
        );
        let d: Result<()> = Err(anyhow!(CommandError::DriftOrConflict("drift".into())));
        assert_eq!(
            exit_code_for(d, OutputFormat::Text),
            ExitCode::DriftOrConflict
        );
        let u: Result<()> = Err(anyhow!(CommandError::Usage("usage".into())));
        assert_eq!(exit_code_for(u, OutputFormat::Text), ExitCode::Usage);
        let e: Result<()> = Err(anyhow!("boom"));
        assert_eq!(exit_code_for(e, OutputFormat::Text), ExitCode::Error);
        assert_eq!(exit_code_for(Ok(()), OutputFormat::Text), ExitCode::Success);
    }

    #[test]
    fn needs_input_maps_to_exit_6_bare_or_wrapped() {
        let bare: Result<()> = Err(anyhow!(ask::NeedsInput {
            command: "promote".into(),
            context: serde_json::json!({}),
            questions: vec![ask::Question::text("x", "?")],
        }));
        assert_eq!(
            exit_code_for(bare, OutputFormat::Text),
            ExitCode::NeedsInput
        );

        let wrapped: Result<()> = Err(anyhow!(CommandError::NeedsInput(ask::NeedsInput {
            command: "promote".into(),
            context: serde_json::json!({}),
            questions: vec![ask::Question::text("x", "?")],
        })));
        assert_eq!(
            exit_code_for(wrapped, OutputFormat::Text),
            ExitCode::NeedsInput
        );
    }
}
