//! Appends Unicode input hints and the optional tactic state to hover responses.
//!
//! When tactic-state hover is enabled, also ask for `$/lean/plainGoal` at the same position.
//! Whichever of the two answers arrives first waits for the other. A hover response
//! waits at most [`HOVER_WAIT`]; the timer thread then sends it with input hints only.

use crate::abbrev::Table;
use crate::document::{Document, column_offset, utf16_len};
use crate::lsp::{self, Position, RequestId, text_document_uri};
use crate::render;
use crate::shared::{Pending, Shared};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

const HOVER_WAIT: Duration = Duration::from_millis(400);

/// One half of a hover pair that is waiting for the other.
pub enum HoverWait {
    Nothing,
    Response(Value),
    Goal(Value),
}

pub struct Hover {
    waiting: HoverWait,
    source: Option<Source>,
    hint: Option<Hint>,
}

struct Source {
    uri: String,
    // A weak reference detects edits and close/reopen even when versions repeat,
    // without retaining the document and making its next edit clone all text.
    document: Weak<Document>,
}

struct Hint {
    markdown: String,
    range: Value,
}

fn unicode_hint(table: &Table, doc: &Document, position: Position) -> Option<Hint> {
    let line = doc.line(position.line);
    let byte = column_offset(line, position.character);
    if utf16_len(&line[..byte]) != position.character {
        return None;
    }
    let symbol = table.symbol_at(line, byte)?;
    let abbreviations = symbol
        .abbreviations
        .iter()
        .map(|abbr| inline_code(&format!("\\{abbr}")))
        .collect::<Vec<_>>()
        .join(", ");
    Some(Hint {
        markdown: format!(
            "**Unicode input** for {}: {abbreviations}",
            inline_code(symbol.text)
        ),
        range: lsp::range(
            Position {
                line: position.line,
                character: utf16_len(&line[..symbol.range.start]),
            },
            Position {
                line: position.line,
                character: utf16_len(&line[..symbol.range.end]),
            },
        ),
    })
}

fn inline_code(text: &str) -> String {
    let longest = text.split(|ch| ch != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest + 1);
    format!("{fence} {text} {fence}")
}

/// Called for a client `textDocument/hover` request.
pub fn start(shared: &Shared, id: &Value, params: &Value, table: Option<&Table>) {
    let Some(uri) = text_document_uri(params) else {
        return;
    };
    let client_id = RequestId::of(id);
    let mut state = shared.state();
    let doc = state.documents.get(uri);
    let source = doc.map(|doc| Source {
        uri: uri.to_owned(),
        document: Arc::downgrade(doc),
    });
    let hint = table.zip(doc).and_then(|(table, doc)| {
        let position = params.get("position").and_then(Position::from_json)?;
        unicode_hint(table, doc, position)
    });
    let goal_hover = shared.config.goal_hover();
    if !goal_hover && hint.is_none() {
        return;
    }
    state.hovers.insert(
        client_id.clone(),
        Hover {
            waiting: if goal_hover {
                HoverWait::Nothing
            } else {
                HoverWait::Goal(Value::Null)
            },
            source,
            hint,
        },
    );
    drop(state);
    if !goal_hover {
        return;
    }
    let params = json!({
        "textDocument": params.get("textDocument"),
        "position": params.get("position"),
    });
    shared.request_server("$/lean/plainGoal", params, Pending::Hover(client_id));
}

/// Whether `id` is a hover response the proxy is waiting for.
pub fn is_tracked(shared: &Shared, id: &RequestId) -> bool {
    shared.state().hovers.contains_key(id)
}

/// Called when the hover response or its goal arrives.
pub fn arrive(shared: &Shared, id: RequestId, half: HoverWait) {
    let mut state = shared.state();
    let Some(mut hover) = state.hovers.remove(&id) else {
        if let HoverWait::Response(response) = half {
            // Expired meanwhile: nothing to merge.
            drop(state);
            shared.send_client(&response);
        }
        return;
    };
    match (
        std::mem::replace(&mut hover.waiting, HoverWait::Nothing),
        half,
    ) {
        (HoverWait::Response(response), HoverWait::Goal(goal))
        | (HoverWait::Goal(goal), HoverWait::Response(response)) => {
            drop(state);
            send(shared, hover, response, &goal);
        }
        (_, half) => {
            let is_response = matches!(half, HoverWait::Response(_));
            hover.waiting = half;
            state.hovers.insert(id.clone(), hover);
            drop(state);
            if is_response {
                shared.hover_deadline(Instant::now() + HOVER_WAIT, id);
            }
        }
    }
}

/// Sends hover responses whose goal did not arrive in time.
pub fn timer(shared: &Shared, deadlines: Receiver<(Instant, RequestId)>) {
    // All deadlines are `now + HOVER_WAIT`, so they arrive in order.
    let mut queue: VecDeque<(Instant, RequestId)> = VecDeque::new();
    loop {
        let wait = queue.front().map_or(Duration::MAX, |(at, _)| {
            at.saturating_duration_since(Instant::now())
        });
        match deadlines.recv_timeout(wait) {
            Ok(deadline) => queue.push_back(deadline),
            Err(RecvTimeoutError::Timeout) => {
                let Some((_, id)) = queue.pop_front() else {
                    continue;
                };
                expire(shared, &id);
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn expire(shared: &Shared, id: &RequestId) {
    let expired = shared.state().hovers.remove(id);
    if let Some(mut hover) = expired
        && let HoverWait::Response(response) =
            std::mem::replace(&mut hover.waiting, HoverWait::Nothing)
    {
        send(shared, hover, response, &Value::Null);
    }
}

fn send(shared: &Shared, hover: Hover, response: Value, goal: &Value) {
    let current = hover.source.as_ref().is_none_or(|source| {
        shared
            .state()
            .documents
            .get(&source.uri)
            .is_some_and(|doc| source.document.ptr_eq(&Arc::downgrade(doc)))
    });
    let response = if current {
        merge(response, goal, hover.hint.as_ref())
    } else {
        response
    };
    shared.send_client(&response);
}

fn merge(mut response: Value, goal: &Value, hint: Option<&Hint>) -> Value {
    // Preserve protocol errors, including cancellation/content-modified errors.
    if response.get("error").is_some() || response.get("result").is_none() {
        return response;
    }
    let goals = render::hover_goals(goal);
    if hint.is_none() && (goals.is_none() || response["result"].is_null()) {
        return response;
    }
    let mut parts = Vec::new();
    if let Some(contents) = response.pointer("/result/contents") {
        let Some(markdown) = contents_markdown(contents) else {
            return response;
        };
        if !markdown.is_empty() {
            parts.push(markdown);
        }
    }
    if let Some(goals) = goals {
        parts.push(goals);
    }
    if let Some(hint) = hint {
        parts.push(hint.markdown.clone());
    }
    if response["result"].is_null() {
        response["result"] = json!({});
    }
    let Some(result) = response["result"].as_object_mut() else {
        return response;
    };
    result.insert(
        "contents".into(),
        json!({"kind": "markdown", "value": parts.join("\n\n---\n\n")}),
    );
    if let Some(hint) = hint {
        result.entry("range").or_insert_with(|| hint.range.clone());
    }
    response
}

/// Preserve all standard LSP hover content forms when adding Markdown.
fn contents_markdown(contents: &Value) -> Option<String> {
    if let Some(text) = contents.as_str() {
        return Some(text.to_owned());
    }
    if let Some(items) = contents.as_array() {
        return items
            .iter()
            .map(contents_markdown)
            .collect::<Option<Vec<_>>>()
            .map(|items| items.join("\n\n"));
    }
    let text = contents.get("value")?.as_str()?;
    match contents.get("kind").and_then(Value::as_str) {
        Some("markdown") => Some(text.to_owned()),
        Some("plaintext") => Some(text.chars().fold(String::new(), |mut out, ch| {
            if ch.is_ascii_punctuation() {
                out.push('\\');
            }
            out.push(ch);
            out
        })),
        _ => {
            let language = contents.get("language")?.as_str()?;
            let longest = text.split(|ch| ch != '`').map(str::len).max().unwrap_or(0);
            let fence = "`".repeat(longest.max(2) + 1);
            Some(format!("{fence}{language}\n{text}\n{fence}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Cursor, Write};
    use std::sync::{Mutex, mpsc};

    #[derive(Clone, Default)]
    struct Output(Arc<Mutex<Vec<u8>>>);

    impl Write for Output {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Output {
        fn messages(&self) -> Vec<Value> {
            let mut reader = Cursor::new(self.0.lock().unwrap().clone());
            std::iter::from_fn(|| {
                crate::framing::read_message(&mut reader)
                    .unwrap()
                    .map(|msg| msg.value())
            })
            .collect()
        }
    }

    fn opened() -> Value {
        json!({"textDocument": {"uri": "file:///Hover.lean", "version": 1, "text": "ℝ"}})
    }

    fn request() -> Value {
        json!({"textDocument": {"uri": "file:///Hover.lean"}, "position": {"line": 0, "character": 0}})
    }

    fn setup(goals: bool) -> (Shared, Output) {
        let output = Output::default();
        let mut config = crate::Config::new("unused.md".into());
        config.hover = goals;
        let shared = Shared::new(
            config,
            output.clone(),
            io::sink(),
            mpsc::channel().0,
            mpsc::channel().0,
        );
        shared.state().documents.open(&opened());
        (shared, output)
    }

    #[test]
    fn hints_cover_multicharacter_symbols_and_use_utf16_ranges() {
        let table = Table::load();
        let doc = Document::new("𝔸 →ₗ x".into(), 1);
        for character in [3, 4] {
            let hint = unicode_hint(&table, &doc, Position { line: 0, character }).unwrap();
            assert_eq!(
                hint.range,
                json!({"start": {"line": 0, "character": 3}, "end": {"line": 0, "character": 5}})
            );
            assert!(hint.markdown.contains("` →ₗ `"));
            assert!(hint.markdown.contains("\\linearmap"));
        }
        for (line, character) in [(0, 1), (0, 5), (0, 6), (0, 99), (10, 0)] {
            assert!(unicode_hint(&table, &doc, Position { line, character }).is_none());
        }
        let pair = unicode_hint(
            &table,
            &Document::new("⟨⟩".into(), 1),
            Position {
                line: 0,
                character: 1,
            },
        )
        .unwrap();
        assert!(pair.markdown.contains("\\<>"));
        assert!(!pair.markdown.contains("CURSOR"));
    }

    #[test]
    fn hints_preserve_hover_content_variants_ranges_and_errors() {
        let table = Table::load();
        let hint = unicode_hint(
            &table,
            &Document::new("ℝ".into(), 1),
            Position {
                line: 0,
                character: 0,
            },
        )
        .unwrap();
        for (contents, expected) in [
            (
                json!({"kind": "markdown", "value": "**native**"}),
                "**native**",
            ),
            (
                json!({"kind": "plaintext", "value": "[native]*`"}),
                "\\[native\\]\\*\\`",
            ),
            (json!("**legacy**"), "**legacy**"),
            (
                json!([{"language": "lean", "value": "x : ℝ"}, "**docs**"]),
                "```lean\nx : ℝ\n```\n\n**docs**",
            ),
        ] {
            let response =
                json!({"id": 1, "result": {"contents": contents, "range": {"native": true}}});
            let merged = merge(response, &json!({"goals": ["⊢ True"]}), Some(&hint));
            let text = merged["result"]["contents"]["value"].as_str().unwrap();
            assert!(text.starts_with(expected), "{text}");
            assert!(text.contains("⊢ True"));
            assert!(text.contains("\\R"));
            assert_eq!(merged["result"]["range"], json!({"native": true}));
        }
        let null = merge(json!({"id": 1, "result": null}), &Value::Null, Some(&hint));
        assert_eq!(null["result"]["range"], hint.range);
        assert_eq!(null["result"]["contents"]["value"], hint.markdown);
        let error = json!({"id": 1, "error": {"code": -32801, "message": "content changed"}});
        assert_eq!(merge(error.clone(), &Value::Null, Some(&hint)), error);
        assert_eq!(inline_code("\\`"), "`` \\` ``");
    }

    #[test]
    fn delayed_responses_do_not_append_hints_after_edits_or_close_and_reopen() {
        let table = Table::load();
        for mode in ["edit", "close", "reopen"] {
            let (shared, output) = setup(false);
            start(&shared, &json!(1), &request(), Some(&table));
            assert_eq!(
                Arc::strong_count(shared.state().documents.get("file:///Hover.lean").unwrap()),
                1
            );
            match mode {
                "edit" => {
                    shared.state().documents.change(&json!({
                        "textDocument": {"uri": "file:///Hover.lean", "version": 1},
                        "contentChanges": [{"text": "α"}]
                    }));
                }
                "close" => shared.state().documents.close(&request()),
                _ => {
                    shared.state().documents.close(&request());
                    shared.state().documents.open(&opened());
                }
            }
            let response = json!({"jsonrpc": "2.0", "id": 1, "result": null});
            arrive(
                &shared,
                RequestId::of(&json!(1)),
                HoverWait::Response(response.clone()),
            );
            assert_eq!(output.messages(), [response], "{mode}");
        }
    }

    #[test]
    fn hints_survive_both_goal_response_orders_and_goal_timeouts() {
        let table = Table::load();
        for order in ["goal-first", "hover-first", "timeout"] {
            let (shared, output) = setup(true);
            let id = RequestId::of(&json!(1));
            start(&shared, &json!(1), &request(), Some(&table));
            let response = json!({"jsonrpc": "2.0", "id": 1, "result": {"contents": {"kind": "markdown", "value": "native"}}});
            if order == "goal-first" {
                arrive(
                    &shared,
                    id.clone(),
                    HoverWait::Goal(json!({"goals": ["⊢ True"]})),
                );
            }
            arrive(&shared, id.clone(), HoverWait::Response(response));
            if order == "hover-first" {
                arrive(
                    &shared,
                    id.clone(),
                    HoverWait::Goal(json!({"goals": ["⊢ True"]})),
                );
            } else if order == "timeout" {
                expire(&shared, &id);
                arrive(
                    &shared,
                    id.clone(),
                    HoverWait::Goal(json!({"goals": ["late"]})),
                );
            }
            let messages = output.messages();
            assert_eq!(messages.len(), 1);
            let text = messages[0]["result"]["contents"]["value"].as_str().unwrap();
            assert!(text.contains("\\R"));
            assert!(text.starts_with("native"));
            assert_eq!(text.contains("⊢ True"), order != "timeout");
            assert!(shared.state().hovers.is_empty());
        }
    }

    #[test]
    fn merge_appends_goals_to_markdown() {
        let response =
            json!({"id": 1, "result": {"contents": {"kind": "markdown", "value": "doc"}}});
        let merged = merge(response, &json!({"goals": ["⊢ True"]}), None);
        let value = merged["result"]["contents"]["value"].as_str().unwrap();
        assert!(value.starts_with("doc\n\n---\n\n"));
        assert!(value.contains("⊢ True"));
    }

    #[test]
    fn merge_keeps_null_and_goalless_hovers() {
        let null = json!({"id": 1, "result": null});
        assert_eq!(
            merge(null.clone(), &json!({"goals": ["⊢ True"]}), None),
            null
        );
        let response =
            json!({"id": 1, "result": {"contents": {"kind": "markdown", "value": "doc"}}});
        assert_eq!(
            merge(response.clone(), &json!({"goals": []}), None),
            response
        );
    }
}
