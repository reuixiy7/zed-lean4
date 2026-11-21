//! Diagnostics: at the cursor line, counts, and all messages of the file.

use super::code::push_code;
use crate::lsp::{Diagnostic, Severity};

/// Rows shown in "All messages"; the rest is summarized.
const MAX_ROWS: usize = 50;
/// Characters of a message shown in an "All messages" row.
const MAX_ROW_CHARS: usize = 100;

const PROVED: &str = "✅";

fn icon(d: &Diagnostic) -> &'static str {
    if d.accomplished {
        return PROVED;
    }
    severity_icon(d.severity)
}

fn severity_icon(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "❌",
        Severity::Warning => "⚠️",
        Severity::Information => "ℹ️",
        Severity::Hint => "💡",
    }
}

/// `❌ 2`, `⚠️ 1`, `✅ 5` (proved theorems) for the status line.
pub fn counts(diagnostics: &[Diagnostic]) -> Vec<String> {
    let (mut errors, mut warnings, mut proved) = (0, 0, 0);
    for d in diagnostics {
        match d.severity {
            _ if d.accomplished => proved += 1,
            _ if d.silent => {}
            Severity::Error => errors += 1,
            Severity::Warning => warnings += 1,
            _ => {}
        }
    }
    [
        (severity_icon(Severity::Error), errors),
        (severity_icon(Severity::Warning), warnings),
        (PROVED, proved),
    ]
    .into_iter()
    .filter(|&(_, n)| n > 0)
    .map(|(icon, n)| format!("{icon} {n}"))
    .collect()
}

/// Appends the messages covering the cursor line; returns whether there were any.
pub fn at_cursor(out: &mut String, diagnostics: &[Diagnostic], line: u64) -> bool {
    let mut any = false;
    for d in diagnostics.iter().filter(|d| d.covers_line(line)) {
        if !any {
            out.push_str("## Messages\n\n");
            any = true;
        }
        let start = d.start;
        out.push_str(&format!(
            "{} `{}:{}`\n\n",
            icon(d),
            start.line + 1,
            start.character + 1
        ));
        push_code(out, &d.message);
    }
    any
}

/// Appends a table of all messages of the file, except silent ones.
pub fn all(out: &mut String, diagnostics: &[Diagnostic]) {
    let mut sorted: Vec<&Diagnostic> = diagnostics.iter().filter(|d| !d.silent).collect();
    if sorted.is_empty() {
        return;
    }
    sorted.sort_by_key(|d| (d.start.line, d.start.character, d.severity));

    out.push_str(&format!("---\n\n## All messages ({})\n\n", sorted.len()));
    out.push_str("| | Line | Message |\n|---|---|---|\n");
    for d in sorted.iter().take(MAX_ROWS) {
        out.push_str(&format!(
            "| {} | {} | {} |\n",
            icon(d),
            d.start.line + 1,
            table_cell(&d.message)
        ));
    }
    if sorted.len() > MAX_ROWS {
        out.push_str(&format!("\n_… and {} more_\n", sorted.len() - MAX_ROWS));
    }
    out.push('\n');
}

/// First line of a message as inline code, safe inside a table cell.
fn table_cell(message: &str) -> String {
    let first = message.lines().next().unwrap_or("").trim();
    let mut text: String = first.chars().take(MAX_ROW_CHARS).collect();
    if first.chars().count() > MAX_ROW_CHARS || message.lines().nth(1).is_some() {
        text.push('…');
    }
    let text = text.replace('`', "'").replace('|', "\\|");
    if text.is_empty() {
        String::new()
    } else {
        format!("`{text}`")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn diag(severity: u64, line: u64, end: u64, message: &str) -> Diagnostic {
        Diagnostic::from_json(&mut json!({"severity": severity, "message": message,
               "range": {"start": {"line": line, "character": 2}, "end": {"line": end, "character": 0}}}))
        .unwrap()
    }

    #[test]
    fn counts_errors_and_warnings() {
        let d = [
            diag(1, 0, 0, "a"),
            diag(2, 1, 1, "b"),
            diag(2, 2, 2, "c"),
            diag(3, 3, 3, "d"),
        ];
        assert_eq!(counts(&d), ["❌ 1", "⚠️ 2"]);
        assert!(counts(&[]).is_empty());
    }

    fn proved(line: u64) -> Diagnostic {
        Diagnostic::from_json(&mut json!({"severity": 3, "message": "Goals accomplished!",
            "isSilent": true, "leanTags": [2],
            "range": {"start": {"line": line, "character": 0}, "end": {"line": line + 1, "character": 0}}}))
        .unwrap()
    }

    #[test]
    fn proved_theorems_are_counted_and_shown_only_at_cursor() {
        let d = [
            proved(0),
            proved(3),
            diag(2, 5, 5, "declaration uses 'sorry'"),
        ];
        assert_eq!(counts(&d), ["⚠️ 1", "✅ 2"]);

        let mut out = String::new();
        assert!(at_cursor(&mut out, &d, 0));
        assert!(out.contains("✅ `1:1`"), "{out}");

        let mut out = String::new();
        all(&mut out, &d);
        assert!(out.contains("## All messages (1)"), "{out}");
        assert!(!out.contains("Goals accomplished"), "{out}");
        let mut out = String::new();
        all(&mut out, &d[..2]);
        assert!(out.is_empty(), "{out}");
    }

    #[test]
    fn messages_at_cursor_line() {
        let d = [diag(3, 4, 6, "Nat : Type"), diag(1, 9, 9, "far")];
        let mut out = String::new();
        assert!(at_cursor(&mut out, &d, 5));
        assert!(
            out.contains("ℹ️ `5:3`\n\n```lean\nNat : Type\n```"),
            "{out}"
        );
        assert!(!out.contains("far"));
        assert!(!at_cursor(&mut String::new(), &d, 0));
    }

    #[test]
    fn all_messages_table_sorted_and_escaped() {
        let d = [
            diag(1, 9, 9, "bad | `x`\nmore"),
            diag(2, 1, 1, "declaration uses 'sorry'"),
        ];
        let mut out = String::new();
        all(&mut out, &d);
        let sorry = out
            .find("| ⚠️ | 2 | `declaration uses 'sorry'` |")
            .expect(&out);
        let bad = out.find("| ❌ | 10 | `bad \\| 'x'…` |").expect(&out);
        assert!(sorry < bad);
    }
}
