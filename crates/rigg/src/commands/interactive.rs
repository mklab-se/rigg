//! Thin wrappers around `inquire` prompts: consistent styling, `--no-color`
//! support, and clean abort (Esc/Ctrl-C → error, nothing written).
//!
//! TODO(adopt wizard task 2): remove this allow once `adopt` wires these in.
#![allow(dead_code)]

use anyhow::{Result, anyhow};
use inquire::ui::RenderConfig;
use inquire::{Confirm, InquireError, MultiSelect, Select, Text};

fn config(plain: bool) -> RenderConfig<'static> {
    if plain {
        RenderConfig::empty()
    } else {
        RenderConfig::default_colored()
    }
}

fn map_err(e: InquireError) -> anyhow::Error {
    match e {
        InquireError::OperationCanceled | InquireError::OperationInterrupted => {
            anyhow!("aborted")
        }
        other => anyhow!(other),
    }
}

pub fn select(prompt: &str, options: Vec<String>, plain: bool) -> Result<String> {
    Select::new(prompt, options)
        .with_render_config(config(plain))
        .prompt()
        .map_err(map_err)
}

/// Returns the indices of the chosen options (order of the input list).
pub fn multi_select(prompt: &str, options: Vec<String>, plain: bool) -> Result<Vec<usize>> {
    let indexed: Vec<String> = options;
    let chosen = MultiSelect::new(prompt, indexed.clone())
        .with_render_config(config(plain))
        .prompt()
        .map_err(map_err)?;
    Ok(chosen
        .into_iter()
        .filter_map(|c| indexed.iter().position(|o| *o == c))
        .collect())
}

pub fn confirm_default_yes(prompt: &str, plain: bool) -> Result<bool> {
    Confirm::new(prompt)
        .with_default(true)
        .with_render_config(config(plain))
        .prompt()
        .map_err(map_err)
}

pub fn confirm_default_no(prompt: &str, plain: bool) -> Result<bool> {
    Confirm::new(prompt)
        .with_default(false)
        .with_render_config(config(plain))
        .prompt()
        .map_err(map_err)
}

pub fn text(prompt: &str, plain: bool) -> Result<String> {
    Text::new(prompt)
        .with_render_config(config(plain))
        .prompt()
        .map_err(map_err)
}
