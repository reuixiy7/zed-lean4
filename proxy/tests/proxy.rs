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
                    "completionProvider": {"resolveProvider": true, "triggerCharacters": ["."]},
                    "executeCommandProvider": {"commands": ["lean.original"], "workDoneProgress": true}},
                    "silent": msg.pointer("/params/capabilities/lean/silentDiagnosticSupport")}),
                Some("$/lean/plainGoal") => json!({"goals": ["x : Nat\n⊢ x = x"], "rendered": ""}),
                Some("$/lean/plainTermGoal") => json!({"goal": "x = x", "range": {}}),
                Some("textDocument/hover") => {
                    if msg.pointer("/params/position/line") == Some(&json!(1)) {
                        Value::Null
                    } else {
                        json!({"contents": {"kind": "markdown", "value": "HOVER"}})
                    }
                }
                Some("textDocument/completion") => json!([{"label": "nativeLeanCompletion"}]),
                Some("completionItem/resolve") => {
                    let mut item = msg["params"].clone();
                    item["detail"] = json!("resolved by Lean");
                    item
                }
                Some("workspace/executeCommand") => {
                    json!({"from": "Lean", "params": msg["params"]})
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
    let goals_file = dir.join(".lake/InfoView.md");
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
            .ends_with("/.lake/InfoView.md")
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
fn unicode_hover_hints_preserve_native_content_and_respect_feature_switches() {
    for (index, (abbreviations, goals)) in
        [(true, true), (true, false), (false, true), (false, false)]
            .into_iter()
            .enumerate()
    {
        let dir = temp_dir(&format!("unicode-hover-{index}"));
        let mut config = Config::new(dir.join("InfoView.md"));
        config.auto_open = false;
        config.abbreviations = abbreviations;
        config.hover = goals;
        let mut proxy = Proxy::start(config);
        let uri = "file:///tmp/Hover.lean";
        let hover = |id, line, character| {
            json!({"jsonrpc": "2.0", "id": id, "method": "textDocument/hover", "params": {
                "textDocument": {"uri": uri}, "position": {"line": line, "character": character}
            }})
        };
        proxy.send([
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "capabilities": {"workspace": {"applyEdit": true}}
            }}),
            json!({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {"textDocument": {
                "uri": uri, "languageId": "lean4", "version": 1, "text": "variable (x : ℝ)\n→ₗ\nNat"
            }}}),
            hover(2, 0, 14),
            hover(3, 1, 0),
            hover(4, 1, 1),
            hover(5, 2, 0),
        ]);
        let received = proxy.receive_until(|r| (2..=5).all(|id| r.iter().any(|m| m["id"] == id)));
        let result = |id: i64| &received.iter().find(|m| m["id"] == id).unwrap()["result"];
        let real = result(2)["contents"]["value"].as_str().unwrap();
        assert!(real.starts_with("HOVER"));
        assert_eq!(real.contains("Unicode input"), abbreviations);
        assert_eq!(real.contains("Tactic state"), goals);
        if abbreviations {
            assert!(real.contains("\\R"));
            assert!(real.contains("\\Real"));
        }
        for id in [3, 4] {
            if abbreviations {
                let symbol = result(id);
                let text = symbol["contents"]["value"].as_str().unwrap();
                assert!(text.contains("→ₗ"));
                assert!(text.contains("\\linearmap"));
                assert_eq!(text.contains("Tactic state"), goals);
                assert_eq!(
                    symbol["range"],
                    json!({"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 2}})
                );
            } else {
                assert!(result(id).is_null(), "native empty hover remains empty");
            }
        }
        let ordinary = result(5)["contents"]["value"].as_str().unwrap();
        assert!(ordinary.starts_with("HOVER"));
        assert!(!ordinary.contains("Unicode input"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

const SHOW_ABBREVIATIONS: &str = "Lean 4: Show Unicode Input Abbreviations";
const OPEN_INFOVIEW: &str = "Lean 4: Open InfoView";

fn execute_command(id: Value, command: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "workspace/executeCommand", "params": {"command": command}})
}

#[test]
fn opens_infoview_on_command_without_auto_open_and_preserves_existing_goals() {
    for (index, filename) in [".lake/InfoView.md", "custom/MyGoals.md"]
        .into_iter()
        .enumerate()
    {
        let dir = temp_dir(&format!("infoview-command-{index}"));
        let goals_file = dir.join(filename);
        let mut config = Config::new(goals_file.clone());
        config.auto_open = false;
        config.abbreviations = false;
        let mut proxy = Proxy::start(config);
        proxy.send([
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "capabilities": {"window": {"showDocument": {"support": true}}}
            }}),
        ]);
        let initialized = proxy.receive_until(|r| r.iter().any(|m| m["id"] == 1));
        let capabilities =
            &initialized.iter().find(|m| m["id"] == 1).unwrap()["result"]["capabilities"];
        assert_eq!(
            capabilities["executeCommandProvider"]["commands"],
            json!(["lean.original", SHOW_ABBREVIATIONS, OPEN_INFOVIEW])
        );
        assert!(
            !initialized
                .iter()
                .any(|m| m["method"] == "window/showDocument")
        );
        let goals = "## Goal\n\n```lean\nx : Nat\n⊢ x = x\n```\n";
        std::fs::write(&goals_file, goals).unwrap();

        for id in ["open-infoview", "reopen-infoview"] {
            proxy.send([execute_command(json!(id), OPEN_INFOVIEW)]);
            let requested =
                proxy.receive_until(|r| r.iter().any(|m| m["method"] == "window/showDocument"));
            let open = requested
                .iter()
                .find(|m| m["method"] == "window/showDocument")
                .unwrap();
            assert_eq!(open["params"]["takeFocus"], true);
            assert_eq!(open["params"]["external"], false);
            assert!(open["params"]["uri"].as_str().unwrap().ends_with(filename));
            assert_eq!(std::fs::read_to_string(&goals_file).unwrap(), goals);
            assert!(!requested.iter().any(|m| m["id"] == id));
            proxy.send([json!({"jsonrpc": "2.0", "id": open["id"], "result": {"success": true}})]);
            let completed = proxy.receive_until(|r| r.iter().any(|m| m["id"] == id));
            assert_eq!(
                completed.iter().find(|m| m["id"] == id).unwrap(),
                &json!({"jsonrpc": "2.0", "id": id, "result": null})
            );
            assert!(goals_file.with_extension("opened").exists());
        }
        assert!(!goals_file.with_file_name("AbbreviationView.md").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn infoview_command_recreates_missing_files_and_reports_write_failures() {
    let dir = temp_dir("infoview-command-missing");
    let goals_file = dir.join(".lake/InfoView.md");
    let mut config = Config::new(goals_file.clone());
    config.auto_open = false;
    let mut proxy = Proxy::start(config);
    proxy.send([
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "capabilities": {"window": {"showDocument": {"support": true}}}
        }}),
    ]);
    assert!(
        proxy
            .receive_until(|r| r.iter().any(|m| m["id"] == 1))
            .iter()
            .any(|m| m["id"] == 1)
    );
    std::fs::remove_file(&goals_file).unwrap();
    proxy.send([execute_command(json!(2), OPEN_INFOVIEW)]);
    let requested = proxy.receive_until(|r| r.iter().any(|m| m["method"] == "window/showDocument"));
    let open = requested
        .iter()
        .find(|m| m["method"] == "window/showDocument")
        .unwrap();
    assert!(
        std::fs::read_to_string(&goals_file)
            .unwrap()
            .contains("Move the cursor inside a Lean file")
    );
    proxy.send([json!({"jsonrpc": "2.0", "id": open["id"], "result": {"success": true}})]);
    assert!(
        proxy
            .receive_until(|r| r.iter().any(|m| m["id"] == 2))
            .iter()
            .any(|m| m["id"] == 2)
    );

    std::fs::remove_file(&goals_file).unwrap();
    std::fs::create_dir(&goals_file).unwrap();
    proxy.send([execute_command(json!(3), OPEN_INFOVIEW)]);
    let received = proxy.receive_until(|r| r.iter().any(|m| m["id"] == 3));
    let response = received.iter().find(|m| m["id"] == 3).unwrap();
    assert_eq!(response["error"]["code"], -32803);
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Cannot write")
    );
    assert!(
        !received
            .iter()
            .any(|m| m["method"] == "window/showDocument")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn opens_the_complete_reference_on_demand_and_preserves_lean_commands() {
    for abbreviations in [true, false] {
        let dir = temp_dir(&format!("reference-{abbreviations}"));
        let reference = dir.join(".lake/AbbreviationView.md");
        let mut config = Config::new(dir.join(".lake/InfoView.md"));
        config.auto_open = false;
        config.abbreviations = abbreviations;
        let mut proxy = Proxy::start(config);
        // Browsing does not need workspace/applyEdit or automatic Unicode input.
        proxy.send([
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "capabilities": {"window": {"showDocument": {"support": true}}}
            }}),
        ]);
        let initialized = proxy.receive_until(|r| r.iter().any(|m| m["id"] == 1));
        let capabilities =
            &initialized.iter().find(|m| m["id"] == 1).unwrap()["result"]["capabilities"];
        assert_eq!(
            capabilities["executeCommandProvider"],
            json!({
                "commands": ["lean.original", SHOW_ABBREVIATIONS, OPEN_INFOVIEW], "workDoneProgress": true,
            })
        );
        assert_eq!(
            capabilities["completionProvider"]["triggerCharacters"],
            json!(["."])
        );
        assert!(
            !reference.exists(),
            "do not create the reference at startup"
        );

        let command_id = json!("browse-unicode");
        proxy.send([execute_command(command_id.clone(), SHOW_ABBREVIATIONS)]);
        let requested =
            proxy.receive_until(|r| r.iter().any(|m| m["method"] == "window/showDocument"));
        let open = requested
            .iter()
            .find(|m| m["method"] == "window/showDocument")
            .unwrap();
        assert_eq!(open["params"]["takeFocus"], true);
        assert_eq!(open["params"]["external"], false);
        assert!(
            open["params"]["uri"]
                .as_str()
                .unwrap()
                .ends_with("/.lake/AbbreviationView.md")
        );
        assert!(
            !requested.iter().any(|m| m["id"] == command_id),
            "wait for the open result"
        );
        let text = std::fs::read_to_string(&reference).unwrap();
        assert!(text.starts_with("| Abbreviation | Unicode symbol |\n| --- | --- |\n"));
        assert!(text.contains("` \\R `, ` \\br `, ` \\bbR `, ` \\Real `, ` \\real `"));
        assert!(text.contains(" | ` ⟨$CURSOR⟩ ` |"));
        assert!(text.contains(" | ` →ₗ ` |"));
        proxy.send([
            json!({"jsonrpc": "2.0", "id": open["id"], "result": {"success": true}}),
            execute_command(json!(2), "lean.original"),
        ]);
        let completed = proxy.receive_until(|r| {
            r.iter().any(|m| m["id"] == command_id) && r.iter().any(|m| m["id"] == 2)
        });
        assert_eq!(
            completed.iter().find(|m| m["id"] == command_id).unwrap(),
            &json!({"jsonrpc": "2.0", "id": command_id, "result": null})
        );
        assert_eq!(
            completed.iter().find(|m| m["id"] == 2).unwrap()["result"],
            json!({"from": "Lean", "params": {"command": "lean.original"}})
        );

        // Reopening unchanged data must not trigger another file rewrite.
        let old_time = std::time::UNIX_EPOCH + Duration::from_secs(946_684_800);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&reference)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(old_time))
            .unwrap();
        proxy.send([execute_command(json!(3), SHOW_ABBREVIATIONS)]);
        let reopened =
            proxy.receive_until(|r| r.iter().any(|m| m["method"] == "window/showDocument"));
        let open = reopened
            .iter()
            .find(|m| m["method"] == "window/showDocument")
            .unwrap();
        assert_eq!(
            std::fs::metadata(&reference).unwrap().modified().unwrap(),
            old_time
        );
        proxy.send([json!({"jsonrpc": "2.0", "id": open["id"], "result": {"success": true}})]);
        assert!(
            proxy
                .receive_until(|r| r.iter().any(|m| m["id"] == 3))
                .iter()
                .any(|m| m["id"] == 3)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn view_commands_are_not_advertised_without_show_document_support() {
    let dir = temp_dir("reference-unsupported");
    let mut config = Config::new(dir.join("InfoView.md"));
    config.auto_open = false;
    let mut proxy = Proxy::start(config);
    proxy.send([
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "capabilities": {"workspace": {"applyEdit": true}}
        }}),
        execute_command(json!(2), SHOW_ABBREVIATIONS),
        execute_command(json!(3), OPEN_INFOVIEW),
    ]);
    let received = proxy.receive_until(|r| (1..=3).all(|id| r.iter().any(|m| m["id"] == id)));
    assert_eq!(
        received.iter().find(|m| m["id"] == 1).unwrap()["result"]["capabilities"]["executeCommandProvider"],
        json!({"commands": ["lean.original"], "workDoneProgress": true})
    );
    for id in [2, 3] {
        assert!(
            received.iter().find(|m| m["id"] == id).unwrap()["error"]["message"]
                .as_str()
                .unwrap()
                .contains("window/showDocument")
        );
    }
    assert!(
        !received
            .iter()
            .any(|m| m["method"] == "window/showDocument")
    );
    assert!(!dir.join("AbbreviationView.md").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn view_commands_report_open_failures_and_protocol_errors() {
    let dir = temp_dir("reference-open-failure");
    let mut config = Config::new(dir.join("InfoView.md"));
    config.auto_open = false;
    let mut proxy = Proxy::start(config);
    proxy.send([
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "capabilities": {"window": {"showDocument": {"support": true}}}
        }}),
    ]);
    for (index, (command, reply)) in [
        (SHOW_ABBREVIATIONS, json!({"result": {"success": false}})),
        (
            SHOW_ABBREVIATIONS,
            json!({"error": {"code": -32603, "message": "file could not be opened"}}),
        ),
        (OPEN_INFOVIEW, json!({"result": {"success": false}})),
        (
            OPEN_INFOVIEW,
            json!({"error": {"code": -32603, "message": "file could not be opened"}}),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let id = json!(index + 2);
        proxy.send([execute_command(id.clone(), command)]);
        let received =
            proxy.receive_until(|r| r.iter().any(|m| m["method"] == "window/showDocument"));
        let open = received
            .iter()
            .find(|m| m["method"] == "window/showDocument")
            .unwrap();
        let mut reply = reply;
        reply["jsonrpc"] = json!("2.0");
        reply["id"] = open["id"].clone();
        proxy.send([reply]);
        let completed = proxy.receive_until(|r| r.iter().any(|m| m["id"] == id));
        let response = completed.iter().find(|m| m["id"] == id).unwrap();
        assert_eq!(response["error"]["code"], -32803);
        assert!(
            response["error"]["message"]
                .as_str()
                .unwrap()
                .contains("Cannot open")
        );
    }
    assert!(!dir.join("InfoView.opened").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn reference_command_reports_write_failures_and_does_not_overwrite_the_infoview() {
    for collision in [true, false] {
        let dir = temp_dir(&format!("reference-write-failure-{collision}"));
        let path = dir.join("AbbreviationView.md");
        let goals = if collision {
            path.clone()
        } else {
            dir.join("InfoView.md")
        };
        let mut config = Config::new(goals.clone());
        config.auto_open = false;
        let mut proxy = Proxy::start(config);
        proxy.send([
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "capabilities": {"window": {"showDocument": {"support": true}}}
            }}),
        ]);
        assert!(
            proxy
                .receive_until(|r| r.iter().any(|m| m["id"] == 1))
                .iter()
                .any(|m| m["id"] == 1)
        );
        let infoview = std::fs::read(&goals).unwrap();
        if !collision {
            std::fs::create_dir(&path).unwrap();
        }
        proxy.send([execute_command(json!(2), SHOW_ABBREVIATIONS)]);
        let received = proxy.receive_until(|r| r.iter().any(|m| m["id"] == 2));
        let response = received.iter().find(|m| m["id"] == 2).unwrap();
        assert_eq!(response["error"]["code"], -32803);
        assert!(
            response["error"]["message"]
                .as_str()
                .unwrap()
                .contains("Cannot write")
        );
        assert!(
            !received
                .iter()
                .any(|m| m["method"] == "window/showDocument")
        );
        assert_eq!(std::fs::read(&goals).unwrap(), infoview);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn expands_abbreviation_with_apply_edit() {
    let dir = temp_dir("abbrev");
    let mut config = Config::new(dir.join("InfoView.md"));
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
    let mut config = Config::new(dir.join("InfoView.md"));
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
    let mut config = Config::new(dir.join("InfoView.md"));
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
    let mut config = Config::new(dir.join("InfoView.md"));
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
fn paired_abbreviations_use_versioned_snippets_when_typed_or_terminated() {
    let dir = temp_dir("paired-edits");
    let mut config = Config::new(dir.join("InfoView.md"));
    config.auto_open = false;
    let mut proxy = Proxy::start(config);
    proxy.send([
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "capabilities": {"workspace": {"applyEdit": true, "workspaceEdit": {
                "documentChanges": true, "snippetEditSupport": true
            }}}
        }}),
    ]);

    for (index, (abbr, snippet)) in [
        ("<>", "⟨${0}⟩"),
        ("[[]]", "⟦${0}⟧"),
        ("f<<>>", "«${0}»"),
        ("norm", "‖${0}‖"),
    ]
    .into_iter()
    .enumerate()
    {
        for terminated in [false, true] {
            let uri = format!("file:///tmp/Pair{index}-{terminated}.lean");
            let (before, typed) = if terminated {
                (abbr, " ")
            } else {
                abbr.split_at(abbr.len() - 1)
            };
            let text = format!("𝔸 \\{before}");
            let character = text.encode_utf16().count();
            proxy.send([
                json!({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {"textDocument": {
                    "uri": uri, "languageId": "lean4", "version": 1, "text": text
                }}}),
                json!({"jsonrpc": "2.0", "method": "textDocument/didChange", "params": {
                    "textDocument": {"uri": uri, "version": 2}, "contentChanges": [{
                        "range": {"start": {"line": 0, "character": character},
                                  "end": {"line": 0, "character": character}}, "text": typed
                    }]
                }}),
            ]);
            let received =
                proxy.receive_until(|r| r.iter().any(|m| m["method"] == "workspace/applyEdit"));
            let request = received
                .iter()
                .find(|m| m["method"] == "workspace/applyEdit")
                .unwrap();
            let change = &request["params"]["edit"]["documentChanges"][0];
            assert_eq!(change["textDocument"], json!({"uri": uri, "version": 2}));
            assert_eq!(
                change["edits"][0],
                json!({
                    "range": {"start": {"line": 0, "character": 3},
                              "end": {"line": 0, "character": 4 + abbr.len()}},
                    "snippet": {"kind": "snippet", "value": snippet}
                }),
                "{abbr}, terminated={terminated}"
            );
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn workspace_and_completion_snippet_capabilities_are_independent() {
    for (index, (document_changes, workspace_snippets, completion_snippets)) in [
        (true, true, true),
        (true, false, true),
        (true, true, false),
        (true, false, false),
        (false, true, true),
    ]
    .into_iter()
    .enumerate()
    {
        let dir = temp_dir(&format!("paired-capabilities-{index}"));
        let mut config = Config::new(dir.join("InfoView.md"));
        config.auto_open = false;
        let mut proxy = Proxy::start(config);
        let uri = "file:///tmp/PartialPair.lean";
        proxy.send([
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"capabilities": {
                "workspace": {"applyEdit": true, "workspaceEdit": {
                    "documentChanges": document_changes, "snippetEditSupport": workspace_snippets
                }},
                "textDocument": {"completion": {"completionItem": {"snippetSupport": completion_snippets}}}
            }}}),
            json!({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {"textDocument": {
                "uri": uri, "languageId": "lean4", "version": 1, "text": "\\nor"
            }}}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "textDocument/completion", "params": {
                "textDocument": {"uri": uri}, "position": {"line": 0, "character": 4}
            }}),
            json!({"jsonrpc": "2.0", "method": "textDocument/didChange", "params": {
                "textDocument": {"uri": uri, "version": 2}, "contentChanges": [{
                    "range": {"start": {"line": 0, "character": 4}, "end": {"line": 0, "character": 4}},
                    "text": " "
                }]
            }}),
        ]);
        let received = proxy.receive_until(|r| {
            r.iter().any(|m| m["id"] == 2) && r.iter().any(|m| m["method"] == "workspace/applyEdit")
        });
        let item = &received.iter().find(|m| m["id"] == 2).unwrap()["result"]["items"][0];
        assert_eq!(
            item["insertTextFormat"],
            if completion_snippets { 2 } else { 1 }
        );
        assert_eq!(
            item["textEdit"]["newText"],
            if completion_snippets {
                "‖${0}‖"
            } else {
                "‖‖"
            }
        );
        let request = received
            .iter()
            .find(|m| m["method"] == "workspace/applyEdit")
            .unwrap();
        let edit = if document_changes {
            &request["params"]["edit"]["documentChanges"][0]["edits"][0]
        } else {
            &request["params"]["edit"]["changes"][uri][0]
        };
        assert_eq!(
            edit["range"]["end"]["character"], 4,
            "the space is outside the replacement"
        );
        if document_changes && workspace_snippets {
            assert_eq!(
                edit["snippet"],
                json!({"kind": "snippet", "value": "‖${0}‖"})
            );
            assert!(edit.get("newText").is_none());
        } else {
            assert_eq!(edit["newText"], "‖‖");
            assert!(edit.get("snippet").is_none());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn opens_infoview_once_per_project() {
    let dir = temp_dir("open-once");
    let goals_file = dir.join(".lake/InfoView.md");
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
    let opened = dir.join(".lake/InfoView.opened");
    wait_for_file(&opened, |_| opened.exists());
    assert!(opened.exists());
    // Zed restores the open preview with the layout; do not open a second one.
    assert!(!open(&mut Proxy::start(Config::new(goals_file))));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn marks_proved_theorems() {
    let dir = temp_dir("status");
    let mut config = Config::new(dir.join(".lake/InfoView.md"));
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
