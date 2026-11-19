//! What the proxy threads share: the two output streams, the router for the
//! proxy's own requests, and the channels to the goals worker and hover timer.

use crate::framing;
use crate::hover::HoverWait;
use crate::lsp::{self, Cursor, Diagnostic, RequestId};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GoalKind {
    /// `$/lean/plainGoal` at the cursor.
    Plain = 0,
    /// `$/lean/plainTermGoal` at the cursor.
    Term = 1,
    /// `$/lean/plainGoal` at the start of the cursor line (state before the tactic).
    Before = 2,
}

/// What a request of ours to the server is for.
pub enum Pending {
    Goal(GoalKind),
    /// The goal for the client's hover request with this id.
    Hover(RequestId),
}

/// Events for the goals worker.
pub enum Event {
    Cursor(Cursor),
    /// The document changed: request goals again.
    Changed {
        uri: String,
    },
    Diagnostics {
        uri: String,
        diagnostics: Vec<Diagnostic>,
    },
    /// The first line Lean is still elaborating; `None` once done.
    Progress {
        uri: String,
        line: Option<u64>,
    },
    Goal {
        id: RequestId,
        kind: GoalKind,
        result: Value,
    },
}

#[derive(Default)]
pub struct State {
    /// Our requests to the server, by id.
    pub pending: HashMap<RequestId, Pending>,
    /// Our requests to the client; their responses are dropped.
    pub client_pending: HashSet<RequestId>,
    /// Client hover requests whose response is merged with a goal.
    pub hovers: HashMap<RequestId, HoverWait>,
}

type Out = Mutex<Box<dyn Write + Send>>;

pub struct Shared {
    pub config: crate::Config,
    state: Mutex<State>,
    to_client: Out,
    to_server: Out,
    events: Sender<Event>,
    hover_deadlines: Sender<(Instant, RequestId)>,
    next_id: AtomicU64,
}

impl Shared {
    pub fn new(
        config: crate::Config,
        to_client: impl Write + Send + 'static,
        to_server: impl Write + Send + 'static,
        events: Sender<Event>,
        hover_deadlines: Sender<(Instant, RequestId)>,
    ) -> Self {
        Self {
            config,
            state: Mutex::default(),
            to_client: Mutex::new(Box::new(to_client)),
            to_server: Mutex::new(Box::new(to_server)),
            events,
            hover_deadlines,
            next_id: AtomicU64::new(1),
        }
    }

    pub fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn write_client(&self, body: &[u8]) {
        write(&self.to_client, body, "client");
    }

    pub fn write_server(&self, body: &[u8]) {
        write(&self.to_server, body, "server");
    }

    pub fn send_client(&self, msg: &Value) {
        self.write_client(&encode(msg));
    }

    pub fn send_server(&self, msg: &Value) {
        self.write_server(&encode(msg));
    }

    pub fn event(&self, event: Event) {
        let _ = self.events.send(event);
    }

    pub fn hover_deadline(&self, at: Instant, id: RequestId) {
        let _ = self.hover_deadlines.send((at, id));
    }

    fn fresh_id(&self) -> RequestId {
        RequestId::own(self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    /// Sends a request of ours to the server; its response is routed by `pending`.
    pub fn request_server(&self, method: &str, params: Value, pending: Pending) -> RequestId {
        let id = self.fresh_id();
        self.state().pending.insert(id.clone(), pending);
        self.send_server(&lsp::request(&id, method, params));
        id
    }

    /// Sends a request of ours to the client; its response is not forwarded.
    pub fn request_client(&self, method: &str, params: Value) {
        let id = self.fresh_id();
        self.state().client_pending.insert(id.clone());
        self.send_client(&lsp::request(&id, method, params));
    }

    /// Cancels those of our server requests that are still pending.
    pub fn cancel_server(&self, ids: impl IntoIterator<Item = RequestId>) {
        let cancelled: Vec<RequestId> = {
            let mut state = self.state();
            ids.into_iter()
                .filter(|id| state.pending.remove(id).is_some())
                .collect()
        };
        for id in cancelled {
            self.send_server(&lsp::notification(
                "$/cancelRequest",
                json!({"id": id.to_value()}),
            ));
        }
    }
}

fn encode(msg: &Value) -> Vec<u8> {
    serde_json::to_vec(msg).unwrap_or_default()
}

fn write(out: &Out, body: &[u8], side: &str) {
    let mut out = out.lock().unwrap_or_else(|e| e.into_inner());
    if let Err(e) = framing::write_raw(&mut *out, body) {
        log(&format!("write to {side} failed: {e}"));
    }
}

pub fn log(msg: &str) {
    eprintln!("[lean-infoview] {msg}");
}
