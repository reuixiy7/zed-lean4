//! Unicode abbreviation input: `\R` → `ℝ` as you type, like the VSCode extension.
//!
//! After each typed insertion the text between the nearest `\` and the cursor is
//! checked against the [`Table`]:
//! - it is a complete abbreviation that nothing else extends (`\alpha`) → replace now;
//! - it was a valid abbreviation and the new character no longer extends any
//!   abbreviation (`\R` + space) → replace the abbreviation, keep the new character.
//!
//! The replacement is sent to Zed as a `workspace/applyEdit` request.

mod table;

pub use table::Table;

use crate::document::{Document, Insertion, column_offset, utf16_len};
use crate::lsp::Position;

#[derive(Debug, PartialEq, Eq)]
pub struct Replacement {
    pub start: Position,
    pub end: Position,
    pub text: String,
}

/// Checks an insertion into `text` (the document after the insertion).
pub fn check(table: &Table, doc: &Document, insertion: &Insertion) -> Option<Replacement> {
    if insertion.text.contains('\n') {
        return None;
    }
    let line = doc.line(insertion.start.line);
    let start = column_offset(line, insertion.start.character);
    let end = start + insertion.text.len();
    if line.get(start..end) != Some(insertion.text.as_str()) {
        return None;
    }

    let backslash = line[..start].rfind('\\')?;
    let typed_before = &line[backslash + 1..start];
    // Only continue an abbreviation that was valid before this keystroke.
    if !table.is_prefix(typed_before) || !doc.allows_abbreviation(insertion.start.line, backslash) {
        return None;
    }

    let position = |byte: usize| Position {
        line: insertion.start.line,
        character: utf16_len(&line[..byte]),
    };
    let typed = &line[backslash + 1..end];

    let (abbreviation, until) = if !insertion.text.contains('\\') && table.is_prefix(typed) {
        // Still typing; replace early only when nothing longer can follow.
        if !table.is_complete(typed) {
            return None;
        }
        (typed, end)
    } else {
        // The new text ended the abbreviation; keep it.
        (typed_before, start)
    };
    Some(Replacement {
        start: position(backslash),
        end: position(until),
        text: table.get(abbreviation)?.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> Table {
        Table::from_pairs(&[
            ("R", "ℝ"),
            ("Rat", "ℚ"),
            ("alpha", "α"),
            ("a", "α"),
            ("t", "▸"),
            ("to", "→"),
            ("top", "⊤"),
        ])
    }

    /// Types `typed` at the end of `line` and runs the check.
    fn typing(line: &str, typed: &str) -> Option<Replacement> {
        let text = format!("x\n{line}{typed}\ny");
        let insertion = Insertion {
            start: Position {
                line: 1,
                character: utf16_len(line),
            },
            text: typed.to_owned(),
        };
        check(&table(), &Document::new(text, 1), &insertion)
    }

    fn rep(start: u64, end: u64, text: &str) -> Option<Replacement> {
        Some(Replacement {
            start: Position {
                line: 1,
                character: start,
            },
            end: Position {
                line: 1,
                character: end,
            },
            text: text.to_owned(),
        })
    }

    #[test]
    fn waits_while_ambiguous() {
        assert_eq!(typing("x : \\", "R"), None);
        assert_eq!(typing("x : \\R", "a"), None);
        assert_eq!(typing("\\t", "o"), None);
    }

    #[test]
    fn replaces_when_terminated() {
        assert_eq!(typing("x : \\R", " "), rep(4, 6, "ℝ"));
        assert_eq!(typing("(x : \\R", ")"), rep(5, 7, "ℝ"));
        assert_eq!(typing("\\to", " "), rep(0, 3, "→"));
    }

    #[test]
    fn replaces_complete_immediately() {
        assert_eq!(typing("x : \\Ra", "t"), rep(4, 8, "ℚ"));
        assert_eq!(typing("\\alph", "a"), rep(0, 6, "α"));
    }

    #[test]
    fn next_backslash_terminates_previous() {
        assert_eq!(typing("\\R", "\\"), rep(0, 2, "ℝ"));
    }

    #[test]
    fn ignores_unknown_and_stale() {
        // `\q` was never a valid abbreviation.
        assert_eq!(typing("\\q", " "), None);
        // An abandoned `\Rx` far back on the line does not fire later.
        assert_eq!(typing("\\Rx foo", " "), None);
    }

    #[test]
    fn utf16_columns() {
        assert_eq!(typing("𝔸 \\R", " "), rep(3, 5, "ℝ"));
    }

    #[test]
    fn ignores_string_literals() {
        assert_eq!(typing("s := \"\\t", "\""), None);
        assert_eq!(typing("s := \"a\" ++ \\t", "o"), None);
        assert_eq!(typing("s := \"a\" \\to", " "), rep(9, 12, "→"));
    }

    fn after_prefix(prefix: &str, typed: &str) -> Option<Replacement> {
        let insertion = Insertion {
            start: Position {
                line: prefix.bytes().filter(|&b| b == b'\n').count() as u64,
                character: utf16_len(prefix.rsplit('\n').next().unwrap()),
            },
            text: typed.to_owned(),
        };
        check(
            &table(),
            &Document::new(format!("{prefix}{typed}"), 1),
            &insertion,
        )
    }

    #[test]
    fn preserves_multiline_raw_and_character_literals() {
        for prefix in [
            "def s := \"first\n\\t",
            "def s := r#\"first\n\" \\t",
            "def s := r##\"first\n\"# \\t",
            "def c := '\\t",
            "def s := s!\"first\n\\t",
            "def s := s!\"{\"nested\"}\n\\t",
        ] {
            assert_eq!(after_prefix(prefix, " "), None, "{prefix}");
        }
    }

    #[test]
    fn comments_do_not_change_string_context() {
        for prefix in [
            "-- unmatched \"\nx : \\alph",
            "/- \" /- nested -/ -/\nx : \\alph",
            "-- math with a quote \" \\alph",
            "/- math with a quote \"\n\\alph",
            "def c := '\"'\nx : \\alph",
            "def s := r##\"\"#\n\"##\nx : \\alph",
            "theorem foo' : \\alph",
        ] {
            assert_eq!(after_prefix(prefix, "a").unwrap().text, "α", "{prefix}");
        }
    }

    #[test]
    fn expands_code_inside_interpolations_and_after_strings() {
        for prefix in [
            "def s := s!\"{(0 : \\alph",
            "def s := s!\"{ { field := \"nested\" } }\"; \\alph",
            "def s := \"line one\nline two\"\nx : \\alph",
        ] {
            assert_eq!(after_prefix(prefix, "a").unwrap().text, "α", "{prefix}");
        }
    }
}
