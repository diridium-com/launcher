// Copyright (c) Diridium Technologies Inc. All rights reserved.
// Licensed under the MPL-2.0 License. See LICENSE file in the project root.

//! Native console support: captures a launched administrator process's stdout
//! and stderr and streams them to a dedicated Tauri webview window.
//!
//! Output is buffered per console (keyed by window label) so that a window
//! which attaches *after* the process has already started still receives every
//! line. The replay-then-attach handshake in [`console_subscribe`] holds the
//! buffer lock across both steps, so no line is ever lost or duplicated between
//! the backlog snapshot and the live stream.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};

/// Max lines retained for replay to a window that attaches late. Live streaming
/// is unbounded; only this backlog is capped, to bound memory on chatty engines.
const MAX_BACKLOG_LINES: usize = 5000;

/// A message streamed to a console window. Serialized with a `kind` tag so the
/// frontend can distinguish log lines, the exit notice, and a relaunch reset.
#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConsoleMsg {
    Line { stream: &'static str, text: String },
    Exit { status: String },
    /// Sent when a connection is relaunched into an already-open console so the
    /// window can clear its "exited" indicator and show a separator.
    Reset { text: String },
}

/// Per-console state: the replay backlog, the live sink (set once a window
/// attaches), and a generation counter that increments on every (re)launch so a
/// superseded process's exit is ignored.
#[derive(Default)]
pub struct ConsoleBuf {
    backlog: Vec<ConsoleMsg>,
    sink: Option<Channel<ConsoleMsg>>,
    generation: u64,
}

/// Handle passed into `WebstartFile::run` for a launch that streams to a
/// console: the shared buffer, the generation this launch owns, and the app
/// handle + window label needed to close the console when the process exits.
pub struct ConsoleSink {
    pub buf: Arc<Mutex<ConsoleBuf>>,
    pub generation: u64,
    pub app: AppHandle,
    pub label: String,
}

/// Maps console window label -> shared buffer. Managed as Tauri state so the
/// `launch` command (which starts the readers) and `console_subscribe` (which
/// the window calls) can find the same buffer.
#[derive(Default)]
pub struct ConsoleRegistry(Mutex<HashMap<String, Arc<Mutex<ConsoleBuf>>>>);

impl ConsoleRegistry {
    /// Return the buffer for `label`, creating it if absent. Reusing the buffer
    /// on a relaunch into an existing window keeps the same live sink.
    pub fn get_or_create(&self, label: &str) -> Arc<Mutex<ConsoleBuf>> {
        let mut map = self.0.lock().expect("console registry mutex poisoned");
        map.entry(label.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(ConsoleBuf::default())))
            .clone()
    }

    fn get(&self, label: &str) -> Option<Arc<Mutex<ConsoleBuf>>> {
        self.0
            .lock()
            .expect("console registry mutex poisoned")
            .get(label)
            .cloned()
    }

    /// Drop a console's buffer (called when its window is destroyed). Reader and
    /// reap threads keep their own `Arc` clones, so a still-running process keeps
    /// pushing harmlessly until it exits; the next launch gets a fresh buffer.
    pub fn remove(&self, label: &str) {
        self.0
            .lock()
            .expect("console registry mutex poisoned")
            .remove(label);
    }
}

/// Append a line to the buffer and forward it live if a window is attached.
pub fn push_line(buf: &Arc<Mutex<ConsoleBuf>>, stream: &'static str, text: String) {
    let msg = ConsoleMsg::Line { stream, text };
    let mut b = buf.lock().expect("console buffer mutex poisoned");
    if let Some(sink) = &b.sink {
        let _ = sink.send(msg.clone());
    }
    b.backlog.push(msg);
    if b.backlog.len() > MAX_BACKLOG_LINES {
        let overflow = b.backlog.len() - MAX_BACKLOG_LINES;
        b.backlog.drain(0..overflow);
    }
}

/// Mark the process exited and notify any attached window — but only if this is
/// still the current generation. A relaunch into the same console bumps the
/// generation, so an older process exiting later must not flip a live console to
/// "exited". Returns true if this was the current generation (i.e. the exit was
/// acted on), so the caller can decide whether to close the window.
pub fn mark_exited(buf: &Arc<Mutex<ConsoleBuf>>, generation: u64, status: String) -> bool {
    let mut b = buf.lock().expect("console buffer mutex poisoned");
    if b.generation != generation {
        return false;
    }
    let msg = ConsoleMsg::Exit { status };
    if let Some(sink) = &b.sink {
        let _ = sink.send(msg.clone());
    }
    b.backlog.push(msg);
    true
}

/// Close a console window (called when its admin process exits cleanly). Runs on
/// the main thread; a no-op if the window was already closed. The window's
/// Destroyed event evicts the buffer from the registry.
pub fn close_window(app: &AppHandle, label: &str) {
    let handle = app.clone();
    let label = label.to_string();
    let _ = app.run_on_main_thread(move || {
        if let Some(w) = handle.get_webview_window(&label) {
            let _ = w.close();
        }
    });
}

/// Prepare a (possibly reused) buffer for a fresh launch and return the new
/// generation. Emits a Reset separator only when there is prior output (i.e. a
/// real relaunch into an open window), so a first launch shows nothing spurious.
pub fn reset_for_relaunch(buf: &Arc<Mutex<ConsoleBuf>>) -> u64 {
    let mut b = buf.lock().expect("console buffer mutex poisoned");
    b.generation += 1;
    if !b.backlog.is_empty() {
        let sep = ConsoleMsg::Reset {
            text: "──────── relaunched ────────".to_string(),
        };
        if let Some(sink) = &b.sink {
            let _ = sink.send(sep.clone());
        }
        b.backlog.push(sep);
    }
    b.generation
}

/// Called by a console window once it is ready. Replays the backlog through the
/// window's channel, then attaches the channel as the live sink. The buffer
/// lock is held across both steps so the live stream begins exactly where the
/// replay ends — no gaps, no duplicates.
#[tauri::command(rename_all = "snake_case")]
pub fn console_subscribe(
    label: String,
    on_line: Channel<ConsoleMsg>,
    registry: State<'_, ConsoleRegistry>,
) -> Result<(), String> {
    let buf = registry
        .get(&label)
        .ok_or_else(|| format!("no console buffer for {}", label))?;
    let mut b = buf.lock().expect("console buffer mutex poisoned");
    for msg in &b.backlog {
        let _ = on_line.send(msg.clone());
    }
    b.sink = Some(on_line);
    Ok(())
}

/// Write the console contents to a user-chosen path. Done in Rust so the save
/// target isn't constrained by the frontend fs capability scope.
#[tauri::command(rename_all = "snake_case")]
pub fn console_save(path: String, content: String) -> Result<(), String> {
    std::fs::write(&path, content).map_err(|e| e.to_string())
}
