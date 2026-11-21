//! The abbreviation table (`R` → `ℝ`, `alpha` → `α`, …).
//!
//! Loaded from the extension's snippets file, whose `lean-*` entries are the Lean 4
//! abbreviations (`"prefix": "\\R", "body": "ℝ"`), so there is a single source of truth.

use serde_json::Value;
use std::collections::BTreeMap;
use std::ops::Bound;

const SNIPPETS: &str = include_str!("../../../snippets/lean 4.json");

pub struct Table {
    map: BTreeMap<String, String>,
}

impl Table {
    pub fn load() -> Self {
        let snippets: Value = serde_json::from_str(SNIPPETS).unwrap_or_default();
        let mut map = BTreeMap::new();
        for (name, snippet) in snippets.as_object().into_iter().flatten() {
            if !name.starts_with("lean-") {
                continue;
            }
            let prefix = snippet.get("prefix").and_then(Value::as_str);
            let body = snippet.get("body").and_then(Value::as_str);
            if let (Some(prefix), Some(body)) = (prefix, body)
                && let Some(abbr) = prefix.strip_prefix('\\')
            {
                // Bracket pairs such as `⟨$0⟩` mark the cursor; an edit cannot place it.
                map.insert(abbr.to_owned(), body.replace("$0", ""));
            }
        }
        Self { map }
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
    fn loads_snippets() {
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
