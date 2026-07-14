//! `rigg concepts` — print rigg's workspace/project mental model.
//!
//! Single-sourced from the repo-root `CONCEPTS.md`, embedded at build time so
//! the CLI and the docs cannot drift.

use std::io::IsTerminal;

use anyhow::Result;
use serde_json::json;

use crate::commands::GlobalContext;

/// The canonical concept guide. Embedding `CONCEPTS.md` guarantees CLI/docs parity.
const CONCEPTS_MD: &str = include_str!("../../CONCEPTS.md");

pub fn run(ctx: &GlobalContext) -> Result<()> {
    if ctx.json() {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "concepts": CONCEPTS_MD }))?
        );
        return Ok(());
    }

    let styled = std::io::stdout().is_terminal() && !ctx.no_color;
    let skin = if styled {
        termimad::MadSkin::default()
    } else {
        termimad::MadSkin::no_style()
    };
    skin.print_text(CONCEPTS_MD);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::CONCEPTS_MD;

    /// Parity guard: the core invariant must survive any future doc rewrite.
    #[test]
    fn concepts_md_states_the_core_invariant() {
        assert!(
            CONCEPTS_MD.contains("A resource belongs to exactly one project."),
            "CONCEPTS.md must state the one-resource-one-project invariant verbatim"
        );
    }
}
