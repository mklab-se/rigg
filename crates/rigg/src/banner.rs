//! ASCII art banner for rigg CLI

use colored::Colorize;

const LOGO: &str = r#"
 ██████╗ ██╗ ██████╗  ██████╗
 ██╔══██╗██║██╔════╝ ██╔════╝
 ██████╔╝██║██║  ███╗██║  ███╗
 ██╔══██╗██║██║   ██║██║   ██║
 ██║  ██║██║╚██████╔╝╚██████╔╝
 ╚═╝  ╚═╝╚═╝ ╚═════╝  ╚═════╝"#;

/// Print the rigg ASCII art banner
pub fn print_banner() {
    for line in LOGO.lines() {
        println!("{}", line.bold());
    }
}

/// Print the banner with version info
pub fn print_banner_with_version() {
    print_banner();
    println!(
        " {} {}",
        "Configuration-as-code for Azure AI Search and Microsoft Foundry".dimmed(),
        format!("v{}", env!("CARGO_PKG_VERSION")).dimmed(),
    );
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_logo_is_not_empty() {
        assert!(!LOGO.is_empty());
    }

    #[test]
    fn test_logo_contains_rigg_letters() {
        // The block letters should spell RIGG
        assert!(LOGO.contains("██████╗")); // R
        assert!(LOGO.contains("██╔══██╗")); // R details
        assert!(LOGO.contains("██████╔╝")); // R/G
    }

    #[test]
    fn test_logo_has_six_lines() {
        let lines: Vec<&str> = LOGO.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(lines.len(), 6, "Logo should have 6 lines of block letters");
    }
}
