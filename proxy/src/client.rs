//! Zed → Lean direction.
//!
//! Most messages are forwarded; the few the infoview needs are also inspected.
//! Unicode completions are answered locally, and responses to the proxy's own
//! requests to Zed are dropped.

use crate::abbrev::{self, Table, completion};
use crate::lsp::{self, Cursor, Position, RequestId, text_document_uri};
use crate::shared::{Event, Shared, log};
use crate::{framing, hover, status, uri};
use serde_json::{Value, json};
use std::io::{self, BufReader, Read};
use std::path::PathBuf;

/// State only this direction uses.
struct Input {
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

        let mut value = msg.value();
        let params = &value["params"];
        // Handle before forwarding, so e.g. a hover is tracked before its response.
        match method {
            "initialize" => {
                on_initialize(shared, &mut input, params);
                if input.abbreviations.is_some() {
                    shared.state().initialize_request = msg.request_id().map(RequestId::of);
                }
                if shared.config.status {
                    status::announce_silent(&mut value["params"]);
                    shared.send_server(&value);
                    continue;
                }
            }
            "textDocument/didOpen" => {
                let mut state = shared.state();
                state.documents.open(params);
                if let Some(uri) = text_document_uri(params) {
                    state.document_changed(uri);
                    shared.event(Event::Changed {
                        uri: uri.to_owned(),
                    });
                }
                drop(state);
                if std::mem::take(&mut input.show_infoview) {
                    show_infoview(shared);
                }
            }
            "textDocument/didChange" => {
                let edit = abbreviation_edit(shared, &input, params);
                // Lean must see the edit before the worker refreshes its goals.
                shared.write_server(msg.raw());
                if let Some(uri) = text_document_uri(params) {
                    shared.event(Event::Changed {
                        uri: uri.to_owned(),
                    });
                }
                if let Some(edit) = edit {
                    shared.request_client("workspace/applyEdit", edit);
                }
                continue;
            }
            "textDocument/didClose" => {
                let mut state = shared.state();
                state.documents.close(params);
                if let Some(uri) = text_document_uri(params) {
                    state.document_closed(uri);
                    shared.event(Event::Changed {
                        uri: uri.to_owned(),
                    });
                }
            }
            "textDocument/documentHighlight" => track_cursor(shared, params, "/position"),
            "textDocument/codeAction" => track_cursor(shared, params, "/range/start"),
            "textDocument/hover" => {
                if let Some(id) = msg.request_id() {
                    hover::start(shared, id, params);
                }
            }
            "textDocument/completion" => {
                if let Some(id) = msg.request_id()
                    && let Some(result) = abbreviation_completion(shared, &input, params)
                {
                    shared.send_client(&json!({"jsonrpc": "2.0", "id": id, "result": result}));
                    continue;
                }
            }
            "completionItem/resolve" => {
                if let Some(id) = msg.request_id()
                    && completion::is_ours(params)
                {
                    shared.send_client(&json!({"jsonrpc": "2.0", "id": id, "result": params}));
                    continue;
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
        "textDocument/completion" | "completionItem/resolve" => shared.config.abbreviations,
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
    if shared.config.auto_open && !opened_marker(shared).exists() {
        input.show_infoview = capability("/window/showDocument/support");
        if !input.show_infoview {
            log("client does not support window/showDocument; open the infoview file manually");
        }
    }
}

/// Marks that the infoview was opened in this project once. Zed restores open
/// files and previews with the layout, so opening it again would add a duplicate.
fn opened_marker(shared: &Shared) -> PathBuf {
    shared.config.goals_file.with_extension("opened")
}

fn show_infoview(shared: &Shared) {
    let uri = uri::from_path(&shared.config.goals_file);
    shared.request_client(
        "window/showDocument",
        json!({"uri": uri, "takeFocus": false}),
    );
    if let Err(e) = std::fs::write(opened_marker(shared), "") {
        log(&format!(
            "cannot write {}: {e}",
            opened_marker(shared).display()
        ));
    }
}

fn track_cursor(shared: &Shared, params: &Value, position: &str) {
    if let (Some(uri), Some(pos)) = (
        text_document_uri(params),
        params.pointer(position).and_then(Position::from_json),
    ) {
        let changed = shared.state().set_cursor(Some(Cursor {
            uri: uri.to_owned(),
            pos,
        }));
        if changed {
            shared.event(Event::Cursor);
        }
    }
}

fn abbreviation_completion(shared: &Shared, input: &Input, params: &Value) -> Option<Value> {
    let table = input.abbreviations.as_ref()?;
    let uri = text_document_uri(params)?;
    let position = Position::from_json(params.get("position")?)?;
    let doc = shared.state().documents.get(uri)?.clone();
    completion::at(table, &doc, position)
}

/// The `workspace/applyEdit` params replacing a just-finished `\abbreviation`.
fn abbreviation_edit(shared: &Shared, input: &Input, params: &Value) -> Option<Value> {
    let mut state = shared.state();
    let uri = text_document_uri(params)?;
    let insertion = state.documents.change(params);
    state.document_changed(uri);
    let insertion = insertion?;
    let table = input.abbreviations.as_ref()?;
    let doc = state.documents.get(uri)?;
    let replacement = abbrev::check(table, doc, &insertion)?;
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
