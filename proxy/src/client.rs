//! Zed → Lean direction.
//!
//! Every message is forwarded; the few the infoview needs are also inspected.
//! Responses to the proxy's own requests to Zed are dropped.

use crate::abbrev::{self, Table};
use crate::document::Documents;
use crate::lsp::{self, Cursor, Position, text_document_uri};
use crate::shared::{Event, Shared, log};
use crate::{framing, hover, uri};
use serde_json::{Value, json};
use std::io::{self, BufReader, Read};

/// State only this direction uses.
struct Input {
    documents: Documents,
    /// Loaded once the client is known to accept `workspace/applyEdit`.
    abbreviations: Option<Table>,
    /// Client accepts versioned `documentChanges` in workspace edits.
    document_changes: bool,
    /// Ask Zed to open the infoview file on the next `didOpen`.
    show_infoview: bool,
}

pub fn forward(shared: &Shared, input: impl Read) -> io::Result<()> {
    let mut reader = BufReader::new(input);
    let mut input = Input {
        documents: Documents::default(),
        abbreviations: None,
        document_changes: false,
        show_infoview: false,
    };
    while let Some(msg) = framing::read_message(&mut reader)? {
        if let Some(id) = msg.response_id()
            && shared.state().client_pending.remove(&id)
        {
            if let Some(err) = msg.value().get("error") {
                log(&format!("client rejected request {id:?}: {err}"));
            }
            continue;
        }
        let Some(method) = msg.method().filter(|m| observed(shared, m)) else {
            shared.write_server(msg.raw());
            continue;
        };

        let value = msg.value();
        let params = &value["params"];
        // Handle before forwarding, so e.g. a hover is tracked before its response.
        match method {
            "initialize" => on_initialize(shared, &mut input, params),
            "textDocument/didOpen" => {
                input.documents.open(params);
                if std::mem::take(&mut input.show_infoview) {
                    let uri = uri::from_path(&shared.config.goals_file);
                    shared.request_client(
                        "window/showDocument",
                        json!({"uri": uri, "takeFocus": false}),
                    );
                }
            }
            "textDocument/didChange" => {
                if let Some(uri) = text_document_uri(params) {
                    shared.event(Event::Changed {
                        uri: uri.to_owned(),
                    });
                }
                if let Some(edit) = abbreviation_edit(&mut input, params) {
                    shared.request_client("workspace/applyEdit", edit);
                }
            }
            "textDocument/didClose" => input.documents.close(params),
            "textDocument/documentHighlight" => track_cursor(shared, params, "/position"),
            "textDocument/codeAction" => track_cursor(shared, params, "/range/start"),
            "textDocument/hover" => {
                if let Some(id) = msg.request_id() {
                    hover::start(shared, id, params);
                }
            }
            _ => {}
        }
        shared.write_server(msg.raw());
    }
    Ok(())
}

/// Methods inspected by [`forward`]; everything else passes through unparsed.
fn observed(shared: &Shared, method: &str) -> bool {
    match method {
        "initialize"
        | "textDocument/didOpen"
        | "textDocument/didChange"
        | "textDocument/didClose"
        | "textDocument/documentHighlight"
        | "textDocument/codeAction" => true,
        "textDocument/hover" => shared.config.hover,
        _ => false,
    }
}

fn on_initialize(shared: &Shared, input: &mut Input, params: &Value) {
    let capability = |path: &str| {
        params
            .pointer(&format!("/capabilities{path}"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    input.document_changes = capability("/workspace/workspaceEdit/documentChanges");
    if shared.config.abbreviations {
        if capability("/workspace/applyEdit") {
            input.abbreviations = Some(Table::load());
        } else {
            log("client does not support workspace/applyEdit; abbreviations are disabled");
        }
    }
    if shared.config.auto_open {
        input.show_infoview = capability("/window/showDocument/support");
        if !input.show_infoview {
            log("client does not support window/showDocument; open the infoview file manually");
        }
    }
}

fn track_cursor(shared: &Shared, params: &Value, position: &str) {
    if let (Some(uri), Some(pos)) = (
        text_document_uri(params),
        params.pointer(position).and_then(Position::from_json),
    ) {
        shared.event(Event::Cursor(Cursor {
            uri: uri.to_owned(),
            pos,
        }));
    }
}

/// The `workspace/applyEdit` params replacing a just-finished `\abbreviation`.
fn abbreviation_edit(input: &mut Input, params: &Value) -> Option<Value> {
    let insertion = input.documents.change(params)?;
    let table = input.abbreviations.as_ref()?;
    let uri = text_document_uri(params)?;
    let doc = input.documents.get(uri)?;
    let replacement = abbrev::check(table, &doc.text, &insertion)?;
    let edit = json!({
        "range": lsp::range(replacement.start, replacement.end),
        "newText": replacement.text,
    });
    let edit = if input.document_changes {
        // Versioned, so Zed maps the range even if more was typed meanwhile.
        json!({"documentChanges": [{
            "textDocument": {"uri": uri, "version": doc.version},
            "edits": [edit],
        }]})
    } else {
        json!({"changes": {uri: [edit]}})
    };
    Some(json!({"label": "Lean abbreviation", "edit": edit}))
}
