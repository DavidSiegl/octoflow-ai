//! serve: the runs of any project, and their agents, over HTTP on 127.0.0.1 for the desktop app. [`serve_command`] is
//! the program's `serve`; [`RunServer`] is the server, on tiny_http, each request answered on a thread of its own;
//! crate::serve::runs is what it serves of a project. docs/architecture.md's "serve" describes the protocol and its
//! security checks.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use nix::sys::signal::{SigSet, Signal};
use tiny_http::{Header, Request, Response};

use crate::acp;
use crate::agents::RunPlaces;
use crate::cli::close::{Outcome, RunClosed, RunStopped, Sweep};
use crate::cli::forget::{self, Left, RunForgotten};
use crate::cli::listing::{RunRecord, RUN_RECORD_VERSION};
use crate::constants::EXIT_ERROR;
use crate::env::ConfigDir;
use crate::error::{os_error, OrchestratorError};
use crate::harness::Adapter;
use crate::herdr::{AgentConsole, ProcessHerdr};
use crate::host::{Host, ProcessHost};
use crate::interrupt::Interrupt;
use close_choice::CloseChoice;
use runs::{plain_name, AgentInput, Event, ProjectRuns, RunReadError, RunWatch};
use serde_json::Value;
use setup::Setup;
use start::{RunRequest, StartWait, Started, START_WAIT};
use url::{parse_qsl, urlsplit};

pub mod close_choice;
#[cfg(test)]
mod fake;
pub mod runs;
pub mod setup;
pub mod start;
pub mod url;
mod workflows;

/// How often an event stream reads its project's runs again.
pub const SERVE_POLL: Duration = Duration::from_secs(1);

/// How long after the save that finishes a run the engine may still be writing the run's files: summary.md and
/// metrics.json (workflow.rs, end_done_run; release writes summary.md). It must cover those writes on a slow machine,
/// whatever SERVE_POLL is; an event stream lists a finished run's files on every poll until its state.json is this old
/// (RunWatch::poll).
pub const FINISHED_RUN_WRITES: Duration = Duration::from_secs(60);

/// The lines of an agent's terminal /agent gives without ?lines=, and the most it gives with one.
pub const AGENT_LINES: (u32, u32) = (200, 2000);

/// The most bytes of a POST's body serve reads; a longer one gets 413.
pub const MAX_BODY_BYTES: usize = 64 * 1024;

/// A project on this machine as Connect reaches it. A run serve starts in it runs detached, its agents over ACP
/// (ProjectRuns::start); the herdr it names is only for the runs started in herdr before, by the CLI or by an app
/// before 0.94.0, whose agents' panes the agent tab still shows.
pub struct Reached {
    /// Reaches the project's files; must stop its commands when the server stops.
    pub host: Arc<dyn Host + Send + Sync>,
    /// The project's absolute directory.
    pub dir: String,
    /// Reaches its herdr runs' agents, each by its AgentAddress.
    pub console: Arc<dyn AgentConsole>,
    /// Tells and closes its herdr runs' agents' places, each from what its run saved, for POST /close.
    pub places: Arc<dyn RunPlaces + Send + Sync>,
}

/// How a RunServer reaches the project a request names: given its directory on this machine, which may start with ~,
/// as the request gives it, and the Interrupt the server raises when it stops, which must stop the Host's commands.
/// Errors: the project cannot be reached. [`project_host`] is the real one.
pub type Connect = dyn Fn(&str, &Interrupt) -> Result<Reached, OrchestratorError> + Send + Sync;

/// The program's `serve`: start a RunServer that also answers origins, its setup marker and workflows in config, its
/// runs started by run_program (None for its own; RunServer::start), print {"url", "token"} as one JSON line on
/// standard output, and serve until SIGINT or SIGTERM; returns 0 then, the server stopped. Returns EXIT_ERROR, with
/// one line on standard error, when no port can be bound, or when the line cannot be printed, as when whoever
/// started serve has stopped reading its standard output: nobody could reach the server then. SIGHUP keeps its
/// default, ending the process.
///
/// Must be called on the main thread before any other thread starts, as crate::interrupt::by_signals must, and
/// instead of it: SIGINT and SIGTERM are blocked in every thread and waited for here, also when the process started
/// with one ignored, which then stops serve too (Linux delivers a blocked signal that
/// is ignored to sigwait; a BSD kernel, macOS's, discards it, so there serve does not stop at an ignored one).
pub fn serve_command(origins: &[String], run_program: Option<String>, config: &ConfigDir) -> i32 {
    let signals: SigSet = [Signal::SIGINT, Signal::SIGTERM].into_iter().collect();
    if let Err(e) = signals.thread_block() {
        eprintln!("error: cannot wait for SIGINT and SIGTERM: {e}");
        return EXIT_ERROR;
    }
    let started = RunServer::start(origins, run_program, Box::new(project_host), Setup::of(config), config.clone());
    serve(started, &mut io::stdout(), &mut io::stderr(), || signals.wait().map(drop).map_err(io::Error::from))
}

/// serve_command's serving, given the started server: print its line on out, wait for until to return, and stop it;
/// the exit status and errors on err as serve_command's, and EXIT_ERROR too when until fails, naming why. The server
/// is stopped when this returns.
fn serve(
    started: io::Result<RunServer>,
    out: &mut dyn Write,
    err: &mut dyn Write,
    until: impl FnOnce() -> io::Result<()>,
) -> i32 {
    let server = match started {
        Ok(server) => server,
        Err(e) => {
            // Nowhere else to say it, if stderr does not take it; the status still tells.
            let _ = writeln!(err, "error: cannot serve on 127.0.0.1: {}", os_error(&e, None));
            return EXIT_ERROR;
        }
    };
    let line = object([("url", Value::from(server.url())), ("token", Value::from(server.token()))]);
    if let Err(e) = writeln!(out, "{line}").and_then(|()| out.flush()) {
        let _ = writeln!(err, "error: cannot print the server's address: {}", os_error(&e, None));
        return EXIT_ERROR;
    }
    match until() {
        Ok(()) => 0,
        Err(e) => {
            let _ = writeln!(err, "error: cannot wait for SIGINT and SIGTERM: {}", os_error(&e, None));
            EXIT_ERROR
        }
    }
}

/// The project in cwd on this machine, which may start with ~: the Host that reaches it, its absolute directory, and
/// this machine's herdr as its herdr runs' console and places. Each command is stopped by stopping. Reading a project
/// sends herdr no command, so a project without herdr is read too. An agent's commands go to the herdr server its run
/// saved (RunState::herdr_server); an agent of a run saved before runs saved it is reached in the one serve's
/// environment picks (HERDR_SOCKET_PATH, else HERDR_SESSION, else the default session). Errors: the directory cannot
/// be entered; the project's own directory has to move from its old name, and that is refused or fails
/// (crate::old_project_dir::move_project_dir, whose line saying it moved goes to serve's standard error).
pub fn project_host(cwd: &str, stopping: &Interrupt) -> Result<Reached, OrchestratorError> {
    let host = ProcessHost::local(stopping);
    let dir = host.resolve_dir(cwd)?;
    let moved = crate::old_project_dir::move_project_dir(&host, &dir, false)?;
    if let Some(moved) = moved {
        // Nowhere else to say it, if stderr does not take it.
        let _ = writeln!(io::stderr(), "{moved}");
    }
    let herdr = Arc::new(ProcessHerdr::new(None, stopping));
    Ok(Reached { host: Arc::new(host), dir, console: herdr.clone(), places: herdr })
}

/// serve's HTTP server, on a free port of 127.0.0.1, with its token, answering until it is dropped. Its endpoints
/// but /setup/checked, /close-choice and the workflow ones each name a project on this machine by ?cwd=, and answer
/// JSON; each is a GET but the fifteen POSTs:
///
/// - /runs: {"version", "runs"}, as `list --json` prints it.
/// - /files?run=ID: {"run", "files"}, the run's files as ProjectRuns::files lists them.
/// - /file?run=ID&name=NAME: {"run", "name", "text"}; 403 for a name outside the run directory or state.json, 404
///   for no such file.
/// - /agent?run=ID&role=ROLE[&lines=N]: {"run", "role", "text"}, the last N lines (AGENT_LINES: 200 by default, at
///   most 2000) of what the run's agent of the role shows (ProjectRuns::agent_text); 404 for no such run or agent, or
///   one that has exited.
/// - POST /agent/input?run=ID&role=ROLE: types into that agent the JSON body's {"text": "..."}, submitted as a prompt,
///   or {"keys": ["esc", ...]}, pressed in order, and answers {"run", "role"}, without waiting for the agent to take
///   it; 400 for a body of another shape, 413 for one over MAX_BODY_BYTES, both before the project is reached, 404 as
///   /agent's, and 502 for a text or key herdr cannot be given (AgentConsole::submit and send_keys say which).
/// - /agent/events?run=ID&role=ROLE[&after=N]: for a run over ACP, {"run", "role", "session_start", "events",
///   "answerable"}, the current session of its agent of the role from its journal (ProjectRuns::conversation): its
///   events after the one numbered N (0 by default), and the seq its session started at; a client that sees
///   session_start change starts the conversation over. answerable is whether the run can be answered now, its
///   orchestrator listening. 400 for an after that is not a whole number, 404 for no such run or a role without a
///   journal (any role of a run in herdr), 409 for a journal of another version of octoflow.
/// - POST /agent/answer?run=ID&role=ROLE: answers the permission request the JSON body's {"request": N, "option":
///   "ID"} names, N being the journal's `request` number and ID one of its options' optionId, through the run's
///   socket (crate::acp::ask); POST /agent/message?run=ID&role=ROLE sends the body's {"text": "..."} as the human's
///   message, the agent's next prompt, while the run waits on the human (the journal's pending `waiting`). Both
///   answer {"run", "role"}; 400 for a body of another shape and 413 as for /agent/input, both before the project is
///   reached; 409 for a run not running or of another version. A refusal of the run's
///   adds "code", the socket's error code: 404 for `no_agent` and `no_request`, 409 for `already_answered` (the
///   first answer won) and `not_waiting`, 400 for `bad_option`.
/// - /setup?cwd=DIR: {"checks", "passed", "adapters"}, the project's setup checks, and the ACP adapters the dialog
///   offers to install, as Setup::checks gives them; 400 for a cwd missing or empty. It does not reach the project as
///   the others do, so that a folder that is missing shows as a failed check, not as 502.
/// - /setup/checked: {"checked"}, whether the setup dialog need not open by itself (Setup::checked); 500 when that
///   cannot be told.
/// - POST /setup/checked: records that it has (Setup::mark), whatever the checks say, and answers {"checked": true};
///   500 when the marker cannot be written.
/// - /setup/adapters: {"adapters"}, the ACP adapters as /setup gives them (Setup::adapters), with no check run, for
///   the dialog to follow an install by.
/// - POST /setup/adapter?agent=NAME: starts an install of the ACP adapter of harness NAME (Setup::install_adapter),
///   which takes minutes, unless one runs, and answers 202 {"installing": true} at once; /setup/adapters tells how
///   it goes. 400 for a NAME that has no adapter (crate::harness::Adapter), 500 for an install that cannot start.
/// - POST /setup/gate?cwd=DIR: sets the project up for `--quality-gate local` (ProjectRuns::set_up_gate) and answers
///   {"files": [{"path", "written"}, ...]}, written false for a file it kept; 400 for a cwd missing, 502 for a project
///   that cannot be reached or a setup that fails, as for a cwd in no git checkout.
/// - /workflows: what a run can be started with, as start::offered_workflows gives it from the configuration
///   directory; 500 when its workflows directory cannot be listed.
/// - /workflow?name=NAME: one workflow, to edit (workflow_file::read_workflow): {"name", "built_in", "path", "text",
///   "hand_written", "definition"}, path and text null for a built-in workflow; 400 for a name missing, 404 for no
///   such workflow, 422 {"error", "path", "text"} for a file that does not load, text null when it cannot be read
///   (the base a delete of it takes), 500 when the directory cannot be listed.
/// - POST /workflow/check: {"problem", "placeholders"} for the JSON body's {"name", "definition"}
///   (workflow_file::check_workflow), problem null when there is none.
/// - POST /workflow/save: saves the body's {"name", "definition", "base"} (workflow_file::save_workflow), base being
///   the file's text the editor started from, null for a new workflow, and answers 201 {"path"}; 422 for a problem,
///   409 when the file is not base, 500 for a write that failed.
/// - POST /workflow/delete: deletes the file of the body's {"name", "base"} (workflow_file::delete_workflow) and
///   answers {"path"}; 400 for a built-in workflow, 404 for no such file, 409 when the file is not base, 500 for a
///   removal that failed. Each of the three workflow POSTs answers 400 for a body of another shape and 413 for one
///   over MAX_BODY_BYTES.
/// - POST /run?cwd=DIR: starts the run the JSON body asks for (start::RunRequest::parse), as a new run id, detached,
///   its agents over ACP (ProjectRuns::start), and waits for it (start::START_WAIT). 201 {"run"} once it has saved its
///   state.json; 422 {"error", "run"} when `run` exited without saving one, error ending with the last lines its log
///   holds; 202 {"run", "listed": false} after 30 seconds of neither. 400 for a body RunRequest::parse refuses and 413
///   as for /agent/input, both before the project is reached; 502 when it cannot be started.
/// - POST /resume?cwd=DIR&run=ID: resumes the desktop app's run ID (ProjectRuns::resume): ends its leftover agents,
///   starts its `resume` detached, and waits for it as POST /run waits for a run. 200 {"run"} once it has taken the run
///   up; 422 {"error", "run"} when `resume` exited without, error ending with its log's last lines; 202 {"run",
///   "listed": false} after 30 seconds of neither. The body is not read. 403 for a run id that is not a plain name,
///   404 for no such run, 409 for a run not the app's (a run in herdr resumes from the CLI) or that a resume would not
///   take up or would find nothing left to do in (the listing's resumable false); 502 for a project that cannot be
///   reached, or a resume that cannot be started.
/// - POST /close?cwd=DIR: with the JSON body {"dry_run": true}, says what closing each run of the project that
///   `close --all` closes would do, changing nothing; with {"close": [{"run", "detail", "discarded"}, ...]}, the
///   "closed" runs of such a dry run's answer, closes those runs and no other, each only while closing it would still
///   do what detail says and discard what discarded lists (crate::cli::close::Sweep::Previewed says how closely).
///   Answers {"runs": [{"run", "outcome", "detail", "discarded"}, ...]}, a run for each the sweep looked at:
///   outcome "closed" (with a dry run: to be closed), "already closed", "skipped" (as is a run that changed since the
///   dry run) or "failed"; detail what was or would be done (`close`'s closed line or its dry run's), what close
///   recorded before, or why the run was skipped or failed; discarded the uncommitted changes, one line of git status
///   each, that went or would go with the run's worktree, a failed run's too. A run that fails does not stop the
///   others. 400 for a body of another shape and 413 as for /agent/input, both before the project is reached; 502 for
///   a project that cannot be reached, or with a dry run one whose runs cannot be read (a close reads each run in its
///   turn, and one it cannot read is "failed").
/// - POST /forget?cwd=DIR: with the JSON body {"dry_run": true}, says what `forget --all` would do with each run of
///   the project, changing nothing; with {"forget": ["RUN", ...]}, the "forgotten" runs of such a dry run's answer,
///   forgets those runs and no other, each only while it is still one forget may delete (crate::cli::forget::Sweep).
///   Answers {"runs": [{"run", "outcome", "detail"}, ...]}, a run for each the sweep looked at: outcome "forgotten"
///   (with a dry run: to be forgotten; its directory deleted, its metrics kept), "not closed", "waiting for prune"
///   (closed, but prune has yet to delete its branch), "skipped" (its state cannot be read, or a listed run is gone)
///   or "failed"; detail why, for "waiting for prune", "skipped" and "failed", else empty. A run that fails does not
///   stop the others. 400 for a body of another shape or a run id that is not a plain name, and 413 as for
///   /agent/input, both before the project is reached; 502 for a project whose runs cannot be read.
/// - POST /stop?cwd=DIR: with the JSON body {"dry_run": true}, says which of the project's desktop app runs are
///   running, which "Stop all" would stop, changing nothing; with {"dry_run": false}, stops each of them through its
///   socket and waits for it to be released as interrupted (ProjectRuns::stop_all). A run in herdr is never stopped.
///   Answers {"runs": [{"run", "error"}, ...]}, in run id order, error null for a run stopped (with a dry run: for
///   every run), else why it could not be: its socket refused, or it still ran a minute after it was told. A run that
///   fails does not stop the others. 400 for a body of another shape and 413 as for /agent/input, both before the
///   project is reached; 502 for a project that cannot be reached or whose runs cannot be read.
/// - /close-choice: {"choice"}, what the app does with its running runs when its window closes (CloseChoice): "ask",
///   "keep" or "stop". 500 when it cannot be read.
/// - POST /close-choice: saves the JSON body's {"choice": "ask" | "keep" | "stop"} and answers {"choice"}; 400 for a
///   body of another shape and 413 as for /agent/input, 500 when it cannot be saved.
/// - /events: a text/event-stream of RunWatch's events, polled every SERVE_POLL, until the client goes or the server
///   stops. An HTTP/1.1 client gets it chunked, so that the
///   end of the stream is the end of the response; an HTTP/1.0 client's ends when the connection closes.
///
/// Every request is checked in this order: a Host header other than the server's own address, or an Origin other
/// than its own or one of its origins, gets 403 {"error"}; a CORS preflight (OPTIONS) that passes gets 204, allowing
/// GET and POST with an Authorization and a Content-Type header; a GET or POST then needs the server's token as an
/// `Authorization: Bearer` header or, for an EventSource, which cannot set one, as ?token=, compared in constant time;
/// else 401. 404 is an unknown endpoint, 400 a missing or malformed parameter, 502 a project that could not be read
/// or herdr failing, each with {"error"}; a POST of any other endpoint, and any other method, gets 501 with
/// {"error"} too. A request with a byte beyond ASCII in its request line or a header is not
/// answered: tiny_http closes its connection. Nothing is logged.
pub struct RunServer {
    /// What its request threads share.
    shared: Arc<Shared>,
    /// tiny_http's server on the port, which the dispatcher takes requests from; None once the server has stopped, and
    /// while the dispatcher waits to start another.
    /// The dispatcher starts another on the same port when tiny_http's stops accepting connections, which it does
    /// at the first error accept gives (tiny_http 0.12's lib.rs, `Err(e) => ... break`), so that a passing EMFILE does
    /// not leave a server that answers nothing.
    listening: Arc<Mutex<Option<Arc<tiny_http::Server>>>>,
    /// The thread that hands each request a thread of its own; None once it has been joined.
    dispatcher: Option<JoinHandle<()>>,
}

/// What a RunServer's threads share.
struct Shared {
    /// `http://127.0.0.1:<port>`.
    url: String,
    /// What every request must carry: 32 random bytes, base64url without padding, as secrets.token_urlsafe(32).
    token: String,
    /// The Host headers a request may carry: anything else came through a name that only resolves here, as a DNS
    /// rebinding page's does.
    hosts: [String; 2],
    /// The Origin headers it answers and allows by CORS: its own, and the origins it was started with.
    origins: Vec<String>,
    /// Raised when the server stops, which ends every event stream and stops every command a project's Host runs.
    stopping: Interrupt,
    /// How a request's project is reached (Connect).
    connect: Box<Connect>,
    /// By the directory a request names, as it spells it, the one project that every request for that directory
    /// shares: two spellings of one directory (`~/proj`, `/home/u/proj/`) map to the same.
    projects: Mutex<HashMap<String, Arc<ProjectRuns>>>,
    /// What /setup and /setup/checked answer from.
    setup: Setup,
    /// How often its event streams poll their projects.
    poll: Duration,
    /// Where /workflows finds the workflow files.
    config: ConfigDir,
    /// How POST /run waits for the run it starts.
    start_wait: StartWait,
    /// The program POST /run and POST /resume start their runs by; None for serve's own (serve_program).
    run_program: Option<String>,
}

impl RunServer {
    /// Bind a free port of 127.0.0.1 and start answering on it, also requests from origins (the desktop app's webview,
    /// such as tauri://localhost), each project reached by connect, its setup checked and marked by setup, the
    /// workflows a run can be started with read from config. Its runs and resumes start by run_program, a program
    /// that takes the CLI's `run` and `resume` as serve's own does, which must outlive serve; None for serve's own.
    /// The desktop app gives its AppImage file while it runs from the AppImage's mount, which ends with the app,
    /// taking serve's program with it. Errors: no port can be bound, or no token made from the system's random
    /// bytes, or no thread started.
    pub fn start(
        origins: &[String],
        run_program: Option<String>,
        connect: Box<Connect>,
        setup: Setup,
        config: ConfigDir,
    ) -> io::Result<RunServer> {
        RunServer::start_polling(origins, run_program, connect, setup, config, SERVE_POLL, START_WAIT)
    }

    /// As start, the event streams polling every poll, and POST /run waiting as start_wait says.
    pub(crate) fn start_polling(
        origins: &[String],
        run_program: Option<String>,
        connect: Box<Connect>,
        setup: Setup,
        config: ConfigDir,
        poll: Duration,
        start_wait: StartWait,
    ) -> io::Result<RunServer> {
        let token = new_token()?;
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let url = format!("http://127.0.0.1:{port}");
        let mut all_origins = vec![url.clone(), format!("http://localhost:{port}")];
        all_origins.extend(origins.iter().cloned());
        let shared = Arc::new(Shared {
            url,
            token,
            hosts: [format!("127.0.0.1:{port}"), format!("localhost:{port}")],
            origins: all_origins,
            stopping: Interrupt::new(),
            connect,
            projects: Mutex::new(HashMap::new()),
            setup,
            poll,
            config,
            start_wait,
            run_program,
        });
        let listening = Arc::new(Mutex::new(Some(Arc::new(on(&listener)?))));
        let (dispatched, to) = (Arc::clone(&listening), Arc::clone(&shared));
        let dispatcher = std::thread::Builder::new()
            .name("serve".to_string())
            .spawn(move || dispatch(&listener, &dispatched, &to))?;
        Ok(RunServer { shared, listening, dispatcher: Some(dispatcher) })
    }

    /// Where it answers: `http://127.0.0.1:<port>`.
    pub fn url(&self) -> &str {
        &self.shared.url
    }

    /// The token every request must carry.
    pub fn token(&self) -> &str {
        &self.shared.token
    }
}

/// Stops it: every event stream ends, the commands its projects' Hosts run are stopped, and no request is taken any
/// more; a request being answered meanwhile may fail with 502.
impl Drop for RunServer {
    fn drop(&mut self) {
        self.shared.stopping.raise();
        // Under the lock dispatch starts a server by, so that it starts none after this.
        if let Some(http) = lock(&self.listening).take() {
            http.unblock();
        }
        if let Some(dispatcher) = self.dispatcher.take() {
            // A dispatcher that panicked has nothing left to stop.
            let _ = dispatcher.join();
        }
    }
}

/// A tiny_http server accepting connections on listener, which it shares. Errors: listener cannot be shared.
fn on(listener: &std::net::TcpListener) -> io::Result<tiny_http::Server> {
    tiny_http::Server::from_listener(listener.try_clone()?, None)
        .map_err(|e| e.downcast::<io::Error>().map_or_else(io::Error::other, |e| *e))
}

/// Hand each request the server in listening takes a thread of its own, answering as the server shared says, until it
/// stops (shared.stopping); when tiny_http's server stops accepting connections meanwhile, start another on listener,
/// 100 ms after the last one stopped or failed to start: a lasting EMFILE fails each new server's first accept, which
/// shows only as its recv failing at once, so without the pause the restarts would spin.
///
/// The old server is dropped (and listening emptied) before the pause, not after its successor starts. Dropping it
/// queues a connection on listener to wake its accept thread, which is expected to take it during the pause and end;
/// under a scheduling delay longer than the pause the successor could take it instead, leaving the old thread blocked.
/// Such a thread takes the next real connection into a queue nobody reads, and tiny_http answers it with an empty 500.
/// tiny_http's own accept error ends its thread, so this arises only when recv was ended by `unblock` while the thread
/// still accepts, which serve itself does only at the stop.
fn dispatch(listener: &std::net::TcpListener, listening: &Mutex<Option<Arc<tiny_http::Server>>>, shared: &Arc<Shared>) {
    loop {
        // None once RunServer's drop has taken it, at the stop.
        let Some(http) = lock(listening).clone() else { return };
        // recv fails once the server is unblocked, at its stop, or when tiny_http can accept no more connections.
        while let Ok(request) = http.recv() {
            let shared = Arc::clone(shared);
            // A request that gets no thread is dropped, which tiny_http answers with 500.
            let _ =
                std::thread::Builder::new().name("serve-request".to_string()).spawn(move || answer(&shared, request));
        }
        // Before the pause and the successor: see the doc comment.
        drop(lock(listening).take());
        drop(http);
        loop {
            // Ends at once at the stop, which the check below then sees.
            shared.stopping.sleep(Duration::from_millis(100));
            // Under the lock RunServer's drop takes the server by, so that no server is started after the stop, and a
            // stop after this one finds the new server there to take. Relies on shared.stopping never being caught or
            // finally-run (Interrupt::catch and finally clear is_raised), which nothing under serve does.
            let mut current = lock(listening);
            if shared.stopping.is_raised() {
                return;
            }
            if let Ok(http) = on(listener) {
                *current = Some(Arc::new(http));
                break;
            }
        }
    }
}

/// The server in listening, its lock held; a poisoned lock only means a thread panicked while it held it.
fn lock(
    listening: &Mutex<Option<Arc<tiny_http::Server>>>,
) -> std::sync::MutexGuard<'_, Option<Arc<tiny_http::Server>>> {
    listening.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Shared {
    /// The project in directory cwd, which may start with ~. Errors: it cannot be reached.
    fn project(&self, cwd: &str) -> Result<Arc<ProjectRuns>, OrchestratorError> {
        if let Some(project) = self.lock_projects().get(cwd) {
            return Ok(Arc::clone(project));
        }
        let reached = (self.connect)(cwd, &self.stopping)?;
        let project = Arc::new(ProjectRuns::new(reached, target_args(cwd)));
        // Every request for one directory, however it spells it and however many connected at once, gets the project
        // stored first, whose lock (ProjectRuns::forgetting) the directory's sweeps must share.
        let mut projects = self.lock_projects();
        let stored = projects.values().find(|p| p.cwd() == project.cwd());
        let project = Arc::clone(stored.unwrap_or(&project));
        Ok(Arc::clone(projects.entry(cwd.to_string()).or_insert(project)))
    }

    fn lock_projects(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<ProjectRuns>>> {
        // A poisoned lock only means a thread panicked while it held it; the map is still whole.
        self.projects.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Whether request came through another name than the server's address, or from a foreign origin.
    fn refused(&self, request: &Request) -> bool {
        let host_ok = header(request, "Host").is_some_and(|host| self.hosts.iter().any(|h| h == host));
        let origin_ok = header(request, "Origin").is_none_or(|origin| self.origins.iter().any(|o| o == origin));
        !(host_ok && origin_ok)
    }

    /// Whether the request carries the token, as its Authorization header or as query_token, the ?token= it gave
    /// (empty for none). Both are compared, each in constant time.
    fn has_token(&self, request: &Request, query_token: &str) -> bool {
        let authorization = header(request, "Authorization").unwrap_or("");
        let by_header = same_bytes(authorization.as_bytes(), format!("Bearer {}", self.token).as_bytes());
        let by_query = same_bytes(query_token.as_bytes(), self.token.as_bytes());
        by_header | by_query
    }
}

/// The GET endpoints of a RunServer, as its doc comment lists them.
#[derive(Clone, Copy)]
enum Endpoint {
    Runs,
    Files,
    File,
    Agent,
    AgentEvents,
    Events,
}

impl Endpoint {
    /// The endpoint at path, and the parameters it needs; None for no endpoint.
    fn at(path: &str) -> Option<(Endpoint, &'static [&'static str])> {
        Some(match path {
            "/runs" => (Endpoint::Runs, &["cwd"]),
            "/files" => (Endpoint::Files, &["cwd", "run"]),
            "/file" => (Endpoint::File, &["cwd", "run", "name"]),
            "/agent" => (Endpoint::Agent, &["cwd", "run", "role"]),
            "/agent/events" => (Endpoint::AgentEvents, &["cwd", "run", "role"]),
            "/events" => (Endpoint::Events, &["cwd"]),
            _ => return None,
        })
    }
}

/// The endpoints a POST may name.
const AGENT_INPUT: &str = "/agent/input";
const AGENT_ANSWER: &str = "/agent/answer";
const AGENT_MESSAGE: &str = "/agent/message";
const SETUP_CHECKED: &str = "/setup/checked";
const SETUP_GATE: &str = "/setup/gate";
const SETUP_ADAPTER: &str = "/setup/adapter";
const RUN: &str = "/run";
const RESUME: &str = "/resume";
const CLOSE: &str = "/close";
const FORGET: &str = "/forget";
const STOP: &str = "/stop";
/// Also a GET, which unreached_answer answers.
const CLOSE_CHOICE: &str = "/close-choice";
const WORKFLOW_CHECK: &str = "/workflow/check";
const WORKFLOW_SAVE: &str = "/workflow/save";
const WORKFLOW_DELETE: &str = "/workflow/delete";
/// The GET endpoints outside Endpoint, which reach no project (unreached_answer): the setup dialog's checks and its
/// adapters' installs, the workflows a run can be started with, and one workflow to edit; and /setup/checked and
/// /close-choice, which are POSTs too.
const SETUP: &str = "/setup";
const SETUP_ADAPTERS: &str = "/setup/adapters";
const WORKFLOWS: &str = "/workflows";
const WORKFLOW: &str = "/workflow";

/// What a request answered with JSON gets: its status and body.
type Answer = (u16, Value);

/// Answer request, as RunServer's doc comment says.
fn answer(shared: &Shared, request: Request) {
    let (path, query) = target(&request);
    match request.method().as_str() {
        "GET" => get(shared, request, &path, &query),
        "POST" if path == AGENT_INPUT => post_input(shared, request, &query),
        "POST" if path == AGENT_ANSWER || path == AGENT_MESSAGE => post_to_agent(shared, request, &path, &query),
        "POST" if path == SETUP_CHECKED => post_checked(shared, request, &query),
        "POST" if path == SETUP_GATE => post_setup_gate(shared, request, &query),
        "POST" if path == SETUP_ADAPTER => post_setup_adapter(shared, request, &query),
        "POST" if path == RUN => post_run(shared, request, &query),
        "POST" if path == RESUME => post_resume(shared, request, &query),
        "POST" if path == CLOSE => post_close(shared, request, &query),
        "POST" if path == FORGET => post_forget(shared, request, &query),
        "POST" if path == STOP => post_stop(shared, request, &query),
        "POST" if path == CLOSE_CHOICE => post_close_choice(shared, request, &query),
        "POST" if path == WORKFLOW_CHECK => post_workflow(shared, request, &query, |_, body| workflows::check(body)),
        "POST" if path == WORKFLOW_SAVE => post_workflow(shared, request, &query, workflows::save),
        "POST" if path == WORKFLOW_DELETE => post_workflow(shared, request, &query, workflows::delete),
        "OPTIONS" => preflight(shared, request),
        method => {
            // Made before send takes request, which method borrows.
            let response = unsupported(method);
            send(request, response);
        }
    }
}

/// The path of request's target and its query's parameters, a parameter given twice taking its last value.
fn target(request: &Request) -> (String, HashMap<String, String>) {
    // A target that starts with // is read with its leading slashes as one, never as a //host authority, so that
    // no answer can redirect a client to another host.
    let target = request.url();
    let target =
        if target.starts_with("//") { format!("/{}", target.trim_start_matches('/')) } else { target.to_string() };
    let url = urlsplit(&target);
    (url.path, parse_qsl(&url.query).into_iter().collect())
}

/// The refusal of a request that fails the Host, Origin or token checks, query being its parameters; None when it
/// passes them.
fn refusal(shared: &Shared, request: &Request, query: &HashMap<String, String>) -> Option<Answer> {
    if shared.refused(request) {
        return Some((403, error("the Host or Origin header is not this server's")));
    }
    if !shared.has_token(request, query.get("token").map_or("", String::as_str)) {
        return Some((401, error("the token is missing or wrong")));
    }
    None
}

/// 400 naming the parameters of params that query lacks; None when it has them all.
fn missing(query: &HashMap<String, String>, params: &[&str]) -> Option<Answer> {
    let missing: Vec<&str> = params.iter().copied().filter(|p| !query.contains_key(*p)).collect();
    (!missing.is_empty()).then(|| (400, error(&format!("missing parameter {}", missing.join(", ")))))
}

/// The project query names, once query holds every one of params, cwd among them. Errors: 400 naming the missing
/// parameters, 502 for a project that cannot be reached.
fn project_of(shared: &Shared, query: &HashMap<String, String>, params: &[&str]) -> Result<Arc<ProjectRuns>, Answer> {
    if let Some(answer) = missing(query, params) {
        return Err(answer);
    }
    shared.project(query.get("cwd").map_or("", String::as_str)).map_err(|e| (502, error(&e.0)))
}

/// The answer to a read of a run that failed with e.
fn read_failure(e: RunReadError) -> Answer {
    match e {
        RunReadError::Refused(why) => (403, error(&why)),
        RunReadError::Missing(why) => (404, error(&why)),
        RunReadError::Conflict(why) => (409, error(&why)),
        RunReadError::Failed(e) => (502, error(&e.0)),
    }
}

/// Answer a CORS preflight that passes the Host and Origin checks with 204, allowing GET and POST with an
/// Authorization and a Content-Type header. A browser sends no Authorization header with a preflight, so there is
/// no token to check.
fn preflight(shared: &Shared, request: Request) {
    if shared.refused(&request) {
        return send_json(request, 403, &error("the Host or Origin header is not this server's"));
    }
    let mut response = Response::empty(204);
    for (name, value) in allowed_origin(&request).into_iter().chain([
        ("Access-Control-Allow-Methods".to_string(), "GET, POST".to_string()),
        ("Access-Control-Allow-Headers".to_string(), "Authorization, Content-Type".to_string()),
    ]) {
        response = with_header(response, &name, &value);
    }
    send(request, response);
}

/// Answer a GET of one of the endpoints, at path with the query's parameters, as target gives them.
fn get(shared: &Shared, request: Request, path: &str, query: &HashMap<String, String>) {
    if let Some((code, body)) = refusal(shared, &request, query) {
        return send_json(request, code, &body);
    }
    let Some((endpoint, params)) = Endpoint::at(path) else {
        let (code, body) =
            unreached_answer(shared, path, query).unwrap_or_else(|| (404, error(&format!("no endpoint {path}"))));
        return send_json(request, code, &body);
    };
    let (code, body) = match project_of(shared, query, params) {
        Ok(project) if matches!(endpoint, Endpoint::Events) => return events(shared, request, &project),
        Ok(project) => read_project(endpoint, &project, query),
        Err(answer) => answer,
    };
    send_json(request, code, &body);
}

/// The answer to a GET of /setup, /setup/adapters, /setup/checked, /workflows, /workflow or /close-choice at path
/// with the query's parameters, as RunServer's doc comment gives them; None for any other path. Unlike Endpoint's
/// they reach no project: the setup checks check what one needs, and the adapters, the workflows and the close choice
/// are this machine's, where a run serve starts runs.
fn unreached_answer(shared: &Shared, path: &str, query: &HashMap<String, String>) -> Option<Answer> {
    Some(match path {
        SETUP => match query.get("cwd").map(String::as_str) {
            None => (400, error("missing parameter cwd")),
            Some("") => (400, error("parameter cwd is empty")),
            Some(cwd) => (200, shared.setup.checks(cwd, &shared.stopping)),
        },
        SETUP_ADAPTERS => (200, object([("adapters", shared.setup.adapters())])),
        SETUP_CHECKED => match shared.setup.checked() {
            Ok(checked) => (200, object([("checked", Value::from(checked))])),
            Err(why) => (500, error(&why)),
        },
        WORKFLOWS => match start::offered_workflows(&shared.config) {
            Ok(workflows) => (200, workflows),
            Err(e) => (500, error(&e.0)),
        },
        WORKFLOW => workflows::read(&shared.config, query),
        CLOSE_CHOICE => match CloseChoice::read(&shared.config) {
            Ok(choice) => (200, object([("choice", Value::from(choice.name()))])),
            Err(why) => (500, error(&why)),
        },
        _ => return None,
    })
}

/// The answer of endpoint, one that reads project and answers at once, with the query's parameters.
fn read_project(endpoint: Endpoint, project: &ProjectRuns, query: &HashMap<String, String>) -> Answer {
    let param = |name: &str| query.get(name).map_or("", String::as_str);
    let (run, name, role) = (param("run"), param("name"), param("role"));
    let answered = match endpoint {
        Endpoint::Runs => project.records().map_err(RunReadError::from).map(|records| {
            let runs = Value::Array(records.iter().map(RunRecord::to_json).collect());
            (200, object([("version", Value::from(RUN_RECORD_VERSION)), ("runs", runs)]))
        }),
        Endpoint::Files => project.files(run).map(|files| (200, object([("run", Value::from(run)), ("files", files)]))),
        Endpoint::File => project.read(run, name).map(|text| match text {
            Some(text) => {
                (200, object([("run", Value::from(run)), ("name", Value::from(name)), ("text", Value::from(text))]))
            }
            None => (404, error(&format!("no file {name} in run {run}"))),
        }),
        Endpoint::Agent => match agent_lines(query.get("lines")) {
            Err(answer) => Ok(answer),
            Ok(lines) => project.agent_text(run, role, lines).map(|text| {
                (200, object([("run", Value::from(run)), ("role", Value::from(role)), ("text", Value::from(text))]))
            }),
        },
        Endpoint::AgentEvents => match query.get("after").map(|a| a.parse::<u64>()) {
            Some(Err(_)) => Ok((400, error("after must be a whole number"))),
            after => project.conversation(run, role, after.and_then(Result::ok).unwrap_or(0)).map(|mut body| {
                body.insert("run".into(), run.into());
                body.insert("role".into(), role.into());
                (200, Value::Object(body))
            }),
        },
        Endpoint::Events => unreachable!("get answers it itself"),
    };
    answered.unwrap_or_else(read_failure)
}

/// The lines /agent gives for its ?lines=, given: AGENT_LINES's default without one. Errors: 400 for one that is not
/// a whole number from 1 to AGENT_LINES's most.
fn agent_lines(given: Option<&String>) -> Result<u32, Answer> {
    let (default, most) = AGENT_LINES;
    let Some(given) = given else { return Ok(default) };
    given
        .parse::<u32>()
        .ok()
        .filter(|n| (1..=most).contains(n))
        .ok_or_else(|| (400, error(&format!("lines must be a whole number from 1 to {most}"))))
}

/// Answer a POST of /agent/input with the query's parameters, as target gives them. The body is read and checked
/// before the project is reached, so that a bad one is told so without a connect that may fail.
fn post_input(shared: &Shared, mut request: Request, query: &HashMap<String, String>) {
    if let Some((code, body)) = refusal(shared, &request, query) {
        return send_json(request, code, &body);
    }
    let input = match agent_input(&mut request) {
        Ok(input) => input,
        Err((code, body)) => return send_json(request, code, &body),
    };
    let project = match project_of(shared, query, &["cwd", "run", "role"]) {
        Ok(project) => project,
        Err((code, body)) => return send_json(request, code, &body),
    };
    let param = |name: &str| query.get(name).map_or("", String::as_str);
    let (run, role) = (param("run"), param("role"));
    let (code, body) = match project.type_into(run, role, &input) {
        Ok(()) => (200, object([("run", Value::from(run)), ("role", Value::from(role))])),
        Err(e) => read_failure(e),
    };
    send_json(request, code, &body);
}

/// Answer a POST of /agent/answer or /agent/message, at path, with the query's parameters, as target gives them: the
/// body's answer or message sent to the run's socket as its command for the role (crate::acp::ask). The body is read
/// and checked before the project is reached, as for /agent/input.
fn post_to_agent(shared: &Shared, mut request: Request, path: &str, query: &HashMap<String, String>) {
    if let Some((code, body)) = refusal(shared, &request, query) {
        return send_json(request, code, &body);
    }
    let param = |name: &str| query.get(name).map_or("", String::as_str);
    let (run, role) = (param("run"), param("role"));
    let command = match read_body(&mut request).and_then(|body| agent_command(path, role, &body)) {
        Ok(command) => command,
        Err((code, body)) => return send_json(request, code, &body),
    };
    let dir = project_of(shared, query, &["cwd", "run", "role"])
        .and_then(|project| project.acp_dir(run).map_err(read_failure));
    let (code, body) = match dir.map(|dir| acp::ask(&dir, &command)) {
        Ok(Ok(_)) => (200, object([("run", Value::from(run)), ("role", Value::from(role))])),
        Ok(Err(e)) => socket_failure(e),
        Err(answer) => answer,
    };
    send_json(request, code, &body);
}

/// The run socket's command (crate::acp::ask) for a POST of path, /agent/answer or /agent/message, to the agent of
/// role, from the request's body: {"request": N, "option": "..."} for an answer, {"text": "..."} for a message.
/// Errors: 400 for a body of another shape.
fn agent_command(path: &str, role: &str, body: &[u8]) -> Result<Value, Answer> {
    let fields = json_object(body);
    let field = |key: &str| fields.as_ref().and_then(|f| f.get(key)).cloned();
    if path == AGENT_ANSWER {
        let (Some(request), Some(option)) =
            (field("request").filter(Value::is_u64), field("option").filter(Value::is_string))
        else {
            return Err((400, error(r#"the body must be {"request": N, "option": "..."}"#)));
        };
        return Ok(serde_json::json!({"answer": {"role": role, "request": request, "option": option}}));
    }
    let Some(text) = field("text").filter(|t| t.as_str().is_some_and(|t| !t.trim().is_empty())) else {
        return Err((400, error(r#"the body must be {"text": "..."}, the text not blank"#)));
    };
    Ok(serde_json::json!({"message": {"role": role, "text": text}}))
}

/// The answer to a POST to an agent whose run's socket did not answer `ok`, as RunServer's doc comment gives it.
fn socket_failure(e: acp::SocketError) -> Answer {
    match e {
        acp::SocketError::Unreachable(why) => {
            (409, error(&format!("the run is not running, so it cannot be answered: {why}")))
        }
        acp::SocketError::Protocol { hello } => {
            let engine = hello.get("engine").and_then(Value::as_str).unwrap_or("another");
            (409, error(&format!("the run is from octoflow {engine}, another version, so it can only be read")))
        }
        acp::SocketError::Refused { code, message } => {
            let http_code = match code.as_str() {
                "no_agent" | "no_request" => 404,
                "already_answered" | "not_waiting" => 409,
                "too_long" => 413,
                "unwritten" => 502,
                _ => 400,
            };
            (http_code, object([("error", Value::from(message)), ("code", Value::from(code))]))
        }
    }
}

/// Answer a POST of /setup/checked, which needs no parameter, by writing the setup marker. It trusts the caller that
/// every Required check passed rather than checking again, which would run the checks twice for one Close: the marker
/// only keeps the dialog from opening by itself (Setup::checked), and "Check setup…" opens it whatever the marker
/// says.
fn post_checked(shared: &Shared, request: Request, query: &HashMap<String, String>) {
    if let Some((code, body)) = refusal(shared, &request, query) {
        return send_json(request, code, &body);
    }
    let (code, body) = match shared.setup.mark() {
        Ok(()) => (200, object([("checked", Value::from(true))])),
        Err(why) => (500, error(&why)),
    };
    send_json(request, code, &body);
}

/// Answer a POST of /setup/adapter with the query's parameters, as target gives them, by starting an install of the
/// adapter its agent names.
fn post_setup_adapter(shared: &Shared, request: Request, query: &HashMap<String, String>) {
    if let Some((code, body)) = refusal(shared, &request, query) {
        return send_json(request, code, &body);
    }
    let agent = query.get("agent").map_or("", String::as_str);
    let (code, body) = match Adapter::named(agent) {
        None => (400, error(&format!("agent {agent:?} has no ACP adapter the app installs"))),
        Some(adapter) => match shared.setup.install_adapter(adapter, &shared.stopping) {
            Ok(()) => (202, object([("installing", Value::from(true))])),
            Err(why) => (500, error(&why)),
        },
    };
    send_json(request, code, &body);
}

/// Answer a POST of /setup/gate with the query's parameters, as target gives them, by setting the project up for the
/// local quality gate.
fn post_setup_gate(shared: &Shared, request: Request, query: &HashMap<String, String>) {
    if let Some((code, body)) = refusal(shared, &request, query) {
        return send_json(request, code, &body);
    }
    let set_up =
        project_of(shared, query, &["cwd"]).and_then(|project| project.set_up_gate().map_err(|e| (502, error(&e.0))));
    let (code, body) = match set_up {
        Ok(files) => {
            let files =
                files.into_iter().map(|f| object([("path", Value::from(f.path)), ("written", Value::from(f.written))]));
            (200, object([("files", Value::Array(files.collect()))]))
        }
        Err(answer) => answer,
    };
    send_json(request, code, &body);
}

/// Answer a POST of /run with the query's parameters, as target gives them: start the run its body asks for, as
/// RunServer's doc comment says. The body is read and checked before the project is reached, as post_input's.
fn post_run(shared: &Shared, mut request: Request, query: &HashMap<String, String>) {
    if let Some((code, body)) = refusal(shared, &request, query) {
        return send_json(request, code, &body);
    }
    let parsed = read_body(&mut request).and_then(|body| RunRequest::parse(&body).map_err(|why| (400, error(&why))));
    let run = match parsed {
        Ok(run) => run,
        Err((code, body)) => return send_json(request, code, &body),
    };
    let (code, body) = match project_of(shared, query, &["cwd"]) {
        Ok(project) => start_answer(shared, &project, &run),
        Err(answer) => answer,
    };
    send_json(request, code, &body);
}

/// Answer a POST of /workflow/check, /workflow/save or /workflow/delete, once refusal passes it with the query's
/// parameters, with what endpoint answers to its body for serve's configuration directory. These reach no project.
fn post_workflow(
    shared: &Shared,
    mut request: Request,
    query: &HashMap<String, String>,
    endpoint: fn(&ConfigDir, &[u8]) -> Answer,
) {
    if let Some((code, body)) = refusal(shared, &request, query) {
        return send_json(request, code, &body);
    }
    let (code, body) = match read_body(&mut request) {
        Ok(body) => endpoint(&shared.config, &body),
        Err(answer) => answer,
    };
    send_json(request, code, &body);
}

/// Answer a POST of /close with the query's parameters, as target gives them: say what closing the project's runs
/// would do, or close the runs a dry run gave, as RunServer's doc comment says (post_sweep).
fn post_close(shared: &Shared, request: Request, query: &HashMap<String, String>) {
    post_sweep(shared, request, query, previewed_runs, |project, previewed, stopping| {
        let sweep = previewed.as_deref().map_or(Sweep::DryRun, Sweep::Previewed);
        Ok(project.close_all(sweep, stopping)?.iter().map(run_closed).collect())
    });
}

/// Answer a POST of one of the endpoints that sweep through the project's runs, /close, /forget and /stop, with the
/// query's parameters: read the body and check it by asked (a 400 or 413 Answer for a bad one) before the project is
/// reached, as post_input does, then answer {"runs": [...]}, each run as sweep gives it, or 502 with the error sweep
/// gives, as when the project's runs cannot be read. stopping, given to sweep, is raised when the server stops.
///
/// serve sweeps in its own process, by the sweeps `close --all` and `forget --all` run (ProjectRuns), rather than by
/// starting its program as `close --all`, as POST /run starts `run`: the sweep's outcomes come back as values, with no
/// output format of the CLI's to keep for serve to parse, and a sweep needs no pane for a human to watch.
fn post_sweep<Asked>(
    shared: &Shared,
    mut request: Request,
    query: &HashMap<String, String>,
    asked: fn(&[u8]) -> Result<Asked, Answer>,
    sweep: impl FnOnce(&ProjectRuns, Asked, &Interrupt) -> Result<Vec<Value>, OrchestratorError>,
) {
    if let Some((code, body)) = refusal(shared, &request, query) {
        return send_json(request, code, &body);
    }
    let asked = match read_body(&mut request).and_then(|body| asked(&body)) {
        Ok(asked) => asked,
        Err((code, body)) => return send_json(request, code, &body),
    };
    let swept = project_of(shared, query, &["cwd"])
        .and_then(|project| sweep(&project, asked, &shared.stopping).map_err(|e| (502, error(&e.0))));
    let (code, body) = match swept {
        Ok(runs) => (200, object([("runs", Value::Array(runs))])),
        Err(answer) => answer,
    };
    send_json(request, code, &body);
}

/// The runs a POST /close body asks to close, each as the dry run gave it (an Outcome::Closed); None for a dry run.
/// The shapes are RunServer's doc comment's. Errors: 400 for a body of another shape, or a run that is not a plain
/// name (runs::plain_name).
fn previewed_runs(body: &[u8]) -> Result<Option<Vec<RunClosed>>, Answer> {
    let shape = || {
        (
            400,
            error(
                r#"the body must be {"dry_run": true} or {"close": [{"run": "...", "detail": "...", "discarded": ["...", ...]}, ...]}"#,
            ),
        )
    };
    let fields = json_object(body).filter(|o| o.len() == 1).ok_or_else(shape)?;
    if fields.get("dry_run") == Some(&Value::Bool(true)) {
        return Ok(None);
    }
    let previewed = |run: &Value| -> Option<RunClosed> {
        let text = |key: &str| run.get(key).and_then(Value::as_str).map(String::from);
        let discarded = run.get("discarded")?.as_array()?;
        let discarded = discarded.iter().map(|line| line.as_str().map(String::from)).collect::<Option<Vec<_>>>()?;
        let run_id = text("run").filter(|id| plain_name(id).is_ok())?;
        Some(RunClosed { run_id, outcome: Outcome::Closed { done: text("detail")?, discarded } })
    };
    let runs = fields.get("close").and_then(Value::as_array).ok_or_else(shape)?;
    runs.iter().map(previewed).collect::<Option<Vec<_>>>().map(Some).ok_or_else(shape)
}

/// Answer a POST of /forget with the query's parameters, as target gives them: say what forgetting the project's runs
/// would do, or forget the runs a dry run gave, as RunServer's doc comment says (post_sweep).
fn post_forget(shared: &Shared, request: Request, query: &HashMap<String, String>) {
    post_sweep(shared, request, query, listed_runs, |project, listed, stopping| {
        let sweep = listed.as_deref().map_or(forget::Sweep::DryRun, forget::Sweep::Listed);
        Ok(project.forget_all(sweep, stopping)?.iter().map(run_forgotten).collect())
    });
}

/// Answer a POST of /stop with the query's parameters, as target gives them: stop the project's running desktop app
/// runs, or say which a stop would stop, as RunServer's doc comment says (post_sweep).
fn post_stop(shared: &Shared, request: Request, query: &HashMap<String, String>) {
    let dry_run = |body: &[u8]| {
        sole_field(body, "dry_run")
            .and_then(|dry_run| dry_run.as_bool())
            .ok_or_else(|| (400, error(r#"the body must be {"dry_run": true} or {"dry_run": false}"#)))
    };
    post_sweep(shared, request, query, dry_run, |project, dry_run, stopping| {
        let run = |r: &RunStopped| {
            object([("run", Value::from(r.run_id.as_str())), ("error", Value::from(r.failure.clone()))])
        };
        Ok(project.stop_all(dry_run, stopping)?.iter().map(run).collect())
    });
}

/// Answer a POST of /close-choice, which needs no parameter, by saving the choice its body names.
fn post_close_choice(shared: &Shared, mut request: Request, query: &HashMap<String, String>) {
    if let Some((code, body)) = refusal(shared, &request, query) {
        return send_json(request, code, &body);
    }
    let choice = read_body(&mut request).and_then(|body| {
        sole_field(&body, "choice")
            .and_then(|choice| choice.as_str().and_then(CloseChoice::named))
            .ok_or_else(|| (400, error(r#"the body must be {"choice": "ask" | "keep" | "stop"}"#)))
    });
    let (code, body) = match choice {
        Ok(choice) => match choice.save(&shared.config) {
            Ok(()) => (200, object([("choice", Value::from(choice.name()))])),
            Err(why) => (500, error(&why)),
        },
        Err(answer) => answer,
    };
    send_json(request, code, &body);
}

/// The run ids a POST /forget body asks to forget; None for a dry run. The shapes are RunServer's doc comment's.
/// Errors: 400 for a body of another shape, or a run id that is not a plain name (runs::plain_name).
fn listed_runs(body: &[u8]) -> Result<Option<Vec<String>>, Answer> {
    let shape = || (400, error(r#"the body must be {"dry_run": true} or {"forget": ["...", ...]}"#));
    let fields = json_object(body).filter(|o| o.len() == 1).ok_or_else(shape)?;
    if fields.get("dry_run") == Some(&Value::Bool(true)) {
        return Ok(None);
    }
    let runs = fields.get("forget").and_then(Value::as_array).ok_or_else(shape)?;
    let run_id = |run: &Value| run.as_str().filter(|id| plain_name(id).is_ok()).map(String::from);
    runs.iter().map(run_id).collect::<Option<Vec<_>>>().map(Some).ok_or_else(shape)
}

/// One run of a POST /forget answer: {"run", "outcome", "detail"}, as RunServer's doc comment says.
fn run_forgotten(run: &RunForgotten) -> Value {
    let (outcome, detail) = match &run.outcome {
        forget::Outcome::Forgotten => ("forgotten", ""),
        forget::Outcome::Left(Left::NotClosed) => ("not closed", ""),
        forget::Outcome::Left(Left::AwaitsPrune) => ("waiting for prune", forget::AWAITS_PRUNE),
        forget::Outcome::Left(Left::Unreadable(why) | Left::AppRun(why)) => ("skipped", why.as_str()),
        forget::Outcome::Gone => ("skipped", "the overview shows no such run any more"),
        forget::Outcome::Failed(error) => ("failed", error.as_str()),
    };
    object([
        ("run", Value::from(run.run_id.as_str())),
        ("outcome", Value::from(outcome)),
        ("detail", Value::from(detail)),
    ])
}

/// The value of key in body when body is a JSON object with that one key; None otherwise.
fn sole_field(body: &[u8], key: &str) -> Option<Value> {
    json_object(body).filter(|fields| fields.len() == 1)?.remove(key)
}

/// The fields of body, when it is a JSON object in UTF-8, as every POST's body is; None otherwise.
fn json_object(body: &[u8]) -> Option<serde_json::Map<String, Value>> {
    match serde_json::from_slice::<Value>(body).ok()? {
        Value::Object(fields) => Some(fields),
        _ => None,
    }
}

/// One run of a POST /close answer: {"run", "outcome", "detail", "discarded"}, as RunServer's doc comment says.
fn run_closed(run: &RunClosed) -> Value {
    let (outcome, detail, discarded): (&str, &str, &[String]) = match &run.outcome {
        Outcome::Closed { done, discarded } => ("closed", done, discarded),
        Outcome::AlreadyClosed(closed) => ("already closed", closed, &[]),
        Outcome::Skipped(why) => ("skipped", why, &[]),
        Outcome::Failed { error, discarded } => ("failed", error, discarded),
    };
    object([
        ("run", Value::from(run.run_id.as_str())),
        ("outcome", Value::from(outcome)),
        ("detail", Value::from(detail)),
        ("discarded", Value::from(discarded.to_vec())),
    ])
}

/// The answer to POST /run once its body is run and its project project: the run started, by run_program's program, as
/// a new run id, and waited for.
fn start_answer(shared: &Shared, project: &ProjectRuns, run: &RunRequest) -> Answer {
    let program = match run_program(shared) {
        Ok(path) => path,
        Err(why) => return (500, error(&why)),
    };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    let run_id = match crate::cli::new_run_id(now, &crate::workflow::machine_zone()) {
        Ok(id) => id,
        Err(e) => return (500, error(&e.0)),
    };
    match project.start(run, &program, &run_id, shared.start_wait, &shared.stopping) {
        Ok(Started::Listed) => (201, object([("run", Value::from(run_id))])),
        Ok(Started::Unlisted) => (202, object([("run", Value::from(run_id)), ("listed", Value::from(false))])),
        Ok(Started::Exited(shown)) => {
            let why = format!("run {run_id} exited before it saved its state; {shown}");
            (422, object([("error", Value::from(why)), ("run", Value::from(run_id))]))
        }
        Err(e) => read_failure(e),
    }
}

/// Answer a POST of /resume with the query's parameters: resume the run it names, as RunServer's doc comment says.
fn post_resume(shared: &Shared, request: Request, query: &HashMap<String, String>) {
    if let Some((code, body)) = refusal(shared, &request, query) {
        return send_json(request, code, &body);
    }
    let (code, body) = match project_of(shared, query, &["cwd", "run"]) {
        Ok(project) => resume_answer(shared, &project, query.get("run").map_or("", String::as_str)),
        Err(answer) => answer,
    };
    send_json(request, code, &body);
}

/// POST /resume's answer, once its project is reached, for the run run_id.
fn resume_answer(shared: &Shared, project: &ProjectRuns, run_id: &str) -> Answer {
    let program = match run_program(shared) {
        Ok(path) => path,
        Err(why) => return (500, error(&why)),
    };
    let fields = |more: Option<(&str, Value)>| {
        let mut fields = crate::json::object([("run", Value::from(run_id))]);
        fields.extend(more.map(|(key, value)| (key.to_string(), value)));
        Value::Object(fields)
    };
    match project.resume(&program, run_id, shared.start_wait, &shared.stopping) {
        Ok(Started::Listed) => (200, fields(None)),
        Ok(Started::Unlisted) => (202, fields(Some(("listed", Value::from(false))))),
        Ok(Started::Exited(shown)) => {
            let why = format!("the resume of run {run_id} exited before it took the run up; {shown}");
            (422, fields(Some(("error", Value::from(why)))))
        }
        Err(e) => read_failure(e),
    }
}

/// The program a run, or its resume, is started by: the one shared was given (RunServer::start) while it is there,
/// else serve's own (serve_program). Errors: serve_program's.
fn run_program(shared: &Shared) -> Result<String, String> {
    // The app's AppImage file can be moved or deleted while the app runs, as an AppImage integrator moves it; the app's
    // mount still holds serve's program then, which beats starting no run at all.
    let given = shared.run_program.as_ref().filter(|path| std::path::Path::new(path).exists());
    given.cloned().map_or_else(serve_program, Ok)
}

/// The path of serve's own program, which POST /run and POST /resume start their runs by unless serve was given
/// another (run_program). Errors: it cannot be told, or is not UTF-8.
fn serve_program() -> Result<String, String> {
    let path = std::env::current_exe().map_err(|e| format!("cannot tell serve's program: {}", os_error(&e, None)))?;
    let path = path.into_os_string().into_string().map_err(|p| format!("serve's program {p:?} is not UTF-8"))?;
    Ok(live_program(path, |p| std::path::Path::new(p).exists()))
}

/// The program at path, as current_exe names it, where a run can start it: Linux's /proc/self/exe names a program
/// replaced while it runs, as a build or an upgrade replaces serve's, as `<path> (deleted)`, and the program now at
/// path, by exists, is the one to start. As it is otherwise.
fn live_program(path: String, exists: impl Fn(&str) -> bool) -> String {
    match path.strip_suffix(" (deleted)") {
        Some(replaced) if exists(replaced) => replaced.to_string(),
        _ => path,
    }
}

/// request's body. Errors: 413 for one over MAX_BODY_BYTES, 400 for one that cannot be read.
fn read_body(request: &mut Request) -> Result<Vec<u8>, Answer> {
    let mut body = Vec::new();
    let limit = MAX_BODY_BYTES as u64 + 1;
    if let Err(e) = request.as_reader().take(limit).read_to_end(&mut body) {
        return Err((400, error(&format!("cannot read the body: {}", os_error(&e, None)))));
    }
    if body.len() > MAX_BODY_BYTES {
        return Err((413, error(&format!("the body is over {MAX_BODY_BYTES} bytes"))));
    }
    Ok(body)
}

/// The input request's body asks /agent/input to type, as RunServer's doc comment gives its shapes. Errors: 413 for a
/// body over MAX_BODY_BYTES, 400 for one that cannot be read or is of another shape.
fn agent_input(request: &mut Request) -> Result<AgentInput, Answer> {
    let body = read_body(request)?;
    let shape = || (400, error(r#"the body must be {"text": "..."} or {"keys": ["...", ...]}"#));
    let fields = json_object(&body).filter(|o| o.len() == 1).ok_or_else(shape)?;
    if let Some(text) = fields.get("text").and_then(Value::as_str).filter(|t| !t.is_empty()) {
        return Ok(AgentInput::Text(text.to_string()));
    }
    let keys = fields.get("keys").and_then(Value::as_array).filter(|keys| !keys.is_empty()).ok_or_else(shape)?;
    let keys: Option<Vec<String>> =
        keys.iter().map(|k| k.as_str().filter(|k| !k.is_empty()).map(String::from)).collect();
    keys.map(AgentInput::Keys).ok_or_else(shape)
}

/// Stream project's events to request's client until it goes or the server stops: a poll's events, or a comment when
/// nothing changed, so that a client that went away fails the write and ends the stream.
fn events(shared: &Shared, request: Request, project: &ProjectRuns) {
    let chunked = *request.http_version() >= (1, 1);
    let head = event_stream_head(&request, chunked);
    let mut out = request.into_writer();
    if out.write_all(head.as_bytes()).and_then(|()| out.flush()).is_err() {
        return;
    }
    let mut watch = RunWatch::new(project);
    while !shared.stopping.is_raised() {
        let events = watch.poll();
        // A poll the stop cut short reports the commands it stopped, which is no problem of the project's.
        if shared.stopping.is_raised() {
            break;
        }
        let text: String =
            if events.is_empty() { ":\n\n".to_string() } else { events.iter().map(Event::sse).collect() };
        let part = if chunked { format!("{:X}\r\n{text}\r\n", text.len()) } else { text };
        if out.write_all(part.as_bytes()).and_then(|()| out.flush()).is_err() {
            return;
        }
        shared.stopping.sleep(shared.poll);
    }
    if chunked {
        // The client may have gone meanwhile; there is no one to tell.
        let _ = out.write_all(b"0\r\n\r\n").and_then(|()| out.flush());
    }
}

/// The status line and headers, through the blank line that ends them, that open an event stream answering request:
/// HTTP/1.1 with chunked transfer encoding when chunked, else HTTP/1.0 with none, allowing request's origin.
fn event_stream_head(request: &Request, chunked: bool) -> String {
    let version = if chunked { "1.1" } else { "1.0" };
    let mut head = format!("HTTP/{version} 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\n");
    for (name, value) in allowed_origin(request) {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    if chunked {
        head.push_str("Transfer-Encoding: chunked\r\n");
    }
    head.push_str("\r\n");
    head
}

/// The CORS headers that allow request's Origin, when it has one that is not empty: only after the Host and Origin
/// checks, so an Origin here is one the server answers.
fn allowed_origin(request: &Request) -> Vec<(String, String)> {
    match header(request, "Origin").filter(|o| !o.is_empty()) {
        Some(origin) => vec![
            ("Access-Control-Allow-Origin".to_string(), origin.to_string()),
            ("Vary".to_string(), "Origin".to_string()),
        ],
        None => vec![],
    }
}

/// Answer request with code and body as compact JSON, allowing its origin unless code is 403, and
/// naming the Bearer scheme with a 401.
fn send_json(request: Request, code: u16, body: &Value) {
    let mut response =
        with_header(Response::from_data(body.to_string()).with_status_code(code), "Content-Type", "application/json");
    if code != 403 {
        for (name, value) in allowed_origin(&request) {
            response = with_header(response, &name, &value);
        }
    }
    if code == 401 {
        response = with_header(response, "WWW-Authenticate", "Bearer");
    }
    send(request, response);
}

/// The 501 a request of method gets: {"error"} naming the method. It allows no origin, as no Host or Origin check came
/// before it.
fn unsupported(method: &str) -> Response<io::Cursor<Vec<u8>>> {
    let body = error(&format!("unsupported method {method:?}")).to_string();
    with_header(Response::from_data(body).with_status_code(501), "Content-Type", "application/json")
}

/// Answer request with response; a client that has gone is not told.
fn send<R: Read>(request: Request, response: Response<R>) {
    let _ = request.respond(response);
}

/// response with the header added; as it was, for a value tiny_http cannot send, which no value here is: each is a
/// constant or a header tiny_http read, all ASCII.
fn with_header<R: Read>(response: Response<R>, name: &str, value: &str) -> Response<R> {
    match Header::from_bytes(name.as_bytes(), value.as_bytes()) {
        Ok(header) => response.with_header(header),
        Err(()) => response,
    }
}

/// The value of request's first header of that name, in any case; None without one.
fn header<'r>(request: &'r Request, name: &'static str) -> Option<&'r str> {
    request.headers().iter().find(|h| h.field.equiv(name)).map(|h| h.value.as_str())
}

fn object<const N: usize>(pairs: [(&str, Value); N]) -> Value {
    Value::Object(crate::json::object(pairs))
}

fn error(message: &str) -> Value {
    object([("error", Value::from(message))])
}

/// The --cwd flag a request's cwd stands for, left out when empty.
fn target_args(cwd: &str) -> Vec<String> {
    let cwd = (!cwd.is_empty()).then(|| cwd.to_string());
    crate::cli::Target { machine: None, cwd, acp: false }.flags()
}

/// Whether given and expected are the same bytes, in a time that depends on their lengths alone and not on where
/// they first differ, as secrets.compare_digest compares them: given is read to its end whatever it holds.
fn same_bytes(given: &[u8], expected: &[u8]) -> bool {
    let mut differ = given.len() ^ expected.len();
    for (i, &b) in given.iter().enumerate() {
        // black_box: so the compiler cannot end the loop at the first difference.
        differ = std::hint::black_box(differ | usize::from(b ^ expected.get(i).copied().unwrap_or(0)));
    }
    differ == 0
}

/// A new token: 32 bytes from the system's random source, in base64url without padding, as
/// secrets.token_urlsafe(32) makes it. Errors: /dev/urandom cannot be read.
fn new_token() -> io::Result<String> {
    let bytes: [u8; 32] = crate::env::random_bytes()?;
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut token = String::with_capacity(43);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
        // A chunk of k bytes makes k + 1 characters.
        for i in 0..=chunk.len() {
            token.push(char::from(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize]));
        }
    }
    Ok(token)
}

#[cfg(test)]
mod tests;
