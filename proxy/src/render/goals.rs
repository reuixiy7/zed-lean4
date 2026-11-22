//! The tactic state and the expected type.

use super::Goals;
use super::code::{goal_list, push_code};
use serde_json::Value;
use std::collections::HashSet;

/// Marks lines of the main goal that the tactic at the cursor changed.
const CHANGED: &str = "  -- ✱";

/// Appends the goals section; returns whether anything was written.
pub fn render(out: &mut String, goals: &Goals) -> bool {
    let mut wrote = false;
    let plain = goal_list(&goals.plain);

    if let Some(plain) = &plain {
        wrote = true;
        if plain.is_empty() {
            out.push_str("## ✅ No goals\n\n");
        }
        let before = goal_list(&goals.before).filter(|before| before != plain);
        for (i, goal) in plain.iter().enumerate() {
            let (case, body) = split_case(goal);
            out.push_str(&heading(i, plain.len(), case));
            match (&before, i) {
                (Some(before), 0) => {
                    let (body, changed) = mark_changed(body, before);
                    push_code(out, &body);
                    if changed {
                        out.push_str("_✱ changed by the tactic at the cursor_\n\n");
                    }
                }
                _ => push_code(out, body),
            }
        }
    }

    if let Some(term) = goals.term.get("goal").and_then(Value::as_str) {
        let duplicate = plain
            .as_ref()
            .and_then(|p| p.first())
            .is_some_and(|main| target(main) == target(term));
        if !duplicate {
            wrote = true;
            out.push_str("## Expected type\n\n");
            push_code(out, term);
        }
    }
    wrote
}

fn heading(index: usize, count: usize, case: Option<&str>) -> String {
    let mut h = match (index, count) {
        (0, 1) => "## Goal".to_owned(),
        (0, n) => format!("## Goal 1 of {n}"),
        (i, n) => format!("### Goal {} of {n}", i + 1),
    };
    if let Some(case) = case {
        h.push_str(&format!(" · `case {case}`"));
    }
    h.push_str("\n\n");
    h
}

/// Splits a leading `case name` line off a goal.
fn split_case(goal: &str) -> (Option<&str>, &str) {
    match goal.strip_prefix("case ") {
        Some(rest) => {
            let (name, body) = rest.split_once('\n').unwrap_or((rest, ""));
            (Some(name.trim()), body)
        }
        None => (None, goal),
    }
}

/// The part after `⊢`.
fn target(goal: &str) -> &str {
    goal.rsplit_once('⊢').map_or(goal, |(_, t)| t).trim()
}

/// Appends [`CHANGED`] to lines not present in any goal before the tactic.
/// Marks nothing if no line is shared (a different goal, not a change).
fn mark_changed(body: &str, before: &[&str]) -> (String, bool) {
    let old: HashSet<&str> = before
        .iter()
        .flat_map(|g| g.lines())
        .map(str::trim_end)
        .collect();
    let lines: Vec<&str> = body.lines().collect();
    let new = lines.iter().filter(|l| !old.contains(l.trim_end())).count();
    if new == 0 || new == lines.len() {
        return (body.to_owned(), false);
    }
    let marked = lines
        .iter()
        .map(|l| {
            if old.contains(l.trim_end()) {
                (*l).to_owned()
            } else {
                format!("{l}{CHANGED}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    (marked, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn render_goals(plain: Value, term: Value, before: Value) -> String {
        let mut out = String::new();
        render(
            &mut out,
            &Goals {
                plain,
                term,
                before,
            },
        );
        out
    }

    #[test]
    fn single_and_multiple_goals() {
        let md = render_goals(
            json!({"goals": ["x : Nat\n⊢ x = x"]}),
            Value::Null,
            Value::Null,
        );
        assert!(
            md.starts_with("## Goal\n\n```lean\nx : Nat\n⊢ x = x\n```"),
            "{md}"
        );
        let md = render_goals(
            json!({"goals": ["case a\n⊢ A", "⊢ B"]}),
            Value::Null,
            Value::Null,
        );
        assert!(
            md.contains("## Goal 1 of 2 · `case a`\n\n```lean\n⊢ A\n```"),
            "{md}"
        );
        assert!(md.contains("### Goal 2 of 2\n\n```lean\n⊢ B\n```"), "{md}");
    }

    #[test]
    fn no_goals() {
        let md = render_goals(json!({"goals": []}), Value::Null, Value::Null);
        assert_eq!(md, "## ✅ No goals\n\n");
    }

    #[test]
    fn expected_type_hidden_when_same_as_goal() {
        let plain = json!({"goals": ["x : Nat\n⊢ x = x"]});
        let md = render_goals(
            plain.clone(),
            json!({"goal": "x : Nat\n⊢ x = x"}),
            Value::Null,
        );
        assert!(!md.contains("Expected type"), "{md}");
        let md = render_goals(plain, json!({"goal": "x : Nat\n⊢ Nat"}), Value::Null);
        assert!(md.contains("## Expected type"), "{md}");
    }

    #[test]
    fn marks_changed_lines() {
        let before = json!({"goals": ["a b : Nat\nh : a = b\n⊢ b = a"]});
        let after = json!({"goals": ["a b : Nat\nh : a = b\nh' : b = a\n⊢ b = a"]});
        let md = render_goals(after, Value::Null, before);
        assert!(md.contains("h' : b = a  -- ✱\n⊢ b = a\n"), "{md}");
        assert!(md.contains("_✱ changed"), "{md}");
    }

    #[test]
    fn no_marks_without_common_lines_or_change() {
        let goal = json!({"goals": ["⊢ A"]});
        assert!(!render_goals(goal.clone(), Value::Null, goal).contains('✱'));
        let md = render_goals(
            json!({"goals": ["⊢ A"]}),
            Value::Null,
            json!({"goals": ["⊢ B"]}),
        );
        assert!(!md.contains('✱'), "{md}");
    }
}
