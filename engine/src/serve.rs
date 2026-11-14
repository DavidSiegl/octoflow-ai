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

use crate::cli::listing::RUN_RECORD_VERSION;
use crate::constants::EXIT_ERROR;
use crate::env::ConfigDir;
use crate::error::{os_error, OrchestratorError};
use crate::herdr::{AgentConsole, Launcher, ProcessHerdr};
use crate::host::{Host, ProcessHost};
use crate::interrupt::Interrupt;
use serde_json::Value;
use url::{parse_qsl, urlsplit};
use runs::{AgentInput, Event, ProjectRuns, RunReadError, RunWatch};
use setup::Setup;
use start::{RunRequest, StartWait, Started, START_WAIT};

#[cfg(test)]
mod fake;
pub mod runs;
pub mod setup;
pub mod start;
pub mod url;

/// How often an event stream reads its project's runs again. A read of a local project starts a few short processes;
/// on a --machine, each of them is an SSH round trip, so a machine's project is read less often.
pub const SERVE_POLL: Polls = Polls { local: Duration::from_secs(1), remote: Duration::from_secs(5) };

/// How often an event stream polls its project, by where the project is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Polls {
    /// For a project on this machine.
    pub local: Duration,
    /// For a project on a --machine, read over SSH.
    pub remote: Duration,
}

/// The lines of an agent's terminal /agent gives without ?lines=, and the most it gives with one.
pub const AGENT_LINES: (u32, u32) = (200, 2000);

/// The most bytes of a POST's body serve reads; a longer one gets 413.
pub const MAX_BODY_BYTES: usize = 64 * 1024;

/// A project as Connect reaches it.
pub struct Reached {
    /// Reaches the project's files; must stop its commands when the server stops.
    pub host: Arc<dyn Host + Send + Sync>,
    /// The project's absolute directory on host.
    pub dir: String,
    /// Reaches its runs' agents, each by its AgentAddress.
    pub console: Arc<dyn AgentConsole>,
    /// Starts a run's process for POST /run, in a pane of this machine's herdr (Launcher::launch says which server),
    /// for a project here and one on a machine alike: a machine's run is driven from here, as from the CLI.
    pub launcher: Arc<dyn Launcher>,
    /// The directory of this machine a run's process starts in: dir for a project here, serve's own working
    /// directory for one on a machine, which the run reaches over SSH.
    pub launch_dir: String,
}

/// How a RunServer reaches the project a request names: given the saved herdr machine (empty for this one) and the
/// directory, which may start with ~, as the request gives them, and the Interrupt the server raises when it stops,
/// which must stop the Host's commands. Errors: the project cannot be reached. [`project_host`] is the real one.
pub type Connect = dyn Fn(&str, &str, &Interrupt) -> Result<Reached, OrchestratorError> + Send + Sync;

/// The program's `serve`: start a RunServer that also answers origins, its setup marker and workflows in config, print {"url", "token"} as one JSON line on
/// standard output, and serve until SIGINT or SIGTERM; returns 0 then, the server stopped. Returns EXIT_ERROR, with
/// one line on standard error, when no port can be bound, or when the line cannot be printed, as when whoever
/// started serve has stopped reading its standard output: nobody could reach the server then. SIGHUP keeps its
/// default, ending the process.
///
/// Must be called on the main thread before any other thread starts, as crate::interrupt::by_signals must, and
/// instead of it: SIGINT and SIGTERM are blocked in every thread and waited for here, also when the process started
/// with one ignored, which then stops serve too (Linux delivers a blocked signal that
/// is ignored to sigwait; a BSD kernel, macOS's, discards it, so there serve does not stop at an ignored one).
pub fn serve_command(origins: &[String], config: &ConfigDir) -> i32 {
    let signals: SigSet = [Signal::SIGINT, Signal::SIGTERM].into_iter().collect();
    if let Err(e) = signals.thread_block() {
        eprintln!("error: cannot wait for SIGINT and SIGTERM: {e}");
        return EXIT_ERROR;
    }
    let started = RunServer::start(origins, Box::new(project_host), Setup::of(config), config.clone());
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

/// The project on the saved herdr machine or, with machine empty, here: the Host that reaches it, its absolute
/// directory, cwd's, which may start with ~, and the console that reaches its agents, this machine's herdr
/// forwarding to machine. Each command is stopped by stopping. Unlike connect, it does not check for a
/// herdr pane, since reading a project sends herdr no command, so a window started from a file manager reads it too.
/// A run it starts is launched in this machine's herdr (Reached::launcher).
/// On this machine, an agent's commands go to the herdr server its run saved (RunState::herdr_server), whatever
/// server serve's own environment picks; an agent of a run saved before runs saved it is reached in the one serve's
/// environment picks (HERDR_SOCKET_PATH, else HERDR_SESSION, else the default session). Errors: the machine is not
/// saved or enabled, or the directory cannot be entered; for a project on a machine, serve's own working directory
/// cannot be told.
pub fn project_host(machine: &str, cwd: &str, stopping: &Interrupt) -> Result<Reached, OrchestratorError> {
    let here = ProcessHost::local(stopping);
    let herdr = ProcessHerdr::new(Some(machine), stopping);
    let host = if machine.is_empty() { here } else { here.over_ssh(&herdr.ssh_target()?) };
    let dir = host.resolve_dir(cwd)?;
    let launch_dir = if machine.is_empty() { dir.clone() } else { serve_dir()? };
    let launcher = Arc::new(ProcessHerdr::new(None, stopping));
    Ok(Reached { host: Arc::new(host), dir, console: Arc::new(herdr), launcher, launch_dir })
}

/// serve's working directory. Errors: it cannot be told, or is not UTF-8.
fn serve_dir() -> Result<String, OrchestratorError> {
    let dir = std::env::current_dir().map_err(|e| OrchestratorError(format!("cannot tell serve's directory: {e}")))?;
    dir.into_os_string().into_string().map_err(|d| OrchestratorError(format!("serve's directory {d:?} is not UTF-8")))
}

/// serve's HTTP server, on a free port of 127.0.0.1, with its token, answering until it is dropped. Its endpoints
/// but /setup/checked and /workflows each name the project by ?cwd= and an optional ?machine=, and answer JSON; each
/// is a GET but the four POSTs:
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
/// - /setup?cwd=DIR: {"checks", "passed"}, the project's setup checks as Setup::checks gives them; 400 for a cwd
///   missing or empty. It does not reach the project as the others do, so that a machine that is not saved or a
///   folder that is missing shows as a failed check, not as 502.
/// - /setup/checked: {"checked"}, whether the setup dialog has closed with every Required check passed
///   (Setup::checked); 500 when that cannot be told.
/// - POST /setup/checked: records that it has (Setup::mark), whatever the checks say, and answers {"checked": true};
///   500 when the marker cannot be written.
/// - POST /setup/gate?cwd=DIR: sets the project up for `--quality-gate local` (ProjectRuns::set_up_gate) and answers
///   {"files": [{"path", "written"}, ...]}, written false for a file it kept; 400 for a cwd missing, 502 for a project
///   that cannot be reached or a setup that fails, as for a cwd in no git checkout.
/// - /workflows: what a run can be started with, as start::offered_workflows gives it from the configuration
///   directory; 500 when its workflows directory cannot be listed.
/// - POST /run?cwd=DIR: starts the run the JSON body asks for (start::RunRequest::parse), as a new run id, in a new
///   herdr workspace of this machine (Reached::launcher) where its `run` command can be watched and interrupted, and
///   waits for it (start::START_WAIT, ProjectRuns::start). 201 {"run"} once it has saved its state.json; 422 {"error",
///   "run"} when `run` exited without saving one, error ending with the last lines its pane showed; 202 {"run",
///   "listed": false} after 30 seconds of neither. A 201 or 202 adds "herdr_session", the named herdr session the
///   run's workspace is in, when serve started one to launch it in (herdr::OWN_SESSION); without it, the workspace
///   is in the herdr server serve's own environment reaches. 400 for a body RunRequest::parse refuses and 413 as for
///   /agent/input, both before the project is reached; 502 when herdr cannot launch it, as when it has
///   no server running here and none can be started.
/// - /events: a text/event-stream of RunWatch's events, polled every SERVE_POLL.local, every SERVE_POLL.remote for a
///   --machine project, until the client goes or the server stops. An HTTP/1.1 client gets it chunked, so that the
///   end of the stream is the end of the response; an HTTP/1.0 client's ends when the connection closes.
///
/// Every request is checked in this order: a Host header other than the server's own address, or an Origin other
/// than its own or one of its origins, gets 403 {"error"}; a CORS preflight (OPTIONS) that passes gets 204, allowing
/// GET and POST with an Authorization and a Content-Type header; a GET or POST then needs the server's token as an
/// `Authorization: Bearer` header or, for an EventSource, which cannot set one, as ?token=, compared in constant time;
/// else 401. 404 is an unknown endpoint, 400 a missing or malformed parameter, 502 a project that could not be read
/// or herdr failing, each with {"error"}; a POST of any other endpoint, and any other method, gets 501 with an HTML
/// error page. A request with a byte beyond ASCII in its request line or a header is not
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
    /// By the machine and directory a request names, the project connect gave for them, each connected once.
    projects: Mutex<HashMap<(String, String), Arc<ProjectRuns>>>,
    /// What /setup and /setup/checked answer from.
    setup: Setup,
    /// How often its event streams poll their projects.
    polls: Polls,
    /// Where /workflows finds the workflow files.
    config: ConfigDir,
    /// How POST /run waits for the run it starts.
    start_wait: StartWait,
}

impl RunServer {
    /// Bind a free port of 127.0.0.1 and start answering on it, also requests from origins (the desktop app's webview,
    /// such as tauri://localhost), each project reached by connect, its setup checked and marked by setup, the
    /// workflows a run can be started with read from config. Errors: no port can be bound, or no token made from the
    /// system's random bytes, or no thread started.
    pub fn start(origins: &[String], connect: Box<Connect>, setup: Setup, config: ConfigDir) -> io::Result<RunServer> {
        RunServer::start_polling(origins, connect, setup, config, SERVE_POLL, START_WAIT)
    }

    /// As start, the event streams polling every polls, and POST /run waiting as start_wait says.
    pub(crate) fn start_polling(
        origins: &[String],
        connect: Box<Connect>,
        setup: Setup,
        config: ConfigDir,
        polls: Polls,
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
            polls,
            config,
            start_wait,
        });
        let listening = Arc::new(Mutex::new(Some(Arc::new(on(&listener)?))));
        let (dispatched, to) = (Arc::clone(&listening), Arc::clone(&shared));
        let dispatcher =
            std::thread::Builder::new().name("serve".to_string()).spawn(move || dispatch(&listener, &dispatched, &to))?;
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
            let _ = std::thread::Builder::new().name("serve-request".to_string()).spawn(move || answer(&shared, request));
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
fn lock(listening: &Mutex<Option<Arc<tiny_http::Server>>>) -> std::sync::MutexGuard<'_, Option<Arc<tiny_http::Server>>> {
    listening.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Shared {
    /// The project in directory cwd, which may start with ~, on the saved herdr machine, or here with machine empty.
    /// Errors: it cannot be reached.
    fn project(&self, machine: &str, cwd: &str) -> Result<Arc<ProjectRuns>, OrchestratorError> {
        let key = (machine.to_string(), cwd.to_string());
        if let Some(project) = self.lock_projects().get(&key) {
            return Ok(Arc::clone(project));
        }
        let reached = (self.connect)(machine, cwd, &self.stopping)?;
        let project = Arc::new(ProjectRuns::new(reached, target_args(machine, cwd)));
        self.lock_projects().insert(key, Arc::clone(&project));
        Ok(project)
    }

    fn lock_projects(&self) -> std::sync::MutexGuard<'_, HashMap<(String, String), Arc<ProjectRuns>>> {
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
            "/events" => (Endpoint::Events, &["cwd"]),
            _ => return None,
        })
    }
}

/// The endpoints a POST may name.
const AGENT_INPUT: &str = "/agent/input";
const SETUP_CHECKED: &str = "/setup/checked";
const SETUP_GATE: &str = "/setup/gate";
const RUN: &str = "/run";
/// The GET endpoints outside Endpoint, which reach no project (unreached_answer): the setup dialog's checks, and the
/// workflows a run can be started with.
const SETUP: &str = "/setup";
const WORKFLOWS: &str = "/workflows";

/// What a request answered with JSON gets: its status and body.
type Answer = (u16, Value);

/// Answer request, as RunServer's doc comment says.
fn answer(shared: &Shared, request: Request) {
    let (path, query) = target(&request);
    match request.method().as_str() {
        "GET" => get(shared, request, &path, &query),
        "POST" if path == AGENT_INPUT => post_input(shared, request, &query),
        "POST" if path == SETUP_CHECKED => post_checked(shared, request, &query),
        "POST" if path == SETUP_GATE => post_setup_gate(shared, request, &query),
        "POST" if path == RUN => post_run(shared, request, &query),
        "OPTIONS" => preflight(shared, request),
        method => {
            let method = format!("{method:?}");
            send(request, unsupported(&method));
        }
    }
}

/// The path of request's target and its query's parameters, a parameter given twice taking its last value.
fn target(request: &Request) -> (String, HashMap<String, String>) {
    // A target that starts with // is read with its leading slashes as one, never as a //host authority, so that
    // no answer can redirect a client to another host.
    let target = request.url();
    let target = if target.starts_with("//") { format!("/{}", target.trim_start_matches('/')) } else { target.to_string() };
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
    let param = |name: &str| query.get(name).map_or("", String::as_str);
    shared.project(param("machine"), param("cwd")).map_err(|e| (502, error(&e.0)))
}

/// The answer to a read of a run that failed with e.
fn read_failure(e: RunReadError) -> Answer {
    match e {
        RunReadError::Refused(why) => (403, error(&why)),
        RunReadError::Missing(why) => (404, error(&why)),
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

/// The answer to a GET of /setup, /setup/checked or /workflows at path with the query's parameters, as RunServer's
/// doc comment gives them; None for any other path. Unlike Endpoint's they reach no project: the setup checks check
/// what one needs, and the workflows are this machine's, where a run serve starts runs.
fn unreached_answer(shared: &Shared, path: &str, query: &HashMap<String, String>) -> Option<Answer> {
    Some(match path {
        SETUP => match query.get("cwd").map(String::as_str) {
            None => (400, error("missing parameter cwd")),
            Some("") => (400, error("parameter cwd is empty")),
            Some(cwd) => {
                let machine = query.get("machine").map_or("", String::as_str);
                (200, shared.setup.checks(machine, cwd, &shared.stopping))
            }
        },
        SETUP_CHECKED => match shared.setup.checked() {
            Ok(checked) => (200, object([("checked", Value::from(checked))])),
            Err(why) => (500, error(&why)),
        },
        WORKFLOWS => match start::offered_workflows(&shared.config) {
            Ok(workflows) => (200, workflows),
            Err(e) => (500, error(&e.0)),
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
            let runs = Value::Array(records.into_iter().map(Value::Object).collect());
            (200, object([("version", Value::from(RUN_RECORD_VERSION)), ("runs", runs)]))
        }),
        Endpoint::Files => project.files(run).map(|files| (200, object([("run", Value::from(run)), ("files", files)]))),
        Endpoint::File => project.read(run, name).map(|text| match text {
            Some(text) => (200, object([("run", Value::from(run)), ("name", Value::from(name)), ("text", Value::from(text))])),
            None => (404, error(&format!("no file {name} in run {run}"))),
        }),
        Endpoint::Agent => match agent_lines(query.get("lines")) {
            Err(answer) => Ok(answer),
            Ok(lines) => project.agent_text(run, role, lines).map(|text| {
                (200, object([("run", Value::from(run)), ("role", Value::from(role)), ("text", Value::from(text))]))
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

/// Answer a POST of /setup/checked, which needs no parameter, by writing the setup marker. It trusts the caller that
/// every Required check passed rather than checking again, which would run the checks twice for one Close: the marker
/// only keeps the dialog from opening by itself, and "Check setup…" opens it whatever the marker says.
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

/// Answer a POST of /setup/gate with the query's parameters, as target gives them, by setting the project up for the
/// local quality gate.
fn post_setup_gate(shared: &Shared, request: Request, query: &HashMap<String, String>) {
    if let Some((code, body)) = refusal(shared, &request, query) {
        return send_json(request, code, &body);
    }
    let set_up = project_of(shared, query, &["cwd"])
        .and_then(|project| project.set_up_gate().map_err(|e| (502, error(&e.0))));
    let (code, body) = match set_up {
        Ok(files) => {
            let files = files.into_iter().map(|f| object([("path", Value::from(f.path)), ("written", Value::from(f.written))]));
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

/// The answer to POST /run once its body is run and its project project: the run started, by this program, as a new
/// run id, and waited for.
fn start_answer(shared: &Shared, project: &ProjectRuns, run: &RunRequest) -> Answer {
    let program = match serve_program() {
        Ok(path) => path,
        Err(why) => return (500, error(&why)),
    };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    let run_id = match crate::cli::new_run_id(now, &crate::workflow::machine_zone()) {
        Ok(id) => id,
        Err(e) => return (500, error(&e.0)),
    };
    let (ended, herdr_session) = match project.start(run, &program, &run_id, shared.start_wait, &shared.stopping) {
        Ok(started) => started,
        Err(e) => return read_failure(e),
    };
    if let Started::Exited(shown) = ended {
        let why = format!("run {run_id} exited before it saved its state; its pane showed:\n{shown}");
        return (422, object([("error", Value::from(why)), ("run", Value::from(run_id))]));
    }
    // A run that started: its id, whether it is listed yet, and the herdr session it is in.
    let mut fields = crate::json::object([("run", Value::from(run_id))]);
    let code = if matches!(ended, Started::Unlisted) {
        fields.insert("listed".to_string(), Value::from(false));
        202
    } else {
        201
    };
    if let Some(session) = herdr_session {
        fields.insert("herdr_session".to_string(), Value::from(session));
    }
    (code, Value::Object(fields))
}

/// The path of serve's own program, which POST /run starts its runs by. Errors: it cannot be told, or is not UTF-8.
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
    let parsed = std::str::from_utf8(&body).ok().and_then(|text| serde_json::from_str::<Value>(text).ok()).ok_or_else(shape)?;
    let fields = parsed.as_object().filter(|o| o.len() == 1).ok_or_else(shape)?;
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
    let interval = if project.remote() { shared.polls.remote } else { shared.polls.local };
    while !shared.stopping.is_raised() {
        let events = watch.poll();
        // A poll the stop cut short reports the commands it stopped, which is no problem of the project's.
        if shared.stopping.is_raised() {
            break;
        }
        let text: String = if events.is_empty() { ":\n\n".to_string() } else { events.iter().map(Event::sse).collect() };
        let part = if chunked { format!("{:X}\r\n{text}\r\n", text.len()) } else { text };
        if out.write_all(part.as_bytes()).and_then(|()| out.flush()).is_err() {
            return;
        }
        shared.stopping.sleep(interval);
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
    let mut response = with_header(Response::from_data(body.to_string()).with_status_code(code), "Content-Type", "application/json");
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

/// The 501 a request of method gets, an HTML error page naming it; the caller passes method already quoted.
fn unsupported(method: &str) -> Response<io::Cursor<Vec<u8>>> {
    let escaped = method.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let page = format!(
        "<!DOCTYPE HTML>\n<html lang=\"en\">\n    <head>\n        <meta charset=\"utf-8\">\n        \
         <style type=\"text/css\">\n            :root {{\n                color-scheme: light dark;\n            }}\n        \
         </style>\n        <title>Error response</title>\n    </head>\n    <body>\n        <h1>Error response</h1>\n        \
         <p>Error code: 501</p>\n        <p>Message: Unsupported method ({escaped}).</p>\n        \
         <p>Error code explanation: 501 - Server does not support this operation.</p>\n    </body>\n</html>\n"
    );
    let response = with_header(Response::from_data(page).with_status_code(501), "Connection", "close");
    with_header(response, "Content-Type", "text/html;charset=utf-8")
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

/// The --machine and --cwd flags a request's machine and cwd stand for, each left out when empty.
fn target_args(machine: &str, cwd: &str) -> Vec<String> {
    let given = |flag: &str| (!flag.is_empty()).then(|| flag.to_string());
    crate::cli::Target { machine: given(machine), cwd: given(cwd) }.flags()
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
