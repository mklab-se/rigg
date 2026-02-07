//! Shared confirmation prompt utilities

use std::io::{self, BufRead, Write};

use anyhow::Result;

/// Prompt the user with a yes/no question. Default is "no".
/// Returns true only if the user types "y" or "yes" (case-insensitive).
pub fn prompt_yes_no(message: &str) -> Result<bool> {
    print!("{} [y/N] ", message);
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().lock().read_line(&mut input)?;

    Ok(matches!(
        input.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

/// Prompt the user with a yes/no question. Default is "yes".
/// Returns false only if the user types "n" or "no" (case-insensitive).
pub fn prompt_yes_default(message: &str) -> Result<bool> {
    print!("{} [Y/n] ", message);
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().lock().read_line(&mut input)?;

    Ok(!matches!(
        input.trim().to_ascii_lowercase().as_str(),
        "n" | "no"
    ))
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_yes_default_parsing_logic() {
        // Test the parsing logic for default-yes prompts
        let check = |input: &str| -> bool {
            !matches!(input.trim().to_ascii_lowercase().as_str(), "n" | "no")
        };

        assert!(check("y"));
        assert!(check("Y"));
        assert!(check("yes"));
        assert!(check("")); // empty = yes (default)
        assert!(check("  ")); // whitespace = yes (default)
        assert!(!check("n"));
        assert!(!check("N"));
        assert!(!check("no"));
        assert!(!check("NO"));
        assert!(!check("No"));
    }

    #[test]
    fn test_yes_no_parsing_logic() {
        // Test the parsing logic without stdin
        let check = |input: &str| -> bool {
            matches!(input.trim().to_ascii_lowercase().as_str(), "y" | "yes")
        };

        assert!(check("y"));
        assert!(check("Y"));
        assert!(check("yes"));
        assert!(check("YES"));
        assert!(check("Yes"));
        assert!(check("  y  "));
        assert!(!check("n"));
        assert!(!check("no"));
        assert!(!check(""));
        assert!(!check("yep"));
        assert!(!check("nope"));
    }
}
