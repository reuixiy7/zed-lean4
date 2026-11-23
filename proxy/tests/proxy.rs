use lean_infoview_proxy::lsp::OWN_ID_PREFIX;
use lean_infoview_proxy::{Config, framing, run};
use serde_json::{Value, json};
use std::io::{BufReader, PipeWriter, Read, pipe};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(5);

/// Spawns a thread sending each message read from `input` to the returned channel.
fn messages(input: impl Read + Send + 'static) -> Receiver<Value> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut reader = BufReader::new(input);
        while let Ok(Some(msg)) = framing::read_message(&mut reader) {
            if tx.send(msg.value()).is_err() {
                break;
            }
        }
    });
    rx
}

/// A fake Lean server answering just enough requests.
fn fake_server(input: impl Read + Send + 'static, mut output: PipeWriter) {
    let requests = messages(input);
    thread::spawn(move || {
        for msg in requests {
            if msg["method"] == "textDocument/didOpen" {
                let params = &msg["params"]["textDocument"];
                let proved = json!({"range": {"start": {"line": 0, "character": 0}, "end": {"line": 1, "character": 0}},
                    "severity": 3, "isSilent": true, "leanTags": [2], "message": "Goals accomplished!"});
                let diagnostics = json!({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
                    "params": {"uri": params["uri"], "version": params["version"], "diagnostics": [proved]}});
                framing::write_message(&mut output, &diagnostics).unwrap();
            }
            let Some(id) = msg.get("id").cloned() else {
                continue;
            };
            let result = match msg["method"].as_str() {
                // Echoes the capability the proxy adds, so tests can see it arrived.
                Some("initialize") => json!({"capabilities": {"hoverProvider": true,
                    "completionProvider": {"resolveProvider": true, "triggerCharacters": ["."]}},
                    "silent": msg.pointer("/params/capabilities/lean/silentDiagnosticSupport")}),
                Some("$/lean/plainGoal") => json!({"goals": ["x : Nat\n⊢ x = x"], "rendered": ""}),
                Some("$/lean/plainTermGoal") => json!({"goal": "x = x", "range": {}}),
                Some("textDocument/hover") => {
                    json!({"contents": {"kind": "markdown", "value": "HOVER"}})
                }
                Some("textDocument/completion") => json!([{"label": "nativeLeanCompletion"}]),
                Some("completionItem/resolve") => {
                    let mut item = msg["params"].clone();
                    item["detail"] = json!("resolved by Lean");
                    item
                }
                _ => Value::Null,
            };
            let response = json!({"jsonrpc": "2.0", "id": id, "result": result});
            framing::write_message(&mut output, &response).unwrap();
        }
    });
}

/// A proxy in front of the fake server: the client's input and output.
struct Proxy {
    input: PipeWriter,
    output: Receiver<Value>,
}

impl Proxy {
    fn start(config: Config) -> Self {
        let (client_in_r, client_in_w) = pipe().unwrap();
        let (client_out_r, client_out_w) = pipe().unwrap();
        let (server_in_r, server_in_w) = pipe().unwrap();
        let (server_out_r, server_out_w) = pipe().unwrap();
        fake_server(server_in_r, server_out_w);
        thread::spawn(move || run(config, client_in_r, client_out_w, server_in_w, server_out_r));
        Self {
            input: client_in_w,
            output: messages(client_out_r),
        }
    }

    fn send(&mut self, messages: impl IntoIterator<Item = Value>) {
        for msg in messages {
            framing::write_message(&mut self.input, &msg).unwrap();
        }
    }

    /// Messages received until `done` holds for the collected list, or the timeout.
    fn receive_until(&self, done: impl Fn(&[Value]) -> bool) -> Vec<Value> {
        let deadline = Instant::now() + TIMEOUT;
        let mut received = Vec::new();
        while !done(&received) && Instant::now() < deadline {
            if let Ok(msg) = self.output.recv_timeout(Duration::from_millis(50)) {
                received.push(msg);
            }
        }
        received
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lean-infoview-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Polls `path` until its contents satisfy `done`.
fn wait_for_file(path: &PathBuf, done: impl Fn(&str) -> bool) -> String {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        if done(&text) || Instant::now() > deadline {
            return text;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn proxies_tracks_cursor_and_writes_goals() {
    let dir = temp_dir("goals");
    let goals_file = dir.join(".lake/infoview.md");
    let mut proxy = Proxy::start(Config::new(goals_file.clone()));

    let uri = "file:///tmp/A.lean";
    proxy.send([
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
               "params": {"capabilities": {"window": {"showDocument": {"support": true}}}}}),
        json!({"jsonrpc": "2.0", "method": "textDocument/didOpen",
               "params": {"textDocument": {"uri": uri, "languageId": "lean4", "version": 1, "text": ""}}}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "textDocument/documentHighlight",
               "params": {"textDocument": {"uri": uri}, "position": {"line": 3, "character": 2}}}),
        json!({"jsonrpc": "2.0", "id": 3, "method": "textDocument/hover",
               "params": {"textDocument": {"uri": uri}, "position": {"line": 3, "character": 2}}}),
    ]);
    let received = proxy.receive_until(|r| {
        r.iter().any(|m| m["id"] == 3) && r.iter().any(|m| m["method"] == "window/showDocument")
    });

    let show = received
        .iter()
        .find(|m| m["method"] == "window/showDocument")
        .expect("showDocument request");
    assert!(
        show["params"]["uri"]
            .as_str()
            .unwrap()
            .ends_with("/.lake/infoview.md")
    );
    assert_eq!(show["params"]["takeFocus"], false);

    let by_id = |id: i64| {
        received
            .iter()
            .find(|m| m["id"] == id && m.get("method").is_none())
    };
    assert!(by_id(1).is_some(), "initialize response forwarded");
    assert!(by_id(2).is_some(), "documentHighlight response forwarded");
    let hover = by_id(3).expect("hover response");
    let value = hover["result"]["contents"]["value"].as_str().unwrap();
    assert!(value.starts_with("HOVER"), "{value}");
    assert!(value.contains("⊢ x = x"), "{value}");

    // No responses to the proxy's own requests leak to the client.
    assert!(received.iter().all(|m| {
        m.get("method").is_some()
            || !m["id"]
                .as_str()
                .is_some_and(|id| id.starts_with(OWN_ID_PREFIX))
    }));

    let text = wait_for_file(&goals_file, |t| t.contains("## Goal"));
    assert!(text.contains("A.lean:4:3"), "{text}");
    assert!(text.contains("x : Nat\n⊢ x = x"), "{text}");
    // The expected type equals the goal target, so it is not repeated.
    assert!(!text.contains("Expected type"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn expands_abbreviation_with_apply_edit() {
    let dir = temp_dir("abbrev");
    let mut config = Config::new(dir.join("infoview.md"));
    config.auto_open = false;
    let mut proxy = Proxy::start(config);

    let uri = "file:///tmp/B.lean";
    let typed = |version: i64, character: u64, text: &str| {
        json!({"jsonrpc": "2.0", "method": "textDocument/didChange", "params": {
            "textDocument": {"uri": uri, "version": version},
            "contentChanges": [{"range": {"start": {"line": 0, "character": character},
                                          "end": {"line": 0, "character": character}}, "text": text}]
        }})
    };
    proxy.send([
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"capabilities": {
            "workspace": {"applyEdit": true, "workspaceEdit": {"documentChanges": true}}}}}),
        json!({"jsonrpc": "2.0", "method": "textDocument/didOpen",
               "params": {"textDocument": {"uri": uri, "languageId": "lean4", "version": 1, "text": "x : \n"}}}),
        typed(2, 4, "\\"),
        typed(3, 5, "R"),
        typed(4, 6, " "),
    ]);

    let is_edit = |m: &Value| m["method"] == "workspace/applyEdit";
    let received = proxy.receive_until(|r| r.iter().any(is_edit));
    let edit = received
        .iter()
        .find(|m| is_edit(m))
        .expect("applyEdit request");
    let change = &edit["params"]["edit"]["documentChanges"][0];
    assert_eq!(change["textDocument"]["version"], 4);
    assert_eq!(change["edits"][0]["newText"], "ℝ");
    assert_eq!(change["edits"][0]["range"]["start"]["character"], 4);
    assert_eq!(change["edits"][0]["range"]["end"]["character"], 6);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn finishes_partial_abbreviations_with_a_versioned_edit() {
    let dir = temp_dir("abbrev-prefix");
    let mut config = Config::new(dir.join("infoview.md"));
    config.auto_open = false;
    let mut proxy = Proxy::start(config);
    let uri = "file:///tmp/Prefix.lean";
    proxy.send([
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"capabilities": {
            "workspace": {"applyEdit": true, "workspaceEdit": {"documentChanges": true}}}}}),
        json!({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {"textDocument": {
            "uri": uri, "languageId": "lean4", "version": 1, "text": "x : \\alp"}}}),
        json!({"jsonrpc": "2.0", "method": "textDocument/didChange", "params": {
            "textDocument": {"uri": uri, "version": 2}, "contentChanges": [{
                "range": {"start": {"line": 0, "character": 8}, "end": {"line": 0, "character": 8}},
                "text": " "
            }]
        }}),
    ]);
    let received = proxy.receive_until(|r| r.iter().any(|m| m["method"] == "workspace/applyEdit"));
    let edit = received
        .iter()
        .find(|m| m["method"] == "workspace/applyEdit")
        .unwrap();
    let change = &edit["params"]["edit"]["documentChanges"][0];
    assert_eq!(change["textDocument"]["version"], 2);
    assert_eq!(
        change["edits"][0],
        json!({
            "range": {"start": {"line": 0, "character": 4}, "end": {"line": 0, "character": 8}},
            "newText": "α"
        })
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn offers_unicode_completions_and_keeps_lean_completions_working() {
    let dir = temp_dir("abbrev-completion");
    let mut config = Config::new(dir.join("infoview.md"));
    config.auto_open = false;
    let mut proxy = Proxy::start(config);
    let uri = "file:///tmp/Completion.lean";
    let complete = |id: i64, line: u64, character: u64| {
        json!({
            "jsonrpc": "2.0", "id": id, "method": "textDocument/completion", "params": {
                "textDocument": {"uri": uri}, "position": {"line": line, "character": character}
            }
        })
    };
    proxy.send([
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "capabilities": {"workspace": {"applyEdit": true}}}}),
        json!({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {"textDocument": {
            "uri": uri, "languageId": "lean4", "version": 1, "text": "def x := \\alp\n\\R"}}}),
        complete(2, 0, 13),
        complete(3, 1, 2),
        complete(4, 0, 3),
    ]);
    let received = proxy.receive_until(|r| (1..=4).all(|id| r.iter().any(|m| m["id"] == id)));
    let by_id = |id: i64| received.iter().find(|m| m["id"] == id).unwrap();
    assert_eq!(
        by_id(1)["result"]["capabilities"]["completionProvider"],
        json!({
            "resolveProvider": true, "triggerCharacters": [".", "\\"]
        })
    );
    let item = &by_id(2)["result"]["items"][0];
    assert_eq!(item["label"], "\\alpha");
    assert_eq!(
        item["textEdit"],
        json!({
            "range": {"start": {"line": 0, "character": 9}, "end": {"line": 0, "character": 13}},
            "newText": "α"
        })
    );
    assert_eq!(by_id(3)["result"]["items"][0]["textEdit"]["newText"], "ℝ");
    assert_eq!(
        by_id(4)["result"],
        json!([{"label": "nativeLeanCompletion"}])
    );

    proxy.send([
        json!({"jsonrpc": "2.0", "id": 5, "method": "completionItem/resolve", "params": item}),
        json!({"jsonrpc": "2.0", "id": 6, "method": "completionItem/resolve", "params": {
            "label": "nativeLeanCompletion", "data": {"native": true}
        }}),
    ]);
    let resolved = proxy.receive_until(|r| (5..=6).all(|id| r.iter().any(|m| m["id"] == id)));
    assert_eq!(
        resolved.iter().find(|m| m["id"] == 5).unwrap()["result"],
        *item
    );
    assert_eq!(
        resolved.iter().find(|m| m["id"] == 6).unwrap()["result"]["detail"],
        "resolved by Lean"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn disabled_abbreviations_leave_completion_capabilities_and_requests_unchanged() {
    let dir = temp_dir("abbrev-disabled");
    let mut config = Config::new(dir.join("infoview.md"));
    config.auto_open = false;
    config.abbreviations = false;
    let mut proxy = Proxy::start(config);
    let uri = "file:///tmp/Disabled.lean";
    proxy.send([
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "capabilities": {"workspace": {"applyEdit": true}}}}),
        json!({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {"textDocument": {
            "uri": uri, "languageId": "lean4", "version": 1, "text": "\\alp"}}}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "textDocument/completion", "params": {
            "textDocument": {"uri": uri}, "position": {"line": 0, "character": 4}
        }}),
    ]);
    let received = proxy.receive_until(|r| (1..=2).all(|id| r.iter().any(|m| m["id"] == id)));
    assert_eq!(
        received.iter().find(|m| m["id"] == 1).unwrap()["result"]["capabilities"]["completionProvider"],
        json!({
            "resolveProvider": true, "triggerCharacters": ["."]
        })
    );
    assert_eq!(
        received.iter().find(|m| m["id"] == 2).unwrap()["result"],
        json!([{"label": "nativeLeanCompletion"}])
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn opens_infoview_once_per_project() {
    let dir = temp_dir("open-once");
    let goals_file = dir.join(".lake/infoview.md");
    let open = |proxy: &mut Proxy| {
        proxy.send([
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                   "params": {"capabilities": {"window": {"showDocument": {"support": true}}}}}),
            json!({"jsonrpc": "2.0", "method": "textDocument/didOpen",
                   "params": {"textDocument": {"uri": "file:///tmp/C.lean", "languageId": "lean4",
                                               "version": 1, "text": ""}}}),
        ]);
        let is_show = |m: &Value| m["method"] == "window/showDocument";
        // showDocument, if sent, follows the initialize response within moments.
        let received = proxy.receive_until(|r| r.iter().any(|m| m["id"] == 1));
        let extra = proxy.output.recv_timeout(Duration::from_millis(300)).ok();
        received.iter().chain(&extra).any(is_show)
    };

    assert!(open(&mut Proxy::start(Config::new(goals_file.clone()))));
    assert!(dir.join(".lake/infoview.opened").exists());
    // Zed restores the open preview with the layout; do not open a second one.
    assert!(!open(&mut Proxy::start(Config::new(goals_file))));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn marks_proved_theorems() {
    let dir = temp_dir("status");
    let mut config = Config::new(dir.join(".lake/infoview.md"));
    config.auto_open = false;
    let goals_file = config.goals_file.clone();
    let mut proxy = Proxy::start(config);

    let uri = "file:///tmp/C.lean";
    proxy.send([
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"capabilities": {}}}),
        json!({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {"textDocument": {
            "uri": uri, "languageId": "lean4", "version": 1,
            "text": "theorem two : 1 + 1 = 2 := rfl\n"}}}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "textDocument/documentHighlight",
               "params": {"textDocument": {"uri": uri}, "position": {"line": 0, "character": 2}}}),
    ]);
    let is_diagnostics = |m: &Value| m["method"] == "textDocument/publishDiagnostics";
    let received = proxy.receive_until(|r| r.iter().any(is_diagnostics));

    let init = received
        .iter()
        .find(|m| m["id"] == 1)
        .expect("initialize response");
    assert_eq!(init["result"]["silent"], true, "silent support announced");

    let d = &received.iter().find(|m| is_diagnostics(m)).unwrap()["params"]["diagnostics"][0];
    assert_eq!(d["message"], "✅ Goals accomplished");
    assert_eq!(d["severity"], 4);
    assert_eq!(d["range"]["start"], json!({"line": 0, "character": 8}));
    assert_eq!(d["range"]["end"], json!({"line": 0, "character": 11}));

    let text = wait_for_file(&goals_file, |t| t.contains("✅ `1:1`"));
    assert!(text.contains("✅ 1"), "{text}");
    assert!(!text.contains("All messages"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}
