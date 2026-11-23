//! LSP proxy between Zed and the Lean server.
//!
//! All traffic is forwarded unchanged, except that the proxy:
//! - learns the cursor position from requests Zed sends on cursor movement
//!   (`textDocument/documentHighlight`, `textDocument/codeAction`);
//! - asks Lean for `$/lean/plainGoal` / `$/lean/plainTermGoal` there (using its own
//!   request ids, whose responses are not forwarded to Zed) and writes the result to
//!   a Markdown file meant to be shown in Zed's Markdown preview;
//! - optionally appends the tactic state to hover responses;
//! - optionally asks Zed to open the infoview file via `window/showDocument`;
//! - expands Unicode abbreviations (`\R` → `ℝ`) via `workspace/applyEdit` and completions;
//! - shows the available input abbreviations when hovering Unicode symbols;
//! - opens the infoview and complete Unicode abbreviation reference through LSP commands;
//! - optionally marks declarations as proved (✅), using `sorry` (⚠️) or failing (❌).
//!
//! Modules:
//! - [`framing`]: LSP `Content-Length` framing, lazily parsed messages;
//! - [`lsp`]: request ids, positions, diagnostics, JSON-RPC envelopes;
//! - `shared`: output streams and the router for the proxy's own requests;
//! - `client`: Zed → Lean direction (cursor, `initialize`, documents, abbreviations);
//! - `commands`: LSP commands for opening the infoview and abbreviation reference;
//! - `server`: Lean → Zed direction (routing our responses, diagnostics, progress);
//! - `hover`: merging Unicode input hints and tactic state into hover responses;
//! - `goals`: the worker that requests goals and writes the infoview file;
//! - `render`: Markdown rendering;
//! - `document`: mirrors of open documents (UTF-16 positions);
//! - `abbrev`: the abbreviation table and when to replace;
//! - `status`: declaration status as diagnostics;
//! - `uri`: `file://` helpers.

mod abbrev;
mod client;
mod commands;
mod document;
pub mod framing;
mod goals;
mod hover;
pub mod lsp;
mod render;
mod server;
mod shared;
mod status;
mod uri;

pub use shared::log;

use shared::Shared;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::thread;

pub struct Config {
    pub goals_file: PathBuf,
    pub hover: bool,
    pub auto_open: bool,
    pub abbreviations: bool,
    /// Declaration status (✅ / ⚠️ / ❌) in the editor's diagnostics.
    pub status: bool,
}

impl Config {
    pub fn new(goals_file: PathBuf) -> Self {
        Self {
            goals_file,
            hover: true,
            auto_open: true,
            abbreviations: true,
            status: true,
        }
    }
}

/// Runs the proxy until either side closes its stream.
pub fn run(
    config: Config,
    client_in: impl Read + Send + 'static,
    client_out: impl Write + Send + 'static,
    server_in: impl Write + Send + 'static,
    server_out: impl Read + Send + 'static,
) -> io::Result<()> {
    goals::prepare(&config.goals_file);

    let (events_tx, events_rx) = mpsc::channel();
    let (hover_tx, hover_rx) = mpsc::channel();
    let shared = Arc::new(Shared::new(
        config, client_out, server_in, events_tx, hover_tx,
    ));
    let (done_tx, done_rx) = mpsc::channel::<io::Result<()>>();

    let done = done_tx.clone();
    spawn(&shared, move |s| {
        let _ = done.send(client::forward(s, client_in));
    });
    spawn(&shared, move |s| {
        let _ = done_tx.send(server::forward(s, server_out));
    });
    spawn(&shared, move |s| goals::worker(s, events_rx));
    spawn(&shared, move |s| hover::timer(s, hover_rx));

    done_rx.recv().unwrap_or(Ok(()))
}

fn spawn(shared: &Arc<Shared>, f: impl FnOnce(&Shared) + Send + 'static) {
    let shared = shared.clone();
    thread::spawn(move || f(&shared));
}
