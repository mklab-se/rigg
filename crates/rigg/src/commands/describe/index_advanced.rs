//! Index advanced configuration descriptions.
//!
//! Handles scoring profiles, custom analyzers, vector search, semantic search,
//! suggesters, CORS, and similarity configuration.

use rigg_diff::{Change, ChangeKind};

use super::helpers::{parse_array_element_path, value_comparison};

// ---------------------------------------------------------------------------
// Section O: Index advanced config
// ---------------------------------------------------------------------------

pub(super) fn describe_index_advanced(
    change: &Change,
    name: &str,
    old_label: &str,
    new_label: &str,
) -> Option<String> {
    let path = &change.path;

    // Scoring profiles
    if path.starts_with("scoringProfiles[") {
        let (profile_name, sub_path) = parse_array_element_path(path, "scoringProfiles");
        return Some(match (sub_path.as_deref(), change.kind) {
            (None, ChangeKind::Added) => format!(
                "Index '{}' has scoring profile '{}' {} that does not exist {}",
                name, profile_name, new_label, old_label
            ),
            (None, ChangeKind::Removed) => format!(
                "Index '{}' has scoring profile '{}' {} that does not exist {}",
                name, profile_name, old_label, new_label
            ),
            _ => format!(
                "Scoring profile '{}' in index '{}': {}",
                profile_name,
                name,
                value_comparison(change, old_label, new_label)
            ),
        });
    }

    // Custom analyzers, tokenizers, token filters, char filters
    for (prefix, label) in [
        ("analyzers", "custom analyzer"),
        ("tokenizers", "custom tokenizer"),
        ("tokenFilters", "custom token filter"),
        ("charFilters", "custom char filter"),
    ] {
        if path.starts_with(&format!("{}[", prefix)) {
            let (item_name, _) = parse_array_element_path(path, prefix);
            return Some(match change.kind {
                ChangeKind::Added => format!(
                    "Index '{}' has {} '{}' {} that does not exist {}",
                    name, label, item_name, new_label, old_label
                ),
                ChangeKind::Removed => format!(
                    "Index '{}' has {} '{}' {} that does not exist {}",
                    name, label, item_name, old_label, new_label
                ),
                _ => format!(
                    "Index '{}' {} '{}' {}",
                    name,
                    label,
                    item_name,
                    value_comparison(change, old_label, new_label)
                ),
            });
        }
    }

    // Vector search
    if path == "vectorSearch" {
        return Some(format!(
            "Index '{}' vector search configuration differs between {} and {}",
            name, old_label, new_label
        ));
    }
    if path.starts_with("vectorSearch.profiles[") {
        let (profile_name, _) = parse_array_element_path(
            path.strip_prefix("vectorSearch.").unwrap_or(path),
            "profiles",
        );
        return Some(match change.kind {
            ChangeKind::Added => format!(
                "Index '{}' has vector search profile '{}' {} that does not exist {}",
                name, profile_name, new_label, old_label
            ),
            ChangeKind::Removed => format!(
                "Index '{}' has vector search profile '{}' {} that does not exist {}",
                name, profile_name, old_label, new_label
            ),
            _ => format!(
                "Index '{}' vector search profile '{}' {}",
                name,
                profile_name,
                value_comparison(change, old_label, new_label)
            ),
        });
    }
    if path.starts_with("vectorSearch.algorithms[") {
        let (alg_name, _) = parse_array_element_path(
            path.strip_prefix("vectorSearch.").unwrap_or(path),
            "algorithms",
        );
        return Some(match change.kind {
            ChangeKind::Added => format!(
                "Index '{}' has vector search algorithm '{}' {} that does not exist {}",
                name, alg_name, new_label, old_label
            ),
            ChangeKind::Removed => format!(
                "Index '{}' has vector search algorithm '{}' {} that does not exist {}",
                name, alg_name, old_label, new_label
            ),
            _ => format!(
                "Index '{}' vector search algorithm '{}' {}",
                name,
                alg_name,
                value_comparison(change, old_label, new_label)
            ),
        });
    }

    // Semantic search
    if path == "semantic" {
        return Some(format!(
            "Index '{}' semantic search configuration differs between {} and {}",
            name, old_label, new_label
        ));
    }
    if path.starts_with("semantic.configurations[") {
        let (config_name, _) = parse_array_element_path(
            path.strip_prefix("semantic.").unwrap_or(path),
            "configurations",
        );
        return Some(match change.kind {
            ChangeKind::Added => format!(
                "Index '{}' has semantic configuration '{}' {} that does not exist {}",
                name, config_name, new_label, old_label
            ),
            ChangeKind::Removed => format!(
                "Index '{}' has semantic configuration '{}' {} that does not exist {}",
                name, config_name, old_label, new_label
            ),
            _ => format!(
                "Index '{}' semantic configuration '{}' {}",
                name,
                config_name,
                value_comparison(change, old_label, new_label)
            ),
        });
    }

    // Suggesters
    if path.starts_with("suggesters[") {
        let (sug_name, _) = parse_array_element_path(path, "suggesters");
        return Some(match change.kind {
            ChangeKind::Added => format!(
                "Index '{}' has suggester '{}' {} that does not exist {}",
                name, sug_name, new_label, old_label
            ),
            ChangeKind::Removed => format!(
                "Index '{}' has suggester '{}' {} that does not exist {}",
                name, sug_name, old_label, new_label
            ),
            _ => format!(
                "Index '{}' suggester '{}' {}",
                name,
                sug_name,
                value_comparison(change, old_label, new_label)
            ),
        });
    }

    // CORS, similarity
    if path == "corsOptions" {
        return Some(format!(
            "Index '{}' CORS configuration differs between {} and {}",
            name, old_label, new_label
        ));
    }
    if path == "similarity" {
        return Some(format!(
            "Index '{}' similarity algorithm changed between {} and {}",
            name, old_label, new_label
        ));
    }

    None
}
