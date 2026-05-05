//! Address pattern matching for Chord subscriptions.
//!
//! Supports OSC-style hierarchical paths with trailing wildcard matching.

/// Check if an address matches any of the given subscription patterns.
pub fn matches_any(patterns: &[String], address: &str) -> bool {
    patterns.iter().any(|pat| matches_pattern(pat, address))
}

/// Match a single pattern against an address.
///
/// Rules:
/// - `"*"` or `"/*"` matches everything.
/// - A pattern ending with `*` matches any address that starts with the prefix
///   before the `*` (e.g. `/midi/*` matches `/midi/ch/1/note`).
/// - Otherwise, the pattern must match the address exactly.
pub fn matches_pattern(pattern: &str, address: &str) -> bool {
    if pattern == "*" || pattern == "/*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        address.starts_with(prefix)
    } else {
        pattern == address
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- Exact matching --

    #[test]
    fn exact_match() {
        assert!(matches_pattern("/synth/note", "/synth/note"));
    }

    #[test]
    fn exact_mismatch() {
        assert!(!matches_pattern("/synth/note", "/synth/cc"));
    }

    #[test]
    fn exact_prefix_is_not_a_match() {
        // Without a wildcard, a prefix should not match.
        assert!(!matches_pattern("/synth", "/synth/note"));
    }

    // -- Wildcard matching --

    #[test]
    fn trailing_wildcard_matches_subtree() {
        assert!(matches_pattern("/midi/*", "/midi/ch/1/note"));
    }

    #[test]
    fn trailing_wildcard_matches_immediate_child() {
        assert!(matches_pattern("/midi/*", "/midi/note"));
    }

    #[test]
    fn trailing_wildcard_does_not_match_unrelated() {
        assert!(!matches_pattern("/midi/*", "/synth/note"));
    }

    #[test]
    fn trailing_wildcard_matches_exact_prefix() {
        // `/midi/*` should match `/midi/` (the prefix itself with a trailing slash).
        assert!(matches_pattern("/midi/*", "/midi/"));
    }

    #[test]
    fn bare_star_matches_everything() {
        assert!(matches_pattern("*", "/anything/at/all"));
    }

    #[test]
    fn slash_star_matches_everything() {
        assert!(matches_pattern("/*", "/anything/at/all"));
    }

    // -- matches_any --

    #[test]
    fn matches_any_with_multiple_patterns() {
        let patterns = vec!["/midi/*".into(), "/clock".into()];
        assert!(matches_any(&patterns, "/midi/ch/1/note"));
        assert!(matches_any(&patterns, "/clock"));
        assert!(!matches_any(&patterns, "/synth/note"));
    }

    #[test]
    fn matches_any_with_empty_patterns() {
        let patterns: Vec<String> = vec![];
        assert!(!matches_any(&patterns, "/anything"));
    }

    // -- Edge cases --

    #[test]
    fn empty_address() {
        assert!(!matches_pattern("/midi/*", ""));
        assert!(matches_pattern("*", ""));
    }

    #[test]
    fn empty_pattern_matches_only_empty() {
        assert!(matches_pattern("", ""));
        assert!(!matches_pattern("", "/something"));
    }
}
