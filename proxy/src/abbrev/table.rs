//! The abbreviation table (`R` → `ℝ`, `alpha` → `α`, …).
//!
//! Loaded from the proxy's embedded abbreviation-to-Unicode mapping.

use std::collections::BTreeMap;
use std::ops::Bound;

const ABBREVIATIONS: &str = include_str!("../../data/abbreviations.json");

pub struct Table {
    map: BTreeMap<String, String>,
}

impl Table {
    pub fn load() -> Self {
        Self {
            map: serde_json::from_str(ABBREVIATIONS)
                .expect("invalid embedded Lean abbreviation table"),
        }
    }

    #[cfg(test)]
    pub fn from_pairs(pairs: &[(&str, &str)]) -> Self {
        Self {
            map: pairs
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect(),
        }
    }

    pub fn get(&self, abbr: &str) -> Option<&str> {
        self.map.get(abbr).map(String::as_str)
    }

    /// Whether some abbreviation starts with `s` (the empty string is a prefix of all).
    pub fn is_prefix(&self, s: &str) -> bool {
        self.first_extending(s, Bound::Included(s))
    }

    /// Whether `s` is an abbreviation that no longer abbreviation extends.
    pub fn is_complete(&self, s: &str) -> bool {
        self.map.contains_key(s) && !self.first_extending(s, Bound::Excluded(s))
    }

    /// Whether the first key from `from` on starts with `s`.
    fn first_extending(&self, s: &str, from: Bound<&str>) -> bool {
        self.map
            .range::<str, _>((from, Bound::Unbounded))
            .next()
            .is_some_and(|(k, _)| k.starts_with(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_abbreviations() {
        let table = Table::load();
        assert!(table.map.len() > 1000);
        assert_eq!(table.get("R"), Some("ℝ"));
        assert_eq!(table.get("alpha"), Some("α"));
        assert_eq!(table.get("<>"), Some("⟨⟩"));
        assert_eq!(table.get("calc"), None);
    }

    #[test]
    fn prefix_and_complete() {
        let table = Table::from_pairs(&[("R", "ℝ"), ("Rat", "ℚ"), ("alpha", "α")]);
        assert!(table.is_prefix(""));
        assert!(table.is_prefix("Ra"));
        assert!(!table.is_prefix("Rb"));
        assert!(!table.is_complete("R"));
        assert!(table.is_complete("Rat"));
        assert!(table.is_complete("alpha"));
        assert!(!table.is_complete("alp"));
    }
}
