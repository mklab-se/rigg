//! Command implementations and shared context.

pub mod ai;
pub mod ai_assist;
pub mod auth;
pub mod ci;
pub mod completion;
pub mod confirm;
pub mod copy;
pub mod delete;
pub mod describe;
pub mod dev;
pub mod diff;
pub mod doctor;
pub mod env;
pub mod init;
pub mod mcp_cmd;
pub mod new;
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

/// Standardized process exit codes (documented, stable, scriptable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    Success = 0,
    Error = 1,
    Usage = 2,
    ValidationFailed = 3,
    AuthDenied = 4,
    DriftOrConflict = 5,
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
}

/// Map a command result to the process exit code, printing errors to stderr.
pub fn exit_code_for(result: Result<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::Success,
        Err(err) => {
            let code = match err.downcast_ref::<CommandError>() {
                Some(CommandError::Validation(_)) => ExitCode::ValidationFailed,
                Some(CommandError::AuthDenied(_)) => ExitCode::AuthDenied,
                Some(CommandError::DriftOrConflict(_)) => ExitCode::DriftOrConflict,
                Some(CommandError::Usage(_)) => ExitCode::Usage,
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

fn is_auth_error(err: &rigg_client::error::ClientError) -> bool {
    matches!(
        err,
        rigg_client::error::ClientError::Api {
            status: 401 | 403,
            ..
        }
    )
}

/// Global flags resolved once per invocation.
#[derive(Debug, Clone)]
pub struct GlobalContext {
    pub env: Option<String>,
    pub output: OutputFormat,
    pub yes: bool,
    pub non_interactive: bool,
    #[allow(dead_code)] // reserved for quiet-mode output tuning
    pub quiet: bool,
    pub no_ai: bool,
}

impl GlobalContext {
    pub fn from_cli(cli: &Cli) -> Self {
        GlobalContext {
            env: cli.env.clone(),
            output: cli.output,
            yes: cli.yes,
            non_interactive: cli.non_interactive || !std::io::stdout().is_terminal(),
            quiet: cli.quiet,
            no_ai: cli.no_ai,
        }
    }

    pub fn json(&self) -> bool {
        self.output == OutputFormat::Json
    }

    /// May we prompt the user interactively?
    pub fn interactive(&self) -> bool {
        !self.non_interactive && !self.yes
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

/// Resolve the environment for this invocation.
pub fn resolve_env(ws: &Workspace, ctx: &GlobalContext) -> Result<ResolvedEnv> {
    Ok(ws.resolve_env(ctx.env.as_deref())?)
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
    }

    #[test]
    fn command_errors_map_to_codes() {
        let v: Result<()> = Err(anyhow!(CommandError::Validation("bad".into())));
        assert_eq!(exit_code_for(v), ExitCode::ValidationFailed);
        let d: Result<()> = Err(anyhow!(CommandError::DriftOrConflict("drift".into())));
        assert_eq!(exit_code_for(d), ExitCode::DriftOrConflict);
        let u: Result<()> = Err(anyhow!(CommandError::Usage("usage".into())));
        assert_eq!(exit_code_for(u), ExitCode::Usage);
        let e: Result<()> = Err(anyhow!("boom"));
        assert_eq!(exit_code_for(e), ExitCode::Error);
        assert_eq!(exit_code_for(Ok(())), ExitCode::Success);
    }
}
