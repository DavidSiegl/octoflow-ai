//! One ACP agent as a child process: JSON-RPC 2.0 over its stdin and stdout, one message a line
//! ([ACP's transport](https://agentclientprotocol.com/protocol/transports)). A thread of its own reads what the agent
//! sends: the replies to the requests made of it, its requests for a permission, which wait for an answer
//! (Connection::answer), and the session updates. What it does goes to its role's journal (super::journal). Every
//! other request of the agent's is refused, since the engine offers it neither files nor terminals.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use nix::sys::signal::Signal;

use super::groups;
use super::journal::Journal;
use super::CommandError;
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

/// Where an agent's connection keeps what it records of the agent, each a path but journal.
pub(super) struct Records<'a> {
    /// The file its stderr is appended to.
    pub stderr: &'a str,
    /// The file the usage of each prompt it answers is appended to (usage_line).
    pub usage: &'a str,
    /// The file its process group is recorded in (groups::record), before it is asked anything.
    pub group: &'a str,
    /// Its role's journal.
    pub journal: Arc<Journal>,
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

/// What the reading thread knows, with the condition variable it signals on each change, and the journal it and the
/// Connection write to.
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    journal: Arc<Journal>,
}

/// A request of the agent's for a permission, not yet answered.
struct Pending {
    /// The number the journal gave it, which an answer names it by.
    number: u64,
    /// Its JSON-RPC id, as the agent sent it.
    id: Value,
    /// The optionId of each option it offers.
    options: Vec<String>,
}

/// The agent as its messages so far show it.
#[derive(Default)]
struct State {
    /// The id the next request takes.
    next_id: u64,
    /// True once its output ended, or it was killed: it exited, or closed its stdout, which it cannot be reached
    /// without.
    exited: bool,
    /// True once the journal has its `exited` line: at its exit, or at the error it answered a prompt with, which
    /// ends it for the run (super::AcpAgents::find), whichever came first.
    exit_journaled: bool,
    /// The replies to Connection::request's requests, by id: None until one comes.
    replies: HashMap<u64, Option<Result<Value, String>>>,
    /// The prompts sent and not yet answered, by request id: when each was sent, in seconds since the epoch, and the
    /// session it was sent in.
    prompts: HashMap<u64, (f64, String)>,
    /// Its requests for a permission not yet answered, oldest first.
    permissions: Vec<Pending>,
    /// Each of its requests for a permission answered, by number: the optionId it was answered with, or `cancelled`,
    /// and by whom, as the journal's `answered` says.
    answered: HashMap<u64, (String, String)>,
    /// The error it answered a prompt with, until Connection::failure takes it.
    failure: Option<String>,
    /// Why the run waits on the human's message to it, as crate::agents::Waiting::name names it, since
    /// Connection::await_human; None again once a prompt is sent, and before.
    waiting: Option<&'static str>,
}

impl Connection {
    /// Start the agent as program, found on env's PATH, with args and the environment env alone, working in cwd, in a
    /// process group of its own as the engine starts every child (crate::host::own_group), recorded where records
    /// says before anything is asked of it, and kept as records says. On Linux it is killed when the thread that
    /// spawns it exits (PR_SET_PDEATHSIG fires at the end of the forking *thread*, prctl(2)), so it must be called on
    /// a thread that lives as long as the orchestrator: the main thread, which drives the Workflow. Errors: the
    /// program does not start, the stderr file cannot be opened, or the group cannot be recorded (the agent is
    /// killed then).
    pub(super) fn spawn(
        program: &str,
        args: &[String],
        env: impl Iterator<Item = (OsString, OsString)>,
        cwd: &str,
        records: Records,
    ) -> Result<Connection, OrchestratorError> {
        let stderr = records.stderr;
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
        #[cfg(target_os = "linux")]
        die_with_parent(&mut command);
        let mut child = command.spawn().map_err(|e| OrchestratorError(format!("cannot start {program}: {e}")))?;
        if let Err(e) = groups::record(records.group, child.id()) {
            // As in kill.
            let _ = kill_group(&child, Signal::SIGKILL);
            let _ = child.wait();
            return Err(e);
        }
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            unreachable!("stdin and stdout are piped")
        };
        let input = Arc::new(Mutex::new(stdin));
        let shared =
            Arc::new(Shared { state: Mutex::new(State::default()), changed: Condvar::new(), journal: records.journal });
        let (reader_input, reader_shared, usage) = (input.clone(), shared.clone(), records.usage.to_string());
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

    /// Send text as the orchestrator's prompt in session, without waiting for the agent's answer, which ends the
    /// prompt (activity). Errors: it cannot be written to the agent.
    pub(super) fn prompt(&self, session: &str, text: &str) -> Result<(), OrchestratorError> {
        let id = self.shared.begin_prompt(&mut self.shared.lock(), session, text, "orchestrator");
        self.send_prompt(id, session, text)
    }

    /// Send text as the human's message in session, its next prompt, as prompt does, while the run waits on the
    /// human (await_human). Errors: NotWaiting when it does not, or a prompt is pending, which ACP allows no second
    /// beside; Unwritten when it cannot be written to the agent.
    pub(super) fn message(&self, session: &str, text: &str) -> Result<(), CommandError> {
        let id = {
            let mut state = self.shared.lock();
            if state.waiting.is_none() || state.exited || !state.prompts.is_empty() {
                return Err(CommandError::NotWaiting);
            }
            self.shared.begin_prompt(&mut state, session, text, "human")
        };
        self.send_prompt(id, session, text).map_err(CommandError::Unwritten)
    }

    /// Take the run as waiting on the human's message to the agent for the reason why, journaled as `waiting`, unless
    /// it is already, or the agent is not idle: it has exited, or a prompt or a permission is pending.
    pub(super) fn await_human(&self, why: &'static str) {
        let mut state = self.shared.lock();
        if state.waiting.is_some() || state.activity() != Some(Activity::Idle) {
            return;
        }
        state.waiting = Some(why);
        self.shared.journal.write("waiting", json!({"why": why}));
    }

    /// Write prompt id, which Shared::begin_prompt began, to the agent: text in session. Errors: it cannot be
    /// written, and the prompt is forgotten.
    fn send_prompt(&self, id: u64, session: &str, text: &str) -> Result<(), OrchestratorError> {
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

    /// Answer the agent's request for a permission that the journal numbered request with option, the optionId of
    /// one of the options it offers, as the human's answer. The first answer to a request wins: it is journaled
    /// before it is sent, and a later one is refused. Errors: NoRequest for no such request of this connection's
    /// agent, AlreadyAnswered for one answered, BadOption for an option the request does not offer, Unwritten when
    /// the answer cannot be written to the agent (it is journaled all the same).
    pub(super) fn answer(&self, request: u64, option: &str) -> Result<(), CommandError> {
        let mut state = self.shared.lock();
        if let Some((option, by)) = state.answered.get(&request) {
            return Err(CommandError::AlreadyAnswered { option: option.clone(), by: by.clone() });
        }
        let Some(n) = state.permissions.iter().position(|p| p.number == request) else {
            return Err(CommandError::NoRequest);
        };
        if !state.permissions[n].options.iter().any(|o| o == option) {
            return Err(CommandError::BadOption);
        }
        let pending = state.permissions.remove(n);
        let outcome = json!({"outcome": "selected", "optionId": option});
        let sent = self.settle(&mut state, &pending, option, "human", outcome);
        drop(state);
        self.shared.changed.notify_all();
        sent.map_err(CommandError::Unwritten)
    }

    /// Stop what the agent is doing, as "Stop all" does: answer each of its requests for a permission `cancelled`,
    /// and cancel each prompt it has not answered yet (ACP's `session/cancel`), which it then answers with the stop
    /// reason `cancelled`. An agent that cannot be written to has exited, and has nothing left to stop.
    pub(super) fn stop(&self) {
        let mut state = self.shared.lock();
        for pending in std::mem::take(&mut state.permissions) {
            let _ = self.settle(&mut state, &pending, "cancelled", "stop", json!({"outcome": "cancelled"}));
        }
        let mut sessions: Vec<String> = state.prompts.values().map(|(_, session)| session.clone()).collect();
        sessions.sort();
        sessions.dedup();
        for session in sessions {
            let _ = self.send(&json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": session}}));
        }
        drop(state);
        self.shared.changed.notify_all();
    }

    /// Record the answer to pending, taken out of state's permissions, in state and the journal as option given by
    /// by, then send the agent outcome as its reply. Called holding state's lock, so that the first answer is the
    /// one journaled and sent. Errors: the reply cannot be written to the agent.
    fn settle(
        &self,
        state: &mut State,
        pending: &Pending,
        option: &str,
        by: &str,
        outcome: Value,
    ) -> Result<(), OrchestratorError> {
        state.answered.insert(pending.number, (option.to_string(), by.to_string()));
        self.shared.journal.write("answered", json!({"request": pending.number, "option": option, "by": by}));
        self.send(&json!({"jsonrpc": "2.0", "id": pending.id, "result": {"outcome": outcome}}))
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
        self.shared.exit();
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

    /// Journal a prompt of text from `orchestrator` or `human` in session, about to be sent, and record it in state,
    /// this Shared's, as pending, the run no longer waiting on the human. Returns its request id.
    fn begin_prompt(&self, state: &mut State, session: &str, text: &str, from: &str) -> u64 {
        self.journal.write("prompt", json!({"text": text, "from": from}));
        state.waiting = None;
        let id = state.take_id();
        state.prompts.insert(id, (now(), session.to_string()));
        id
    }

    /// Take the agent as exited, and journal it so where nothing has yet.
    fn exit(&self) {
        let mut state = self.lock();
        state.exited = true;
        if !std::mem::replace(&mut state.exit_journaled, true) {
            self.journal.write("exited", json!({}));
        }
        drop(state);
        self.changed.notify_all();
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
                let options = params.get("options").and_then(Value::as_array).into_iter().flatten();
                let options = options.filter_map(|o| o.get("optionId").and_then(Value::as_str).map(String::from));
                let options = options.collect();
                // Numbered and journaled under the state's lock, so that no answer can come before it is pending.
                let mut state = shared.lock();
                let number = shared.journal.permission(&params);
                state.permissions.push(Pending { number, id, options });
            }
            (Some(_), Some(id)) => {
                let refusal = json!({"code": METHOD_NOT_FOUND, "message": "the client offers no such method"});
                // An agent that cannot be written to has exited, which the end of its output tells.
                let _ = send(input, &json!({"jsonrpc": "2.0", "id": id, "error": refusal}));
            }
            (Some("session/update"), None) => {
                let update = message.get("params").and_then(|p| p.get("update")).cloned().unwrap_or(Value::Null);
                shared.journal.write("update", json!({"update": update}));
                continue;
            }
            // Another notification, which ACP's client has none of.
            (Some(_), None) => continue,
            (None, Some(id)) => replied(shared, &mut shared.lock(), id.as_u64(), &message, usage),
            (None, None) => continue,
        }
        shared.changed.notify_all();
    }
    shared.exit();
}

/// Take the agent's reply message to the request of id into state, shared's: a prompt's ends it, its usage appended
/// to the file at usage and journaled as `turn_end`, or its error kept as the agent's failure and journaled as
/// `exited`; a request's is handed to the Connection::request waiting for it. A reply to no request still waiting is
/// passed over.
fn replied(shared: &Shared, state: &mut State, id: Option<u64>, message: &serde_json::Map<String, Value>, usage: &str) {
    let Some(id) = id else { return };
    let reply = match (message.get("result"), message.get("error")) {
        (_, Some(error)) => Err(error.get("message").and_then(Value::as_str).unwrap_or("an error").to_string()),
        (Some(result), None) => Ok(result.clone()),
        (None, None) => Ok(Value::Null),
    };
    if let Some((sent, session)) = state.prompts.remove(&id) {
        match reply {
            Ok(result) => {
                let reason = result.get("stopReason").cloned().unwrap_or(Value::Null);
                shared.journal.write("turn_end", json!({"stopReason": reason}));
                if let Some(line) = usage_line(&session, sent, &result) {
                    // A usage not recorded costs only its turn's token count.
                    let _ = OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(usage)
                        .and_then(|mut f: File| f.write_all(line.as_bytes()));
                }
            }
            Err(e) => {
                if !std::mem::replace(&mut state.exit_journaled, true) {
                    shared.journal.write("exited", json!({"error": e}));
                }
                state.failure = Some(e);
            }
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

/// Have command's process killed (SIGKILL) when the thread that spawns it exits (PR_SET_PDEATHSIG), and never start
/// when that thread is gone before the setting took: its parent is no longer the spawning process then.
#[cfg(target_os = "linux")]
fn die_with_parent(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    let parent = nix::unistd::getpid();
    // SAFETY: the closure runs in the child between fork and exec, where only async-signal-safe functions may be
    // called; prctl, getppid and raise are system calls that allocate nothing (signal-safety(7) lists getppid and
    // raise; prctl is a plain system call).
    #[allow(
        unsafe_code,
        reason = "CommandExt::pre_exec is unsafe, and nix has no safe way to set a child's death signal"
    )]
    unsafe {
        command.pre_exec(move || {
            nix::sys::prctl::set_pdeathsig(Signal::SIGKILL).map_err(std::io::Error::from)?;
            if nix::unistd::getppid() != parent {
                nix::sys::signal::raise(Signal::SIGKILL).map_err(std::io::Error::from)?;
            }
            Ok(())
        });
    }
}

/// The time, in seconds since the epoch.
fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}
