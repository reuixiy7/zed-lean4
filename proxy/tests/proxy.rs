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
            let Some(id) = msg.get("id").cloned() else {
                continue;
            };
            let result = match msg["method"].as_str() {
                Some("initialize") => json!({"capabilities": {"hoverProvider": true}}),
                Some("$/lean/plainGoal") => json!({"goals": ["x : Nat\n⊢ x = x"], "rendered": ""}),
                Some("$/lean/plainTermGoal") => json!({"goal": "x = x", "range": {}}),
                Some("textDocument/hover") => {
                    json!({"contents": {"kind": "markdown", "value": "HOVER"}})
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
    let received = proxy.receive_until(|r| r.len() >= 4);

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
