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

use crate::document::{Insertion, column_offset, line_range, utf16_len};
use crate::lsp::Position;

#[derive(Debug, PartialEq, Eq)]
pub struct Replacement {
    pub start: Position,
    pub end: Position,
    pub text: String,
}

/// Checks an insertion into `text` (the document after the insertion).
pub fn check(table: &Table, text: &str, insertion: &Insertion) -> Option<Replacement> {
    if insertion.text.contains('\n') {
        return None;
    }
    let line = &text[line_range(text, insertion.start.line)];
    let start = column_offset(line, insertion.start.character);
    let end = start + insertion.text.len();
    if line.get(start..end) != Some(insertion.text.as_str()) {
        return None;
    }

    let backslash = line[..start].rfind('\\')?;
    let typed_before = &line[backslash + 1..start];
    // Only continue an abbreviation that was valid before this keystroke.
    if !table.is_prefix(typed_before) || inside_string(&line[..backslash]) {
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

/// Whether the end of `prefix` is inside a string literal (odd number of unescaped quotes).
fn inside_string(prefix: &str) -> bool {
    let mut inside = false;
    let mut escaped = false;
    for c in prefix.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' if inside => escaped = true,
            '"' => inside = !inside,
            _ => {}
        }
    }
    inside
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
        check(&table(), &text, &insertion)
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
}
