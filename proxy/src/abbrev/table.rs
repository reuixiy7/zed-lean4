//! The abbreviation table (`R` → `ℝ`, `alpha` → `α`, …).
//!
//! Loaded from the proxy's embedded mapping. `$CURSOR` markers are compiled once
//! into snippets, alongside plain text fallbacks for clients without snippet support.

use std::collections::BTreeMap;
use std::ops::Bound;

const ABBREVIATIONS: &str = include_str!("../../data/abbreviations.json");

#[derive(Debug, PartialEq, Eq)]
pub struct Expansion {
    pub text: String,
    pub snippet: Option<String>,
}

impl Expansion {
    fn new(value: String) -> Self {
        let Some((before, after)) = value.split_once("$CURSOR") else {
            return Self {
                text: value,
                snippet: None,
            };
        };
        Self {
            text: format!("{before}{after}"),
            snippet: Some(format!(
                "{}${{0}}{}",
                escape_snippet(before),
                escape_snippet(after)
            )),
        }
    }
}

/// Escape literal text so only the generated cursor marker is interpreted.
fn escape_snippet(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        if matches!(ch, '\\' | '$' | '}') {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

pub struct Table {
    map: BTreeMap<String, Expansion>,
}

impl Table {
    pub fn load() -> Self {
        Self {
            map: serde_json::from_str::<BTreeMap<String, String>>(ABBREVIATIONS)
                .expect("invalid embedded Lean abbreviation table")
                .into_iter()
                .map(|(abbr, value)| (abbr, Expansion::new(value)))
                .collect(),
        }
    }

    #[cfg(test)]
    pub fn from_pairs(pairs: &[(&str, &str)]) -> Self {
        Self {
            map: pairs
                .iter()
                .map(|(a, b)| (a.to_string(), Expansion::new(b.to_string())))
                .collect(),
        }
    }

    #[cfg(test)]
    pub fn get(&self, abbr: &str) -> Option<&str> {
        self.map.get(abbr).map(|expansion| expansion.text.as_str())
    }

    /// The shortest abbreviation starting with `prefix`; ties use lexical order.
    /// Empty prefixes never choose an arbitrary symbol.
    pub fn shortest_completion(&self, prefix: &str) -> Option<(&str, &Expansion)> {
        if prefix.is_empty() {
            return None;
        }
        if let Some((abbr, symbol)) = self.map.get_key_value(prefix) {
            return Some((abbr.as_str(), symbol));
        }
        self.map
            .range::<str, _>((Bound::Included(prefix), Bound::Unbounded))
            .take_while(|(abbr, _)| abbr.starts_with(prefix))
            .min_by_key(|(abbr, _)| abbr.len())
            .map(|(abbr, symbol)| (abbr.as_str(), symbol))
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

    #[test]
    fn shortest_completion_prefers_exact_then_shortest_then_lexical_order() {
        let table = Table::from_pairs(&[
            ("R", "ℝ"),
            ("Rat", "ℚ"),
            ("Real", "ℝ"),
            ("alpine", "longer"),
            ("alpha", "α"),
            ("ax", "first"),
            ("ay", "second"),
        ]);
        let shortest = |prefix| {
            table
                .shortest_completion(prefix)
                .map(|(abbr, expansion)| (abbr, expansion.text.as_str()))
        };
        assert_eq!(shortest("R"), Some(("R", "ℝ")));
        assert_eq!(shortest("Ra"), Some(("Rat", "ℚ")));
        assert_eq!(shortest("alp"), Some(("alpha", "α")));
        assert_eq!(shortest("a"), Some(("ax", "first")));
        assert_eq!(table.shortest_completion(""), None);
        assert_eq!(table.shortest_completion("unknown"), None);
    }

    #[test]
    fn cursor_markers_produce_safe_snippets_and_plain_text_fallbacks() {
        let table = Table::load();
        for (abbr, text, snippet) in [
            ("<>", "⟨⟩", "⟨${0}⟩"),
            ("[[]]", "⟦⟧", "⟦${0}⟧"),
            ("f<<>>", "«»", "«${0}»"),
            ("norm", "‖‖", "‖${0}‖"),
            ("nfloor", "⌊⌋₊", "⌊${0}⌋₊"),
        ] {
            let (_, expansion) = table.shortest_completion(abbr).unwrap();
            assert_eq!(expansion.text, text);
            assert_eq!(expansion.snippet.as_deref(), Some(snippet));
        }
        assert_eq!(
            table.map.values().filter(|e| e.snippet.is_some()).count(),
            26
        );
        assert!(table.map.values().all(|e| !e.text.contains("$CURSOR")));
        let escaped = Expansion::new("\\${$CURSOR}1".into());
        assert_eq!(escaped.text, "\\${}1");
        assert_eq!(escaped.snippet.as_deref(), Some("\\\\\\${${0}\\}1"));
        assert!(
            table
                .shortest_completion("alpha")
                .unwrap()
                .1
                .snippet
                .is_none()
        );
    }
}
