//! Local AI agent integration
//!
//! Invokes local CLI AI tools (claude, codex, copilot) as subprocesses
//! to generate text responses for diff explanations.

use std::time::Duration;

use hoist_core::AiProvider;
use tokio::process::Command;
use tracing::debug;

use crate::error::ClientError;

/// Timeout for local agent invocations (2 minutes)
const AGENT_TIMEOUT: Duration = Duration::from_secs(120);

/// Check if a local AI agent binary is available on the system
pub fn is_available(provider: &AiProvider) -> bool {
    let Some(binary) = provider.binary_name() else {
        return false;
    };
    which(binary)
}

/// Detect all available local AI agent providers on the system
pub fn detect_available_providers() -> Vec<AiProvider> {
    AiProvider::all()
        .iter()
        .filter(|p| match p {
            AiProvider::AzureOpenai => which("az"),
            AiProvider::Ollama => is_ollama_reachable_sync(),
            _ => is_available(p),
        })
        .cloned()
        .collect()
}

/// Send a prompt to a local AI agent and return the response text.
pub async fn generate_text(
    provider: &AiProvider,
    model: Option<&str>,
    system_prompt: &str,
    user_prompt: &str,
) -> Result<String, ClientError> {
    let output = match provider {
        AiProvider::Claude => invoke_claude(model, system_prompt, user_prompt).await?,
        AiProvider::Codex => invoke_codex(model, system_prompt, user_prompt).await?,
        AiProvider::Copilot => invoke_copilot(model, system_prompt, user_prompt).await?,
        _ => {
            return Err(ClientError::local_agent(format!(
                "{} is not a local CLI agent",
                provider.display_name()
            )));
        }
    };

    if output.trim().is_empty() {
        return Err(ClientError::local_agent(format!(
            "No response from {}. Verify the tool is configured and authenticated.",
            provider.display_name()
        )));
    }

    Ok(output)
}

/// Invoke Claude CLI in print mode with system prompt support
async fn invoke_claude(
    model: Option<&str>,
    system_prompt: &str,
    user_prompt: &str,
) -> Result<String, ClientError> {
    let mut cmd = Command::new("claude");
    cmd.arg("-p")
        .arg(user_prompt)
        .arg("--system-prompt")
        .arg(system_prompt)
        .arg("--output-format")
        .arg("text")
        .arg("--no-session-persistence");

    if let Some(m) = model {
        cmd.arg("--model").arg(m);
    }

    // Remove CLAUDECODE env var to avoid nested session detection
    cmd.env_remove("CLAUDECODE");

    debug!("Invoking claude CLI");
    run_command(cmd, "claude").await
}

/// Invoke OpenAI Codex CLI in exec mode
async fn invoke_codex(
    model: Option<&str>,
    system_prompt: &str,
    user_prompt: &str,
) -> Result<String, ClientError> {
    let combined = format!("{system_prompt}\n\n{user_prompt}");

    let mut cmd = Command::new("codex");
    cmd.arg("exec")
        .arg(&combined)
        .arg("-a")
        .arg("never")
        .arg("--sandbox")
        .arg("read-only")
        .arg("--ephemeral")
        .arg("--color")
        .arg("never");

    if let Some(m) = model {
        cmd.arg("--model").arg(m);
    }

    debug!("Invoking codex CLI");
    run_command(cmd, "codex").await
}

/// Invoke GitHub Copilot CLI in prompt mode
async fn invoke_copilot(
    model: Option<&str>,
    system_prompt: &str,
    user_prompt: &str,
) -> Result<String, ClientError> {
    let combined = format!("{system_prompt}\n\n{user_prompt}");

    let mut cmd = Command::new("copilot");
    cmd.arg("-p")
        .arg(&combined)
        .arg("--allow-all-tools")
        .arg("--no-ask-user")
        .arg("-s")
        .arg("--no-color");

    if let Some(m) = model {
        cmd.arg("--model").arg(m);
    }

    debug!("Invoking copilot CLI");
    run_command(cmd, "copilot").await
}

/// Run a command with timeout and capture stdout
async fn run_command(mut cmd: Command, name: &str) -> Result<String, ClientError> {
    let result = tokio::time::timeout(AGENT_TIMEOUT, cmd.output()).await;

    match result {
        Ok(Ok(output)) => {
            if output.status.success() {
                String::from_utf8(output.stdout).map_err(|e| {
                    ClientError::local_agent(format!("{name} produced invalid UTF-8: {e}"))
                })
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let code = output
                    .status
                    .code()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                Err(ClientError::local_agent(format!(
                    "{name} exited with code {code}: {stderr}"
                )))
            }
        }
        Ok(Err(e)) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                Err(ClientError::local_agent(format!(
                    "'{name}' not found. Install it or choose a different AI provider via `hoist ai init`."
                )))
            } else {
                Err(ClientError::local_agent(format!(
                    "Failed to start {name}: {e}"
                )))
            }
        }
        Err(_) => Err(ClientError::local_agent(format!(
            "{name} timed out after {} seconds",
            AGENT_TIMEOUT.as_secs()
        ))),
    }
}

/// Check if a binary exists on PATH
fn which(binary: &str) -> bool {
    std::process::Command::new("which")
        .arg(binary)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Synchronously check if Ollama server is reachable
fn is_ollama_reachable_sync() -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], 11434)),
        Duration::from_millis(500),
    )
    .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_which_existing_binary() {
        assert!(which("sh"));
    }

    #[test]
    fn test_which_nonexistent_binary() {
        assert!(!which("definitely-not-a-real-binary-12345"));
    }

    #[test]
    fn test_is_available_azure_returns_false() {
        assert!(!is_available(&AiProvider::AzureOpenai));
    }
}
