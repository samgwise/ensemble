//! Björklund's algorithm for generating Euclidean rhythms.
//!
//! A Euclidean rhythm distributes `hits` as evenly as possible across `steps`.
//! The algorithm is based on the Euclidean GCD algorithm and produces rhythms
//! that are mathematically optimal in terms of evenness.
//!
//! # Examples
//!
//! ```
//! // E(3,8) = [X . . X . . X .] — the "tresillo" rhythm
//! // E(4,16) with rotation 0 = [X . . . X . . . X . . . X . . .]
//! ```

/// Generate a Euclidean rhythm pattern.
///
/// Distributes `hits` as evenly as possible across `steps`, with an optional
/// `rotation` offset. Returns a vector of booleans where `true` indicates a hit.
///
/// # Arguments
///
/// * `steps` — total number of steps in the pattern (must be > 0)
/// * `hits` — number of hits to distribute (must be <= steps)
/// * `rotation` — cyclic rotation offset (0 = no rotation)
///
/// # Panics
///
/// Panics if `steps` is 0 or `hits > steps`.
pub fn euclidean(steps: usize, hits: usize, rotation: usize) -> Vec<bool> {
    assert!(steps > 0, "steps must be > 0");
    assert!(hits <= steps, "hits must be <= steps");

    if hits == 0 {
        return vec![false; steps];
    }
    if hits == steps {
        return vec![true; steps];
    }

    // Björklund's algorithm (simplified bucket-fill approach).
    // Use the Bresenham-like method: track an accumulator that increments
    // by `hits` each step. When it reaches or exceeds `steps`, emit a hit
    // and subtract `steps`.
    let mut result = Vec::with_capacity(steps);
    let mut acc = 0usize;
    for _ in 0..steps {
        acc += hits;
        if acc >= steps {
            result.push(true);
            acc -= steps;
        } else {
            result.push(false);
        }
    }

    // Apply rotation.
    let rot = rotation % steps;
    if rot > 0 {
        result.rotate_left(rot);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_euclidean_3_8() {
        // E(3,8) — the "tresillo" rhythm
        let pattern = euclidean(8, 3, 0);
        assert_eq!(pattern.len(), 8);
        assert_eq!(pattern.iter().filter(|&&b| b).count(), 3);
        // Bucket-fill produces: [. . X . . X . X]
        assert_eq!(pattern, vec![false, false, true, false, false, true, false, true]);
    }

    #[test]
    fn test_euclidean_4_16() {
        // E(4,16) — four-on-the-floor with gaps
        let pattern = euclidean(16, 4, 0);
        assert_eq!(pattern.len(), 16);
        assert_eq!(pattern.iter().filter(|&&b| b).count(), 4);
        // Bucket-fill produces: [. . . X . . . X . . . X . . . X]
        assert_eq!(
            pattern,
            vec![
                false, false, false, true,
                false, false, false, true,
                false, false, false, true,
                false, false, false, true,
            ]
        );
    }

    #[test]
    fn test_euclidean_all_hits() {
        let pattern = euclidean(4, 4, 0);
        assert_eq!(pattern, vec![true, true, true, true]);
    }

    #[test]
    fn test_euclidean_no_hits() {
        let pattern = euclidean(4, 0, 0);
        assert_eq!(pattern, vec![false, false, false, false]);
    }

    #[test]
    fn test_euclidean_rotation() {
        let pattern = euclidean(8, 3, 2);
        assert_eq!(pattern.len(), 8);
        assert_eq!(pattern.iter().filter(|&&b| b).count(), 3);
        // Base pattern [. . X . . X . X] rotated left by 2 = [X . . X . X . .]
        assert_eq!(pattern, vec![true, false, false, true, false, true, false, false]);
    }

    #[test]
    fn test_euclidean_5_8() {
        // E(5,8) — the "cinquillo" rhythm
        let pattern = euclidean(8, 5, 0);
        assert_eq!(pattern.len(), 8);
        assert_eq!(pattern.iter().filter(|&&b| b).count(), 5);
    }

    #[test]
    fn test_euclidean_2_5() {
        // E(2,5) — the "khafif-e-ramal" rhythm
        let pattern = euclidean(5, 2, 0);
        assert_eq!(pattern.len(), 5);
        assert_eq!(pattern.iter().filter(|&&b| b).count(), 2);
        // Bucket-fill produces: [. . X . X]
        assert_eq!(pattern, vec![false, false, true, false, true]);
    }
}
