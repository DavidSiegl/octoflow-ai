//! A run's socket: the Unix socket its orchestrator listens on while the run runs over ACP, where a client (serve,
//! for the app) answers an agent's permission or stops the run. What the agents do is not on it but in their
//! journals (super::journal). The protocol is design-03-run-socket.md's, in `.scratch/herdr-gui-decoupling/`.
//!
//! The socket is `sock/run.sock` in the run directory, the directory mode 0700 and the socket 0600; where that path
//! does not fit a socket address (SUN_PATH_MAX), it is `<run key>.sock` in `/tmp/octoflow-<uid>/`, a 0700 directory
//! the user owns. Not `$XDG_RUNTIME_DIR`: it may be unset, and systemd removes it at logout under a run that goes on.
//! The path bound is written to SOCKET_RECORD in the run directory, one line, so that a client reads one file to find
//! it. Only a peer of the orchestrator's own uid is served.
//!
//! A connection carries newline-delimited JSON, one object a line, at most MAX_LINE bytes: first the client's
//! `{"hello": {"protocol": N}}`, answered `{"hello": {"protocol", "engine", "run"}}` (on another N, with an `error`
//! of code `protocol` beside it, and closed); then commands, each answered with one line, `{"ok": {...}}` or
//! `{"error": {"code", "message"}}`, in order:
//!
//! - `{"answer": {"role", "request", "option"}}`: answer a permission the role's agent waits on (request being the
//!   journal's number for it) with one of its options. Errors: `no_agent`, `no_request`, `already_answered` (with
//!   `option` and `by` of the answer that won), `bad_option`.
//! - `{"message": {"role", "text"}}`: send the role's agent text, the human's message, as its next prompt, while the
//!   run waits on the human's message to it (its journal's `waiting`). Errors: `no_agent`, `not_waiting` (it does not
//!   wait, a prompt of it is pending, which ACP allows no second beside, or the agent was stopped).
//! - `{"stop": {}}`: answered `ok` at once; then the run stops, as at a Ctrl-C, its agents' permissions cancelled and
//!   their prompts too (Commands::stop).
//!
//! Any command may get `bad_request` (unknown, or a field missing or ill-typed); a line over MAX_LINE gets `too_long`
//! and closes the connection. A connection idle for IDLE is closed.

use std::fs::{DirBuilder, Permissions};
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::journal::PROTOCOL;
use super::CommandError;
use crate::error::OrchestratorError;

/// The file in the run directory that names the socket its orchestrator listens on, one line.
pub const SOCKET_RECORD: &str = "socket";

/// The longest path a socket address takes, its terminating NUL left out: sockaddr_un's sun_path is 108 bytes on
/// Linux and 104 on macOS and the BSDs.
#[cfg(target_os = "linux")]
const SUN_PATH_MAX: usize = 107;
#[cfg(not(target_os = "linux"))]
const SUN_PATH_MAX: usize = 103;

/// The longest line either end sends, in bytes, its newline left out: serve's limit on a POST's body.
const MAX_LINE: usize = crate::serve::MAX_BODY_BYTES;

/// How long a connection may send nothing before it is closed.
const IDLE: Duration = Duration::from_secs(30);

/// How often a waiting thread of the socket looks whether the socket is closing.
const TICK: Duration = Duration::from_millis(100);

/// What a run's socket asks of its orchestrator.
pub(super) trait Commands: Send + Sync {
    /// Answer request, the permission the role's agent's journal numbered so, with option, as AcpAgents::answer.
    fn answer(&self, role: &str, request: u64, option: &str) -> Result<(), CommandError>;
    /// Send the role's agent text as the human's message, as AcpAgents::message.
    fn message(&self, role: &str, text: &str) -> Result<(), CommandError>;
    /// Stop the run, as AcpAgents::stop.
    fn stop(&self);
}

/// A run's socket, listening, its connections served each on a thread of its own, until it is dropped: then the
/// socket and SOCKET_RECORD are removed, and no connection is served any more.
pub(super) struct Listener {
    /// The socket's path.
    path: String,
    /// SOCKET_RECORD's path.
    record: String,
    /// True once the socket is closing: its threads end at their next look.
    closing: Arc<AtomicBool>,
    accepting: Option<JoinHandle<()>>,
}

impl Listener {
    /// Listen on run_dir's socket for the run run_id, whose key is key, as the module's comment says, answering by
    /// commands, and record its path. A socket file left by an orchestrator of the run before, which was killed, is
    /// replaced. Errors: the socket's directory cannot be made or is not the user's own, or the socket cannot be
    /// bound or recorded.
    pub(super) fn bind(
        run_dir: &str,
        run_id: &str,
        key: &str,
        commands: Arc<dyn Commands>,
    ) -> Result<Listener, OrchestratorError> {
        let uid = nix::unistd::getuid().as_raw();
        let path = socket_path(run_dir, key, uid);
        let dir = path.rsplit_once('/').map_or("/", |(dir, _)| dir).to_string();
        private_dir(&dir, uid)?;
        match std::fs::remove_file(&path) {
            Err(e) if e.kind() != ErrorKind::NotFound => {
                return Err(OrchestratorError(format!("cannot remove the old socket {path}: {e}")))
            }
            _ => {}
        }
        let listener = UnixListener::bind(&path).map_err(|e| OrchestratorError(format!("cannot bind {path}: {e}")))?;
        let cannot = |what: &str, e: std::io::Error| OrchestratorError(format!("cannot {what} {path}: {e}"));
        std::fs::set_permissions(&path, Permissions::from_mode(0o600)).map_err(|e| cannot("restrict", e))?;
        listener.set_nonblocking(true).map_err(|e| cannot("listen on", e))?;
        let record = format!("{run_dir}/{SOCKET_RECORD}");
        std::fs::write(&record, format!("{path}\n"))
            .map_err(|e| OrchestratorError(format!("cannot write {record}: {e}")))?;
        let closing = Arc::new(AtomicBool::new(false));
        let hello = json!({"protocol": PROTOCOL, "engine": env!("CARGO_PKG_VERSION"), "run": run_id});
        let accepting = {
            let closing = closing.clone();
            thread::spawn(move || accept(&listener, uid, &hello, &commands, &closing))
        };
        Ok(Listener { path, record, closing, accepting: Some(accepting) })
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.closing.store(true, Ordering::Relaxed);
        if let Some(accepting) = self.accepting.take() {
            // A thread that panicked has stopped accepting all the same.
            let _ = accepting.join();
        }
        // What is gone already needs no removing; what cannot be removed is replaced by the run's next bind.
        let _ = std::fs::remove_file(&self.path);
        if std::fs::read_to_string(&self.record).is_ok_and(|named| named.trim_end() == self.path) {
            let _ = std::fs::remove_file(&self.record);
        }
    }
}

/// The socket's path for the run directory run_dir, of the run whose key is key, of the user uid, as the module's
/// comment says.
fn socket_path(run_dir: &str, key: &str, uid: u32) -> String {
    let inside = format!("{run_dir}/sock/run.sock");
    if inside.len() <= SUN_PATH_MAX {
        inside
    } else {
        format!("/tmp/octoflow-{uid}/{key}.sock")
    }
}

/// Make dir a directory only uid reaches (0700), or make sure the one there is: a directory, not a link, owned by
/// uid. Errors: it cannot be made or restricted, or is not such a directory.
fn private_dir(dir: &str, uid: u32) -> Result<(), OrchestratorError> {
    match DirBuilder::new().mode(0o700).create(dir) {
        Err(e) if e.kind() != ErrorKind::AlreadyExists => {
            return Err(OrchestratorError(format!("cannot make {dir}: {e}")))
        }
        _ => {}
    }
    let meta = std::fs::symlink_metadata(dir).map_err(|e| OrchestratorError(format!("cannot look at {dir}: {e}")))?;
    if !meta.is_dir() || meta.uid() != uid {
        return Err(OrchestratorError(format!("{dir} is not a directory of user {uid}'s own")));
    }
    if meta.mode() & 0o777 != 0o700 {
        std::fs::set_permissions(dir, Permissions::from_mode(0o700))
            .map_err(|e| OrchestratorError(format!("cannot restrict {dir}: {e}")))?;
    }
    Ok(())
}

/// The listening thread: serve each connection to listener from a peer of uid on a thread of its own (serve), until
/// closing. A connection that fails before it is served is dropped.
fn accept(listener: &UnixListener, uid: u32, hello: &Value, commands: &Arc<dyn Commands>, closing: &Arc<AtomicBool>) {
    while !closing.load(Ordering::Relaxed) {
        let stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                thread::sleep(TICK);
                continue;
            }
            // As ECONNABORTED: a client that left before it was accepted.
            Err(_) => continue,
        };
        if peer_uid(&stream) != Some(uid) {
            continue;
        }
        let (hello, commands, closing) = (hello.clone(), commands.clone(), closing.clone());
        thread::spawn(move || serve(stream, &hello, &*commands, &closing));
    }
}

/// The uid of stream's peer; None when it cannot be told.
fn peer_uid(stream: &UnixStream) -> Option<u32> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        use nix::sys::socket::{getsockopt, sockopt::PeerCredentials};
        getsockopt(stream, PeerCredentials).ok().map(|c| c.uid())
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        nix::unistd::getpeereid(stream).ok().map(|(uid, _)| uid.as_raw())
    }
}

/// One connection: the handshake, answered with hello, then each command, done by commands, until the client closes
/// it, it fails, it stays idle for IDLE, or closing. A `stop` is answered before it is done.
fn serve(stream: UnixStream, hello: &Value, commands: &dyn Commands, closing: &AtomicBool) {
    // Short reads, so that closing is looked at between them; a write that cannot finish in IDLE gives up.
    if stream.set_nonblocking(false).is_err()
        || stream.set_read_timeout(Some(TICK)).is_err()
        || stream.set_write_timeout(Some(IDLE)).is_err()
    {
        return;
    }
    let Ok(mut out) = stream.try_clone() else { return };
    let mut lines = BufReader::new(stream);
    let Some(first) = next_request(&mut lines, &mut out, closing) else { return };
    let (greeted, accepted) = greeting(&first, hello);
    if reply(&mut out, &greeted).is_err() || !accepted {
        return;
    }
    while let Some(request) = next_request(&mut lines, &mut out, closing) {
        let stop = request.get("stop").is_some_and(Value::is_object);
        let answer = if stop { json!({"ok": {}}) } else { command(&request, commands) };
        if reply(&mut out, &answer).is_err() {
            return;
        }
        if stop {
            commands.stop();
        }
    }
}

/// The next request from lines, as JSON (null for a line that is none), waited for as read_line waits; None once the
/// connection ends. A line over MAX_LINE is answered `too_long` on out, and ends it.
fn next_request(lines: &mut BufReader<UnixStream>, out: &mut UnixStream, closing: &AtomicBool) -> Option<Value> {
    match read_line(lines, closing) {
        Ok(line) => Some(serde_json::from_slice::<Value>(&line).unwrap_or(Value::Null)),
        Err(Unread::TooLong) => {
            // The connection ends either way; a client that left misses only this reply.
            let _ = reply(out, &error("too_long", &format!("a line is at most {MAX_LINE} bytes")));
            None
        }
        Err(Unread::Ended) => None,
    }
}

/// The reply to request, a client's first, and whether the connection goes on: hello for a `hello` of PROTOCOL, else
/// hello with an error of code `protocol`, after which it does not.
fn greeting(request: &Value, hello: &Value) -> (Value, bool) {
    let protocol = request.pointer("/hello/protocol").and_then(Value::as_u64);
    let mut answer = json!({"hello": hello});
    if protocol == Some(PROTOCOL) {
        return (answer, true);
    }
    let theirs = protocol.map_or_else(|| "none".to_string(), |p| p.to_string());
    answer["error"] = json!({"code": "protocol", "message": format!(
        "this run speaks protocol {PROTOCOL}, not {theirs}; it is from another version of octoflow"
    )});
    (answer, false)
}

/// The reply to request, a command other than `stop`, done by commands.
fn command(request: &Value, commands: &dyn Commands) -> Value {
    let field = |command: &Value, key: &str| command.get(key).and_then(Value::as_str).map(str::to_string);
    let (role, done) = if let Some(answer) = request.get("answer") {
        let (Some(role), Some(number), Some(option)) =
            (field(answer, "role"), answer.get("request").and_then(Value::as_u64), field(answer, "option"))
        else {
            return error("bad_request", "answer takes a role, a request number and an option");
        };
        match commands.answer(&role, number, &option) {
            Err(CommandError::NoRequest) => {
                return error("no_request", &format!("the {role} agent's session has no permission request {number}"))
            }
            Err(CommandError::AlreadyAnswered { option, by }) => {
                let mut e = error("already_answered", &format!("request {number} was answered {option} by the {by}"));
                e["error"]["option"] = option.into();
                e["error"]["by"] = by.into();
                return e;
            }
            Err(CommandError::BadOption) => {
                return error("bad_option", &format!("request {number} offers no option {option}"))
            }
            done => (role, done),
        }
    } else if let Some(message) = request.get("message") {
        let (Some(role), Some(text)) = (field(message, "role"), field(message, "text")) else {
            return error("bad_request", "message takes a role and a text");
        };
        let done = commands.message(&role, &text);
        (role, done)
    } else {
        return error("bad_request", "the command is none of answer, message and stop");
    };
    match done {
        Ok(()) => json!({"ok": {}}),
        Err(CommandError::NoAgent) => error("no_agent", &format!("the {role} role has no agent running")),
        Err(CommandError::NotWaiting) => error(
            "not_waiting",
            &format!("the {role} agent is not waiting for a message: it is working or stopped, or the run does not wait on you"),
        ),
        Err(CommandError::Unwritten(e)) => error("unwritten", &e.0),
        // Only an answer is refused so, and it is answered above.
        Err(e @ (CommandError::NoRequest | CommandError::AlreadyAnswered { .. } | CommandError::BadOption)) => {
            error("bad_request", &format!("{e:?}"))
        }
    }
}

/// An error reply of code, with message for the human.
fn error(code: &str, message: &str) -> Value {
    json!({"error": {"code": code, "message": message}})
}

/// Why a connection gave no line.
enum Unread {
    /// It sent a line over MAX_LINE.
    TooLong,
    /// It closed, failed, stayed idle for IDLE, or the socket is closing.
    Ended,
}

/// The next line from lines, its newline left out, waiting at most IDLE for it and while not closing; lines must
/// time out its reads (a read timeout under IDLE).
fn read_line(lines: &mut BufReader<UnixStream>, closing: &AtomicBool) -> Result<Vec<u8>, Unread> {
    let mut line = vec![];
    let start = Instant::now();
    loop {
        let limit = (MAX_LINE + 2).saturating_sub(line.len()) as u64;
        // read_until keeps what it read before a timeout in line, so a read that times out loses nothing.
        match lines.by_ref().take(limit).read_until(b'\n', &mut line) {
            Ok(0) => return Err(Unread::Ended),
            Ok(_) if line.ends_with(b"\n") => {
                line.pop();
                return if line.len() > MAX_LINE { Err(Unread::TooLong) } else { Ok(line) };
            }
            Ok(_) if line.len() > MAX_LINE => return Err(Unread::TooLong),
            Ok(_) => {}
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => {}
            Err(_) => return Err(Unread::Ended),
        }
        if closing.load(Ordering::Relaxed) || start.elapsed() >= IDLE {
            return Err(Unread::Ended);
        }
    }
}

/// Send answer to out, one line. Errors: it cannot be written.
fn reply(out: &mut UnixStream, answer: &Value) -> std::io::Result<()> {
    out.write_all(format!("{answer}\n").as_bytes())
}

/// Why ask got no `ok`.
#[derive(Debug, PartialEq)]
pub enum SocketError {
    /// No socket is recorded in the run directory, or it cannot be connected to or talked with: the run is not
    /// running over ACP, or its orchestrator is gone. The text says which.
    Unreachable(String),
    /// The run speaks another protocol: it is from another version. hello is what it said of itself
    /// (`{"protocol", "engine", "run"}`).
    Protocol { hello: Value },
    /// The run refused the command, with the error's code and message as the module's comment lists them.
    Refused { code: String, message: String },
}

impl std::fmt::Display for SocketError {
    /// What went wrong, for a human: the Unreachable text, the version the run is from, or the refusal's message and
    /// code.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SocketError::Unreachable(why) => f.write_str(why),
            SocketError::Protocol { hello } => {
                let engine = hello.get("engine").and_then(Value::as_str).unwrap_or("another release");
                write!(f, "the run is from octoflow {engine}, which speaks another protocol")
            }
            SocketError::Refused { code, message } => write!(f, "{message} ({code})"),
        }
    }
}

/// Send command, one of the module's comment's commands, to the socket of the run whose directory is run_dir, and
/// return its `ok` reply's object. The connection is closed after it. Errors: SocketError's, as it says.
pub fn ask(run_dir: &str, command: &Value) -> Result<Value, SocketError> {
    let unreachable = |what: &str, e: &dyn std::fmt::Display| SocketError::Unreachable(format!("{what}: {e}"));
    let record = format!("{run_dir}/{SOCKET_RECORD}");
    let path = std::fs::read_to_string(&record).map_err(|e| unreachable(&format!("cannot read {record}"), &e))?;
    let path = path.trim_end();
    let stream = UnixStream::connect(path).map_err(|e| unreachable(&format!("cannot connect to {path}"), &e))?;
    stream.set_read_timeout(Some(IDLE)).map_err(|e| unreachable("cannot set a timeout", &e))?;
    let mut out = stream.try_clone().map_err(|e| unreachable("cannot use the socket", &e))?;
    let mut lines = BufReader::new(stream);
    let mut exchange = |request: &Value| -> Result<Value, SocketError> {
        reply(&mut out, request).map_err(|e| unreachable(&format!("cannot write to {path}"), &e))?;
        let mut line = String::new();
        match lines.by_ref().take(MAX_LINE as u64 + 2).read_line(&mut line) {
            Ok(0) => Err(SocketError::Unreachable(format!("{path} closed the connection"))),
            Ok(_) => serde_json::from_str(&line).map_err(|e| unreachable(&format!("{path} answered no JSON"), &e)),
            Err(e) => Err(unreachable(&format!("cannot read from {path}"), &e)),
        }
    };
    let greeted = exchange(&json!({"hello": {"protocol": PROTOCOL}}))?;
    if greeted.pointer("/error/code").and_then(Value::as_str) == Some("protocol") {
        return Err(SocketError::Protocol { hello: greeted.get("hello").cloned().unwrap_or(Value::Null) });
    }
    let answer = exchange(command)?;
    if let Some(ok) = answer.get("ok") {
        return Ok(ok.clone());
    }
    let field = |key: &str| answer.pointer(&format!("/error/{key}")).and_then(Value::as_str).unwrap_or("").to_string();
    Err(SocketError::Refused { code: field("code"), message: field("message") })
}

#[cfg(test)]
mod tests;
