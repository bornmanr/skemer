//! Package name validation and search ranking.
//!
//! The helper resolves every name against libalpm before pacman sees it. These
//! checks are the cheap first line: they reject flags, paths, globs, and
//! oversized requests before any database or process work happens.

/// Maximum number of package names accepted in one install or removal.
pub const MAX_PACKAGES: usize = 64;
/// Maximum number of search results the helper will return.
pub const MAX_RESULTS: usize = 100;
/// Default number of search results.
pub const DEFAULT_RESULTS: usize = 50;

/// A name pacman could accept as a target. Version constraints and providers
/// are resolved by libalpm; this only rejects injection-shaped input.
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('-')
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '@' | '.' | '_' | '+' | '-')
        })
}

/// Validate a full request. Returns the first problem as a user-facing string.
pub fn validate_names(names: &[String]) -> Result<(), String> {
    if names.is_empty() {
        return Err("Select at least one package.".to_string());
    }
    if names.len() > MAX_PACKAGES {
        return Err(format!(
            "At most {MAX_PACKAGES} packages can be changed at once."
        ));
    }
    for name in names {
        if !is_valid_name(name) {
            return Err(format!("Not a valid package name: {name}"));
        }
    }
    let mut seen = std::collections::HashSet::new();
    for name in names {
        if !seen.insert(name.as_str()) {
            return Err(format!("The package {name} is listed more than once."));
        }
    }
    Ok(())
}

/// Lower is a better match. `None` means the query does not match the name.
pub fn search_rank(query: &str, name: &str) -> Option<u8> {
    let query = query.trim().to_ascii_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    let name = name.to_ascii_lowercase();
    if name == query {
        Some(0)
    } else if name.starts_with(&query) {
        Some(1)
    } else if name.split(['-', '_', '.', '@']).any(|token| token == query) {
        Some(2)
    } else if name.contains(&query) {
        Some(3)
    } else {
        None
    }
}

/// Clamp a client-supplied result limit.
pub fn clamp_limit(limit: usize) -> usize {
    limit.clamp(1, MAX_RESULTS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_real_package_names() {
        for name in ["steam", "openai-codex", "lib32-glibc", "g++", "python@3.13"] {
            assert!(is_valid_name(name), "{name}");
        }
    }

    #[test]
    fn rejects_flags_paths_and_globs() {
        for name in ["-Syu", "extra/steam", "steam*", "a b", "../etc", "", "a\nb"] {
            assert!(!is_valid_name(name), "{name:?}");
        }
    }

    #[test]
    fn rejects_empty_duplicate_and_oversized_requests() {
        assert!(validate_names(&[]).is_err());
        assert!(validate_names(&["steam".into(), "steam".into()]).is_err());

        let many: Vec<String> = (0..=MAX_PACKAGES)
            .map(|index| format!("pkg{index}"))
            .collect();
        assert!(validate_names(&many).is_err());
    }

    #[test]
    fn ranks_exact_then_prefix_then_token_then_substring() {
        assert_eq!(search_rank("steam", "steam"), Some(0));
        assert_eq!(search_rank("ste", "steam"), Some(1));
        assert_eq!(search_rank("codex", "openai-codex"), Some(2));
        assert_eq!(search_rank("team", "steam"), Some(3));
        assert_eq!(search_rank("zzz", "steam"), None);
    }

    #[test]
    fn empty_query_matches_everything() {
        assert_eq!(search_rank("  ", "steam"), Some(0));
    }

    #[test]
    fn limits_are_bounded() {
        assert_eq!(clamp_limit(0), 1);
        assert_eq!(clamp_limit(50), 50);
        assert_eq!(clamp_limit(10_000), MAX_RESULTS);
    }
}
