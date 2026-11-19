//! Lean → Zed direction.
//!
//! Every message is forwarded except responses to the proxy's own requests (routed
//! to their consumer) and hover responses held back to merge in the tactic state.

use crate::framing;
use crate::hover::{self, HoverWait};
use crate::lsp::{Diagnostic, text_document_uri};
use crate::shared::{Event, Pending, Shared};
use serde_json::Value;
use std::io::{self, BufReader, Read};

pub fn forward(shared: &Shared, input: impl Read) -> io::Result<()> {
    let mut reader = BufReader::new(input);
    while let Some(msg) = framing::read_message(&mut reader)? {
        if let Some(id) = msg.response_id() {
            let pending = shared.state().pending.remove(&id);
            if let Some(pending) = pending {
                let result = match msg.value() {
                    Value::Object(mut response) => response.remove("result"),
                    _ => None,
                };
                let result = result.unwrap_or_default();
                match pending {
                    Pending::Goal(kind) => shared.event(Event::Goal { id, kind, result }),
                    Pending::Hover(client_id) => {
                        hover::arrive(shared, client_id, HoverWait::Goal(result))
                    }
                }
            } else if hover::is_tracked(shared, &id) {
                hover::arrive(shared, id, HoverWait::Response(msg.value()));
            } else {
                shared.write_client(msg.raw());
            }
            continue;
        }

        shared.write_client(msg.raw());
        match msg.method() {
            Some("textDocument/publishDiagnostics") => on_diagnostics(shared, msg.value()),
            Some("$/lean/fileProgress") => on_file_progress(shared, &msg.value()),
            _ => {}
        }
    }
    Ok(())
}

fn on_diagnostics(shared: &Shared, mut msg: Value) {
    let Some(uri) = msg.pointer("/params/uri").and_then(Value::as_str) else {
        return;
    };
    let uri = uri.to_owned();
    let diagnostics = match msg.pointer_mut("/params/diagnostics").map(Value::take) {
        Some(Value::Array(diagnostics)) => diagnostics
            .into_iter()
            .filter_map(|mut d| Diagnostic::from_json(&mut d))
            .collect(),
        _ => return,
    };
    shared.event(Event::Diagnostics { uri, diagnostics });
}

fn on_file_progress(shared: &Shared, msg: &Value) {
    let params = &msg["params"];
    let Some(uri) = text_document_uri(params) else {
        return;
    };
    let line = params["processing"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r.pointer("/range/start/line").and_then(Value::as_u64))
        .min();
    shared.event(Event::Progress {
        uri: uri.to_owned(),
        line,
    });
}
