//! The goals worker: follows the cursor, requests goals from Lean and writes the
//! rendered infoview file.
//!
//! Each request round asks for the goal and expected type at the cursor, plus the
//! goal at the start of the line (the state before the tactic, used to mark changes).
//! The file is written once all answers of a round are in, so the view does not
//! flicker. If they take longer than [`STALE_AFTER`], the previous goals stay on
//! screen marked as updating. Answers that are empty only because Lean is still
//! elaborating do not replace the previous goals.

use crate::lsp::{Cursor, Diagnostic, Position, RequestId};
use crate::render::{self, Goals};
use crate::shared::{Event, GoalKind, GoalTarget, Pending, Shared, log};
use crate::uri;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

/// Debounce for cursor movement and document changes.
const DEBOUNCE: Duration = Duration::from_millis(100);
/// How long to keep showing the previous goals before marking them as updating.
const STALE_AFTER: Duration = Duration::from_millis(300);

struct Worker<'a> {
    shared: &'a Shared,
    target: Option<GoalTarget>,
    round: Option<GoalTarget>,
    /// Display name of the cursor's file.
    file: String,
    diagnostics: HashMap<String, Vec<Diagnostic>>,
    /// First line still being elaborated, per document being processed.
    elaborating: HashMap<String, u64>,
    /// When to send the next request round (debounce).
    request_at: Option<Instant>,
    /// When to stop waiting and show the previous goals as stale.
    stale_at: Option<Instant>,
    /// Ids of the current round's requests.
    outstanding: Vec<RequestId>,
    /// Answers of the current round, indexed by [`GoalKind`]; `None` until they arrive.
    answers: [Option<Value>; 3],
    shown: Goals,
    stale: bool,
    last_written: String,
}

pub fn worker(shared: &Shared, events: Receiver<Event>) {
    let mut worker = Worker::new(shared);
    loop {
        let wait = [worker.request_at, worker.stale_at]
            .into_iter()
            .flatten()
            .min()
            .map_or(Duration::MAX, |at| {
                at.saturating_duration_since(Instant::now())
            });
        match events.recv_timeout(wait) {
            Ok(event) => worker.on_event(event),
            Err(RecvTimeoutError::Timeout) => worker.on_timeout(),
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

impl<'a> Worker<'a> {
    fn new(shared: &'a Shared) -> Self {
        Self {
            shared,
            target: None,
            round: None,
            file: String::new(),
            diagnostics: HashMap::new(),
            elaborating: HashMap::new(),
            request_at: None,
            stale_at: None,
            outstanding: Vec::new(),
            answers: Default::default(),
            shown: Goals::default(),
            stale: false,
            last_written: String::from(render::PLACEHOLDER),
        }
    }

    fn on_event(&mut self, event: Event) {
        // Client state changes synchronously, while events from both directions
        // can arrive in either order. Check it before consuming any goal answer.
        self.sync_target();
        match event {
            Event::Cursor => {}
            Event::Changed { uri } => {
                if self.shared.state().documents.get(&uri).is_none() {
                    self.diagnostics.remove(&uri);
                    self.elaborating.remove(&uri);
                }
                self.refresh(&uri);
            }
            Event::Diagnostics { uri, diagnostics } => {
                self.diagnostics.insert(uri.clone(), diagnostics);
                self.refresh(&uri);
            }
            Event::Progress { uri, line } => {
                let was = match line {
                    Some(line) => self.elaborating.insert(uri.clone(), line),
                    None => self.elaborating.remove(&uri),
                };
                if was.is_some() && line.is_none() {
                    // Finished: answers given during processing may be empty, ask again.
                    self.refresh(&uri);
                } else if was != line && self.is_cursor_uri(&uri) {
                    self.write();
                }
            }
            Event::Goal { id, kind, result } => self.on_goal(&id, kind, result),
        }
    }

    fn is_cursor_uri(&self, uri: &str) -> bool {
        self.target.as_ref().is_some_and(|t| t.cursor.uri == uri)
    }

    fn cancel_round(&mut self) {
        self.shared.cancel_server(self.outstanding.drain(..));
        self.answers = Default::default();
        self.round = None;
        self.stale_at = None;
    }

    fn sync_target(&mut self) {
        let target = self.shared.goal_target();
        if target == self.target {
            return;
        }
        self.cancel_round();
        if self.target.as_ref().map(|t| &t.cursor.uri) != target.as_ref().map(|t| &t.cursor.uri) {
            // Never retain another file's goals under the new file's heading.
            self.shown = Goals::default();
        }
        self.target = target;
        self.file = self
            .target
            .as_ref()
            .map_or_else(String::new, |t| uri::display_name(&t.cursor.uri));
        self.request_at = self.target.as_ref().map(|_| Instant::now() + DEBOUNCE);
        self.stale = true;
        if self.target.is_none() {
            self.write();
        }
    }

    /// Requests goals again soon if `uri` is the cursor's document.
    fn refresh(&mut self, uri: &str) {
        if self.is_cursor_uri(uri) {
            let at = Instant::now() + DEBOUNCE;
            self.request_at = Some(self.request_at.map_or(at, |d| d.min(at)));
        }
    }

    fn on_timeout(&mut self) {
        self.sync_target();
        let now = Instant::now();
        if self.stale_at.is_some_and(|at| at <= now) {
            self.stale_at = None;
            self.stale = true;
            self.write();
        }
        if self.request_at.is_some_and(|at| at <= now) {
            self.request_at = None;
            self.request_goals();
        }
    }

    fn on_goal(&mut self, id: &RequestId, kind: GoalKind, result: Value) {
        if self.round != self.shared.goal_target() || !self.outstanding.contains(id) {
            return;
        }
        self.answers[kind as usize] = Some(result);
        if self.answers.iter().any(Option::is_none) {
            return;
        }
        self.outstanding.clear();
        self.stale_at = None;
        let [plain, term, before] =
            std::mem::take(&mut self.answers).map(Option::unwrap_or_default);
        let elaborating = self
            .target
            .as_ref()
            .is_some_and(|t| self.elaborating.contains_key(&t.cursor.uri));
        if plain.is_null() && term.is_null() && elaborating {
            // Lean answers `null` for parts it has not elaborated yet; keep the previous
            // goals until the refresh after processing finishes.
            self.stale = true;
        } else {
            self.shown = Goals {
                plain,
                term,
                before,
            };
            self.stale = false;
        }
        self.write();
    }

    /// Cancels outdated requests and starts a new round at the cursor.
    fn request_goals(&mut self) {
        self.sync_target();
        self.cancel_round();
        let Some(target) = self.target.clone() else {
            return;
        };
        let cursor = &target.cursor;
        self.round = Some(target.clone());
        self.stale_at = Some(Instant::now() + STALE_AFTER);
        // Keep disk writes behind the same debounce as goal requests.
        self.write();

        let line_start = Cursor {
            uri: cursor.uri.clone(),
            pos: Position {
                line: cursor.pos.line,
                character: 0,
            },
        };
        let mut requests = vec![
            (GoalKind::Plain, "$/lean/plainGoal", cursor.params()),
            (GoalKind::Term, "$/lean/plainTermGoal", cursor.params()),
        ];
        if cursor.pos.character > 0 {
            requests.push((GoalKind::Before, "$/lean/plainGoal", line_start.params()));
        } else {
            // The state before the tactic is the state at the cursor.
            self.answers[GoalKind::Before as usize] = Some(Value::Null);
        }
        for (kind, method, params) in requests {
            let id = self
                .shared
                .request_server(method, params, Pending::Goal(kind));
            self.outstanding.push(id);
        }
    }

    fn write(&mut self) {
        if self.target != self.shared.goal_target() {
            return;
        }
        let Some(target) = &self.target else {
            if self.last_written != render::PLACEHOLDER {
                write_file(&self.shared.config.goals_file, render::PLACEHOLDER);
                self.last_written = render::PLACEHOLDER.to_owned();
            }
            return;
        };
        let cursor = &target.cursor;
        let text = render::render(&render::View {
            file: &self.file,
            line: cursor.pos.line,
            character: cursor.pos.character,
            elaborating: self.elaborating.get(&cursor.uri).copied(),
            stale: self.stale,
            goals: &self.shown,
            diagnostics: self.diagnostics.get(&cursor.uri).map_or(&[], Vec::as_slice),
        });
        if text != self.last_written {
            write_file(&self.shared.config.goals_file, &text);
            self.last_written = text;
        }
    }
}

fn write_file(path: &Path, text: &str) {
    if let Err(e) = std::fs::write(path, text) {
        log(&format!("cannot write {}: {e}", path.display()));
    }
}

/// Creates the infoview file (and its directory) with the placeholder text.
pub fn prepare(path: &Path) {
    if let Some(parent) = path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        log(&format!("cannot create {}: {e}", parent.display()));
    }
    write_file(path, render::PLACEHOLDER);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::mpsc;

    struct Fixture(Shared);

    impl Fixture {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("lean-goals-{name}-{}.md", std::process::id()));
            Self(Shared::new(
                crate::Config::new(path),
                std::io::sink(),
                std::io::sink(),
                mpsc::channel().0,
                mpsc::channel().0,
            ))
        }

        fn cursor(&self, uri: &str, character: u64) {
            self.0.state().set_cursor(Some(Cursor {
                uri: uri.to_owned(),
                pos: Position { line: 0, character },
            }));
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0.config.goals_file);
        }
    }

    fn answer(worker: &mut Worker<'_>, ids: &[RequestId], label: &str) {
        for (id, kind) in ids
            .iter()
            .zip([GoalKind::Plain, GoalKind::Term, GoalKind::Before])
        {
            worker.on_event(Event::Goal {
                id: id.clone(),
                kind,
                result: if kind == GoalKind::Plain {
                    json!({"goals": [label]})
                } else {
                    Value::Null
                },
            });
        }
    }

    #[test]
    fn delayed_answers_cannot_cross_files_even_before_the_cursor_event() {
        let fixture = Fixture::new("cross-file");
        fixture.cursor("file:///A.lean", 2);
        let mut worker = Worker::new(&fixture.0);
        worker.request_goals();
        let old = worker.outstanding.clone();

        fixture.cursor("file:///B.lean", 2);
        answer(&mut worker, &old, "FROM_A");
        assert!(worker.shown.plain.is_null());
        worker.request_goals();
        let current = worker.outstanding.clone();
        answer(&mut worker, &old, "FROM_A");
        answer(&mut worker, &current, "FROM_B");
        assert!(worker.last_written.contains("B.lean:1:3"));
        assert!(worker.last_written.contains("FROM_B"));
        assert!(!worker.last_written.contains("FROM_A"));
        assert!(!worker.stale);
    }

    #[test]
    fn moving_away_and_back_does_not_revalidate_old_answers() {
        let fixture = Fixture::new("cursor-generation");
        fixture.cursor("file:///A.lean", 2);
        let mut worker = Worker::new(&fixture.0);
        worker.request_goals();
        let old = worker.outstanding.clone();
        fixture.cursor("file:///A.lean", 3);
        fixture.cursor("file:///A.lean", 2);
        answer(&mut worker, &old, "OLD_POSITION");
        assert!(worker.shown.plain.is_null());
        assert!(worker.stale);
        assert!(worker.outstanding.is_empty());
    }

    #[test]
    fn edits_invalidate_answers_and_closing_clears_the_view() {
        let fixture = Fixture::new("edit-close");
        let uri = "file:///A.lean";
        fixture
            .0
            .state()
            .documents
            .open(&json!({"textDocument": {"uri": uri, "text": "old", "version": 1}}));
        fixture.cursor(uri, 2);
        let mut worker = Worker::new(&fixture.0);
        worker.request_goals();
        let old = worker.outstanding.clone();
        {
            let mut state = fixture.0.state();
            state.documents.change(&json!({"textDocument": {"uri": uri, "version": 2}, "contentChanges": [{"text": "new"}]}));
            state.document_changed(uri);
        }
        answer(&mut worker, &old, "OLD_VERSION");
        assert!(worker.shown.plain.is_null());
        worker.request_goals();
        let current = worker.outstanding.clone();
        answer(&mut worker, &current, "NEW_VERSION");
        assert!(worker.last_written.contains("NEW_VERSION"));
        {
            let mut state = fixture.0.state();
            state
                .documents
                .close(&json!({"textDocument": {"uri": uri}}));
            state.document_closed(uri);
        }
        worker.on_event(Event::Changed {
            uri: uri.to_owned(),
        });
        assert_eq!(worker.last_written, render::PLACEHOLDER);
        assert!(worker.target.is_none());
    }

    #[test]
    fn progress_cannot_display_previous_files_goals() {
        let fixture = Fixture::new("progress");
        fixture.cursor("file:///A.lean", 2);
        let mut worker = Worker::new(&fixture.0);
        worker.request_goals();
        let ids = worker.outstanding.clone();
        answer(&mut worker, &ids, "FROM_A");
        fixture.cursor("file:///B.lean", 2);
        worker.on_event(Event::Progress {
            uri: "file:///B.lean".to_owned(),
            line: Some(1),
        });
        assert!(worker.last_written.contains("B.lean"));
        assert!(!worker.last_written.contains("FROM_A"));
        assert!(worker.stale);
    }
}
