//! Thin wrappers around `inquire` prompts: consistent styling, `--no-color`
//! support, and clean abort (Esc/Ctrl-C → error, nothing written).

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

/// Multi-select with every option pre-checked when `checked` is true.
pub fn multi_select_checked(
    prompt: &str,
    options: Vec<String>,
    checked: bool,
    plain: bool,
) -> Result<Vec<usize>> {
    let indexed: Vec<String> = options;
    let mut ms = MultiSelect::new(prompt, indexed.clone()).with_render_config(config(plain));
    let all: Vec<usize> = (0..indexed.len()).collect();
    if checked {
        ms = ms.with_default(&all);
    }
    let chosen = ms.prompt().map_err(map_err)?;
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

/// Text prompt pre-filled with a default (Enter accepts it).
pub fn text_with_default(prompt: &str, default: &str, plain: bool) -> Result<String> {
    Text::new(prompt)
        .with_default(default)
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
