//! Interactive prompt utilities for init workflows

use std::io::{self, BufRead, Write};

use anyhow::Result;

use hoist_core::config::FoundryServiceConfig;

/// Prompt user to select one or more items from a list.
/// Auto-selects if there is exactly one item.
pub(super) fn prompt_multi_selection<'a, T: std::fmt::Display>(
    prompt: &str,
    items: &'a [T],
) -> Result<Vec<&'a T>> {
    if items.is_empty() {
        return Ok(vec![]);
    }
    if items.len() == 1 {
        println!("  Found: {}", items[0]);
        return Ok(vec![&items[0]]);
    }
    for (i, item) in items.iter().enumerate() {
        println!("  [{}] {}", i + 1, item);
    }
    print!("{} (comma-separated, Enter to skip): ", prompt);
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().lock().read_line(&mut input)?;
    let input = input.trim();
    if input.is_empty() {
        return Ok(vec![]);
    }
    let mut selected = Vec::new();
    for part in input.split(',') {
        let idx: usize = part
            .trim()
            .parse()
            .map_err(|_| anyhow::anyhow!("Invalid selection: {}", part.trim()))?;
        if idx < 1 || idx > items.len() {
            anyhow::bail!("Selection out of range: {}", idx);
        }
        selected.push(&items[idx - 1]);
    }
    Ok(selected)
}

/// Prompt for search service name manually (no ARM discovery)
pub(super) fn prompt_search_service_manual() -> Result<Option<(String, Option<String>)>> {
    if !crate::commands::confirm::prompt_yes_default("Manage Azure AI Search resources?")? {
        return Ok(None);
    }

    let name = prompt_service_name()?;
    Ok(Some((name, None)))
}

/// Prompt for Foundry service configuration manually (no ARM discovery)
pub(super) fn prompt_foundry_service_manual() -> Result<Option<FoundryServiceConfig>> {
    if !crate::commands::confirm::prompt_yes_default("Manage Microsoft Foundry agents?")? {
        return Ok(None);
    }

    print!("AI Services account name (e.g., my-ai-service): ");
    io::stdout().flush()?;
    let mut svc_input = String::new();
    io::stdin().lock().read_line(&mut svc_input)?;
    let svc_name = svc_input.trim().to_string();
    if svc_name.is_empty() {
        anyhow::bail!("AI Services account name is required");
    }

    print!("Foundry project name (e.g., my-project): ");
    io::stdout().flush()?;
    let mut proj_input = String::new();
    io::stdin().lock().read_line(&mut proj_input)?;
    let proj_name = proj_input.trim().to_string();
    if proj_name.is_empty() {
        anyhow::bail!("Foundry project name is required");
    }

    Ok(Some(FoundryServiceConfig {
        name: svc_name,
        project: proj_name,
        label: None,
        api_version: "2025-05-15-preview".to_string(),
        endpoint: None,
        subscription: None,
        resource_group: None,
    }))
}

/// Prompt user to select from a numbered list
pub(super) fn prompt_selection<'a, T: std::fmt::Display>(
    prompt: &str,
    items: &'a [T],
    default: usize,
) -> Result<&'a T> {
    for (i, item) in items.iter().enumerate() {
        let marker = if i == default { " [default]" } else { "" };
        println!("  [{}] {}{}", i + 1, item, marker);
    }

    print!("{} [{}]: ", prompt, default + 1);
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().lock().read_line(&mut input)?;
    let input = input.trim();

    if input.is_empty() {
        return Ok(&items[default]);
    }

    let index: usize = input
        .parse::<usize>()
        .map_err(|_| anyhow::anyhow!("Invalid selection: {}", input))?;

    if index < 1 || index > items.len() {
        anyhow::bail!("Selection out of range: {}", index);
    }

    Ok(&items[index - 1])
}

/// Prompt for an Azure Search service name
pub(super) fn prompt_service_name() -> Result<String> {
    print!("Azure Search service name: ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().lock().read_line(&mut input)?;

    let name = input.trim().to_string();
    if name.is_empty() {
        anyhow::bail!("Service name is required");
    }

    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prompt_multi_selection_empty_items() {
        let items: Vec<String> = vec![];
        let result = prompt_multi_selection("Select", &items).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_multi_select_single_item_auto_selects() {
        let items = vec!["only-one".to_string()];
        let result = prompt_multi_selection("Select", &items).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(*result[0], "only-one");
    }
}
