//! LSP commands for opening the infoview and Unicode abbreviation reference.

use crate::abbrev::reference;
use crate::lsp::RequestId;
use crate::shared::{ClientPending, Shared, log};
use crate::{render, uri};
use serde_json::{Value, json};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub const SHOW_ABBREVIATIONS: &str = "Lean 4: Show Unicode Input Abbreviations";
pub const OPEN_INFOVIEW: &str = "Lean 4: Open InfoView";
const COMMANDS: [&str; 2] = [SHOW_ABBREVIATIONS, OPEN_INFOVIEW];

/// Append our commands while preserving Lean's commands and provider options.
pub fn announce(response: &mut Value, infoview: bool) {
    let Some(capabilities) = response
        .pointer_mut("/result/capabilities")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    let provider = capabilities
        .entry("executeCommandProvider")
        .or_insert_with(|| json!({"commands": []}));
    let Some(provider) = provider.as_object_mut() else {
        return;
    };
    let commands = provider.entry("commands").or_insert_with(|| json!([]));
    if let Some(commands) = commands.as_array_mut() {
        for command in COMMANDS {
            if command == OPEN_INFOVIEW && !infoview {
                continue;
            }
            if !commands.iter().any(|existing| existing == command) {
                commands.push(json!(command));
            }
        }
    }
}

pub fn is_ours(command: &str) -> bool {
    COMMANDS.contains(&command)
}

pub fn execute(shared: &Shared, id: &Value, command: &str, show_document: bool) {
    if !show_document {
        fail(
            shared,
            id,
            "The client does not support window/showDocument",
        );
        return;
    }
    let path = match command {
        OPEN_INFOVIEW => {
            if !shared.config.infoview {
                fail(shared, id, "InfoView is disabled; enable infoview.enabled");
                return;
            }
            let path = &shared.config.goals_file;
            if let Err(error) = ensure_infoview(path) {
                fail(
                    shared,
                    id,
                    &format!("Cannot write {}: {error}", path.display()),
                );
                return;
            }
            path.clone()
        }
        SHOW_ABBREVIATIONS => match reference::prepare(shared) {
            Ok(path) => path,
            Err(error) => {
                fail(shared, id, &error.to_string());
                return;
            }
        },
        _ => return,
    };
    // Complete the command only after the editor reports whether opening worked.
    shared.request_client_with(
        "window/showDocument",
        json!({"uri": uri::from_path(&path), "external": false, "takeFocus": true}),
        ClientPending::ShowDocument {
            command_id: RequestId::of(id),
            path,
        },
    );
}

pub fn opened(shared: &Shared, id: RequestId, path: &Path, response: &Value) {
    if response.pointer("/result/success").and_then(Value::as_bool) == Some(true) {
        if path == shared.config.goals_file {
            mark_infoview_opened(shared);
        }
        shared.send_client(&json!({"jsonrpc": "2.0", "id": id.to_value(), "result": null}));
    } else {
        let message = response
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("the editor did not open the document");
        fail(
            shared,
            &id.to_value(),
            &format!("Cannot open {}: {message}", path.display()),
        );
    }
}

fn fail(shared: &Shared, id: &Value, message: &str) {
    shared.send_client(&json!({
        "jsonrpc": "2.0", "id": id,
        "error": {"code": -32803, "message": message},
    }));
}

/// Reopening must preserve the current goals. Only recreate a missing file.
fn ensure_infoview(path: &Path) -> io::Result<()> {
    if path.is_file() {
        return Ok(());
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => file.write_all(render::PLACEHOLDER.as_bytes()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists && path.is_file() => Ok(()),
        Err(error) => Err(error),
    }
}

/// Zed restores previously opened files with the project layout.
pub fn opened_marker(shared: &Shared) -> PathBuf {
    shared.config.goals_file.with_extension("opened")
}

fn mark_infoview_opened(shared: &Shared) {
    let marker = opened_marker(shared);
    if let Err(error) = std::fs::write(&marker, "") {
        log(&format!("cannot write {}: {error}", marker.display()));
    }
}

pub fn auto_open_infoview(shared: &Shared) {
    shared.request_client(
        "window/showDocument",
        json!({"uri": uri::from_path(&shared.config.goals_file), "takeFocus": false}),
    );
    mark_infoview_opened(shared);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn announces_both_commands_once_without_changing_leans_provider() {
        for provider in [
            Value::Null,
            json!({"commands": ["lean.original"], "workDoneProgress": true}),
        ] {
            let mut response = json!({"result": {"capabilities": {"hoverProvider": true}}});
            if !provider.is_null() {
                response["result"]["capabilities"]["executeCommandProvider"] = provider.clone();
            }
            announce(&mut response, true);
            announce(&mut response, true);
            let commands = response["result"]["capabilities"]["executeCommandProvider"]["commands"]
                .as_array()
                .unwrap();
            for command in COMMANDS {
                assert_eq!(commands.iter().filter(|c| *c == command).count(), 1);
            }
            assert_eq!(response["result"]["capabilities"]["hoverProvider"], true);
            if !provider.is_null() {
                assert_eq!(commands[0], "lean.original");
                assert_eq!(
                    response["result"]["capabilities"]["executeCommandProvider"]["workDoneProgress"],
                    true
                );
            }
        }
        let mut failed = json!({"error": {"code": -32603}});
        announce(&mut failed, true);
        assert!(failed.get("result").is_none());
    }
}
