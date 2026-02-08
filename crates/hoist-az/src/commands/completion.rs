//! Shell completion generation

use anyhow::Result;
use clap::CommandFactory;
use clap_complete::{generate, Shell as ClapShell};

use crate::cli::{Cli, Shell};

pub fn run(shell: Shell) -> Result<()> {
    let mut cmd = Cli::command();
    let name = cmd.get_name().to_string();

    let clap_shell = match shell {
        Shell::Bash => ClapShell::Bash,
        Shell::Zsh => ClapShell::Zsh,
        Shell::Fish => ClapShell::Fish,
        Shell::Powershell => ClapShell::PowerShell,
    };

    generate(clap_shell, &mut cmd, name, &mut std::io::stdout());

    Ok(())
}
