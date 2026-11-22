//! Appends the tactic state to hover responses.
//!
//! For each client hover the proxy also asks for `$/lean/plainGoal` at the same position.
//! Whichever of the two answers arrives first waits for the other. A hover response
//! waits at most [`HOVER_WAIT`]; the timer thread then sends it unchanged.

use crate::lsp::{RequestId, text_document_uri};
use crate::render;
use crate::shared::{Pending, Shared};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

const HOVER_WAIT: Duration = Duration::from_millis(400);

/// One half of a hover pair that is waiting for the other.
pub enum HoverWait {
    Nothing,
    Response(Value),
    Goal(Value),
}

/// Called for a client `textDocument/hover` request.
pub fn start(shared: &Shared, id: &Value, params: &Value) {
    if text_document_uri(params).is_none() {
        return;
    }
    let client_id = RequestId::of(id);
    shared
        .state()
        .hovers
        .insert(client_id.clone(), HoverWait::Nothing);
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
    let Some(waiting) = state.hovers.remove(&id) else {
        if let HoverWait::Response(response) = half {
            // Expired meanwhile: nothing to merge.
            drop(state);
            shared.send_client(&response);
        }
        return;
    };
    match (waiting, half) {
        (HoverWait::Response(response), HoverWait::Goal(goal))
        | (HoverWait::Goal(goal), HoverWait::Response(response)) => {
            drop(state);
            shared.send_client(&merge(response, &goal));
        }
        (_, half) => {
            let is_response = matches!(half, HoverWait::Response(_));
            state.hovers.insert(id.clone(), half);
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
                let expired = shared.state().hovers.remove(&id);
                if let Some(HoverWait::Response(response)) = expired {
                    shared.send_client(&response);
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn merge(mut response: Value, goal: &Value) -> Value {
    let Some(goals) = render::hover_goals(goal) else {
        return response;
    };
    if let Some(value) = response.pointer_mut("/result/contents/value")
        && let Some(text) = value.as_str()
    {
        *value = Value::String(format!("{text}\n\n---\n\n{goals}"));
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_appends_goals_to_markdown() {
        let response =
            json!({"id": 1, "result": {"contents": {"kind": "markdown", "value": "doc"}}});
        let merged = merge(response, &json!({"goals": ["⊢ True"]}));
        let value = merged["result"]["contents"]["value"].as_str().unwrap();
        assert!(value.starts_with("doc\n\n---\n\n"));
        assert!(value.contains("⊢ True"));
    }

    #[test]
    fn merge_keeps_null_and_goalless_hovers() {
        let null = json!({"id": 1, "result": null});
        assert_eq!(merge(null.clone(), &json!({"goals": ["⊢ True"]})), null);
        let response =
            json!({"id": 1, "result": {"contents": {"kind": "markdown", "value": "doc"}}});
        assert_eq!(merge(response.clone(), &json!({"goals": []})), response);
    }
}
