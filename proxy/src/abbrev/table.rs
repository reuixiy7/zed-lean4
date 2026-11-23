//! The abbreviation table (`R` → `ℝ`, `alpha` → `α`, …).
//!
//! Loaded from the proxy's embedded mapping. `$CURSOR` markers are compiled once
//! into snippets, alongside plain text fallbacks for clients without snippet support.

use std::collections::BTreeMap;
use std::ops::{Bound, Range};

pub(super) const ABBREVIATIONS: &str = include_str!("../../data/abbreviations.json");

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
    reverse: BTreeMap<String, Vec<String>>,
    max_symbol_bytes: usize,
}

pub struct Symbol<'a> {
    pub range: Range<usize>,
    pub text: &'a str,
    pub abbreviations: &'a [String],
}

impl Table {
    pub fn load() -> Self {
        Self::new(
            serde_json::from_str::<BTreeMap<String, String>>(ABBREVIATIONS)
                .expect("invalid embedded Lean abbreviation table")
                .into_iter()
                .map(|(abbr, value)| (abbr, Expansion::new(value)))
                .collect(),
        )
    }

    fn new(map: BTreeMap<String, Expansion>) -> Self {
        let mut reverse: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (abbr, expansion) in &map {
            // Do not clutter ordinary ASCII code with Unicode input hints.
            if !expansion.text.is_ascii() {
                reverse
                    .entry(expansion.text.clone())
                    .or_default()
                    .push(abbr.clone());
            }
        }
        for abbreviations in reverse.values_mut() {
            abbreviations.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
        }
        let max_symbol_bytes = reverse.keys().map(String::len).max().unwrap_or(0);
        Self {
            map,
            reverse,
            max_symbol_bytes,
        }
    }

    #[cfg(test)]
    pub fn from_pairs(pairs: &[(&str, &str)]) -> Self {
        Self::new(
            pairs
                .iter()
                .map(|(a, b)| (a.to_string(), Expansion::new(b.to_string())))
                .collect(),
        )
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

    /// Longest complete Unicode mapping covering the hovered character.
    /// Inspect only a window bounded by the longest symbol, never the whole table.
    pub fn symbol_at<'a>(&'a self, line: &str, byte: usize) -> Option<Symbol<'a>> {
        if byte >= line.len() || !line.is_char_boundary(byte) {
            return None;
        }
        let mut best: Option<Symbol<'_>> = None;
        let first = byte.saturating_sub(self.max_symbol_bytes.saturating_sub(1));
        for start in (first..=byte).filter(|&start| line.is_char_boundary(start)) {
            let last = (start + self.max_symbol_bytes).min(line.len());
            for end in (byte + 1..=last)
                .rev()
                .filter(|&end| line.is_char_boundary(end))
            {
                if best
                    .as_ref()
                    .is_some_and(|best| best.text.len() >= end - start)
                {
                    break;
                }
                if let Some((text, abbreviations)) = self.reverse.get_key_value(&line[start..end]) {
                    best = Some(Symbol {
                        range: start..end,
                        text,
                        abbreviations,
                    });
                    break;
                }
            }
        }
        best
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
    fn reverse_lookup_prefers_complete_symbols_and_lists_all_aliases() {
        let table = Table::load();
        for byte in [1, 4] {
            let symbol = table.symbol_at(" →ₗ ", byte).unwrap();
            assert_eq!(symbol.text, "→ₗ");
            assert_eq!(symbol.range, 1..7);
            assert_eq!(
                symbol.abbreviations,
                ["rl", "r_l", "tol", "to_l", "linearmap"]
            );
        }
        let real = table.symbol_at("ℝ", 0).unwrap();
        assert_eq!(real.abbreviations, ["R", "br", "bbR", "Real", "real"]);
        let pair = table.symbol_at("⟨⟩", 0).unwrap();
        assert!(pair.abbreviations.iter().any(|abbr| abbr == "<>"));
        for (symbol, abbreviations) in &table.reverse {
            for (byte, _) in symbol.char_indices() {
                let found = table.symbol_at(symbol, byte).unwrap();
                assert_eq!(found.text, symbol);
                assert_eq!(found.abbreviations, abbreviations);
            }
        }
        for (text, byte) in [
            ("Nat", 0),
            ("{}", 0),
            ("ℝ ", 3),
            ("ℝ", 1),
            ("ℝ", 3),
            ("", 0),
        ] {
            assert!(table.symbol_at(text, byte).is_none(), "{text}, {byte}");
        }
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
