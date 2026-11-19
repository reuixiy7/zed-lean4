//! Diagnostics: at the cursor line, counts, and all messages of the file.

use super::code::push_code;
use crate::lsp::{Diagnostic, Severity};

/// Rows shown in "All messages"; the rest is summarized.
const MAX_ROWS: usize = 50;
/// Characters of a message shown in an "All messages" row.
const MAX_ROW_CHARS: usize = 100;

fn icon(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "❌",
        Severity::Warning => "⚠️",
        Severity::Information => "ℹ️",
        Severity::Hint => "💡",
    }
}

/// `❌ 2`, `⚠️ 1` for the status line.
pub fn counts(diagnostics: &[Diagnostic]) -> Vec<String> {
    let (mut errors, mut warnings) = (0, 0);
    for d in diagnostics {
        match d.severity {
            Severity::Error => errors += 1,
            Severity::Warning => warnings += 1,
            _ => {}
        }
    }
    [(Severity::Error, errors), (Severity::Warning, warnings)]
        .into_iter()
        .filter(|&(_, n)| n > 0)
        .map(|(severity, n)| format!("{} {n}", icon(severity)))
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
            icon(d.severity),
            start.line + 1,
            start.character + 1
        ));
        push_code(out, &d.message);
    }
    any
}

/// Appends a table of all messages of the file.
pub fn all(out: &mut String, diagnostics: &[Diagnostic]) {
    if diagnostics.is_empty() {
        return;
    }
    let mut sorted: Vec<&Diagnostic> = diagnostics.iter().collect();
    sorted.sort_by_key(|d| (d.start.line, d.start.character, d.severity));

    out.push_str(&format!("---\n\n## All messages ({})\n\n", sorted.len()));
    out.push_str("| | Line | Message |\n|---|---|---|\n");
    for d in sorted.iter().take(MAX_ROWS) {
        out.push_str(&format!(
            "| {} | {} | {} |\n",
            icon(d.severity),
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
