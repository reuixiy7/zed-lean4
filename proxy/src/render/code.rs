//! Lean code blocks, and the tactic state appended to hovers.

use serde_json::Value;

/// Appends a fenced `lean` code block, with a fence longer than any backtick run inside.
pub fn push_code(out: &mut String, code: &str) {
    let longest = code.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    out.push_str(&format!("{fence}lean\n{}\n{fence}\n\n", code.trim_end()));
}

/// The goals of a `$/lean/plainGoal` result; `None` when there is no tactic state.
pub fn goal_list(plain_goal: &Value) -> Option<Vec<&str>> {
    let goals = plain_goal.get("goals")?.as_array()?;
    Some(goals.iter().filter_map(Value::as_str).collect())
}

/// Markdown for the goal part of a hover, or `None` if there is nothing to show.
pub fn hover_goals(plain_goal: &Value) -> Option<String> {
    let goals = goal_list(plain_goal).filter(|goals| !goals.is_empty())?;
    let mut out = String::from("**Tactic state**\n\n");
    for g in goals {
        push_code(&mut out, g);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fence_longer_than_backticks_in_code() {
        let mut out = String::new();
        push_code(&mut out, "a ```` b");
        assert!(out.starts_with("`````lean\n"));
    }

    #[test]
    fn hover_only_with_goals() {
        assert_eq!(hover_goals(&json!({"goals": []})), None);
        assert!(
            hover_goals(&json!({"goals": ["⊢ True"]}))
                .unwrap()
                .contains("⊢ True")
        );
    }
}
