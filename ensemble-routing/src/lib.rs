//! Ensemble routing — segment-based address pattern matching.
//!
//! This crate implements the Ensemble Routing v1 specification:
//!
//! - Exact segment match
//! - Single-segment wildcard (`*`)
//! - Recursive wildcard (`**`, final segment only)
//! - Named capture (`{name}`)
//!
//! Invalid patterns are rejected at parse time with a structured [`PatternError`].

use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Pattern segment types
// ---------------------------------------------------------------------------

/// A single segment of a parsed pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    /// An exact literal segment (e.g. `track`, `17`).
    Exact(String),
    /// Single-segment wildcard `*` — matches exactly one segment.
    Wildcard,
    /// Recursive wildcard `**` — matches zero or more remaining segments.
    /// Only valid as the final segment.
    RecursiveWildcard,
    /// Named capture `{name}` — matches one segment and captures it.
    Capture(String),
}

// ---------------------------------------------------------------------------
// PatternError
// ---------------------------------------------------------------------------

/// Errors produced when parsing an invalid pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatternError {
    /// The pattern string was empty or did not begin with `/`.
    InvalidAddress,
    /// `**` appeared in a position other than the final segment.
    RecursiveWildcardNotFinal,
    /// A capture name was empty: `{}`.
    EmptyCaptureName,
    /// A capture name contained a forbidden character (`/`, `{`, `}`, `*`).
    InvalidCaptureName(String),
    /// A segment contained a character class `[...]`.
    CharacterClassNotSupported,
    /// A segment contained alternation `{a,b}`.
    AlternationNotSupported,
    /// A typed capture like `{id:int}` was used.
    TypedCaptureNotSupported,
    /// A recursive capture like `{path**}` was used.
    RecursiveCaptureNotSupported,
    /// A segment contained regex syntax.
    RegexNotSupported,
    /// Negative matching (`!...`) was used.
    NegativeMatchingNotSupported,
    /// Nested braces in a capture name.
    NestedBraces,
}

impl std::fmt::Display for PatternError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidAddress => write!(f, "pattern must start with '/'"),
            Self::RecursiveWildcardNotFinal => {
                write!(f, "'**' must be the final segment of a pattern")
            }
            Self::EmptyCaptureName => write!(f, "capture name must not be empty"),
            Self::InvalidCaptureName(name) => {
                write!(f, "capture name contains forbidden characters: '{name}'")
            }
            Self::CharacterClassNotSupported => {
                write!(f, "character classes are not supported in routing v1")
            }
            Self::AlternationNotSupported => {
                write!(f, "alternation is not supported in routing v1")
            }
            Self::TypedCaptureNotSupported => {
                write!(f, "typed captures are not supported in routing v1")
            }
            Self::RecursiveCaptureNotSupported => {
                write!(f, "recursive captures are not supported in routing v1")
            }
            Self::RegexNotSupported => {
                write!(f, "regular expressions are not supported in routing v1")
            }
            Self::NegativeMatchingNotSupported => {
                write!(f, "negative matching is not supported in routing v1")
            }
            Self::NestedBraces => write!(f, "nested braces in capture name"),
        }
    }
}

impl std::error::Error for PatternError {}

// ---------------------------------------------------------------------------
// CaptureSet
// ---------------------------------------------------------------------------

/// Named captures extracted from a matched address.
///
/// All capture values are strings; no type conversion is performed at routing
/// time. Clients are responsible for interpretation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CaptureSet {
    inner: HashMap<String, String>,
}

impl CaptureSet {
    /// Look up a captured value by name.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.inner.get(name).map(|s| s.as_str())
    }

    /// Iterate over all captured name-value pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.inner.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// Number of captures.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Whether the capture set is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Pattern
// ---------------------------------------------------------------------------

/// A parsed, validated routing pattern.
///
/// Patterns are immutable after construction. Use [`Pattern::parse`] to
/// construct one from a string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    /// The original pattern string.
    source: String,
    /// Parsed segments.
    segments: Vec<Segment>,
}

impl Pattern {
    /// Parse a pattern string into a validated [`Pattern`].
    ///
    /// Returns a [`PatternError`] if the pattern is invalid per the Ensemble
    /// Routing v1 specification.
    pub fn parse(pattern: &str) -> Result<Self, PatternError> {
        // Must start with '/'.
        if !pattern.starts_with('/') {
            return Err(PatternError::InvalidAddress);
        }

        // Split into segments (skip the leading empty segment from the '/').
        let raw_segments: Vec<&str> = pattern[1..].split('/').collect();

        // An empty pattern after the leading '/' means zero segments — that's
        // just "/", which we treat as valid with no segments (matches only "/").
        // Actually per spec addresses must contain one or more segments, but
        // "/" alone is a degenerate case. We'll allow parsing it; matching
        // behaviour follows naturally.

        let mut segments = Vec::with_capacity(raw_segments.len());

        for (i, &raw) in raw_segments.iter().enumerate() {
            let is_last = i == raw_segments.len() - 1;

            // Check for unsupported features first.
            if raw.contains('[') || raw.contains(']') {
                return Err(PatternError::CharacterClassNotSupported);
            }
            if raw.starts_with('!') {
                return Err(PatternError::NegativeMatchingNotSupported);
            }
            // Check for regex-like characters (but not '*' which we handle).
            // We detect common regex chars: +, ?, |, (, ), '.', '^', '$'
            // But only outside of our own syntax. We need to be careful:
            // '*' and '**' are valid, but '+', '?', etc. are not.
            if raw.contains('+')
                || raw.contains('?')
                || raw.contains('(')
                || raw.contains(')')
                || raw.contains('^')
                || raw.contains('$')
                || raw.contains('.')
            {
                // Only flag as regex if it's not a valid pattern segment.
                // Actually, these characters are simply not part of routing v1
                // at all, so reject them.
                if raw != "*" && raw != "**" {
                    return Err(PatternError::RegexNotSupported);
                }
            }

            if raw == "**" {
                if !is_last {
                    return Err(PatternError::RecursiveWildcardNotFinal);
                }
                segments.push(Segment::RecursiveWildcard);
            } else if raw == "*" {
                segments.push(Segment::Wildcard);
            } else if raw.starts_with('{') || raw.contains('}') {
                // Could be a capture or could be invalid syntax.
                let seg = Self::parse_capture_segment(raw)?;
                segments.push(seg);
            } else if raw.is_empty() {
                // Empty segment — this happens for trailing '/' or '//' in the
                // pattern. We treat this as an exact empty segment, which will
                // never match a real address segment. This is fine.
                segments.push(Segment::Exact(String::new()));
            } else {
                segments.push(Segment::Exact(raw.to_string()));
            }
        }

        Ok(Self {
            source: pattern.to_string(),
            segments,
        })
    }

    /// Parse a segment that looks like it contains `{...}`.
    fn parse_capture_segment(raw: &str) -> Result<Segment, PatternError> {
        // Check for nested braces.
        let open_count = raw.chars().filter(|&c| c == '{').count();
        let close_count = raw.chars().filter(|&c| c == '}').count();
        if open_count > 1 || close_count > 1 {
            return Err(PatternError::NestedBraces);
        }

        // Must be exactly `{name}` — the whole segment.
        if !raw.starts_with('{') || !raw.ends_with('}') {
            // Contains braces but not in the right form.
            // Check for alternation: {foo,bar}
            if raw.contains(',') {
                return Err(PatternError::AlternationNotSupported);
            }
            return Err(PatternError::InvalidCaptureName(raw.to_string()));
        }

        let name = &raw[1..raw.len() - 1];

        // Empty capture name.
        if name.is_empty() {
            return Err(PatternError::EmptyCaptureName);
        }

        // Check for typed capture: {name:type}
        if name.contains(':') {
            return Err(PatternError::TypedCaptureNotSupported);
        }

        // Check for recursive capture: {name**}
        if name.contains("**") {
            return Err(PatternError::RecursiveCaptureNotSupported);
        }

        // Forbidden characters in capture name: / { } * ,
        // Comma is rejected as alternation syntax (e.g. {foo,bar}).
        if name.contains('/')
            || name.contains('{')
            || name.contains('}')
            || name.contains('*')
            || name.contains(',')
        {
            return Err(PatternError::InvalidCaptureName(name.to_string()));
        }

        Ok(Segment::Capture(name.to_string()))
    }

    /// Match this pattern against an address string.
    ///
    /// Returns `Some(CaptureSet)` if the address matches (captures may be
    /// empty if the pattern has no `{name}` segments), or `None` if it does
    /// not match.
    pub fn matches(&self, address: &str) -> Option<CaptureSet> {
        // Address must start with '/'.
        if !address.starts_with('/') {
            return None;
        }

        let addr_segments: Vec<&str> = address[1..].split('/').collect();
        let mut captures = HashMap::new();

        if Self::match_segments(&self.segments, &addr_segments, &mut captures) {
            Some(CaptureSet { inner: captures })
        } else {
            None
        }
    }

    /// Recursive segment matching.
    fn match_segments(
        pattern: &[Segment],
        address: &[&str],
        captures: &mut HashMap<String, String>,
    ) -> bool {
        let mut pi = 0;
        let mut ai = 0;

        while pi < pattern.len() {
            match &pattern[pi] {
                Segment::Exact(expected) => {
                    if ai >= address.len() {
                        return false;
                    }
                    if address[ai] != expected.as_str() {
                        return false;
                    }
                    pi += 1;
                    ai += 1;
                }
                Segment::Wildcard => {
                    // Must match exactly one segment.
                    if ai >= address.len() {
                        return false;
                    }
                    pi += 1;
                    ai += 1;
                }
                Segment::Capture(name) => {
                    // Captures exactly one segment.
                    if ai >= address.len() {
                        return false;
                    }
                    captures.insert(name.clone(), address[ai].to_string());
                    pi += 1;
                    ai += 1;
                }
                Segment::RecursiveWildcard => {
                    // Matches zero or more remaining segments.
                    // Since ** must be the final segment, we can just accept
                    // everything remaining.
                    return true;
                }
            }
        }

        // All pattern segments consumed; address must also be fully consumed.
        ai == address.len()
    }

    /// The original pattern string.
    pub fn source(&self) -> &str {
        &self.source
    }
}

impl std::fmt::Display for Pattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.source)
    }
}

// ---------------------------------------------------------------------------
// Convenience functions
// ---------------------------------------------------------------------------

/// Check if an address matches any of the given patterns.
///
/// Returns `true` as soon as the first matching pattern is found.
pub fn matches_any(patterns: &[Pattern], address: &str) -> bool {
    patterns.iter().any(|p| p.matches(address).is_some())
}

// ---------------------------------------------------------------------------
// Tests — routing conformance corpus
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Exact match --

    #[test]
    fn exact_match() {
        let pat = Pattern::parse("/foo/bar").unwrap();
        assert!(pat.matches("/foo/bar").is_some());
    }

    #[test]
    fn exact_match_failure() {
        let pat = Pattern::parse("/foo/bar").unwrap();
        assert!(pat.matches("/foo/baz").is_none());
    }

    #[test]
    fn exact_match_different_length() {
        let pat = Pattern::parse("/foo/bar").unwrap();
        assert!(pat.matches("/foo").is_none());
        assert!(pat.matches("/foo/bar/baz").is_none());
    }

    // -- Single-segment wildcard --

    #[test]
    fn wildcard_matches_single_segment() {
        let pat = Pattern::parse("/foo/*/bar").unwrap();
        assert!(pat.matches("/foo/baz/bar").is_some());
    }

    #[test]
    fn wildcard_does_not_match_wrong_tail() {
        let pat = Pattern::parse("/foo/*/bar").unwrap();
        assert!(pat.matches("/foo/baz/qux").is_none());
    }

    #[test]
    fn wildcard_matches_various_single_segments() {
        let pat = Pattern::parse("/track/*/volume").unwrap();
        assert!(pat.matches("/track/1/volume").is_some());
        assert!(pat.matches("/track/17/volume").is_some());
        assert!(pat.matches("/track/foo/volume").is_some());
    }

    #[test]
    fn wildcard_does_not_match_fewer_segments() {
        let pat = Pattern::parse("/track/*/volume").unwrap();
        assert!(pat.matches("/track/1").is_none());
    }

    #[test]
    fn wildcard_does_not_match_more_segments() {
        let pat = Pattern::parse("/track/*/volume").unwrap();
        assert!(pat.matches("/track/1/mixer/volume").is_none());
    }

    // -- Recursive wildcard --

    #[test]
    fn recursive_wildcard_matches_deep() {
        let pat = Pattern::parse("/foo/**").unwrap();
        assert!(pat.matches("/foo/a/b/c").is_some());
    }

    #[test]
    fn recursive_wildcard_matches_zero_remaining() {
        // /track/** should match /track (zero remaining segments after /track).
        let pat = Pattern::parse("/track/**").unwrap();
        assert!(pat.matches("/track").is_some());
    }

    #[test]
    fn recursive_wildcard_matches_one_remaining() {
        let pat = Pattern::parse("/track/**").unwrap();
        assert!(pat.matches("/track/1").is_some());
    }

    #[test]
    fn recursive_wildcard_matches_many_remaining() {
        let pat = Pattern::parse("/track/**").unwrap();
        assert!(pat.matches("/track/1/volume").is_some());
        assert!(pat.matches("/track/1/sends/reverb").is_some());
    }

    #[test]
    fn recursive_wildcard_does_not_match_different_prefix() {
        let pat = Pattern::parse("/track/**").unwrap();
        assert!(pat.matches("/foo/track").is_none());
    }

    // -- Named capture --

    #[test]
    fn named_capture_single() {
        let pat = Pattern::parse("/track/{id}/volume").unwrap();
        let caps = pat.matches("/track/17/volume").unwrap();
        assert_eq!(caps.get("id"), Some("17"));
    }

    #[test]
    fn named_capture_multiple() {
        let pat = Pattern::parse("/device/{device}/control/{control}").unwrap();
        let caps = pat.matches("/device/mixer/control/gain").unwrap();
        assert_eq!(caps.get("device"), Some("mixer"));
        assert_eq!(caps.get("control"), Some("gain"));
    }

    #[test]
    fn named_capture_unicode() {
        let pat = Pattern::parse("/track/{番号}/volume").unwrap();
        let caps = pat.matches("/track/17/volume").unwrap();
        assert_eq!(caps.get("番号"), Some("17"));
    }

    #[test]
    fn named_capture_no_match() {
        let pat = Pattern::parse("/track/{id}/volume").unwrap();
        assert!(pat.matches("/track/17/pan").is_none());
    }

    #[test]
    fn captures_are_strings() {
        // Even numeric-looking captures are strings.
        let pat = Pattern::parse("/track/{id}/volume").unwrap();
        let caps = pat.matches("/track/42/volume").unwrap();
        assert_eq!(caps.get("id"), Some("42"));
        // Confirm it's a string, not a number.
        assert_eq!(caps.get("id").unwrap().parse::<i64>().unwrap(), 42);
    }

    // -- Invalid patterns --

    #[test]
    fn reject_recursive_wildcard_not_final() {
        assert_eq!(
            Pattern::parse("/foo/**/bar"),
            Err(PatternError::RecursiveWildcardNotFinal)
        );
    }

    #[test]
    fn reject_alternation() {
        assert!(Pattern::parse("/{foo,bar}").is_err());
    }

    #[test]
    fn reject_typed_capture() {
        assert!(Pattern::parse("/{id:int}").is_err());
    }

    #[test]
    fn reject_character_class() {
        assert!(Pattern::parse("/track/[0-9]+").is_err());
    }

    #[test]
    fn reject_recursive_capture() {
        assert!(Pattern::parse("/{path**}").is_err());
    }

    #[test]
    fn reject_empty_capture_name() {
        assert!(Pattern::parse("/{}").is_err());
    }

    #[test]
    fn reject_slash_in_capture_name() {
        assert!(Pattern::parse("/{foo/bar}").is_err());
    }

    #[test]
    fn reject_nested_braces() {
        assert!(Pattern::parse("/{foo{bar}}").is_err());
    }

    #[test]
    fn reject_star_in_capture_name() {
        assert!(Pattern::parse("/{foo*bar}").is_err());
    }

    #[test]
    fn reject_pattern_not_starting_with_slash() {
        assert!(Pattern::parse("foo/bar").is_err());
    }

    // -- matches_any convenience --

    #[test]
    fn matches_any_with_multiple_patterns() {
        let p1 = Pattern::parse("/midi/**").unwrap();
        let p2 = Pattern::parse("/clock").unwrap();
        let patterns: Vec<Pattern> = vec![p1, p2];
        assert!(matches_any(&patterns, "/midi/ch/1/note"));
        assert!(matches_any(&patterns, "/clock"));
        assert!(!matches_any(&patterns, "/synth/note"));
    }

    #[test]
    fn matches_any_with_empty_patterns() {
        let patterns: Vec<Pattern> = vec![];
        assert!(!matches_any(&patterns, "/anything"));
    }

    // -- Edge cases --

    #[test]
    fn address_must_start_with_slash() {
        let pat = Pattern::parse("/foo").unwrap();
        assert!(pat.matches("foo").is_none());
    }

    #[test]
    fn empty_address_does_not_match_nonempty_pattern() {
        let pat = Pattern::parse("/foo").unwrap();
        assert!(pat.matches("").is_none());
    }

    #[test]
    fn pattern_source_preserved() {
        let pat = Pattern::parse("/track/{id}/volume").unwrap();
        assert_eq!(pat.source(), "/track/{id}/volume");
    }

    #[test]
    fn capture_set_len_and_is_empty() {
        let pat = Pattern::parse("/track/{id}/volume").unwrap();
        let caps = pat.matches("/track/7/volume").unwrap();
        assert_eq!(caps.len(), 1);
        assert!(!caps.is_empty());

        let pat2 = Pattern::parse("/track/*/volume").unwrap();
        let caps2 = pat2.matches("/track/7/volume").unwrap();
        assert_eq!(caps2.len(), 0);
        assert!(caps2.is_empty());
    }

    // -- Conformance fixture examples from conformance-testing.md --

    #[test]
    fn conformance_exact_match() {
        let pat = Pattern::parse("/foo/bar").unwrap();
        assert!(pat.matches("/foo/bar").is_some());
    }

    #[test]
    fn conformance_exact_match_failure() {
        let pat = Pattern::parse("/foo/bar").unwrap();
        assert!(pat.matches("/foo/baz").is_none());
    }

    #[test]
    fn conformance_wildcard() {
        let pat = Pattern::parse("/foo/*/bar").unwrap();
        assert!(pat.matches("/foo/baz/bar").is_some());
    }

    #[test]
    fn conformance_recursive_wildcard() {
        let pat = Pattern::parse("/foo/**").unwrap();
        assert!(pat.matches("/foo/a/b/c").is_some());
    }

    #[test]
    fn conformance_named_capture() {
        let pat = Pattern::parse("/track/{id}/volume").unwrap();
        let caps = pat.matches("/track/17/volume").unwrap();
        assert_eq!(caps.get("id"), Some("17"));
    }

    #[test]
    fn conformance_unicode_capture() {
        let pat = Pattern::parse("/track/{番号}/volume").unwrap();
        let caps = pat.matches("/track/17/volume").unwrap();
        assert_eq!(caps.get("番号"), Some("17"));
    }

    #[test]
    fn conformance_invalid_pattern_rejected() {
        assert!(Pattern::parse("/foo/**/bar").is_err());
    }

    // -- Additional spec examples from routing.md --

    #[test]
    fn spec_example_wildcard_failure() {
        let pat = Pattern::parse("/foo/*/baz").unwrap();
        assert!(pat.matches("/foo/bar/qux").is_none());
    }

    #[test]
    fn spec_example_capture() {
        let pat = Pattern::parse("/foo/{name}/baz").unwrap();
        let caps = pat.matches("/foo/alice/baz").unwrap();
        assert_eq!(caps.get("name"), Some("alice"));
    }

    #[test]
    fn spec_example_recursive_wildcard() {
        let pat = Pattern::parse("/foo/**").unwrap();
        assert!(pat.matches("/foo/bar/baz").is_some());
    }

    // -- Multiple overlapping subscriptions (no precedence) --

    #[test]
    fn multiple_patterns_all_match() {
        // All three patterns match /track/7/volume — the hub should deliver
        // to all matching subscribers (no "best match" selection).
        let p1 = Pattern::parse("/track/{id}/volume").unwrap();
        let p2 = Pattern::parse("/track/*/volume").unwrap();
        let p3 = Pattern::parse("/track/**").unwrap();
        let addr = "/track/7/volume";
        assert!(p1.matches(addr).is_some());
        assert!(p2.matches(addr).is_some());
        assert!(p3.matches(addr).is_some());
    }
}
