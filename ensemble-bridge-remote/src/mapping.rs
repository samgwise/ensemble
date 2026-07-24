//! Address mapping engine.
//!
//! Translates Ensemble addresses between local and remote hubs using
//! pattern-based rules with capture substitution and `**` passthrough.

use ensemble_routing::Pattern;

use crate::config::MappingConfig;

/// Direction a mapping rule applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Local hub → remote peer.
    Outbound,
    /// Remote peer → local hub.
    Inbound,
    /// Both directions.
    Both,
}

impl Direction {
    /// Parse a direction string from config.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "outbound" => Some(Self::Outbound),
            "inbound" => Some(Self::Inbound),
            "both" => Some(Self::Both),
            _ => None,
        }
    }

    /// Whether this direction includes outbound (local → remote).
    pub fn includes_outbound(self) -> bool {
        matches!(self, Direction::Outbound | Direction::Both)
    }

    /// Whether this direction includes inbound (remote → local).
    pub fn includes_inbound(self) -> bool {
        matches!(self, Direction::Inbound | Direction::Both)
    }
}

/// A single compiled mapping rule.
pub struct MappingRule {
    /// Parsed pattern to match against.
    pub pattern: Pattern,
    /// Template for the output address.
    pub to_template: String,
    /// Direction this rule applies to.
    pub direction: Direction,
    /// Optional signal type filter (empty = all types).
    pub signal_filter: Vec<String>,
}

/// Engine that applies mapping rules to addresses.
pub struct MappingEngine {
    rules: Vec<MappingRule>,
}

impl MappingEngine {
    /// Build a mapping engine from configuration rules.
    ///
    /// Rules with invalid patterns are skipped (with a warning printed to stderr).
    pub fn new(configs: &[MappingConfig]) -> Self {
        let mut rules = Vec::new();
        for cfg in configs {
            let direction = match Direction::parse(&cfg.direction) {
                Some(d) => d,
                None => {
                    eprintln!(
                        "Warning: invalid direction '{}' in mapping rule, skipping",
                        cfg.direction
                    );
                    continue;
                }
            };
            match Pattern::parse(&cfg.from_pattern) {
                Ok(pattern) => {
                    rules.push(MappingRule {
                        pattern,
                        to_template: cfg.to_template.clone(),
                        direction,
                        signal_filter: cfg.signal_filter.clone(),
                    });
                }
                Err(e) => {
                    eprintln!(
                        "Warning: invalid pattern '{}' in mapping rule: {}, skipping",
                        cfg.from_pattern, e
                    );
                }
            }
        }
        Self { rules }
    }

    /// Map an address in the given direction.
    ///
    /// Returns the mapped address if a matching rule is found, or `None` if no
    /// rule matches. The first matching rule wins.
    ///
    /// The `signal_type` parameter allows filtering by signal type (e.g. "param",
    /// "event", "stream"). Pass `None` to skip signal type filtering.
    pub fn map(
        &self,
        address: &str,
        direction: Direction,
        signal_type: Option<&str>,
    ) -> Option<String> {
        for rule in &self.rules {
            // Check direction.
            let dir_match = match direction {
                Direction::Outbound => rule.direction.includes_outbound(),
                Direction::Inbound => rule.direction.includes_inbound(),
                Direction::Both => unreachable!(),
            };
            if !dir_match {
                continue;
            }

            // Check signal type filter.
            if let Some(st) = signal_type {
                if !rule.signal_filter.is_empty() && !rule.signal_filter.iter().any(|f| f == st) {
                    continue;
                }
            }

            // Try to match the pattern.
            if let Some(captures) = rule.pattern.matches(address) {
                return Some(apply_template(
                    &rule.to_template,
                    &captures,
                    address,
                    &rule.pattern,
                ));
            }
        }
        None
    }

    /// Get all outbound subscription patterns (for subscribing to the local hub).
    pub fn outbound_patterns(&self) -> Vec<&Pattern> {
        self.rules
            .iter()
            .filter(|r| r.direction.includes_outbound())
            .map(|r| &r.pattern)
            .collect()
    }
}

/// Apply a template string, substituting captures and `**` passthrough.
///
/// The template may contain:
/// - `{name}` — replaced with the captured value
/// - `**` — replaced with the remaining path suffix after the pattern match
fn apply_template(
    template: &str,
    captures: &ensemble_routing::CaptureSet,
    address: &str,
    pattern: &Pattern,
) -> String {
    let mut result = template.to_string();

    // Substitute named captures.
    for (name, value) in captures.iter() {
        result = result.replace(&format!("{{{}}}", name), value);
    }

    // Handle ** passthrough: find the suffix of the address not consumed by the pattern.
    if template.contains("**") {
        let suffix = compute_suffix(address, pattern);
        result = result.replace("**", &suffix);
    }

    result
}

/// Compute the path suffix not consumed by the pattern.
///
/// For example, pattern `/transport/**` matching `/transport/bpm/now`
/// leaves suffix `bpm/now`.
fn compute_suffix(address: &str, pattern: &Pattern) -> String {
    let pattern_str = pattern.source();

    // Find the prefix before ** in the pattern.
    if let Some(pos) = pattern_str.find("**") {
        let prefix = &pattern_str[..pos];
        // The suffix is everything in the address after the prefix.
        if let Some(addr_pos) = address.find(&prefix[prefix.len().saturating_sub(1)..]) {
            let after_prefix = &address[addr_pos + prefix.len().saturating_sub(1)..];
            // Strip leading slash if present.
            return after_prefix.trim_start_matches('/').to_string();
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MappingConfig;

    fn make_rule(from: &str, to: &str, dir: &str) -> MappingConfig {
        MappingConfig {
            from_pattern: from.to_string(),
            to_template: to.to_string(),
            direction: dir.to_string(),
            signal_filter: vec![],
        }
    }

    fn make_rule_with_filter(from: &str, to: &str, dir: &str, filter: &[&str]) -> MappingConfig {
        MappingConfig {
            from_pattern: from.to_string(),
            to_template: to.to_string(),
            direction: dir.to_string(),
            signal_filter: filter.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn exact_match_mapping() {
        let engine = MappingEngine::new(&[make_rule("/clock", "/remote/clock", "both")]);
        assert_eq!(
            engine.map("/clock", Direction::Outbound, None),
            Some("/remote/clock".to_string())
        );
        assert_eq!(
            engine.map("/clock", Direction::Inbound, None),
            Some("/remote/clock".to_string())
        );
        assert_eq!(engine.map("/other", Direction::Outbound, None), None);
    }

    #[test]
    fn capture_substitution() {
        let engine = MappingEngine::new(&[make_rule(
            "/track/{id}/volume",
            "/mixer/{id}/gain",
            "outbound",
        )]);
        assert_eq!(
            engine.map("/track/7/volume", Direction::Outbound, None),
            Some("/mixer/7/gain".to_string())
        );
        assert_eq!(
            engine.map("/track/42/volume", Direction::Outbound, None),
            Some("/mixer/42/gain".to_string())
        );
    }

    #[test]
    fn wildcard_passthrough() {
        let engine =
            MappingEngine::new(&[make_rule("/transport/**", "/remote/transport/**", "both")]);
        assert_eq!(
            engine.map("/transport/bpm", Direction::Outbound, None),
            Some("/remote/transport/bpm".to_string())
        );
        assert_eq!(
            engine.map("/transport/bpm/now", Direction::Outbound, None),
            Some("/remote/transport/bpm/now".to_string())
        );
    }

    #[test]
    fn direction_filtering() {
        let engine = MappingEngine::new(&[make_rule("/sensor/{name}", "/input/{name}", "inbound")]);
        // Inbound should match.
        assert_eq!(
            engine.map("/sensor/temp", Direction::Inbound, None),
            Some("/input/temp".to_string())
        );
        // Outbound should NOT match.
        assert_eq!(engine.map("/sensor/temp", Direction::Outbound, None), None);
    }

    #[test]
    fn signal_type_filtering() {
        let engine = MappingEngine::new(&[make_rule_with_filter(
            "/data/**",
            "/filtered/**",
            "both",
            &["param", "stream"],
        )]);
        // Param should match.
        assert_eq!(
            engine.map("/data/value", Direction::Outbound, Some("param")),
            Some("/filtered/value".to_string())
        );
        // Event should NOT match.
        assert_eq!(
            engine.map("/data/value", Direction::Outbound, Some("event")),
            None
        );
        // No filter (None) should match.
        assert_eq!(
            engine.map("/data/value", Direction::Outbound, None),
            Some("/filtered/value".to_string())
        );
    }

    #[test]
    fn first_match_wins() {
        let engine = MappingEngine::new(&[
            make_rule("/track/1/volume", "/special/vol", "both"),
            make_rule("/track/{id}/volume", "/mixer/{id}/gain", "both"),
        ]);
        // First rule matches exactly.
        assert_eq!(
            engine.map("/track/1/volume", Direction::Outbound, None),
            Some("/special/vol".to_string())
        );
        // Second rule matches by pattern.
        assert_eq!(
            engine.map("/track/2/volume", Direction::Outbound, None),
            Some("/mixer/2/gain".to_string())
        );
    }

    #[test]
    fn outbound_patterns_collected() {
        let engine = MappingEngine::new(&[
            make_rule("/a/**", "/x/**", "outbound"),
            make_rule("/b/**", "/y/**", "inbound"),
            make_rule("/c/**", "/z/**", "both"),
        ]);
        let patterns = engine.outbound_patterns();
        assert_eq!(patterns.len(), 2); // outbound + both
    }

    #[test]
    fn invalid_direction_skipped() {
        let engine = MappingEngine::new(&[make_rule("/foo", "/bar", "sideways")]);
        assert_eq!(engine.map("/foo", Direction::Outbound, None), None);
    }

    #[test]
    fn invalid_pattern_skipped() {
        let engine = MappingEngine::new(&[make_rule("/foo/**/bar", "/baz", "both")]);
        assert_eq!(engine.map("/foo/x/bar", Direction::Outbound, None), None);
    }
}
