//! Renders the infoview as Markdown for Zed's Markdown preview.
//!
//! Layout, top to bottom:
//! - status line: position, elaboration progress, error / warning counts (`header`);
//! - goals and expected type (`goals`);
//! - messages at the cursor line and all messages of the file (`messages`).

mod code;
mod goals;
mod messages;

pub use code::hover_goals;

use crate::lsp::Diagnostic;
use serde_json::Value;

/// Lean's answers for one cursor position (`null` where there is nothing).
#[derive(Default)]
pub struct Goals {
    /// `$/lean/plainGoal` at the cursor.
    pub plain: Value,
    /// `$/lean/plainTermGoal` at the cursor.
    pub term: Value,
    /// `$/lean/plainGoal` at the start of the line: the state before the tactic.
    pub before: Value,
}

pub struct View<'a> {
    /// Display name of the file, e.g. `Mathlib/Foo.lean`.
    pub file: &'a str,
    /// Zero-based LSP position.
    pub line: u64,
    pub character: u64,
    /// Line Lean is elaborating, while the file is being processed.
    pub elaborating: Option<u64>,
    /// The goals shown are from an earlier position or version; new ones are on the way.
    pub stale: bool,
    pub goals: &'a Goals,
    /// All diagnostics of the file.
    pub diagnostics: &'a [Diagnostic],
}

pub const PLACEHOLDER: &str = "_Move the cursor inside a Lean file to see goals._\n";

pub fn render(view: &View) -> String {
    let mut out = String::new();
    header(&mut out, view);
    let goals = goals::render(&mut out, view.goals);
    let messages = messages::at_cursor(&mut out, view.diagnostics, view.line);
    if !goals && !messages {
        out.push_str("_No info at cursor._\n\n");
    }
    messages::all(&mut out, view.diagnostics);
    out
}

fn header(out: &mut String, view: &View) {
    let mut parts = vec![format!(
        "`{}:{}:{}`",
        view.file,
        view.line + 1,
        view.character + 1
    )];
    parts.push(match view.elaborating {
        Some(line) => format!("⏳ elaborating line {}", line + 1),
        None => "✓ ready".into(),
    });
    if view.stale {
        parts.push("_updating…_".into());
    }
    parts.extend(messages::counts(view.diagnostics));
    out.push_str(&parts.join(" · "));
    out.push_str("\n\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn full_view() {
        let plain = json!({"goals": ["case succ\nn : Nat\n⊢ P (n + 1)", "case zero\n⊢ P 0"]});
        let diags =
            [
                Diagnostic::from_json(&mut json!({"severity": 1, "message": "unsolved goals",
            "range": {"start": {"line": 2, "character": 0}, "end": {"line": 2, "character": 3}}}))
                .unwrap(),
            ];
        let md = render(&View {
            file: "A.lean",
            line: 2,
            character: 4,
            elaborating: Some(9),
            stale: true,
            goals: &Goals {
                plain,
                ..Goals::default()
            },
            diagnostics: &diags,
        });
        assert!(
            md.starts_with("`A.lean:3:5` · ⏳ elaborating line 10 · _updating…_ · ❌ 1\n"),
            "{md}"
        );
        assert!(md.contains("## Goal 1 of 2 · `case succ`"), "{md}");
        assert!(md.contains("### Goal 2 of 2 · `case zero`"), "{md}");
        assert!(md.contains("## Messages"), "{md}");
        assert!(md.contains("## All messages (1)"), "{md}");
        assert!(!md.contains("No info"), "{md}");
    }

    #[test]
    fn empty_view() {
        let md = render(&View {
            file: "A.lean",
            line: 0,
            character: 0,
            elaborating: None,
            stale: false,
            goals: &Goals::default(),
            diagnostics: &[],
        });
        assert_eq!(md, "`A.lean:1:1` · ✓ ready\n\n_No info at cursor._\n\n");
    }
}
