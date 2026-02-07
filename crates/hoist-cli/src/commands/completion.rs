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

    eprintln!();
    eprintln!("# To install completions:");
    match shell {
        Shell::Bash => {
            eprintln!("# Add to ~/.bashrc:");
            eprintln!("#   source <(hoist completion bash)");
            eprintln!("# Or save to a file:");
            eprintln!("#   hoist completion bash > /etc/bash_completion.d/hoist");
        }
        Shell::Zsh => {
            eprintln!("# Add to ~/.zshrc:");
            eprintln!("#   source <(hoist completion zsh)");
            eprintln!("# Or save to fpath:");
            eprintln!("#   hoist completion zsh > ~/.zsh/completions/_hoist");
        }
        Shell::Fish => {
            eprintln!("# Save to completions directory:");
            eprintln!("#   hoist completion fish > ~/.config/fish/completions/hoist.fish");
        }
        Shell::Powershell => {
            eprintln!("# Add to PowerShell profile:");
            eprintln!("#   hoist completion powershell >> $PROFILE");
        }
    }

    Ok(())
}
