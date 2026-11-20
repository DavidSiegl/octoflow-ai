//! One ACP agent as a child process: JSON-RPC 2.0 over its stdin and stdout, one message a line
//! ([ACP's transport](https://agentclientprotocol.com/protocol/transports)). A thread of its own reads what the agent
//! sends: the replies to the requests made of it, its requests for a permission, which wait for an answer
//! (Connection::answer), and the session updates, which nothing reads yet. Every other request of the agent's is
//! refused, since the engine offers it neither files nor terminals.

use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use nix::sys::signal::Signal;

use crate::agents::Activity;
use crate::error::OrchestratorError;
use crate::harness::Tokens;
use crate::host::{kill_group, own_group};

/// JSON-RPC's code for a method the receiver does not have.
const METHOD_NOT_FOUND: i64 = -32601;

/// Why a request of the agent's got no result.
#[derive(Debug)]
pub(super) enum Failure {
    /// The agent answered with this error.
    Refused(String),
    /// It exited, or its output could not be read, first.
    Exited,
    /// No answer came within the time given.
    TimedOut,
    /// The request could not be written to it, and it did not exit within the time given.
    Unwritten(OrchestratorError),
}

/// A running ACP agent: requests to it, and what it is doing. Dropping it kills the agent.
pub(super) struct Connection {
    /// The agent's process, leading a process group of its own; None once it and its group are killed and it is
    /// reaped.
    child: Mutex<Option<Child>>,
    /// Its stdin, where every message to it goes, one at a time.
    input: Arc<Mutex<ChildStdin>>,
    /// What the reading thread has seen, and the id of the next request.
    shared: Arc<Shared>,
}

/// What the reading thread knows, with the condition variable it signals on each change.
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

/// The agent as its messages so far show it.
#[derive(Default)]
struct State {
    /// The id the next request takes.
    next_id: u64,
    /// True once its output ended, or it was killed: it exited, or closed its stdout, which it cannot be reached
    /// without.
    exited: bool,
    /// The replies to Connection::request's requests, by id: None until one comes.
    replies: HashMap<u64, Option<Result<Value, String>>>,
    /// The prompts sent and not yet answered, by request id: when each was sent, in seconds since the epoch, and the
    /// session it was sent in.
    prompts: HashMap<u64, (f64, String)>,
    /// Its requests for a permission not yet answered, oldest first: each one's id and params, as it sent them.
    permissions: VecDeque<(Value, Value)>,
    /// The error it answered a prompt with, until Connection::failure takes it.
    failure: Option<String>,
}

impl Connection {
    /// Start the agent as program, found on env's PATH, with args and the environment env alone, working in cwd, in a
    /// process group of its own as the engine starts every child (crate::host::own_group); its
    /// stderr is appended to the file at stderr, and the usage of each prompt it answers to the file at usage
    /// (usage_line). Errors: the program does not start, or the stderr file cannot be opened.
    pub(super) fn spawn(
        program: &str,
        args: &[String],
        env: impl Iterator<Item = (OsString, OsString)>,
        cwd: &str,
        stderr: &str,
        usage: String,
    ) -> Result<Connection, OrchestratorError> {
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(stderr)
            .map_err(|e| OrchestratorError(format!("cannot open {stderr}: {e}")))?;
        let mut command = Command::new(program);
        command
            .args(args)
            .env_clear()
            .envs(env)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(log);
        own_group(&mut command);
        let mut child = command.spawn().map_err(|e| OrchestratorError(format!("cannot start {program}: {e}")))?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            unreachable!("stdin and stdout are piped")
        };
        let input = Arc::new(Mutex::new(stdin));
        let shared = Arc::new(Shared { state: Mutex::new(State::default()), changed: Condvar::new() });
        let (reader_input, reader_shared) = (input.clone(), shared.clone());
        thread::spawn(move || read(BufReader::new(stdout), &reader_input, &reader_shared, &usage));
        Ok(Connection { child: Mutex::new(Some(child)), input, shared })
    }

    /// Ask the agent method with params and wait at most timeout for its result.
    pub(super) fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, Failure> {
        let id = {
            let mut state = self.shared.lock();
            let id = state.take_id();
            state.replies.insert(id, None);
            id
        };
        // A write that fails is waited out too: an agent that has exited closes its stdin before its output ends, and
        // is reported as exited rather than as unwritable once the reading thread sees that end.
        let unwritten = self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})).err();
        let deadline = Instant::now() + timeout;
        let mut state = self.shared.lock();
        loop {
            if let Some(Some(reply)) = state.replies.get(&id) {
                let reply = reply.clone();
                state.replies.remove(&id);
                return reply.map_err(Failure::Refused);
            }
            if state.exited {
                state.replies.remove(&id);
                return Err(Failure::Exited);
            }
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                state.replies.remove(&id);
                return Err(unwritten.map_or(Failure::TimedOut, Failure::Unwritten));
            };
            state = self.shared.wait(state, left);
        }
    }

    /// Send text as a prompt in session, without waiting for the agent's answer, which ends the prompt (activity).
    /// Errors: it cannot be written to the agent.
    pub(super) fn prompt(&self, session: &str, text: &str) -> Result<(), OrchestratorError> {
        let id = {
            let mut state = self.shared.lock();
            let id = state.take_id();
            state.prompts.insert(id, (now(), session.to_string()));
            id
        };
        let params = json!({"sessionId": session, "prompt": [{"type": "text", "text": text}]});
        let sent = self.send(&json!({"jsonrpc": "2.0", "id": id, "method": "session/prompt", "params": params}));
        if sent.is_err() {
            self.shared.lock().prompts.remove(&id);
        }
        sent
    }

    /// What the agent is doing: Blocked while a request for a permission waits, else Working while a prompt does,
    /// else Idle; None once it has exited.
    pub(super) fn activity(&self) -> Option<Activity> {
        self.shared.lock().activity()
    }

    /// Wait at most seconds until the agent waits on no permission, or has exited; false when the time runs out
    /// first.
    pub(super) fn wait_unblocked(&self, seconds: u64) -> bool {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        let mut state = self.shared.lock();
        loop {
            if state.exited || state.permissions.is_empty() {
                return true;
            }
            let Some(left) = deadline.checked_duration_since(Instant::now()) else { return false };
            state = self.shared.wait(state, left);
        }
    }

    /// Answer the agent's oldest request for a permission with the option of option_id, one of those the request
    /// offers. Returns false when no request waits. Errors: the answer cannot be written to the agent.
    pub(super) fn answer(&self, option_id: &str) -> Result<bool, OrchestratorError> {
        let Some((id, _)) = self.shared.lock().permissions.pop_front() else { return Ok(false) };
        self.shared.changed.notify_all();
        let outcome = json!({"outcome": {"outcome": "selected", "optionId": option_id}});
        self.send(&json!({"jsonrpc": "2.0", "id": id, "result": outcome}))?;
        Ok(true)
    }

    /// The error the agent answered a prompt with, once: None when it answered none with one, or it was taken before.
    pub(super) fn failure(&self) -> Option<String> {
        self.shared.lock().failure.take()
    }

    /// Kill the agent and every process of its group, and reap it; nothing to do once done. From then on it is
    /// exited (activity), whether or not its output has ended yet.
    pub(super) fn kill(&self) {
        let Some(mut child) = self.child.lock().unwrap_or_else(|e| e.into_inner()).take() else { return };
        // A group that cannot be signalled has no process left; the wait reaps the agent all the same.
        let _ = kill_group(&child, Signal::SIGKILL);
        let _ = child.wait();
        self.shared.lock().exited = true;
        self.shared.changed.notify_all();
    }

    /// Write message to the agent, as one line.
    fn send(&self, message: &Value) -> Result<(), OrchestratorError> {
        send(&self.input, message)
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.kill();
    }
}

impl Shared {
    /// The state, whatever thread panicked holding it: every change to it is whole.
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Wait at most timeout for a change to state.
    fn wait<'s>(&'s self, state: MutexGuard<'s, State>, timeout: Duration) -> MutexGuard<'s, State> {
        self.changed.wait_timeout(state, timeout).map(|(state, _)| state).unwrap_or_else(|e| e.into_inner().0)
    }
}

impl State {
    /// A request id no other request took.
    fn take_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// As Connection::activity.
    fn activity(&self) -> Option<Activity> {
        if self.exited {
            None
        } else if !self.permissions.is_empty() {
            Some(Activity::Blocked)
        } else if !self.prompts.is_empty() {
            Some(Activity::Working)
        } else {
            Some(Activity::Idle)
        }
    }
}

/// Write message to input as one line. Errors: the agent's stdin is closed, as when it has exited.
fn send(input: &Mutex<ChildStdin>, message: &Value) -> Result<(), OrchestratorError> {
    let mut input = input.lock().unwrap_or_else(|e| e.into_inner());
    let line = message.to_string() + "\n";
    input
        .write_all(line.as_bytes())
        .and_then(|()| input.flush())
        .map_err(|e| OrchestratorError(format!("cannot write to the agent: {e}")))
}

/// The reading thread: take each of the agent's messages from output into shared until it ends, answering input's
/// agent where it asks for what the engine does not offer, and appending each answered prompt's usage to the file at
/// usage. A line that is not a JSON object is passed over.
fn read(output: impl BufRead, input: &Mutex<ChildStdin>, shared: &Shared, usage: &str) {
    for line in output.lines() {
        let Ok(line) = line else { break };
        let Ok(Value::Object(message)) = serde_json::from_str::<Value>(&line) else { continue };
        let id = message.get("id").cloned();
        match (message.get("method").and_then(Value::as_str), id) {
            (Some("session/request_permission"), Some(id)) => {
                let params = message.get("params").cloned().unwrap_or(Value::Null);
                shared.lock().permissions.push_back((id, params));
            }
            (Some(_), Some(id)) => {
                let refusal = json!({"code": METHOD_NOT_FOUND, "message": "the client offers no such method"});
                // An agent that cannot be written to has exited, which the end of its output tells.
                let _ = send(input, &json!({"jsonrpc": "2.0", "id": id, "error": refusal}));
            }
            // A notification: a session update.
            (Some(_), None) => continue,
            (None, Some(id)) => replied(&mut shared.lock(), id.as_u64(), &message, usage),
            (None, None) => continue,
        }
        shared.changed.notify_all();
    }
    shared.lock().exited = true;
    shared.changed.notify_all();
}

/// Take the agent's reply message to the request of id into state: a prompt's ends it, its usage appended to the
/// file at usage and its error kept as the agent's failure; a request's is handed to the Connection::request waiting
/// for it. A reply to no request still waiting is passed over.
fn replied(state: &mut State, id: Option<u64>, message: &serde_json::Map<String, Value>, usage: &str) {
    let Some(id) = id else { return };
    let reply = match (message.get("result"), message.get("error")) {
        (_, Some(error)) => Err(error.get("message").and_then(Value::as_str).unwrap_or("an error").to_string()),
        (Some(result), None) => Ok(result.clone()),
        (None, None) => Ok(Value::Null),
    };
    if let Some((sent, session)) = state.prompts.remove(&id) {
        match reply {
            Ok(result) => {
                if let Some(line) = usage_line(&session, sent, &result) {
                    // A usage not recorded costs only its turn's token count.
                    let _ = OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(usage)
                        .and_then(|mut f: File| f.write_all(line.as_bytes()));
                }
            }
            Err(e) => state.failure = Some(e),
        }
        return;
    }
    if let Some(waiting) = state.replies.get_mut(&id) {
        *waiting = Some(reply);
    }
}

/// The line of the usage file (super::session_usage) for a prompt sent in session at sent, in seconds since the epoch,
/// from the result the agent answered it with: None when that has no usage, or one without its input or output
/// tokens. A cache count it leaves out is 0. The time is the prompt's sending, not its
/// answer's, so the usage counts to the turn that sent it (crate::summary::turn_tokens), even when the agent answers
/// after it has written the turn's handoff file.
fn usage_line(session: &str, sent: f64, result: &Value) -> Option<String> {
    let usage = result.get("usage")?.as_object()?;
    let count = |key: &str| usage.get(key).and_then(Value::as_i64);
    let tokens = Tokens {
        input: count("inputTokens")?,
        output: count("outputTokens")?,
        cache_read: count("cachedReadTokens").unwrap_or(0),
        cache_write: count("cachedWriteTokens").unwrap_or(0),
    };
    Some(json!({"session": session, "when": sent, "tokens": tokens.to_json()}).to_string() + "\n")
}

/// The time, in seconds since the epoch.
fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}
