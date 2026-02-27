//! Fractional indexing for ordered feature lists.
//!
//! Uses lexicographic ordering of byte sequences to allow inserting items
//! between any two existing items without renumbering.

use serde::{Deserialize, Serialize};

/// A position in an ordered list, supporting arbitrary insertions.
///
/// Implemented as a variable-length byte sequence with lexicographic ordering.
/// `between(a, b)` produces a position strictly between `a` and `b`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FractionalIndex(Vec<u8>);

impl FractionalIndex {
    /// Create a position between two optional bounds.
    ///
    /// - `between(None, None)` → middle of the range
    /// - `between(None, Some(b))` → before b
    /// - `between(Some(a), None)` → after a
    /// - `between(Some(a), Some(b))` → between a and b
    pub fn between(lo: Option<&FractionalIndex>, hi: Option<&FractionalIndex>) -> Self {
        match (lo, hi) {
            (None, None) => FractionalIndex(vec![128]),
            (None, Some(h)) => {
                // Before h: find a position less than h
                let mut result = Vec::new();
                for &b in &h.0 {
                    if b > 0 {
                        result.push(b / 2);
                        if result.last() != Some(&0) || result.len() > 1 {
                            return FractionalIndex(result);
                        }
                    }
                    result.push(b);
                }
                // All zeros — extend
                result.push(0);
                FractionalIndex(result)
            }
            (Some(l), None) => {
                // After l: find a position greater than l
                let mut result = Vec::new();
                for &b in &l.0 {
                    if b < 255 {
                        result.push(b + (255 - b).div_ceil(2));
                        return FractionalIndex(result);
                    }
                    result.push(b);
                }
                // All 255s — extend
                result.push(128);
                FractionalIndex(result)
            }
            (Some(l), Some(h)) => {
                // Between l and h: find midpoint
                let max_len = l.0.len().max(h.0.len()) + 1;
                let mut result = Vec::new();

                for i in 0..max_len {
                    let lb = l.0.get(i).copied().unwrap_or(0);
                    let hb = h.0.get(i).copied().unwrap_or(255);

                    if hb - lb > 1 {
                        result.push(lb + (hb - lb) / 2);
                        return FractionalIndex(result);
                    }

                    result.push(lb);
                }

                // Extend with midpoint
                result.push(128);
                FractionalIndex(result)
            }
        }
    }

    /// Get the raw bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl PartialOrd for FractionalIndex {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FractionalIndex {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.cmp(&other.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_between_none_none() {
        let mid = FractionalIndex::between(None, None);
        assert_eq!(mid.0, vec![128]);
    }

    #[test]
    fn test_between_ordering() {
        let a = FractionalIndex::between(None, None);
        let b = FractionalIndex::between(Some(&a), None);
        let c = FractionalIndex::between(None, Some(&a));

        assert!(c < a);
        assert!(a < b);
    }

    #[test]
    fn test_between_two_positions() {
        let a = FractionalIndex::between(None, None);
        let b = FractionalIndex::between(Some(&a), None);
        let mid = FractionalIndex::between(Some(&a), Some(&b));

        assert!(a < mid);
        assert!(mid < b);
    }

    #[test]
    fn test_many_insertions() {
        let mut positions = vec![FractionalIndex::between(None, None)];

        // Insert 20 items after the last
        for _ in 0..20 {
            let last = positions.last().unwrap();
            positions.push(FractionalIndex::between(Some(last), None));
        }

        // Verify strict ordering
        for pair in positions.windows(2) {
            assert!(pair[0] < pair[1], "{:?} should be < {:?}", pair[0], pair[1]);
        }
    }

    #[test]
    fn test_many_insertions_between() {
        let lo = FractionalIndex::between(None, None);
        let hi = FractionalIndex::between(Some(&lo), None);
        let mut current_hi = hi.clone();

        // Insert 10 items between fixed lo and a shrinking hi
        for _ in 0..10 {
            let mid = FractionalIndex::between(Some(&lo), Some(&current_hi));
            assert!(lo < mid, "{:?} should be < {:?}", lo, mid);
            assert!(mid < current_hi, "{:?} should be < {:?}", mid, current_hi);
            current_hi = mid;
        }
    }
}
