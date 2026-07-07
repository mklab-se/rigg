//! `rigg ci init <provider>` — scaffold CI/CD workflows (spec §8.3).

use anyhow::{Result, anyhow, bail};
use colored::Colorize;

use crate::cli::CiCommands;
use crate::commands::{CommandError, GlobalContext, load_workspace};

const VALIDATE_YML: &str = include_str!("ci_templates/rigg-validate.yml");
const DEPLOY_YML: &str = include_str!("ci_templates/rigg-deploy.yml");
const DRIFT_YML: &str = include_str!("ci_templates/rigg-drift.yml");

pub fn run(_ctx: &GlobalContext, cmd: CiCommands) -> Result<()> {
    match cmd {
        CiCommands::Init { provider, force } => init(&provider, force),
    }
}

fn init(provider: &str, force: bool) -> Result<()> {
    if provider != "github" {
        return Err(anyhow!(CommandError::Usage(format!(
            "unsupported CI provider '{provider}' (supported: github; Azure DevOps templates are documented in docs/)"
        ))));
    }
    let ws = load_workspace()?;
    let env = ws.default_env_name().unwrap_or("dev").to_string();
    let dir = ws.root.join(".github").join("workflows");
    std::fs::create_dir_all(&dir)?;

    let files = [
        ("rigg-validate.yml", VALIDATE_YML),
        ("rigg-deploy.yml", DEPLOY_YML),
        ("rigg-drift.yml", DRIFT_YML),
    ];
    for (name, template) in files {
        let path = dir.join(name);
        if path.exists() && !force {
            bail!(
                "{} already exists — pass --force to overwrite",
                path.display()
            );
        }
        std::fs::write(&path, template.replace("{{RIGG_ENV}}", &env))?;
        println!("  created {}", path.display());
    }

    println!();
    println!(
        "{} GitHub workflows created. To finish setup:",
        "✓".green().bold()
    );
    println!("  1. Create an Entra app registration with federated credentials for this repo");
    println!("     (workload identity federation — no client secrets):");
    println!("       az ad app create --display-name rigg-deploy");
    println!(
        "       az ad app federated-credential create ... (subject: repo:<owner>/<repo>:ref:refs/heads/main)"
    );
    println!("  2. Grant it the data-plane roles rigg needs (Search Service Contributor,");
    println!(
        "     Search Index Data Contributor, Foundry Project Manager) — or run `rigg auth doctor`."
    );
    println!(
        "  3. Add repository variables: AZURE_CLIENT_ID, AZURE_TENANT_ID, AZURE_SUBSCRIPTION_ID."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_are_valid_yaml_with_placeholder() {
        for (name, t) in [
            ("validate", VALIDATE_YML),
            ("deploy", DEPLOY_YML),
            ("drift", DRIFT_YML),
        ] {
            assert!(t.contains("{{RIGG_ENV}}"), "{name} missing env placeholder");
            let replaced = t.replace("{{RIGG_ENV}}", "prod");
            let parsed: std::result::Result<serde_yaml::Value, _> = serde_yaml::from_str(&replaced);
            assert!(parsed.is_ok(), "{name} is not valid YAML: {parsed:?}");
        }
    }
}
