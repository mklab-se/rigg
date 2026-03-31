//! Service domain definitions for multi-provider support

use serde::{Deserialize, Serialize};
use std::fmt;

/// The service domain a resource belongs to
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ServiceDomain {
    /// Azure AI Search resources
    Search,
    /// Microsoft Foundry resources
    Foundry,
}

impl ServiceDomain {
    /// Human-readable display name
    pub fn display_name(&self) -> &'static str {
        match self {
            ServiceDomain::Search => "Azure AI Search",
            ServiceDomain::Foundry => "Microsoft Foundry",
        }
    }

    /// Top-level directory prefix for resources of this domain
    pub fn directory_prefix(&self) -> &'static str {
        match self {
            ServiceDomain::Search => "search",
            ServiceDomain::Foundry => "foundry",
        }
    }
}

impl fmt::Display for ServiceDomain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_display_names() {
        assert_eq!(ServiceDomain::Search.display_name(), "Azure AI Search");
        assert_eq!(ServiceDomain::Foundry.display_name(), "Microsoft Foundry");
    }

    #[test]
    fn test_directory_prefixes() {
        assert_eq!(ServiceDomain::Search.directory_prefix(), "search");
        assert_eq!(ServiceDomain::Foundry.directory_prefix(), "foundry");
    }

    #[test]
    fn test_display_trait() {
        assert_eq!(format!("{}", ServiceDomain::Search), "Azure AI Search");
        assert_eq!(format!("{}", ServiceDomain::Foundry), "Microsoft Foundry");
    }

    #[test]
    fn test_serde_roundtrip() {
        let domain = ServiceDomain::Foundry;
        let json = serde_json::to_string(&domain).unwrap();
        assert_eq!(json, "\"foundry\"");
        let back: ServiceDomain = serde_json::from_str(&json).unwrap();
        assert_eq!(back, domain);
    }

    #[test]
    fn test_serde_roundtrip_search() {
        let domain = ServiceDomain::Search;
        let json = serde_json::to_string(&domain).unwrap();
        assert_eq!(json, "\"search\"");
        let back: ServiceDomain = serde_json::from_str(&json).unwrap();
        assert_eq!(back, domain);
    }
}
