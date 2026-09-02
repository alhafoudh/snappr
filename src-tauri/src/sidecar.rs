//! The Ruby pipeline as a child process, spoken to in NDJSON over stdin/stdout.
//!
//! The Ruby side handles one request at a time, in order, which is what makes this
//! simple: a single writer thread owns stdin, a single reader thread owns stdout,
//! and requests are matched to responses by id. It also means this queue is the
//! deduplication mechanism - the same photo cannot be in flight twice because it
//! cannot be sent twice.
//!
//! Ruby's stdout is protocol and Ruby's stderr is log output. That separation is
//! the whole contract, and `Snappr.logger` writing to stderr is what upholds it on
//! the other side.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::{json, Value};

use crate::runtime::Runtime;

/// How long to wait for one photo before giving up on it. A 12MP HEIC composites
/// in about a second; this is generous enough to be certain something is wrong.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

pub struct Sidecar {
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    next_id: AtomicU64,
    /// Response channels, keyed by request id, for whoever is waiting on them.
    pending: Arc<Mutex<HashMap<u64, Sender<Value>>>>,
}

impl Sidecar {
    /// Spawns Ruby and completes the handshake, so a `Sidecar` that exists is one
    /// that is known to work. Config travels in the handshake rather than being
    /// read from `config/snappr.yml` by the child, so there is one source of truth.
    pub fn start(runtime: &Runtime, config: Value, preview_dir: &str) -> Result<Self, String> {
        let mut child = Command::new(&runtime.ruby)
            .args(runtime.args())
            .envs(runtime.env())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot start the Ruby sidecar at {}: {e}", runtime.ruby.display()))?;

        let stdin = child.stdin.take().ok_or("the sidecar has no stdin")?;
        let stdout = child.stdout.take().ok_or("the sidecar has no stdout")?;
        let stderr = child.stderr.take().ok_or("the sidecar has no stderr")?;

        let pending: Arc<Mutex<HashMap<u64, Sender<Value>>>> = Arc::default();

        // Reader: every line is one frame. Frames carrying "ok" complete a request;
        // "event" frames are progress and are logged but not awaited, so a slow
        // photo still reports that it started.
        let routes = Arc::clone(&pending);
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let Ok(frame) = serde_json::from_str::<Value>(&line) else {
                    log::warn!("sidecar wrote a non-JSON line on stdout: {line}");
                    continue;
                };
                if frame.get("ok").is_none() {
                    log::debug!("sidecar event: {frame}");
                    continue;
                }
                let Some(id) = frame.get("id").and_then(Value::as_u64) else {
                    log::warn!("sidecar sent a result with no id: {frame}");
                    continue;
                };
                if let Some(tx) = routes.lock().unwrap().remove(&id) {
                    let _ = tx.send(frame);
                }
            }
            log::info!("sidecar stdout closed");
        });

        // Ruby's own logging. Surfacing it as our log is what makes a failure in the
        // pipeline diagnosable from the app's console rather than invisible.
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                log::info!("ruby: {line}");
            }
        });

        let sidecar = Self {
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            next_id: AtomicU64::new(1),
            pending,
        };

        let hello = sidecar.request(json!({
            "cmd": "hello",
            "config": config,
            "preview_dir": preview_dir,
        }))?;
        log::info!(
            "sidecar ready: ruby {}, libvips {}",
            hello.get("ruby").and_then(Value::as_str).unwrap_or("?"),
            hello.get("vips").and_then(Value::as_str).unwrap_or("?")
        );

        Ok(sidecar)
    }

    pub fn overlay(&self, source: &str) -> Result<Value, String> {
        self.request(json!({ "cmd": "overlay", "source": source }))
    }

    /// Hands a running child new settings, so changing an overlay does not mean
    /// restarting Ruby. `configure` is the same handler as `hello` on the other
    /// side, which rebuilds Config, Overlay and Preview from scratch.
    ///
    /// Needs no locking of its own: the child handles one frame at a time, so
    /// this simply queues behind any photo already in flight - the same property
    /// the module comment above relies on for deduplication.
    pub fn reconfigure(&self, config: Value, preview_dir: &str) -> Result<Value, String> {
        self.request(json!({
            "cmd": "configure",
            "config": config,
            "preview_dir": preview_dir,
        }))
    }

    /// Sends one frame and blocks until its response arrives. Callers run this off
    /// the main thread; the UI is driven by events, not by waiting on this.
    fn request(&self, mut frame: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        frame["id"] = json!(id);

        let (tx, rx): (Sender<Value>, Receiver<Value>) = channel();
        self.pending.lock().unwrap().insert(id, tx);

        let line = format!("{}\n", serde_json::to_string(&frame).map_err(|e| e.to_string())?);
        {
            let mut stdin = self.stdin.lock().unwrap();
            stdin
                .write_all(line.as_bytes())
                .and_then(|_| stdin.flush())
                .map_err(|e| {
                    self.pending.lock().unwrap().remove(&id);
                    format!("the sidecar is not accepting input: {e}")
                })?;
        }

        let response = rx.recv_timeout(REQUEST_TIMEOUT).map_err(|_| {
            self.pending.lock().unwrap().remove(&id);
            format!("the sidecar did not answer within {}s", REQUEST_TIMEOUT.as_secs())
        })?;

        if response.get("ok").and_then(Value::as_bool) == Some(true) {
            Ok(response)
        } else {
            let error = response.get("error");
            Err(format!(
                "{}: {}",
                error.and_then(|e| e.get("class")).and_then(Value::as_str).unwrap_or("error"),
                error
                    .and_then(|e| e.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("the sidecar reported a failure"),
            ))
        }
    }

    /// Closing stdin is the documented shutdown: `Server#run` reads to EOF, so the
    /// child finishes whatever it is doing and exits on its own.
    pub fn shutdown(&self) {
        if let Ok(mut child) = self.child.lock() {
            drop(self.stdin.lock().map(|s| drop(s)));
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
