//! Declaration status in the editor, in place of VS Code's gutter icons.
//!
//! Lean marks a proved theorem with a silent `Goals accomplished!` diagnostic, served
//! only to clients that announce `lean.silentDiagnosticSupport`. The proxy announces it
//! and turns those into hints on the declaration name; with Zed's inline diagnostics
//! every declaration then ends in ✅, ⚠️ (`sorry`) or ❌ (an error).

use crate::document::{column_offset, line_range, utf16_len};
use crate::lsp::{LEAN_TAG_GOALS_ACCOMPLISHED, Position, lean_tags};
use serde_json::{Map, Value, json};

const PROVED: &str = "✅ Goals accomplished";
const SORRY: &str = "⚠️ ";
const ERROR: &str = "❌ ";
const TRY: &str = "💡";

/// Keywords whose next word is the declaration name.
const NAMED: [&str; 6] = ["theorem", "lemma", "def", "instance", "abbrev", "opaque"];

/// Adds `lean.silentDiagnosticSupport` to the client's `initialize` params.
pub fn announce_silent(params: &mut Value) {
    let Some(params) = params.as_object_mut() else {
        return;
    };
    let lean = object(object(params, "capabilities"), "lean");
    lean.insert("silentDiagnosticSupport".into(), Value::Bool(true));
}

fn object<'a>(map: &'a mut Map<String, Value>, key: &str) -> &'a mut Map<String, Value> {
    let value = map.entry(key).or_insert_with(|| json!({}));
    if !value.is_object() {
        *value = json!({});
    }
    value.as_object_mut().expect("just made an object")
}

/// Rewrites `publishDiagnostics` params for display; `text` is the document, if known.
pub fn decorate(params: &mut Value, text: Option<&str>) {
    let Some(Value::Array(diagnostics)) = params.get_mut("diagnostics") else {
        return;
    };
    diagnostics.retain_mut(|d| decorate_one(d, text));
}

/// Returns whether to keep the diagnostic.
fn decorate_one(d: &mut Value, text: Option<&str>) -> bool {
    let Some(d) = d.as_object_mut() else {
        return true;
    };
    if d.remove("isSilent") == Some(Value::Bool(true)) {
        if !lean_tags(d).any(|t| t == LEAN_TAG_GOALS_ACCOMPLISHED) {
            // Other silent messages are not shown in the editor, as in VS Code.
            return false;
        }
        d.insert("message".into(), PROVED.into());
        d.insert("severity".into(), 4.into());
        let start = d.get("range").and_then(|r| r.get("start"));
        if let Some(range) = start
            .and_then(Position::from_json)
            .zip(text)
            .and_then(|(start, text)| name_range(text, start))
        {
            d.insert("range".into(), range);
        }
        return true;
    }
    if let Some(suggestion) = message(d).and_then(try_this) {
        d.insert("message".into(), suggestion.into());
        return true;
    }
    let prefix = match d.get("severity").and_then(Value::as_u64) {
        Some(1) => ERROR,
        Some(2) if message(d).is_some_and(is_sorry) => SORRY,
        _ => return true,
    };
    if let Some(Value::String(message)) = d.get_mut("message")
        && !message.starts_with(prefix)
    {
        message.insert_str(0, prefix);
    }
    true
}

fn message(d: &Map<String, Value>) -> Option<&str> {
    d.get("message")?.as_str()
}

/// `Try this:` suggestions on one line, since inline diagnostics show only the first:
/// `Try this:\n  [apply] simp only [h]` becomes `💡 Try this: simp only [h]`, and the
/// items of `Try these:` are joined with ` · `. Long suggestions wrapped by Lean are
/// joined with spaces.
fn try_this(message: &str) -> Option<String> {
    let mut lines = message.lines();
    let head = lines.next()?.trim_end();
    if head != "Try this:" && head != "Try these:" {
        return None;
    }
    let mut items: Vec<String> = Vec::new();
    for line in lines.map(str::trim).filter(|l| !l.is_empty()) {
        match line.strip_prefix("• ") {
            Some(item) => items.push(item.to_owned()),
            None => match items.last_mut() {
                Some(last) => {
                    last.push(' ');
                    last.push_str(line);
                }
                None => items.push(line.to_owned()),
            },
        }
    }
    let items: Vec<&str> = items
        .iter()
        .map(|i| i.strip_prefix("[apply] ").unwrap_or(i))
        .collect();
    if items.is_empty() {
        return None;
    }
    Some(format!("{TRY} {head} {}", items.join(" · ")))
}

fn is_sorry(message: &str) -> bool {
    message.starts_with("declaration uses") && message.contains("sorry")
}

/// The range of the declaration name on the line of `start`, or of the keyword for
/// `example` and anonymous instances.
fn name_range(text: &str, start: Position) -> Option<Value> {
    let line = &text[line_range(text, start.line)];
    let from = column_offset(line, start.character);
    let (keyword, after) =
        words(line, from).find(|&(w, _)| w == "example" || NAMED.contains(&w))?;
    let keyword_start = after - keyword.len();
    let (name_start, name_end) = match words(line, after).next() {
        Some((name, end)) if keyword != "example" && is_name(name) => (end - name.len(), end),
        _ => (keyword_start, after),
    };
    let column = |byte: usize| utf16_len(&line[..byte]);
    Some(crate::lsp::range(
        Position {
            line: start.line,
            character: column(name_start),
        },
        Position {
            line: start.line,
            character: column(name_end),
        },
    ))
}

fn is_name(word: &str) -> bool {
    !word.starts_with(['(', '[', '{', '⦃', ':', '|'])
}

/// Words of `line` from byte `from`, with the byte offset of each word's end.
/// A word ends at whitespace or at a binder, so `foo(x)` and `foo:` give `foo`;
/// `«a b»` is one word.
fn words(line: &str, from: usize) -> impl Iterator<Item = (&str, usize)> {
    let mut rest = from;
    std::iter::from_fn(move || {
        let tail = &line[rest..];
        let start = rest + (tail.len() - tail.trim_start().len());
        let word = &line[start..];
        if word.starts_with('«')
            && let Some(close) = word.find('»')
        {
            rest = start + close + '»'.len_utf8();
            return Some((&line[start..rest], rest));
        }
        let len = word
            .char_indices()
            .find(|&(i, c)| c.is_whitespace() || (i > 0 && "([{⦃:".contains(c)))
            .map_or(word.len(), |(i, _)| i);
        if len == 0 {
            return None;
        }
        rest = start + len;
        Some((&line[start..rest], rest))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(line: u64, start: u64, end: u64) -> Value {
        json!({"start": {"line": line, "character": start}, "end": {"line": line, "character": end}})
    }

    fn proved(line: u64) -> Value {
        json!({"range": {"start": {"line": line, "character": 0}, "end": {"line": line + 1, "character": 0}},
               "severity": 3, "isSilent": true, "leanTags": [2], "message": "Goals accomplished!"})
    }

    fn decorated(diagnostics: Value, text: &str) -> Vec<Value> {
        let mut params = json!({"uri": "file:///A.lean", "diagnostics": diagnostics});
        decorate(&mut params, Some(text));
        params["diagnostics"].as_array().unwrap().clone()
    }

    #[test]
    fn announces_silent_support() {
        let mut params = json!({"capabilities": {"lean": {"rpcWireFormat": "v1"}}});
        announce_silent(&mut params);
        assert_eq!(
            params["capabilities"]["lean"]["silentDiagnosticSupport"],
            true
        );
        assert_eq!(params["capabilities"]["lean"]["rpcWireFormat"], "v1");
        let mut params = json!({});
        announce_silent(&mut params);
        assert_eq!(
            params["capabilities"]["lean"]["silentDiagnosticSupport"],
            true
        );
    }

    #[test]
    fn goals_accomplished_moves_to_the_name() {
        let text = "@[simp] private theorem add_comm' (a b : ℕ) :\n    a + b = b + a := by omega\n";
        let d = decorated(json!([proved(0)]), text);
        assert_eq!(d[0]["message"], PROVED);
        assert_eq!(d[0]["severity"], 4);
        assert!(d[0].get("isSilent").is_none());
        assert_eq!(d[0]["range"], range(0, 24, 33));
    }

    #[test]
    fn examples_and_anonymous_instances_mark_the_keyword() {
        let text = "example : 1 = 1 := rfl\ninstance : Inhabited ℕ := ⟨0⟩\ntheorem «weird name» : True := trivial\n";
        let d = decorated(json!([proved(0), proved(1), proved(2)]), text);
        assert_eq!(d[0]["range"], range(0, 0, 7));
        assert_eq!(d[1]["range"], range(1, 0, 8));
        assert_eq!(d[2]["range"], range(2, 8, 20));
    }

    #[test]
    fn keeps_range_without_a_keyword() {
        let d = decorated(json!([proved(0)]), "mutual\n");
        assert_eq!(d[0]["range"]["end"]["line"], 1);
    }

    #[test]
    fn try_this_on_one_line() {
        assert_eq!(
            try_this("Try this:\n  [apply] simp only [h, Nat.add_zero]").as_deref(),
            Some("💡 Try this: simp only [h, Nat.add_zero]")
        );
        assert_eq!(
            try_this("Try this:\n  [apply] simp only [a, b,\n    c]").as_deref(),
            Some("💡 Try this: simp only [a, b, c]")
        );
        assert_eq!(
            try_this("Try these:\n  • [apply] exact h\n  • [apply] omega").as_deref(),
            Some("💡 Try these: exact h · omega")
        );
        assert_eq!(try_this("Try this later"), None);
        assert_eq!(try_this("Try this:"), None);
    }

    #[test]
    fn marks_sorry_and_errors_and_drops_other_silent() {
        let d = decorated(
            json!([
                {"range": range(0, 8, 9), "severity": 2, "message": "declaration uses `sorry`"},
                {"range": range(1, 0, 1), "severity": 1, "leanTags": [1], "message": "unsolved goals\n⊢ False"},
                {"range": range(2, 0, 1), "severity": 2, "message": "unused variable `x`"},
                {"range": range(3, 0, 1), "severity": 3, "isSilent": true, "message": "quiet"},
            ]),
            "",
        );
        assert_eq!(d.len(), 3);
        assert_eq!(d[0]["message"], "⚠️ declaration uses `sorry`");
        assert_eq!(d[1]["message"], "❌ unsolved goals\n⊢ False");
        assert_eq!(d[2]["message"], "unused variable `x`");
    }
}
