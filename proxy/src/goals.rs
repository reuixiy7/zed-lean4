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
use crate::shared::{Event, GoalKind, Pending, Shared, log};
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
    cursor: Option<Cursor>,
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
    let mut worker = Worker {
        shared,
        cursor: None,
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
    };
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

impl Worker<'_> {
    fn on_event(&mut self, event: Event) {
        match event {
            Event::Cursor(cursor) => {
                if self.cursor.as_ref() != Some(&cursor) {
                    if self.cursor.as_ref().is_none_or(|c| c.uri != cursor.uri) {
                        self.file = uri::display_name(&cursor.uri);
                    }
                    self.cursor = Some(cursor);
                    // Restart the debounce window while the cursor keeps moving.
                    self.request_at = Some(Instant::now() + DEBOUNCE);
                }
            }
            Event::Changed { uri } => self.refresh(&uri),
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
        self.cursor.as_ref().is_some_and(|c| c.uri == uri)
    }

    /// Requests goals again soon if `uri` is the cursor's document.
    fn refresh(&mut self, uri: &str) {
        if self.is_cursor_uri(uri) {
            let at = Instant::now() + DEBOUNCE;
            self.request_at = Some(self.request_at.map_or(at, |d| d.min(at)));
        }
    }

    fn on_timeout(&mut self) {
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
        if !self.outstanding.contains(id) {
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
            .cursor
            .as_ref()
            .is_some_and(|c| self.elaborating.contains_key(&c.uri));
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
        let Some(cursor) = &self.cursor else { return };
        self.shared.cancel_server(self.outstanding.drain(..));
        self.answers = Default::default();
        self.stale_at = Some(Instant::now() + STALE_AFTER);

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
        let Some(cursor) = &self.cursor else { return };
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
