//! AI feature management
//!
//! `rigg ai`         — show status
//! `rigg ai test`    — test AI connection
//! `rigg ai enable`  — enable AI for rigg
//! `rigg ai disable` — disable AI for rigg
//! `rigg ai config`  — interactive AI node configuration

use anyhow::Result;

use ailloy::config::Config;
use ailloy::config_tui;

use crate::cli::AiCommands;

pub async fn run(cmd: Option<AiCommands>) -> Result<()> {
    match cmd {
        None => config_tui::print_ai_status("rigg", &["chat"]),
        Some(AiCommands::Test { message }) => config_tui::run_test_chat("rigg", message).await,
        Some(AiCommands::Enable) => config_tui::enable_ai("rigg"),
        Some(AiCommands::Disable) => config_tui::disable_ai("rigg"),
        Some(AiCommands::Config) => {
            let mut config = Config::load_global()?;
            config_tui::run_interactive_config(&mut config, &["chat"]).await?;
            Ok(())
        }
        Some(AiCommands::Status) => config_tui::print_ai_status("rigg", &["chat"]),
        Some(AiCommands::Skill { emit, reference }) => {
            crate::commands::skill::run(emit, reference);
            Ok(())
        }
    }
}

/// Check if AI features are active (configured via ailloy + enabled for this tool).
pub fn is_ai_active() -> bool {
    config_tui::is_ai_active("rigg")
}
