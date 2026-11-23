//! Lean → Zed direction.
//!
//! Every message is forwarded except responses to the proxy's own requests (routed
//! to their consumer) and hover responses held back to merge in the tactic state.

use crate::abbrev::completion;
use crate::framing;
use crate::hover::{self, HoverWait};
use crate::lsp::{Diagnostic, text_document_uri};
use crate::shared::{Event, Pending, Shared};
use crate::status;
use serde_json::Value;
use std::io::{self, BufReader, Read};

pub fn forward(shared: &Shared, input: impl Read) -> io::Result<()> {
    let mut reader = BufReader::new(input);
    while let Some(msg) = framing::read_message(&mut reader)? {
        if let Some(id) = msg.response_id() {
            let (initialize, pending) = {
                let mut state = shared.state();
                let initialize = state
                    .initialize_request
                    .take_if(|request| *request == id)
                    .is_some();
                (initialize, state.pending.remove(&id))
            };
            if initialize {
                let mut response = msg.value();
                completion::announce(&mut response);
                shared.send_client(&response);
                continue;
            }
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
            } else if shared.owns_request(&id) {
                // Cancellation does not suppress Lean's eventual response.
            } else if hover::is_tracked(shared, &id) {
                hover::arrive(shared, id, HoverWait::Response(msg.value()));
            } else {
                shared.write_client(msg.raw());
            }
            continue;
        }

        match msg.method() {
            Some("textDocument/publishDiagnostics") => {
                let value = msg.value();
                on_diagnostics(shared, &value);
                if shared.config.status {
                    shared.send_client(&decorated(shared, value));
                } else {
                    shared.write_client(msg.raw());
                }
            }
            Some("$/lean/fileProgress") => {
                shared.write_client(msg.raw());
                on_file_progress(shared, &msg.value());
            }
            _ => shared.write_client(msg.raw()),
        }
    }
    Ok(())
}

/// The diagnostics as Zed shows them, with the declaration status.
fn decorated(shared: &Shared, mut msg: Value) -> Value {
    let uri = msg
        .pointer("/params/uri")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let version = msg.pointer("/params/version").and_then(Value::as_i64);
    // Names are looked up only in the text the diagnostics are for.
    // The immutable snapshot lets decoration run without the shared state lock.
    let doc = uri
        .and_then(|uri| shared.state().documents.get(&uri).cloned())
        .filter(|doc| version.is_none_or(|v| v == doc.version));
    status::decorate(&mut msg["params"], doc.as_deref());
    msg
}

fn on_diagnostics(shared: &Shared, msg: &Value) {
    let Some(uri) = msg.pointer("/params/uri").and_then(Value::as_str) else {
        return;
    };
    let uri = uri.to_owned();
    let diagnostics = match msg.pointer("/params/diagnostics").and_then(Value::as_array) {
        Some(diagnostics) => diagnostics
            .iter()
            .filter_map(Diagnostic::from_json)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::RequestId;
    use crate::shared::GoalKind;
    use serde_json::json;
    use std::io::{Cursor, Write};
    use std::sync::{Arc, Mutex, mpsc};

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

    #[test]
    fn cancelled_success_and_error_responses_stay_internal() {
        let output = Output::default();
        let shared = Shared::new(
            crate::Config::new("unused.md".into()),
            output.clone(),
            io::sink(),
            mpsc::channel().0,
            mpsc::channel().0,
        );
        let success = shared.request_server(
            "$/lean/plainGoal",
            Value::Null,
            Pending::Goal(GoalKind::Plain),
        );
        let error = shared.request_server(
            "$/lean/plainGoal",
            Value::Null,
            Pending::Goal(GoalKind::Plain),
        );
        shared.cancel_server([success.clone(), error.clone()]);
        assert!(shared.state().pending.is_empty());

        let normal = json!({"jsonrpc": "2.0", "id": 42, "result": "client response"});
        let external = json!({"jsonrpc": "2.0", "id": "external-42", "result": null});
        let mut input = Vec::new();
        for msg in [
            json!({"jsonrpc": "2.0", "id": success.to_value(), "result": {"goals": ["old"]}}),
            json!({"jsonrpc": "2.0", "id": error.to_value(), "error": {"code": -32800, "message": "cancelled"}}),
            normal.clone(),
            external.clone(),
        ] {
            framing::write_message(&mut input, &msg).unwrap();
        }
        forward(&shared, Cursor::new(input)).unwrap();
        let bytes = output.0.lock().unwrap().clone();
        let mut reader = Cursor::new(bytes);
        assert_eq!(
            framing::read_message(&mut reader).unwrap().unwrap().value(),
            normal
        );
        assert_eq!(
            framing::read_message(&mut reader).unwrap().unwrap().value(),
            external
        );
        assert!(framing::read_message(&mut reader).unwrap().is_none());
        assert!(!shared.owns_request(&RequestId::own(0)));
        assert!(!shared.owns_request(&RequestId::own(100)));
    }

    #[test]
    fn declaration_names_are_only_read_from_the_matching_version() {
        let shared = Shared::new(
            crate::Config::new("unused.md".into()),
            io::sink(),
            io::sink(),
            mpsc::channel().0,
            mpsc::channel().0,
        );
        let uri = "file:///A.lean";
        shared.state().documents.open(&json!({"textDocument": {
            "uri": uri, "version": 2, "text": "theorem renamed : True := trivial\n",
        }}));
        let mut msg = json!({"params": {"uri": uri, "version": 1, "diagnostics": [{
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 1, "character": 0}},
            "message": "Goals accomplished!", "isSilent": true, "leanTags": [2],
        }]}});
        let old_range = msg["params"]["diagnostics"][0]["range"].clone();
        assert_eq!(
            decorated(&shared, msg.clone())["params"]["diagnostics"][0]["range"],
            old_range
        );
        msg["params"]["version"] = json!(2);
        assert_eq!(
            decorated(&shared, msg)["params"]["diagnostics"][0]["range"]["start"]["character"],
            8
        );
    }
}
