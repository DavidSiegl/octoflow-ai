//! serve's endpoints, security checks and events, here and on a machine, and serve_command's in-process cases: a
//! RunServer on a free port of 127.0.0.1, asked over a TCP connection of the test's own, so that each header
//! and byte on the wire is the test's. The process cases are engine/tests/program.rs'.

use std::io::{BufRead, BufReader};
use std::net::TcpStream;
use std::sync::mpsc;
use std::time::Instant;

use super::*;
use crate::cli::doctor::{Check, Level};
use crate::cli::listing::run_record;
use crate::constants::{APPROVE, RUNS_DIR, STALE_SECONDS, WORKTREES_DIR};
use crate::herdr::{HerdrError, HerdrServer, OWN_SESSION};
use crate::host::tests::Repo;
use crate::http::quote;
use crate::json::{Object, ValueExt};
use crate::serve::fake::{reached, FakeConsole, ServeHost};
use crate::state::RunState;
use crate::test_support::TempDir;

/// The pairs as a query string, each name and value quoted (crate::http::quote), in the order given.
fn urlencode(pairs: &[(&str, &str)]) -> String {
    pairs.iter().map(|(name, value)| format!("{}={}", quote(name), quote(value))).collect::<Vec<_>>().join("&")
}

const SERVE_ID: &str = "20261024-120000-5e7e00";
const TAURI_ORIGIN: &str = "tauri://localhost";

fn run_dir() -> String {
    format!("/proj/{RUNS_DIR}/{SERVE_ID}")
}

/// A run's state.json as serve's tests save it, for run_id: a fresh run in round 1's build, with the extra keys.
fn serve_state(run_id: &str, extra: &[(&str, Value)]) -> String {
    let mut fresh = crate::state::RunState::new(run_id, "add a rate limiter", "/proj");
    (fresh.phase, fresh.round) = ("build".into(), 1);
    let Value::Object(mut state) = fresh.to_json() else { unreachable!("a state is saved as an object") };
    for (key, value) in extra {
        state.insert(key.to_string(), value.clone());
    }
    Value::Object(state).to_string()
}

/// A ServeHost for /proj that holds the run's state, as serve_state() saves it.
fn host_with_run(ssh_target: Option<&str>) -> Arc<ServeHost> {
    let host = Arc::new(ServeHost::on(ssh_target));
    host.state().files.insert(format!("{}/state.json", run_dir()), serve_state(SERVE_ID, &[]));
    host
}

/// What a test's RunServer is started with, beside its projects and origins.
struct Options {
    /// How often its event streams poll.
    polls: Polls,
    /// Its setup checks and marker.
    setup: Setup,
    /// Where its workflow files are; by default a directory that is not there, so only the built-in ones.
    config: ConfigDir,
    /// How POST /run waits.
    wait: StartWait,
}

impl Default for Options {
    fn default() -> Options {
        Options { polls: SERVE_POLL, setup: unchecked(), config: ConfigDir::at("/nowhere/config"), wait: START_WAIT }
    }
}

/// A RunServer for a test, the projects it connected, as (machine, cwd), and the herdr of every project.
struct Served {
    server: RunServer,
    port: u16,
    connects: Arc<Mutex<Vec<(String, String)>>>,
    console: Arc<FakeConsole>,
}

impl Served {
    /// A server whose every project is host's /proj.
    fn fake(host: &Arc<ServeHost>, origins: &[&str]) -> Served {
        Served::fake_polling(host, origins, SERVE_POLL)
    }

    fn fake_polling(host: &Arc<ServeHost>, origins: &[&str], polls: Polls) -> Served {
        Served::fake_with(host, origins, Options { polls, ..Options::default() })
    }

    /// A server whose every project is host's /proj, started with options.
    fn fake_with(host: &Arc<ServeHost>, origins: &[&str], options: Options) -> Served {
        let host = Arc::clone(host);
        let connects = Arc::new(Mutex::new(vec![]));
        let seen = Arc::clone(&connects);
        let console = Arc::new(FakeConsole::default());
        let herdr = Arc::clone(&console);
        let connect = move |machine: &str, cwd: &str, _: &Interrupt| {
            seen.lock().unwrap().push((machine.to_string(), cwd.to_string()));
            Ok(reached(&host, &herdr))
        };
        Served { console, ..Served::start(Box::new(connect), origins, connects, options) }
    }

    /// A server whose projects are the real directories here.
    fn real() -> Served {
        Served::start(Box::new(project_host), &[], Arc::default(), Options::default())
    }

    fn start(
        connect: Box<Connect>,
        origins: &[&str],
        connects: Arc<Mutex<Vec<(String, String)>>>,
        options: Options,
    ) -> Served {
        let origins: Vec<String> = origins.iter().map(|o| o.to_string()).collect();
        let Options { polls, setup, config, wait } = options;
        let server =
            RunServer::start_polling(&origins, connect, setup, config, polls, wait).expect("the server starts");
        let port = port_of(server.url());
        Served { server, port, connects, console: Arc::default() }
    }

    fn token(&self) -> String {
        self.server.token().to_string()
    }

    /// The reply to a GET of target with the token as a Bearer header, the server's Host and the headers given.
    fn get(&self, target: &str, headers: &[(&str, &str)]) -> Reply {
        let bearer = format!("Bearer {}", self.token());
        let mut all = vec![("Authorization", bearer.as_str())];
        all.extend_from_slice(headers);
        self.ask("GET", target, &all)
    }

    /// The reply to a POST of target with body, the token as a Bearer header and the server's Host.
    fn post(&self, target: &str, body: &str) -> Reply {
        let bearer = format!("Bearer {}", self.token());
        let length = body.len().to_string();
        let headers =
            [("Authorization", bearer.as_str()), ("Content-Type", "application/json"), ("Content-Length", &length)];
        self.ask_with_body("POST", target, &headers, body)
    }

    /// The reply to method of target, with the server's Host header unless headers name one, and headers, each value's
    /// characters sent as Latin-1 bytes.
    fn ask(&self, method: &str, target: &str, headers: &[(&str, &str)]) -> Reply {
        self.ask_with_body(method, target, headers, "")
    }

    /// As ask, with body after the headers.
    fn ask_with_body(&self, method: &str, target: &str, headers: &[(&str, &str)], body: &str) -> Reply {
        let mut raw = format!("{method} {target} HTTP/1.1\r\nConnection: close\r\n").into_bytes();
        if !headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("Host")) {
            raw.extend_from_slice(format!("Host: 127.0.0.1:{}\r\n", self.port).as_bytes());
        }
        for (name, value) in headers {
            raw.extend_from_slice(format!("{name}: ").as_bytes());
            raw.extend(value.chars().map(|c| u8::try_from(u32::from(c)).expect("a Latin-1 header")));
            raw.extend_from_slice(b"\r\n");
        }
        raw.extend_from_slice(b"\r\n");
        raw.extend_from_slice(body.as_bytes());
        exchange(self.port, &raw)
    }

    /// endpoint with cwd=~/proj and params as its query.
    fn query(endpoint: &str, params: &[(&str, &str)]) -> String {
        let mut all = vec![("cwd", "~/proj")];
        all.extend_from_slice(params);
        format!("{endpoint}?{}", urlencode(&all))
    }
}

fn port_of(url: &str) -> u16 {
    url.strip_prefix("http://127.0.0.1:").and_then(|p| p.parse().ok()).expect("a url of 127.0.0.1 with a port")
}

/// A reply as the client read it: its status (None when the connection closed unanswered), headers and body.
#[derive(Debug)]
struct Reply {
    status: Option<u16>,
    headers: Vec<(String, String)>,
    body: String,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    fn json(&self) -> Value {
        assert_eq!(self.header("Content-Type"), Some("application/json"), "{self:?}");
        serde_json::from_str::<Value>(&self.body).expect("a JSON body")
    }

    fn error(&self) -> String {
        self.json().get("error").map(Value::text).expect("an error")
    }
}

/// Send raw on a new connection to port, and read the reply to the connection's end, or for 5 s at most: an event
/// stream the server should have refused never ends, and fails the test on what came meanwhile.
fn exchange(port: u16, raw: &[u8]) -> Reply {
    let mut conn = TcpStream::connect(("127.0.0.1", port)).expect("the server takes the connection");
    conn.write_all(raw).expect("the request is sent");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut got = vec![];
    let mut buf = [0u8; 4096];
    while let Some(left) = deadline.checked_duration_since(Instant::now()).filter(|d| !d.is_zero()) {
        conn.set_read_timeout(Some(left)).unwrap();
        // A reset after the reply is read is the end of it too, as is a read that times out at the deadline.
        match conn.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => got.extend_from_slice(&buf[..n]),
        }
    }
    let text = String::from_utf8_lossy(&got).into_owned();
    let Some((head, body)) = text.split_once("\r\n\r\n") else {
        return Reply { status: None, headers: vec![], body: text };
    };
    let mut lines = head.split("\r\n");
    let status = lines.next().and_then(|l| l.split(' ').nth(1)).and_then(|s| s.parse().ok());
    let headers = lines.filter_map(|l| l.split_once(": ")).map(|(n, v)| (n.to_string(), v.to_string())).collect();
    Reply { status, headers, body: body.to_string() }
}

/// The status of a GET of / at port; None when the connection is refused or closed unanswered.
fn status_at(port: u16) -> Option<u16> {
    let mut conn = TcpStream::connect(("127.0.0.1", port)).ok()?;
    conn.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    conn.write_all(b"GET / HTTP/1.0\r\n\r\n").ok()?;
    let mut got = String::new();
    let _ = conn.read_to_string(&mut got);
    got.split(' ').nth(1).and_then(|s| s.parse().ok())
}

/// A Setup for a test that asks for no setup: checking a project fails the test, and the marker is nowhere.
fn unchecked() -> Setup {
    Setup::new(
        Box::new(|_: &str, _: &str, _: &Interrupt| panic!("no setup check was expected")),
        "/nowhere/setup-checked".into(),
    )
}

/// A project directory of the test's own, which serve is asked about by its path, removed when it is dropped.
struct Project(TempDir);

impl Project {
    fn new() -> Project {
        Project(TempDir::new())
    }

    /// The directory's path, which a request's cwd names.
    fn root(&self) -> String {
        self.0.path()
    }

    fn write(&self, path: &str, text: &str) {
        let path = self.0.join(path);
        std::fs::create_dir_all(std::path::Path::new(&path).parent().unwrap()).unwrap();
        std::fs::write(path, text).expect("the file is written");
    }

    fn save(&self, run_id: &str, extra: &[(&str, Value)]) {
        self.write(&format!("{RUNS_DIR}/{run_id}/state.json"), &serve_state(run_id, extra));
    }

    fn query(&self, endpoint: &str) -> String {
        format!("{endpoint}?{}", urlencode(&[("cwd", &self.root())]))
    }
}

/// record without the keys that age with every read.
fn without_aging(mut record: Object) -> Value {
    for key in runs::AGING_RECORD_KEYS {
        record.remove(key);
    }
    Value::Object(record)
}

// -- The endpoints -----------------------------------------------------------------------------------------------

#[test]
fn runs_are_list_json_records_on_a_machine() {
    let host = Arc::new(ServeHost::on(Some("remote-host")));
    let owner = object([
        ("host", Value::from(crate::runs::local_host())),
        ("pid", Value::from(i64::from(std::process::id()))),
        ("started_at", Value::from("x")),
    ]);
    host.state().files.insert(format!("{}/state.json", run_dir()), serve_state(SERVE_ID, &[("owner", owner)]));
    host.state().ages.insert(SERVE_ID.to_string(), STALE_SECONDS + 1);
    let served = Served::fake(&host, &[]);
    let reply = served.get(&Served::query("/runs", &[("machine", "remote")]), &[]);
    assert_eq!(reply.status, Some(200));
    assert_eq!(*served.connects.lock().unwrap(), [("remote".to_string(), "~/proj".to_string())]);
    // As list --json --machine remote --cwd ~/proj prints it.
    let target = ["--machine", "remote", "--cwd", "~/proj"].map(String::from);
    let listed: Vec<Value> = host
        .run_states_inside("/proj")
        .unwrap()
        .iter()
        .map(|s| Value::Object(run_record(s, &crate::runs::local_host(), &crate::runs::pid_alive, &target)))
        .collect();
    let doc = reply.json();
    assert_eq!(doc, object([("version", Value::from(1)), ("runs", Value::Array(listed))]));
    assert_eq!(
        doc.get("runs").and_then(Value::as_array).and_then(|r| r[0].get("resume_command")),
        Some(&Value::from("octoflow resume 5e7e00 --machine remote --cwd '~/proj'"))
    );
}

#[test]
fn runs_are_list_json_records_locally() {
    let d = Project::new();
    d.save(SERVE_ID, &[]);
    d.save("x-beef00", &[("phase", Value::from("done")), ("verdict", Value::from(APPROVE))]);
    let served = Served::real();
    let reply = served.get(&d.query("/runs"), &[]);
    assert_eq!(reply.status, Some(200));
    let target = ["--cwd".to_string(), d.root()];
    // A second may pass between the two reads; everything else is the same.
    let listed: Vec<Value> = ProcessHost::local(&Interrupt::new())
        .run_states(&d.root())
        .unwrap()
        .iter()
        .map(|s| without_aging(run_record(s, &crate::runs::local_host(), &crate::runs::pid_alive, &target)))
        .collect();
    let doc = reply.json();
    assert_eq!(doc.get("version"), Some(&Value::from(1)));
    let served_runs: Vec<Value> = doc
        .get("runs")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .map(|r| without_aging(r.as_object().unwrap().clone()))
        .collect();
    assert_eq!(served_runs.len(), 2);
    assert_eq!(served_runs, listed);
}

#[test]
fn a_project_is_connected_once() {
    let served = Served::fake(&Arc::new(ServeHost::on(None)), &[]);
    served.get(&Served::query("/runs", &[]), &[]);
    served.get(&Served::query("/runs", &[]), &[]);
    served.get(&Served::query("/runs", &[("machine", "")]), &[]);
    assert_eq!(*served.connects.lock().unwrap(), [(String::new(), "~/proj".to_string())]);
}

#[test]
fn files_with_their_mtimes() {
    let host = host_with_run(None);
    {
        let mut s = host.state();
        s.files.insert(format!("{}/spec.md", run_dir()), "# Spec\n".to_string());
        s.files.insert(format!("{}/build-1.md", run_dir()), "built".to_string());
        s.mtimes.insert(format!("{}/spec.md", run_dir()), 100);
        s.mtimes.insert(format!("{}/build-1.md", run_dir()), 200);
    }
    let served = Served::fake(&host, &[]);
    let reply = served.get(&Served::query("/files", &[("run", SERVE_ID)]), &[]);
    assert_eq!(reply.status, Some(200));
    assert_eq!(
        reply.body,
        format!(
            r#"{{"run":"{SERVE_ID}","files":[{{"name":"spec.md","mtime":100}},{{"name":"build-1.md","mtime":200}}]}}"#
        )
    );
}

#[test]
fn file() {
    let host = Arc::new(ServeHost::on(None));
    host.state().files.insert(format!("{}/spec.md", run_dir()), "# Spec\n".to_string());
    let served = Served::fake(&host, &[]);
    let reply = served.get(&Served::query("/file", &[("run", SERVE_ID), ("name", "spec.md")]), &[]);
    assert_eq!(reply.status, Some(200));
    assert_eq!(reply.body, format!(r##"{{"run":"{SERVE_ID}","name":"spec.md","text":"# Spec\n"}}"##));
}

#[test]
fn missing_file() {
    let served = Served::fake(&Arc::new(ServeHost::on(None)), &[]);
    let reply = served.get(&Served::query("/file", &[("run", SERVE_ID), ("name", "review-1.md")]), &[]);
    assert_eq!(reply.status, Some(404));
    assert_eq!(reply.error(), format!("no file review-1.md in run {SERVE_ID}"));
}

#[test]
fn paths_outside_the_run_directory_are_refused_unread() {
    let host = host_with_run(None);
    host.state().files.insert("/proj/secret".to_string(), "secret".to_string());
    let served = Served::fake(&host, &[]);
    let run_dot_dot = format!("{SERVE_ID}/..");
    let cases: [(&str, &str); 11] = [
        (SERVE_ID, "../../../secret"),
        (SERVE_ID, "/proj/secret"),
        (SERVE_ID, "state.json"),
        (SERVE_ID, ".."),
        (SERVE_ID, "."),
        (SERVE_ID, ""),
        (SERVE_ID, "spec\0.md"),
        ("..", "secret"),
        (".", "secret"),
        (&run_dot_dot, "secret"),
        ("/proj", "secret"),
    ];
    for (run, name) in cases {
        let reply = served.get(&Served::query("/file", &[("run", run), ("name", name)]), &[]);
        assert_eq!(reply.status, Some(403), "{run:?} {name:?}");
        assert_eq!(reply.json().get("text"), None);
        let why = if name == "state.json" {
            "state.json is not a handoff file".to_string()
        } else {
            format!("{:?} is not a name in the run directory", if run == SERVE_ID { name } else { run })
        };
        assert_eq!(reply.error(), why, "{run:?} {name:?}");
    }
    assert_eq!(host.state().reads, Vec::<String>::new());
}

#[test]
fn files_of_a_run_outside_the_runs_directory_are_refused() {
    let served = Served::fake(&Arc::new(ServeHost::on(None)), &[]);
    for run in ["../..", "..", "/proj", ""] {
        assert_eq!(served.get(&Served::query("/files", &[("run", run)]), &[]).status, Some(403), "{run:?}");
    }
}

#[test]
fn a_link_that_leads_out_of_the_run_directory_is_refused() {
    // An agent may leave a symbolic link in its run directory, and a run directory may itself be one; the name asked
    // for is still a plain name (serve-run-files/02, #231).
    let d = Project::new();
    d.save(SERVE_ID, &[]);
    d.write("outside.txt", "outside");
    d.write(&format!("{RUNS_DIR}/{SERVE_ID}/spec.md"), "spec");
    let (runs, link) =
        (format!("{}/{RUNS_DIR}", d.root()), |target: &str, at: &str| std::os::unix::fs::symlink(target, at).unwrap());
    link(&format!("{}/outside.txt", d.root()), &format!("{runs}/{SERVE_ID}/out.md"));
    link("../../../outside.txt", &format!("{runs}/{SERVE_ID}/up.md"));
    link("spec.md", &format!("{runs}/{SERVE_ID}/in.md"));
    link(&d.root(), &format!("{runs}/elsewhere"));
    link(SERVE_ID, &format!("{runs}/alias"));
    let served = Served::real();
    let file = |run: &str, name: &str| {
        served.get(&format!("/file?{}", urlencode(&[("cwd", &d.root()), ("run", run), ("name", name)])), &[])
    };
    let names = |run: &str| {
        let reply = served.get(&format!("/files?{}", urlencode(&[("cwd", &d.root()), ("run", run)])), &[]);
        assert_eq!(reply.status, Some(200), "{reply:?}");
        let mut names: Vec<String> = reply
            .json()
            .get("files")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .map(|f| f.get("name").unwrap().text())
            .collect();
        names.sort();
        names
    };
    // A link that leads out is neither read nor listed; one that stays inside is both.
    for name in ["out.md", "up.md"] {
        let reply = file(SERVE_ID, name);
        assert_eq!((reply.status, reply.json().get("text")), (Some(403), None), "{name}");
        assert_eq!(
            reply.error(),
            format!("{name:?} leads out of run {SERVE_ID}'s directory, or the directory out of the runs directory")
        );
    }
    assert_eq!(
        (file(SERVE_ID, "in.md").status, file(SERVE_ID, "in.md").json().get("text")),
        (Some(200), Some(&Value::from("spec")))
    );
    assert_eq!(names(SERVE_ID), ["in.md", "spec.md"]);
    // A run directory that leads out of the runs directory lists nothing and reads nothing; one that is a link to
    // another run directory is that run's.
    assert_eq!(names("elsewhere"), Vec::<String>::new());
    assert_eq!(file("elsewhere", "outside.txt").status, Some(403));
    assert_eq!(file("alias", "spec.md").json().get("text"), Some(&Value::from("spec")));
    assert_eq!(names("alias"), ["in.md", "spec.md"]);
}

#[test]
fn a_run_whose_state_json_leads_out_of_its_run_directory_is_no_run() {
    // serve-run-files/02's ruling (2026-11-04, the user): a run's state.json is refused as its files are, so a
    // run that is a link out of the runs directory, or whose state.json is a link out of its run directory, is left
    // out of /runs and the event stream, as a run without a state.json is.
    let d = Project::new();
    d.save(SERVE_ID, &[]);
    let runs = format!("{}/{RUNS_DIR}", d.root());
    let link = |target: &str, at: &str| std::os::unix::fs::symlink(target, at).unwrap();
    d.write("elsewhere/state.json", &serve_state("x-away00", &[]));
    link(&format!("{}/elsewhere", d.root()), &format!("{runs}/x-away00"));
    d.write("outside.json", &serve_state("x-out000", &[]));
    std::fs::create_dir_all(format!("{runs}/x-out000")).unwrap();
    link(&format!("{}/outside.json", d.root()), &format!("{runs}/x-out000/state.json"));
    // A state.json that is a link inside its own run directory is that run's.
    d.write(&format!("{RUNS_DIR}/x-in0000/saved.json"), &serve_state("x-in0000", &[]));
    link("saved.json", &format!("{runs}/x-in0000/state.json"));
    let served = Served::real();
    let reply = served.get(&d.query("/runs"), &[]);
    let mut ids: Vec<String> = reply
        .json()
        .get("runs")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .map(|r| r.get("id").unwrap().text())
        .collect();
    ids.sort();
    assert_eq!((reply.status, ids), (Some(200), vec![SERVE_ID.to_string(), "x-in0000".to_string()]));
    let agent = served
        .get(&format!("/agent?{}", urlencode(&[("cwd", &d.root()), ("run", "x-out000"), ("role", "build")])), &[]);
    assert_eq!((agent.status, agent.error()), (Some(404), "no run x-out000".to_string()));
    // The stream gives a record and a files event for each run it lists, and none for the two refused.
    let stream = EventStream::open(&served, &d.query("/events"), "");
    let mut recorded: Vec<String> = (0..4)
        .map(|_| stream.next(Duration::from_secs(2)))
        .filter(|(kind, _)| kind == "record")
        .map(|(_, payload)| payload.get("run").unwrap().text())
        .collect();
    recorded.sort();
    assert_eq!(recorded, [SERVE_ID, "x-in0000"]);
}

#[test]
fn missing_parameter() {
    let served = Served::fake(&Arc::new(ServeHost::on(None)), &[]);
    for (path, missing) in [
        ("/runs".to_string(), "cwd"),
        (Served::query("/file", &[("run", SERVE_ID)]), "name"),
        (Served::query("/files", &[]), "run"),
        ("/file".to_string(), "cwd, run, name"),
        ("/events?machine=x".to_string(), "cwd"),
    ] {
        let reply = served.get(&path, &[]);
        assert_eq!(reply.status, Some(400), "{path}");
        assert_eq!(reply.error(), format!("missing parameter {missing}"));
    }
    assert_eq!(*served.connects.lock().unwrap(), Vec::<(String, String)>::new());
}

/// Each (machine, cwd) a project's setup was checked for, in order.
type Asked = Arc<Mutex<Vec<(String, String)>>>;

/// A server for the setup tests, its marker at marker, its every project checked as checks, each check's (machine,
/// cwd) recorded in the Asked it returns.
fn setup_served(marker: &str, checks: Vec<Check>) -> (Served, Asked) {
    let asked = Arc::new(Mutex::new(vec![]));
    let seen = Arc::clone(&asked);
    let check = move |machine: &str, cwd: &str, _: &Interrupt| {
        seen.lock().unwrap().push((machine.to_string(), cwd.to_string()));
        checks.clone()
    };
    let setup = Setup::new(Box::new(check), marker.to_string());
    (Served::fake_with(&Arc::new(ServeHost::on(None)), &[], Options { setup, ..Options::default() }), asked)
}

fn a_check(name: &str, level: Level, ok: bool) -> Check {
    let fix = if ok { String::new() } else { format!("install {name}") };
    Check { name: name.to_string(), level, ok, detail: format!("{name} detail"), fix }
}

#[test]
fn setup_checks_the_project_without_reaching_it() {
    let checks = vec![a_check("herdr", Level::Required, true), a_check("gh", Level::NeededForPr, false)];
    let (served, asked) = setup_served("/nowhere/setup-checked", checks);
    let reply = served.get(&Served::query("/setup", &[("machine", "box")]), &[]);
    assert_eq!(reply.status, Some(200));
    assert_eq!(
        reply.body,
        r#"{"checks":[{"name":"herdr","level":"required","ok":true,"detail":"herdr detail","fix":""},"#.to_string()
            + r#"{"name":"gh","level":"needed-for-pr","ok":false,"detail":"gh detail","fix":"install gh"}],"passed":true}"#
    );
    assert_eq!(*asked.lock().unwrap(), [("box".to_string(), "~/proj".to_string())]);
    assert_eq!(*served.connects.lock().unwrap(), Vec::<(String, String)>::new(), "a project was reached");
}

#[test]
fn setup_has_not_passed_with_a_required_check_failed() {
    let checks = vec![a_check("herdr", Level::Required, false), a_check("codex CLI", Level::Optional, false)];
    let (served, _) = setup_served("/nowhere/setup-checked", checks);
    let reply = served.get(&Served::query("/setup", &[]), &[]);
    assert_eq!(reply.json().get("passed"), Some(&Value::from(false)));
}

#[test]
fn setup_needs_the_project() {
    let (served, asked) = setup_served("/nowhere/setup-checked", vec![]);
    let reply = served.get("/setup?machine=box", &[]);
    assert_eq!((reply.status, reply.error()), (Some(400), "missing parameter cwd".to_string()));
    // An empty cwd would check the sidecar's own directory, a project nobody named.
    let reply = served.get("/setup?cwd=&machine=box", &[]);
    assert_eq!((reply.status, reply.error()), (Some(400), "parameter cwd is empty".to_string()));
    assert!(asked.lock().unwrap().is_empty());
}

#[test]
fn the_setup_marker_is_written_and_read() {
    let d = Project::new();
    let marker = format!("{}/config/setup-checked", d.root());
    let (served, asked) = setup_served(&marker, vec![]);
    let checked = |served: &Served| served.get("/setup/checked", &[]).json().get("checked").cloned();
    assert_eq!(checked(&served), Some(Value::from(false)));
    let reply = served.post("/setup/checked", "");
    assert_eq!((reply.status, reply.body.as_str()), (Some(200), r#"{"checked":true}"#));
    assert_eq!(std::fs::read(&marker).expect("the marker is written"), b"");
    assert_eq!(checked(&served), Some(Value::from(true)));
    // Writing it again leaves it.
    assert_eq!(served.post("/setup/checked", "").status, Some(200));
    assert!(asked.lock().unwrap().is_empty(), "the marker needs no check");
}

#[test]
fn a_setup_marker_that_cannot_be_written_or_looked_for_is_500() {
    let d = Project::new();
    d.write("config", "a file where the directory should be");
    let marker = format!("{}/config/setup-checked", d.root());
    let (served, _) = setup_served(&marker, vec![]);
    let reply = served.post("/setup/checked", "");
    assert_eq!(reply.status, Some(500));
    assert!(reply.error().starts_with(&format!("cannot create the directory of {marker}: ")), "{}", reply.error());
    let reply = served.get("/setup/checked", &[]);
    assert_eq!(reply.status, Some(500));
    assert!(reply.error().starts_with(&format!("cannot look for {marker}: ")), "{}", reply.error());
}

#[test]
fn the_setup_marker_needs_the_token() {
    let (served, _) = setup_served("/nowhere/setup-checked", vec![]);
    let port = served.port;
    let raw = format!(
        "POST /setup/checked HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    assert_eq!(exchange(port, raw.as_bytes()).status, Some(401));
}

#[test]
fn setup_gate_names_each_file_and_pins_no_release() {
    let repo = crate::host::tests::Repo::new(&[]);
    let served = Served::real();
    let path = format!("/setup/gate?{}", urlencode(&[("cwd", &repo.path)]));
    let reply = served.post(&path, "");
    assert_eq!(reply.status, Some(200), "{reply:?}");
    assert_eq!(reply.json(), serde_json::json!({"files": [{"path": "gate.toml", "written": true}]}));
    assert!(!std::path::Path::new(&format!("{}/ci/gate.pin", repo.path)).exists(), "the project follows the built-in");
    let reply = served.post(&path, "");
    assert_eq!(reply.json(), serde_json::json!({"files": [{"path": "gate.toml", "written": false}]}));
}

#[test]
fn setup_gate_needs_a_checkout_and_a_cwd() {
    let repo = crate::host::tests::Repo::new(&[]);
    let served = Served::real();
    let reply = served.post(&format!("/setup/gate?{}", urlencode(&[("cwd", &repo.outside)])), "");
    assert_eq!(reply.status, Some(502), "{reply:?}");
    assert!(reply.error().contains("not a git repository"), "{reply:?}");
    let reply = served.post("/setup/gate", "");
    assert_eq!((reply.status, reply.error()), (Some(400), "missing parameter cwd".to_string()));
}

#[test]
fn unknown_endpoint() {
    let served = Served::fake(&Arc::new(ServeHost::on(None)), &[]);
    let reply = served.get(&Served::query("/state", &[]), &[]);
    assert_eq!((reply.status, reply.error()), (Some(404), "no endpoint /state".to_string()));
}

#[test]
fn an_unreadable_project_is_a_bad_gateway() {
    let connect = |_: &str, _: &str, _: &Interrupt| Err(OrchestratorError::new("no saved herdr machine named nosuch"));
    let served = Served::start(Box::new(connect), &[], Arc::default(), Options::default());
    let reply = served.get(&Served::query("/runs", &[("machine", "nosuch")]), &[]);
    assert_eq!(reply.status, Some(502));
    assert_eq!(reply.body, r#"{"error":"no saved herdr machine named nosuch"}"#);
}

#[test]
fn runs_that_cannot_be_read_are_a_bad_gateway() {
    let host = Arc::new(ServeHost::on(None));
    host.state().run_states_inside_error = Some("ssh: connection refused".to_string());
    let served = Served::fake(&host, &[]);
    let reply = served.get(&Served::query("/runs", &[]), &[]);
    assert_eq!((reply.status, reply.error()), (Some(502), "ssh: connection refused".to_string()));
}

#[test]
fn a_target_read_as_http_server_reads_it() {
    // A path that starts with //, a fragment, an absolute URL, and a parameter given twice, the last one counting.
    let host = Arc::new(ServeHost::on(None));
    host.state().files.insert(format!("{}/spec.md", run_dir()), "x".to_string());
    let served = Served::fake(&host, &[]);
    let base = format!("cwd=%7E%2Fproj&run={SERVE_ID}&name=nope&name=spec.md");
    for target in [
        format!("//file?{base}"),
        format!("/file?{base}#frag"),
        format!("http://127.0.0.1:{}/file?{base}", served.port),
    ] {
        let reply = served.get(&target, &[]);
        assert_eq!((reply.status, reply.json().get("text")), (Some(200), Some(&Value::from("x"))), "{target}");
    }
    let reply = served.get(&format!("/file?{base}&name=..%2Fx"), &[]);
    assert_eq!((reply.status, reply.error()), (Some(403), "\"../x\" is not a name in the run directory".to_string()));
}

// -- The security checks -----------------------------------------------------------------------------------------

#[test]
fn no_token() {
    let served = Served::fake(&Arc::new(ServeHost::on(None)), &[]);
    let reply = served.ask("GET", &Served::query("/runs", &[]), &[]);
    assert_eq!(reply.status, Some(401));
    assert_eq!(reply.header("WWW-Authenticate"), Some("Bearer"));
    assert_eq!(reply.error(), "the token is missing or wrong");
    assert_eq!(*served.connects.lock().unwrap(), Vec::<(String, String)>::new());
}

#[test]
fn wrong_token() {
    let served = Served::fake(&Arc::new(ServeHost::on(None)), &[]);
    let token = served.token();
    let longer = format!("Bearer {token}x");
    let shorter = format!("Bearer {}", &token[..token.len() - 1]);
    let last = if token.ends_with('A') { 'B' } else { 'A' };
    let other_last = format!("Bearer {}{last}", &token[..token.len() - 1]);
    let lower = format!("bearer {token}");
    for header in ["Bearer nope", &token, &longer, &shorter, &other_last, &lower, "Bearer"] {
        let reply = served.ask("GET", &Served::query("/runs", &[]), &[("Authorization", header)]);
        assert_eq!(reply.status, Some(401), "{header:?}");
    }
    for query_token in ["nope", "", &token[1..], &format!("{token}x"), &format!("Bearer {token}")] {
        let reply = served.ask("GET", &Served::query("/runs", &[("token", query_token)]), &[]);
        assert_eq!(reply.status, Some(401), "{query_token:?}");
    }
}

#[test]
fn non_ascii_tokens() {
    let served = Served::fake(&Arc::new(ServeHost::on(None)), &[]);
    // é's UTF-8 bytes on the wire, each sent as a Latin-1 character. tiny_http closes the connection of a request with
    // a header beyond ASCII unanswered (TODO.md, "serve answers some malformed requests abruptly or silently").
    for header in ["Bearer \u{c3}\u{a9}".to_string(), format!("Bearer {}\u{c3}\u{a9}", served.token())] {
        let reply = served.ask("GET", &Served::query("/runs", &[]), &[("Authorization", &header)]);
        assert_eq!(reply.status, None, "{header:?}");
    }
    // é, an encoded lone surrogate, and a byte that is not UTF-8.
    for token in ["%C3%A9", "%ED%B2%80", "%FF"] {
        let reply = served.ask("GET", &format!("{}&token={token}", Served::query("/runs", &[])), &[]);
        assert_eq!(reply.status, Some(401), "{token}");
    }
}

#[test]
fn token_in_the_query() {
    let served = Served::fake(&Arc::new(ServeHost::on(None)), &[]);
    let token = served.token();
    assert_eq!(served.ask("GET", &Served::query("/runs", &[("token", &token)]), &[]).status, Some(200));
    // Either place is enough: a wrong token in the other does not undo it.
    let reply = served.ask("GET", &Served::query("/runs", &[("token", &token)]), &[("Authorization", "Bearer x")]);
    assert_eq!(reply.status, Some(200));
    assert_eq!(served.get(&Served::query("/runs", &[("token", "x")]), &[]).status, Some(200));
}

#[test]
fn tokens_differ_per_server() {
    let host = Arc::new(ServeHost::on(None));
    let (one, other) = (Served::fake(&host, &[]), Served::fake(&host, &[]));
    assert_ne!(one.token(), other.token());
    assert_eq!(one.token().len(), 43);
}

#[test]
fn tokens_are_32_random_bytes_in_base64url() {
    // Every character of base64url's 64 turns up among a few hundred tokens; the last of each token's 43 holds the 4
    // bits left of 32 bytes, its low 2 bits 0, so it is one of 16.
    let tokens: Vec<String> = (0..300).map(|_| new_token().unwrap()).collect();
    let seen: std::collections::BTreeSet<char> = tokens.iter().flat_map(|t| t.chars()).collect();
    assert_eq!(seen.len(), 64, "{seen:?}");
    assert!(seen.iter().all(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_'), "{seen:?}");
    for t in &tokens {
        assert_eq!(t.len(), 43);
        assert!("AEIMQUYcgkosw048".contains(t.chars().last().unwrap()), "{t}");
    }
}

#[test]
fn bytes_are_the_same_only_when_equal() {
    assert!(same_bytes(b"", b""));
    assert!(same_bytes(b"abc", b"abc"));
    let differ: [(&[u8], &[u8]); 7] = [
        (b"abd", b"abc"),
        (b"xbc", b"abc"),
        (b"ab", b"abc"),
        (b"abcd", b"abc"),
        (b"", b"a"),
        (b"a", b""),
        (b"abc\0", b"abc"),
    ];
    for (given, expected) in differ {
        assert!(!same_bytes(given, expected), "{given:?} {expected:?}");
    }
}

#[test]
fn bound_to_loopback_only() {
    let served = Served::fake(&Arc::new(ServeHost::on(None)), &[]);
    assert_eq!(served.server.url(), format!("http://127.0.0.1:{}", served.port));
    let http = lock(&served.server.listening).clone().expect("it listens");
    let bound = http.server_addr().to_ip().map(|a| a.ip().to_string());
    assert_eq!(bound.as_deref(), Some("127.0.0.1"));
}

#[test]
fn foreign_host_header() {
    let served = Served::fake(&Arc::new(ServeHost::on(None)), &[]);
    let port = served.port;
    for host in [
        "evil.example".to_string(),
        format!("evil.example:{port}"),
        "127.0.0.1".to_string(),
        format!("127.0.0.1:{}", port + 1),
        format!("LOCALHOST:{port}"),
        format!("[::1]:{port}"),
        String::new(),
    ] {
        let reply = served.get(&Served::query("/runs", &[]), &[("Host", &host)]);
        assert_eq!(reply.status, Some(403), "{host:?}");
        assert_eq!(reply.error(), "the Host or Origin header is not this server's");
    }
    assert_eq!(*served.connects.lock().unwrap(), Vec::<(String, String)>::new());
}

#[test]
fn a_request_without_a_host_header_is_refused() {
    let served = Served::fake(&Arc::new(ServeHost::on(None)), &[]);
    let raw =
        format!("GET {} HTTP/1.0\r\nAuthorization: Bearer {}\r\n\r\n", Served::query("/runs", &[]), served.token());
    assert_eq!(exchange(served.port, raw.as_bytes()).status, Some(403));
}

#[test]
fn own_host_headers() {
    let served = Served::fake(&Arc::new(ServeHost::on(None)), &[]);
    for host in [format!("127.0.0.1:{}", served.port), format!("localhost:{}", served.port)] {
        assert_eq!(served.get(&Served::query("/runs", &[]), &[("Host", &host)]).status, Some(200), "{host}");
    }
}

#[test]
fn foreign_origin() {
    let served = Served::fake(&Arc::new(ServeHost::on(None)), &[TAURI_ORIGIN]);
    for origin in ["http://evil.example", "null", "http://localhost:1420", "", "TAURI://localhost"] {
        let reply = served.get(&Served::query("/runs", &[]), &[("Origin", origin)]);
        assert_eq!(reply.status, Some(403), "{origin:?}");
        assert_eq!(reply.header("Access-Control-Allow-Origin"), None);
    }
}

#[test]
fn foreign_origin_even_without_a_token() {
    let served = Served::fake(&Arc::new(ServeHost::on(None)), &[]);
    let reply = served.ask("GET", &Served::query("/runs", &[]), &[("Origin", "http://evil.example")]);
    assert_eq!(reply.status, Some(403));
}

#[test]
fn the_webview_origin_is_allowed_by_cors() {
    let served = Served::fake(&Arc::new(ServeHost::on(None)), &[TAURI_ORIGIN]);
    let reply = served.get(&Served::query("/runs", &[]), &[("Origin", TAURI_ORIGIN)]);
    assert_eq!(reply.status, Some(200));
    assert_eq!(reply.header("Access-Control-Allow-Origin"), Some(TAURI_ORIGIN));
    assert_eq!(reply.header("Vary"), Some("Origin"));
    // A refusal after the Origin check allows it too, so the webview can read why.
    let reply = served.ask("GET", &Served::query("/runs", &[]), &[("Origin", TAURI_ORIGIN)]);
    assert_eq!((reply.status, reply.header("Access-Control-Allow-Origin")), (Some(401), Some(TAURI_ORIGIN)));
}

#[test]
fn own_origin_and_none() {
    let served = Served::fake(&Arc::new(ServeHost::on(None)), &[]);
    let reply = served.get(&Served::query("/runs", &[]), &[]);
    assert_eq!(reply.status, Some(200));
    assert_eq!(reply.header("Access-Control-Allow-Origin"), None);
    for origin in [format!("http://localhost:{}", served.port), format!("http://127.0.0.1:{}", served.port)] {
        assert_eq!(served.get(&Served::query("/runs", &[]), &[("Origin", &origin)]).status, Some(200), "{origin}");
    }
}

#[test]
fn preflight_of_the_webview() {
    let served = Served::fake(&Arc::new(ServeHost::on(None)), &[TAURI_ORIGIN]);
    let reply = served.ask("OPTIONS", "/runs", &[("Origin", TAURI_ORIGIN)]);
    assert_eq!(reply.status, Some(204));
    assert_eq!(reply.header("Access-Control-Allow-Origin"), Some(TAURI_ORIGIN));
    assert_eq!(reply.header("Access-Control-Allow-Headers"), Some("Authorization, Content-Type"));
    assert_eq!(reply.header("Access-Control-Allow-Methods"), Some("GET, POST"));
    assert_eq!(reply.body, "");
}

#[test]
fn preflight_of_a_foreign_origin() {
    let served = Served::fake(&Arc::new(ServeHost::on(None)), &[TAURI_ORIGIN]);
    let reply = served.ask("OPTIONS", "/runs", &[("Origin", "http://evil.example")]);
    assert_eq!(reply.status, Some(403));
    assert_eq!(reply.header("Access-Control-Allow-Origin"), None);
    let reply = served.ask("OPTIONS", "/runs", &[("Origin", TAURI_ORIGIN), ("Host", "evil.example")]);
    assert_eq!(reply.status, Some(403));
}

#[test]
fn writes_are_not_served() {
    let served = Served::fake(&Arc::new(ServeHost::on(None)), &[]);
    for method in ["POST", "PUT", "DELETE", "PATCH", "HEAD", "get"] {
        let reply = served.get(&Served::query("/runs", &[]), &[]);
        assert_eq!(reply.status, Some(200));
        let bearer = format!("Bearer {}", served.token());
        let reply = served.ask(method, &Served::query("/runs", &[]), &[("Authorization", &bearer)]);
        assert_eq!(reply.status, Some(501), "{method}");
        assert_eq!(reply.header("Content-Type"), Some("application/json"));
        if method != "HEAD" {
            assert_eq!(reply.body, format!(r#"{{"error":"unsupported method \"{method}\""}}"#), "{method}");
        }
    }
    assert_eq!(*served.connects.lock().unwrap(), [(String::new(), "~/proj".to_string())]);
}

// -- The agents' endpoints ------------------------------------------------------------------------------------------

/// A server whose project holds the run with a build agent named build-5e7e00, which shows "> ready".
fn served_with_agent() -> Served {
    let host = Arc::new(ServeHost::on(None));
    let agents = r#"{"spec": {"name": "", "pane": "", "kind": "claude"}, "build": {"name": "build-5e7e00", "pane": "w1:p2", "kind": "claude"}}"#;
    let state = serve_state(SERVE_ID, &[("agents", serde_json::from_str::<Value>(agents).unwrap())]);
    host.state().files.insert(format!("{}/state.json", run_dir()), state);
    let served = Served::fake(&host, &[]);
    served.console.state().screens.insert("build-5e7e00".to_string(), "> ready\n".to_string());
    served
}

#[test]
fn an_agent_s_text_is_read_through_herdr_by_run_and_role() {
    let served = served_with_agent();
    let reply = served.get(&Served::query("/agent", &[("run", SERVE_ID), ("role", "build")]), &[]);
    assert_eq!(reply.status, Some(200));
    assert_eq!(
        reply.json(),
        object([("run", Value::from(SERVE_ID)), ("role", Value::from("build")), ("text", Value::from("> ready\n"))])
    );
    let reply = served.get(&Served::query("/agent", &[("run", SERVE_ID), ("role", "build"), ("lines", "50")]), &[]);
    assert_eq!(reply.status, Some(200));
    assert_eq!(served.console.state().typed, ["read build-5e7e00 200", "read build-5e7e00 50"]);
    let wrong = "lines must be a whole number from 1 to 2000".to_string();
    for lines in ["0", "2001", "x", "-1"] {
        let query = Served::query("/agent", &[("run", SERVE_ID), ("role", "build"), ("lines", lines)]);
        let reply = served.get(&query, &[]);
        assert_eq!((reply.status, reply.error()), (Some(400), wrong.clone()), "{lines}");
    }
}

#[test]
fn an_agent_that_is_not_there_is_not_found() {
    let served = served_with_agent();
    let ask = |run: &str, role: &str| {
        let reply = served.get(&Served::query("/agent", &[("run", run), ("role", role)]), &[]);
        (reply.status, reply.error())
    };
    assert_eq!(ask(SERVE_ID, "review"), (Some(404), format!("run {SERVE_ID} has no review agent")));
    assert_eq!(ask(SERVE_ID, "spec"), (Some(404), format!("run {SERVE_ID} has no spec agent")));
    assert_eq!(ask("nosuch", "build"), (Some(404), "no run nosuch".to_string()));
    assert_eq!(ask("../x", "build"), (Some(403), "\"../x\" is not a name in the run directory".to_string()));
    served.console.state().screens.clear();
    assert_eq!(ask(SERVE_ID, "build"), (Some(404), "the build agent has exited".to_string()));
    served.console.state().down = true;
    assert_eq!(ask(SERVE_ID, "build"), (Some(502), "herdr is not on PATH".to_string()));
    let reply = served.get(&Served::query("/agent", &[("run", SERVE_ID)]), &[]);
    assert_eq!((reply.status, reply.error()), (Some(400), "missing parameter role".to_string()));
}

#[test]
fn input_is_typed_into_the_agent_as_a_prompt_or_keys() {
    let served = served_with_agent();
    let target = Served::query("/agent/input", &[("run", SERVE_ID), ("role", "build")]);
    let reply = served.post(&target, r#"{"text": "per client, please"}"#);
    assert_eq!(reply.status, Some(200));
    assert_eq!(reply.json(), object([("run", Value::from(SERVE_ID)), ("role", Value::from("build"))]));
    assert_eq!(served.post(&target, r#"{"keys": ["esc", "ctrl+c"]}"#).status, Some(200));
    let typed = ["submit build-5e7e00 per client, please", "keys build-5e7e00 esc ctrl+c"];
    assert_eq!(served.console.state().typed, typed);
    let wrong = r#"the body must be {"text": "..."} or {"keys": ["...", ...]}"#;
    let bodies = ["", "x", "[]", "{}", r#"{"text": ""}"#, r#"{"keys": []}"#, r#"{"keys": ["esc", 1]}"#];
    for body in bodies.into_iter().chain([r#"{"text": "a", "keys": ["esc"]}"#]) {
        let reply = served.post(&target, body);
        assert_eq!((reply.status, reply.error()), (Some(400), wrong.to_string()), "{body}");
    }
    let long = format!(r#"{{"text": "{}"}}"#, "x".repeat(MAX_BODY_BYTES));
    assert_eq!(served.post(&target, &long).status, Some(413));
    assert_eq!(served.console.state().typed.len(), 2);
    let other = Served::query("/agent/input", &[("run", SERVE_ID), ("role", "review")]);
    assert_eq!(served.post(&other, r#"{"text": "hi"}"#).status, Some(404));
}

#[test]
fn input_needs_the_token_and_the_server_s_own_origin() {
    let served = served_with_agent();
    let target = Served::query("/agent/input", &[("run", SERVE_ID), ("role", "build")]);
    let body = r#"{"text": "rm -rf /"}"#;
    let length = body.len().to_string();
    let reply = served.ask_with_body("POST", &target, &[("Content-Length", &length)], body);
    assert_eq!(reply.status, Some(401));
    let bearer = format!("Bearer {}", served.token());
    let headers = [("Authorization", bearer.as_str()), ("Origin", "http://evil.example"), ("Content-Length", &length)];
    assert_eq!(served.ask_with_body("POST", &target, &headers, body).status, Some(403));
    assert!(served.console.state().typed.is_empty());
}

#[test]
fn a_bad_input_is_refused_before_the_project_is_reached() {
    let connects = Arc::new(Mutex::new(vec![]));
    let seen = Arc::clone(&connects);
    let connect = move |machine: &str, cwd: &str, _: &Interrupt| {
        seen.lock().unwrap().push((machine.to_string(), cwd.to_string()));
        Err(OrchestratorError::new("ssh: connection refused"))
    };
    let served = Served::start(Box::new(connect), &[], Arc::clone(&connects), Options::default());
    let target = Served::query("/agent/input", &[("run", SERVE_ID), ("role", "build"), ("machine", "box")]);
    assert_eq!(served.post(&target, r#"{"keys": []}"#).status, Some(400));
    let long = format!(r#"{{"text": "{}"}}"#, "x".repeat(MAX_BODY_BYTES));
    assert_eq!(served.post(&target, &long).status, Some(413));
    assert!(connects.lock().unwrap().is_empty(), "a bad body reached the project");
    let reply = served.post(&target, r#"{"text": "go"}"#);
    assert_eq!((reply.status, reply.error()), (Some(502), "ssh: connection refused".to_string()));
}

// -- POST /run and GET /workflows: starting a run ---------------------------------------------------------------

/// A wait for a started run short enough for a test.
const QUICK_WAIT: StartWait = StartWait { every: Duration::from_millis(10), limit: Duration::from_millis(300) };

/// A server whose every project is host's /proj, waiting QUICK_WAIT for a run it starts.
fn starting(host: &Arc<ServeHost>) -> Served {
    Served::fake_with(host, &[], Options { wait: QUICK_WAIT, ..Options::default() })
}

/// The run id an argv of POST /run's starts, from its --run-id.
fn started_id(argv: &[String]) -> String {
    argv.iter().find_map(|w| w.strip_prefix("--run-id=")).expect("a --run-id").to_string()
}

#[test]
fn a_run_that_saves_its_state_is_answered_with_its_id_and_runs_its_own_program_in_a_workspace() {
    // A project on a machine: its run is given the machine and the cwd as the client named them.
    let host = Arc::new(ServeHost::on(Some("me@box")));
    let served = starting(&host);
    let saving = Arc::clone(&host);
    served.console.state().on_launch = Some(Box::new(move |argv: &[String]| {
        let id = started_id(argv);
        saving.state().files.insert(format!("/proj/{RUNS_DIR}/{id}/state.json"), serve_state(&id, &[]));
    }));
    let body = r#"{"task": "--add a rate limiter", "workflow": "quick", "agent": "codex", "model": "-m",
                   "role_agents": {"review": "pi", "build": "claude"}, "role_models": {"review": "o3"},
                   "no_pr": true, "worktree": true}"#;
    let reply = served.post(&Served::query("/run", &[("machine", "box")]), body);
    assert_eq!(reply.status, Some(201), "{}", reply.body);
    let id = reply.json()["run"].as_str().expect("the run's id").to_string();
    let launches = served.console.state().launches.clone();
    let [(cwd, label, argv)] = &launches[..] else { panic!("one launch: {launches:?}") };
    assert_eq!(cwd, "/proj");
    assert_eq!(*label, crate::workflow::workspace_label(&id, "--add a rate limiter"));
    let program = std::env::current_exe().unwrap().into_os_string().into_string().unwrap();
    let expected = [
        program.as_str(),
        "run",
        &format!("--run-id={id}"),
        "--machine",
        "box",
        "--cwd",
        "~/proj",
        "--workflow=quick",
        "--agent=codex",
        "--model=-m",
        "--role-agent=review=pi",
        "--role-agent=build=claude",
        "--role-model=review=o3",
        "--no-pr",
        "--worktree",
        "--",
        "--add a rate limiter",
    ];
    assert_eq!(argv, &expected);
    // An id as run's own, which `run --run-id` takes.
    let argv = ["run", "--run-id", &id, "t"].map(std::ffi::OsString::from);
    assert!(crate::cli::parse_args(argv, &ConfigDir::at("/nowhere")).is_ok(), "{id}");
}

#[test]
fn a_run_of_a_project_here_is_given_the_directory_serve_resolved() {
    let host = Arc::new(ServeHost::on(None));
    let served = starting(&host);
    // ~/proj, as the client named it, resolves to /proj (the fake connect's), which the pane starts in too.
    assert_eq!(served.post(&Served::query("/run", &[]), r#"{"task": "t"}"#).status, Some(202));
    let argv = served.console.state().launches[0].2.clone();
    assert_eq!(argv[3..], ["--cwd", "/proj", "--", "t"]);
}

#[test]
fn a_run_is_refused_before_its_workspace_opens_when_herdrs_panes_start_a_shell_it_cannot_type_into() {
    let host = Arc::new(ServeHost::on(None));
    let served = starting(&host);
    host.state().pane_shell = "/usr/bin/nu\n/home/me/.config/herdr/config.toml\n".to_string();
    let reply = served.post(&Served::query("/run", &[]), r#"{"task": "t"}"#);
    assert_eq!(reply.status, Some(502), "{}", reply.body);
    let why = reply.json()["error"].as_str().unwrap_or_default().to_string();
    assert!(why.starts_with("herdr's panes start /usr/bin/nu ($SHELL)"), "{why}");
    assert!(why.ends_with("POSIX shell (sh, bash, zsh, dash, ksh) or fish"), "{why}");
    assert!(served.console.state().launches.is_empty());
    // One herdr's config.toml names, which wins over $SHELL.
    host.state().pane_shell.push_str("[terminal]\ndefault_shell = \"zsh\"\n");
    assert_eq!(served.post(&Served::query("/run", &[]), r#"{"task": "t"}"#).status, Some(202));
    assert_eq!(served.console.state().launches.len(), 1);
}

#[test]
fn a_look_that_fails_once_the_run_is_launched_is_no_answer() {
    let host = Arc::new(ServeHost::on(None));
    let served = starting(&host);
    let saving = Arc::clone(&host);
    served.console.state().on_launch = Some(Box::new(move |argv: &[String]| {
        let id = started_id(argv);
        saving.state().files.insert(format!("/proj/{RUNS_DIR}/{id}/state.json"), serve_state(&id, &[]));
    }));
    // A pane the human closed: the run's state.json still tells.
    served.console.state().exited_error = Some(HerdrError::new("pane_not_found", "pane w9:p1 not found"));
    let reply = served.post(&Served::query("/run", &[]), r#"{"task": "t"}"#);
    assert_eq!(reply.status, Some(201), "{}", reply.body);
    // Neither can be looked at: the client still gets the id of the run that is running.
    host.state().read_error = Some("ssh: connection reset".to_string());
    let reply = served.post(&Served::query("/run", &[]), r#"{"task": "t"}"#);
    assert_eq!(reply.status, Some(202), "{}", reply.body);
    assert!(reply.json()["run"].is_string());
}

#[test]
fn a_replaced_program_is_started_as_the_one_now_at_its_path() {
    let there = |p: &str| p == "/bin/octoflow";
    assert_eq!(live_program("/bin/octoflow (deleted)".into(), there), "/bin/octoflow");
    assert_eq!(live_program("/gone/octoflow (deleted)".into(), there), "/gone/octoflow (deleted)");
    assert_eq!(live_program("/bin/octoflow".into(), there), "/bin/octoflow");
}

#[test]
fn a_run_that_exits_without_its_state_is_answered_with_what_its_pane_showed() {
    let host = Arc::new(ServeHost::on(None));
    let served = starting(&host);
    served.console.state().exited = Some("error: invalid value 'nope' for '--workflow <WORKFLOW>'".to_string());
    let reply = served.post(&Served::query("/run", &[]), r#"{"task": "t", "workflow": "nope"}"#);
    assert_eq!(reply.status, Some(422));
    let id = reply.json()["run"].as_str().expect("the run's id").to_string();
    let why = format!("run {id} exited before it saved its state; its pane showed:\nerror: invalid value 'nope' for '--workflow <WORKFLOW>'");
    assert_eq!(reply.error(), why);
}

#[test]
fn a_run_neither_listed_nor_exited_in_time_is_answered_as_not_yet_listed() {
    let host = Arc::new(ServeHost::on(None));
    let served = starting(&host);
    let started = Instant::now();
    let reply = served.post(&Served::query("/run", &[]), r#"{"task": "t"}"#);
    assert!(started.elapsed() >= QUICK_WAIT.limit, "it waited {:?}", started.elapsed());
    assert_eq!(reply.status, Some(202));
    let answer = reply.json();
    assert_eq!(answer["listed"], Value::from(false));
    // In the server serve's environment picks: no session is named.
    assert_eq!(answer.as_object().map(|o| o.len()), Some(2), "{answer}");
    // A run that saved its state.json just before it exited is listed.
    let saving = Arc::clone(&host);
    let mut console = served.console.state();
    console.exited = Some("error: the checkout is busy".to_string());
    console.on_launch = Some(Box::new(move |argv: &[String]| {
        let id = started_id(argv);
        saving.state().files.insert(format!("/proj/{RUNS_DIR}/{id}/state.json"), serve_state(&id, &[]));
    }));
    drop(console);
    assert_eq!(served.post(&Served::query("/run", &[]), r#"{"task": "t"}"#).status, Some(201));
}

#[test]
fn a_run_launched_into_the_own_session_is_answered_with_that_session() {
    let host = Arc::new(ServeHost::on(None));
    let served = starting(&host);
    served.console.state().launched_server = Some(HerdrServer::own_session());
    let reply = served.post(&Served::query("/run", &[]), r#"{"task": "t"}"#);
    assert_eq!(reply.status, Some(202), "{}", reply.body);
    assert_eq!(reply.json()["herdr_session"], Value::from(OWN_SESSION));
    let saving = Arc::clone(&host);
    served.console.state().on_launch = Some(Box::new(move |argv: &[String]| {
        let id = started_id(argv);
        saving.state().files.insert(format!("/proj/{RUNS_DIR}/{id}/state.json"), serve_state(&id, &[]));
    }));
    let reply = served.post(&Served::query("/run", &[]), r#"{"task": "t"}"#);
    assert_eq!(reply.status, Some(201), "{}", reply.body);
    assert_eq!(reply.json()["herdr_session"], Value::from(OWN_SESSION));
}

#[test]
fn a_run_herdr_cannot_launch_is_refused_with_herdr_s_error() {
    let host = Arc::new(ServeHost::on(None));
    let served = starting(&host);
    served.console.state().launch_error = Some(HerdrError::new("timeout", "herdr workspace create did not return"));
    let reply = served.post(&Served::query("/run", &[]), r#"{"task": "t"}"#);
    assert_eq!(
        (reply.status, reply.error()),
        (Some(502), "herdr timeout: herdr workspace create did not return".to_string())
    );
}

#[test]
fn a_bad_run_body_is_refused_before_the_project_is_reached_and_nothing_is_launched() {
    let connects = Arc::new(Mutex::new(vec![]));
    let seen = Arc::clone(&connects);
    let connect = move |machine: &str, cwd: &str, _: &Interrupt| {
        seen.lock().unwrap().push((machine.to_string(), cwd.to_string()));
        Err(OrchestratorError::new("ssh: connection refused"))
    };
    let served = Served::start(Box::new(connect), &[], Arc::clone(&connects), Options::default());
    let target = Served::query("/run", &[("machine", "box")]);
    let reply = served.post(&target, r#"{"task": ""}"#);
    assert_eq!((reply.status, reply.error()), (Some(400), "task must be a string that is not empty".to_string()));
    let long = format!(r#"{{"task": "{}"}}"#, "x".repeat(MAX_BODY_BYTES));
    assert_eq!(served.post(&target, &long).status, Some(413));
    assert!(connects.lock().unwrap().is_empty(), "a bad body reached the project");
    let reply = served.post(&target, r#"{"task": "go"}"#);
    assert_eq!((reply.status, reply.error()), (Some(502), "ssh: connection refused".to_string()));
    // Without a cwd.
    assert_eq!(served.post("/run", r#"{"task": "go"}"#).status, Some(400));
}

#[test]
fn a_run_needs_the_token_and_the_server_s_own_origin() {
    let host = Arc::new(ServeHost::on(None));
    let served = starting(&host);
    let (target, body) = (Served::query("/run", &[]), r#"{"task": "t"}"#);
    let length = body.len().to_string();
    assert_eq!(served.ask_with_body("POST", &target, &[("Content-Length", &length)], body).status, Some(401));
    let bearer = format!("Bearer {}", served.token());
    let headers = [("Authorization", bearer.as_str()), ("Origin", "http://evil.example"), ("Content-Length", &length)];
    assert_eq!(served.ask_with_body("POST", &target, &headers, body).status, Some(403));
    assert!(served.console.state().launches.is_empty());
}

// -- POST /close: closing a project's runs ---------------------------------------------------------------------

/// The runs of closing()'s project, in run id order.
const CLOSE_FINISHED: &str = "20261001-110000-aaaaa1";
const CLOSE_FAILED: &str = "20261001-120000-a1b2c3";
const CLOSE_RUNNING: &str = "20261001-130000-ccccc3";
const CLOSE_CLOSED: &str = "20261001-140000-ddddd4";
/// CLOSE_FAILED's branch, which holds no commit beyond its base.
const CLOSE_BRANCH: &str = "orchestrator/x-a1b2c3";

/// A git repository with four runs, and a server whose every project is that repository, with the server's console
/// as its herdr: CLOSE_FINISHED, finished in place in workspace w-fin, which herdr has open; CLOSE_FAILED, failed in a
/// worktree (the second value) on CLOSE_BRANCH, its app.py changed; CLOSE_RUNNING, just saved by a live run; and
/// CLOSE_CLOSED, closed.
fn closing() -> (Repo, String, Served) {
    let repo = Repo::new(&[("app.py", "one\n"), (".octoflow-ai/.gitignore", "*\n")]);
    let base = repo.base().to_string();
    let wt = format!("{}/{WORKTREES_DIR}/a1b2c3", repo.path);
    repo.host.add_worktree(&repo.path, &wt, &base).unwrap();
    repo.host.git(&wt, &["switch", "--quiet", "-c", CLOSE_BRANCH]).unwrap();
    crate::host::tests::put(&format!("{wt}/app.py"), "changed\n");
    let save = |id: &str, set: &dyn Fn(&mut RunState)| {
        let mut state = RunState::new(id, "a task", &repo.path);
        (state.phase, state.round) = ("build".into(), 1);
        set(&mut state);
        repo.host.write(&format!("{}/state.json", state.dir()), &state.to_text(), false).unwrap();
    };
    save(CLOSE_FINISHED, &|s| {
        (s.phase, s.verdict, s.workspace_id) = ("done".into(), Some(APPROVE.into()), "w-fin".into())
    });
    save(CLOSE_FAILED, &|s| {
        (s.error, s.worktree, s.branch) = (Some("interrupted".into()), Some(wt.clone()), Some(CLOSE_BRANCH.into()));
        s.base = Some(base.clone());
    });
    save(CLOSE_RUNNING, &|_| {});
    save(CLOSE_CLOSED, &|s| (s.error, s.closed) = (Some("interrupted".into()), Some("workspace closed".into())));
    let console = Arc::new(FakeConsole::default());
    console.state().workspaces.insert("w-fin".to_string());
    let (herdr, dir) = (Arc::clone(&console), repo.path.clone());
    let connect = move |_: &str, _: &str, stopping: &Interrupt| {
        let host = Arc::new(ProcessHost::local(stopping));
        let (console, workspaces, launcher) = (herdr.clone(), herdr.clone(), herdr.clone());
        Ok(Reached { host, dir: dir.clone(), console, workspaces, launcher, launch_dir: dir.clone() })
    };
    let served = Served::start(Box::new(connect), &[], Arc::default(), Options::default());
    (repo, wt, Served { console, ..served })
}

/// The runs of a 200 answer to POST /close, each as (run, outcome, detail, discarded).
fn closed_runs(reply: &Reply) -> Vec<(String, String, String, Vec<String>)> {
    assert_eq!(reply.status, Some(200), "{reply:?}");
    let runs = reply.json().get("runs").and_then(Value::as_array).cloned().expect("runs");
    let field = |run: &Value, key: &str| run.get(key).map(Value::text).expect(key);
    let discarded = |run: &Value| {
        run.get("discarded").and_then(Value::as_array).expect("discarded").iter().map(Value::text).collect()
    };
    runs.iter().map(|run| (field(run, "run"), field(run, "outcome"), field(run, "detail"), discarded(run))).collect()
}

/// The body of a POST /close that closes the runs of closed_runs' dry run whose outcome is "closed", as it gave them.
fn close_body(preview: &[(String, String, String, Vec<String>)]) -> String {
    let closing = preview.iter().filter(|(_, outcome, _, _)| outcome == "closed");
    let runs: Vec<Value> = closing
        .map(|(run, _, detail, discarded)| serde_json::json!({"run": run, "detail": detail, "discarded": discarded}))
        .collect();
    serde_json::json!({ "close": runs }).to_string()
}

/// The closed of the run of repo whose id is run_id, as its state.json saves it; None when it has none.
fn saved_closed(repo: &Repo, run_id: &str) -> Option<String> {
    let path = format!("{}/{RUNS_DIR}/{run_id}/state.json", repo.path);
    let saved = serde_json::from_str::<Value>(&repo.host.read(&path).unwrap().unwrap()).unwrap();
    saved.get("closed").filter(|c| !c.is_null()).map(Value::text)
}

#[test]
fn a_dry_run_says_what_closing_each_run_would_do_and_changes_nothing() {
    let (repo, wt, served) = closing();
    let target = format!("/close?{}", urlencode(&[("cwd", &repo.path)]));
    let mut runs = closed_runs(&served.post(&target, r#"{"dry_run": true}"#));
    let running = runs.remove(2);
    assert_eq!(
        runs,
        [
            (CLOSE_FINISHED.into(), "closed".into(), "close workspace w-fin".into(), vec![]),
            (
                CLOSE_FAILED.into(),
                "closed".into(),
                format!("delete worktree {wt}; delete branch {CLOSE_BRANCH}"),
                vec![" M app.py".into()]
            ),
            (CLOSE_CLOSED.into(), "already closed".into(), "workspace closed".into(), vec![]),
        ]
    );
    assert_eq!((running.0.as_str(), running.1.as_str()), (CLOSE_RUNNING, "skipped"));
    assert!(running.2.contains("is still running"), "{}", running.2);
    assert!(std::path::Path::new(&wt).exists());
    assert_eq!(repo.host.branch_commit(&repo.path, CLOSE_BRANCH).unwrap(), repo.base.clone());
    assert_eq!(saved_closed(&repo, CLOSE_FINISHED), None);
    assert_eq!(saved_closed(&repo, CLOSE_FAILED), None);
    let s = served.console.state();
    assert_eq!(s.workspace_calls, [("exists w-fin".to_string(), None)]);
    assert!(s.workspaces.contains("w-fin"));
}

#[test]
fn the_previewed_runs_are_closed_and_say_what_was_done_and_discarded() {
    let (repo, wt, served) = closing();
    let target = format!("/close?{}", urlencode(&[("cwd", &repo.path)]));
    let preview = closed_runs(&served.post(&target, r#"{"dry_run": true}"#));
    let runs = closed_runs(&served.post(&target, &close_body(&preview)));
    let outcomes: Vec<_> = runs.iter().map(|(run, outcome, _, _)| (run.as_str(), outcome.as_str())).collect();
    assert_eq!(outcomes, [(CLOSE_FINISHED, "closed"), (CLOSE_FAILED, "closed")]);
    let failed =
        ("workspace already gone; worktree deleted; branch deleted".to_string(), vec![" M app.py".to_string()]);
    assert_eq!((runs[1].2.clone(), runs[1].3.clone()), failed);
    assert_eq!(saved_closed(&repo, CLOSE_FINISHED).as_deref(), Some("workspace closed"));
    assert_eq!(saved_closed(&repo, CLOSE_FAILED), Some(failed.0));
    assert_eq!(saved_closed(&repo, CLOSE_RUNNING), None);
    assert!(!std::path::Path::new(&wt).exists());
    assert!(served.console.state().workspaces.is_empty());
    // A second dry run finds only the running run to close.
    let again = closed_runs(&served.post(&target, r#"{"dry_run": true}"#));
    let outcomes: Vec<_> = again.iter().map(|(_, outcome, _, _)| outcome.as_str()).collect();
    assert_eq!(outcomes, ["already closed", "already closed", "skipped", "already closed"]);
}

#[test]
fn a_run_that_changed_since_the_preview_is_skipped_and_one_not_previewed_is_not_looked_at() {
    // CLOSE_FAILED's worktree gains a change the preview did not list; the body leaves CLOSE_FINISHED out, as a client
    // whose preview found it running would.
    let (repo, wt, served) = closing();
    let target = format!("/close?{}", urlencode(&[("cwd", &repo.path)]));
    let preview = closed_runs(&served.post(&target, r#"{"dry_run": true}"#));
    crate::host::tests::put(&format!("{wt}/new.py"), "new\n");
    let failed_only: Vec<_> = preview.into_iter().filter(|(run, _, _, _)| run == CLOSE_FAILED).collect();
    let runs = closed_runs(&served.post(&target, &close_body(&failed_only)));
    let now = format!("delete worktree {wt}; delete branch {CLOSE_BRANCH}");
    let skipped = format!("run {CLOSE_FAILED} changed since the preview: closing it would now {now}");
    assert_eq!(runs, [(CLOSE_FAILED.to_string(), "skipped".to_string(), skipped, vec![])]);
    assert!(std::path::Path::new(&wt).exists());
    assert_eq!((saved_closed(&repo, CLOSE_FAILED), saved_closed(&repo, CLOSE_FINISHED)), (None, None));
    assert!(served.console.state().workspaces.contains("w-fin"));
}

#[test]
fn a_bad_close_body_is_refused_before_the_project_is_reached() {
    let connects = Arc::new(Mutex::new(vec![]));
    let seen = Arc::clone(&connects);
    let connect = move |machine: &str, cwd: &str, _: &Interrupt| {
        seen.lock().unwrap().push((machine.to_string(), cwd.to_string()));
        Err(OrchestratorError::new("ssh: connection refused"))
    };
    let served = Served::start(Box::new(connect), &[], Arc::clone(&connects), Options::default());
    let target = Served::query("/close", &[("machine", "box")]);
    let wrong = r#"the body must be {"dry_run": true} or {"close": [{"run": "...", "detail": "...", "discarded": ["...", ...]}, ...]}"#;
    let bad = [
        "",
        "x",
        "[]",
        "{}",
        r#"{"dry_run": 1}"#,
        r#"{"dry_run": "true"}"#,
        r#"{"dry_run": true, "x": 1}"#,
        // A close needs the preview it closes: closing whatever is there now could discard what no one was shown.
        r#"{"dry_run": false}"#,
        r#"{"close": {}}"#,
        r#"{"close": [{"run": "r1", "detail": "d"}]}"#,
        r#"{"close": [{"run": "r1", "detail": "d", "discarded": [1]}]}"#,
        r#"{"close": [{"run": "../r1", "detail": "d", "discarded": []}]}"#,
    ];
    for body in bad {
        let reply = served.post(&target, body);
        assert_eq!((reply.status, reply.error()), (Some(400), wrong.to_string()), "{body}");
    }
    let long = format!(r#"{{"dry_run": true, "x": "{}"}}"#, "x".repeat(MAX_BODY_BYTES));
    assert_eq!(served.post(&target, &long).status, Some(413));
    assert!(connects.lock().unwrap().is_empty(), "a bad body reached the project");
    let reply = served.post(&target, r#"{"dry_run": true}"#);
    assert_eq!((reply.status, reply.error()), (Some(502), "ssh: connection refused".to_string()));
    assert_eq!(served.post("/close", r#"{"dry_run": true}"#).status, Some(400));
}

#[test]
fn a_close_skips_a_run_whose_state_json_is_not_json_saying_why() {
    let project = Project::new();
    project.write(&format!("{RUNS_DIR}/{SERVE_ID}/state.json"), "not JSON");
    let reply = Served::real().post(&project.query("/close"), r#"{"dry_run": true}"#);
    assert_eq!(reply.status, Some(200), "{reply:?}");
    let runs = reply.json().get("runs").and_then(Value::as_array).cloned().unwrap();
    let [run] = runs.as_slice() else { panic!("{runs:?}") };
    assert_eq!((run.get("run"), run.get("outcome")), (Some(&Value::from(SERVE_ID)), Some(&Value::from("skipped"))));
    assert!(run.get("detail").unwrap().text().contains(&format!("{SERVE_ID}/state.json is not JSON (")), "{run:?}");
}

#[test]
fn a_close_needs_the_token_and_the_server_s_own_origin() {
    let host = Arc::new(ServeHost::on(None));
    let served = Served::fake(&host, &[]);
    let (target, body) = (Served::query("/close", &[]), r#"{"dry_run": true}"#);
    let length = body.len().to_string();
    assert_eq!(served.ask_with_body("POST", &target, &[("Content-Length", &length)], body).status, Some(401));
    let bearer = format!("Bearer {}", served.token());
    let headers = [("Authorization", bearer.as_str()), ("Origin", "http://evil.example"), ("Content-Length", &length)];
    assert_eq!(served.ask_with_body("POST", &target, &headers, body).status, Some(403));
    assert_eq!(host.state().run_states_inside_calls, 0);
}

const FORGET_CLOSED: &str = "20261001-120000-a1b2c3";
const FORGET_OPEN: &str = "20261001-120100-d4e5f6";

/// A project with FORGET_CLOSED, closed, with a metrics.json, and FORGET_OPEN, not closed; and a server that reaches it.
fn forgetting() -> Project {
    let project = Project::new();
    project.save(FORGET_CLOSED, &[("closed", Value::from("workspace closed"))]);
    project
        .write(&format!("{RUNS_DIR}/{FORGET_CLOSED}/metrics.json"), &format!("{{\"run_id\": \"{FORGET_CLOSED}\"}}\n"));
    project.save(FORGET_OPEN, &[]);
    project
}

/// The runs of a 200 answer to POST /forget, each as (run, outcome, detail).
fn forgotten_runs(reply: &Reply) -> Vec<(String, String, String)> {
    assert_eq!(reply.status, Some(200), "{reply:?}");
    let runs = reply.json().get("runs").and_then(Value::as_array).cloned().expect("runs");
    let field = |run: &Value, key: &str| run.get(key).map(Value::text).expect(key);
    runs.iter().map(|run| (field(run, "run"), field(run, "outcome"), field(run, "detail"))).collect()
}

#[test]
fn a_forget_dry_run_names_the_closed_run_and_a_listed_forget_deletes_it_keeping_its_metrics() {
    let (project, served) = (forgetting(), Served::real());
    let target = project.query("/forget");
    let closed_dir = project.0.join(&format!("{RUNS_DIR}/{FORGET_CLOSED}"));
    let preview = forgotten_runs(&served.post(&target, r#"{"dry_run": true}"#));
    let expected =
        [(FORGET_CLOSED, "forgotten"), (FORGET_OPEN, "not closed")].map(|(r, o)| (r.into(), o.into(), String::new()));
    assert_eq!(preview, expected);
    assert!(std::path::Path::new(&closed_dir).exists(), "a dry run deleted nothing");

    let body = serde_json::json!({ "forget": [FORGET_CLOSED] }).to_string();
    assert_eq!(
        forgotten_runs(&served.post(&target, &body)),
        [(FORGET_CLOSED.into(), "forgotten".into(), String::new())]
    );
    assert!(!std::path::Path::new(&closed_dir).exists());
    let archive = std::fs::read_to_string(project.0.join(".octoflow-ai/metrics-archive.jsonl")).unwrap();
    assert!(archive.contains(FORGET_CLOSED), "{archive}");
    assert!(std::path::Path::new(&project.0.join(&format!("{RUNS_DIR}/{FORGET_OPEN}"))).exists());

    // Forgotten since the dry run, as by a second client: skipped, and nothing else is looked at.
    let again = forgotten_runs(&served.post(&target, &body));
    assert_eq!(again, [(FORGET_CLOSED.into(), "skipped".into(), "the overview shows no such run any more".into())]);
}

/// The real Host, but each read of the metrics archive waits, after reading it, until a second has been made or a
/// second has passed: two forget sweeps that reach it together both read it before either appends to it.
struct ArchiveReadsMeet {
    /// The Host every call goes to.
    host: Arc<dyn crate::host::Host + Send + Sync>,
    /// How many reads of the archive have been made.
    reads: Mutex<usize>,
    /// Notified each time reads grows.
    met: std::sync::Condvar,
}

impl crate::host::Host for ArchiveReadsMeet {
    fn ssh_target(&self) -> Option<&str> {
        self.host.ssh_target()
    }

    fn exec(
        &self,
        argv: &[&str],
        stdin: Option<&[u8]>,
        timeout: u64,
    ) -> Result<crate::host::Exec, crate::host::ExecError> {
        self.host.exec(argv, stdin, timeout)
    }

    fn read(&self, path: &str) -> Result<Option<String>, OrchestratorError> {
        let text = self.host.read(path);
        if path.ends_with("/metrics-archive.jsonl") {
            let mut reads = self.reads.lock().unwrap();
            *reads += 1;
            self.met.notify_all();
            drop(self.met.wait_timeout_while(reads, Duration::from_secs(1), |n| *n < 2).unwrap());
        }
        text
    }
}

#[test]
fn two_forgets_of_one_run_at_once_forget_it_once() {
    let project = forgetting();
    let connect = |machine: &str, cwd: &str, stopping: &Interrupt| {
        let mut reached = project_host(machine, cwd, stopping)?;
        reached.host = Arc::new(ArchiveReadsMeet { host: reached.host, reads: Mutex::new(0), met: Default::default() });
        Ok(reached)
    };
    let served = Served::start(Box::new(connect), &[], Arc::default(), Options::default());
    // The project spelled two ways, as two clients may name it: both are one project to the lock.
    let spelled_with_slash = format!("/forget?{}", urlencode(&[("cwd", &format!("{}/", project.root()))]));
    let targets = [project.query("/forget"), spelled_with_slash];
    let body = serde_json::json!({ "forget": [FORGET_CLOSED] }).to_string();
    let outcomes: Vec<String> = std::thread::scope(|scope| {
        let asks: Vec<_> = targets.iter().map(|t| scope.spawn(|| forgotten_runs(&served.post(t, &body)))).collect();
        asks.into_iter().map(|ask| ask.join().unwrap()[0].1.clone()).collect()
    });
    let mut sorted = outcomes.clone();
    sorted.sort();
    assert_eq!(sorted, ["forgotten", "skipped"], "{outcomes:?}");
    let archive = std::fs::read_to_string(project.0.join(".octoflow-ai/metrics-archive.jsonl")).unwrap();
    assert_eq!(archive.matches(FORGET_CLOSED).count(), 1, "{archive}");
}

#[test]
fn the_sweeps_leave_out_a_run_the_overview_does_not_show() {
    // A run directory linked from outside the runs directory: the overview leaves it out (Host::run_states_inside),
    // so close and forget, which act on what it shows, must too (todo/75, #507).
    const OUTSIDE: &str = "20261001-120200-0a0b0c";
    let project = Project::new();
    project.save(FORGET_CLOSED, &[("closed", Value::from("workspace closed"))]);
    project.write(&format!("elsewhere/{OUTSIDE}/state.json"), &serve_state(OUTSIDE, &[("closed", Value::from("x"))]));
    project.write(&format!("elsewhere/{OUTSIDE}/metrics.json"), &format!("{{\"run_id\": \"{OUTSIDE}\"}}\n"));
    let (outside, link) =
        (project.0.join(&format!("elsewhere/{OUTSIDE}")), project.0.join(&format!("{RUNS_DIR}/{OUTSIDE}")));
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let served = Served::real();

    let closing = closed_runs(&served.post(&project.query("/close"), r#"{"dry_run": true}"#));
    assert_eq!(closing.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(), [FORGET_CLOSED]);
    let forgetting = forgotten_runs(&served.post(&project.query("/forget"), r#"{"dry_run": true}"#));
    assert_eq!(forgetting.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(), [FORGET_CLOSED]);

    let body = serde_json::json!({ "forget": [OUTSIDE] }).to_string();
    let forgotten = forgotten_runs(&served.post(&project.query("/forget"), &body));
    assert_eq!(forgotten, [(OUTSIDE.into(), "skipped".into(), "the overview shows no such run any more".into())]);
    assert!(std::path::Path::new(&link).exists(), "the link is kept");
    assert!(
        !std::path::Path::new(&project.0.join(".octoflow-ai/metrics-archive.jsonl")).exists(),
        "no metrics archived"
    );
}

#[test]
fn a_bad_forget_body_is_refused_before_the_project_is_reached() {
    let connects = Arc::new(Mutex::new(vec![]));
    let seen = Arc::clone(&connects);
    let connect = move |machine: &str, cwd: &str, _: &Interrupt| {
        seen.lock().unwrap().push((machine.to_string(), cwd.to_string()));
        Err(OrchestratorError::new("ssh: connection refused"))
    };
    let served = Served::start(Box::new(connect), &[], Arc::clone(&connects), Options::default());
    let target = Served::query("/forget", &[("machine", "box")]);
    let wrong = r#"the body must be {"dry_run": true} or {"forget": ["...", ...]}"#;
    let bad = [
        "",
        "[]",
        "{}",
        r#"{"dry_run": false}"#,
        r#"{"forget": "r1"}"#,
        r#"{"forget": [1]}"#,
        r#"{"forget": ["../r1"]}"#,
        r#"{"forget": ["r1"], "x": 1}"#,
    ];
    for body in bad {
        let reply = served.post(&target, body);
        assert_eq!((reply.status, reply.error()), (Some(400), wrong.to_string()), "{body}");
    }
    let long = format!(r#"{{"dry_run": true, "x": "{}"}}"#, "x".repeat(MAX_BODY_BYTES));
    assert_eq!(served.post(&target, &long).status, Some(413));
    assert!(connects.lock().unwrap().is_empty(), "a bad body reached the project");
    let reply = served.post(&target, r#"{"dry_run": true}"#);
    assert_eq!((reply.status, reply.error()), (Some(502), "ssh: connection refused".to_string()));
}

#[test]
fn workflows_are_the_configuration_s_with_their_roles_and_the_harnesses() {
    let d = TempDir::new();
    std::fs::create_dir_all(d.join("workflows")).unwrap();
    let solo =
        "description = \"one builder\"\n[roles]\nbuild = { label = \"Solo\", agent = \"codex\", model = \"o3\" }\n\
                [[steps]]\nid = \"build\"\nfile = \"build.md\"\nprompt = \"Build {task} in {cwd}; report to {path}.\"\n\
                edits = true\n";
    std::fs::write(d.join("workflows/solo.toml"), solo).unwrap();
    std::fs::write(d.join("workflows/broken.toml"), "not toml [").unwrap();
    let served = Served::fake_with(
        &Arc::new(ServeHost::on(None)),
        &[],
        Options { config: ConfigDir::at(&d.path()), ..Options::default() },
    );
    let reply = served.get("/workflows", &[]);
    assert_eq!(reply.status, Some(200), "{}", reply.body);
    let answer = reply.json();
    assert_eq!(answer["agents"], serde_json::json!(crate::constants::HARNESS_NAMES));
    let workflows = answer["workflows"].as_array().unwrap();
    let names: Vec<&str> = workflows.iter().map(|w| w["name"].as_str().unwrap()).collect();
    let mut expected: Vec<&str> = crate::workflow_file::WORKFLOWS.iter().copied().chain(["broken", "solo"]).collect();
    expected.sort_unstable();
    assert_eq!(names, expected);
    let named = |name: &str| workflows.iter().find(|w| w["name"] == name).unwrap().clone();
    let roles = serde_json::json!([
        {"role": "spec", "label": "Spec Collector", "agent": "claude", "model": null},
        {"role": "build", "label": "Builder", "agent": "claude", "model": null},
        {"role": "review", "label": "Reviewer", "agent": "claude", "model": null},
    ]);
    assert_eq!(named("default")["roles"], roles);
    let solo = named("solo");
    assert_eq!(solo["description"], "one builder", "{solo}");
    assert_eq!(solo["roles"], serde_json::json!([{"role": "build", "label": "Solo", "agent": "codex", "model": "o3"}]));
    let broken = named("broken");
    let broken_file = d.join("workflows/broken.toml");
    assert!(broken["error"].as_str().is_some_and(|e| e.starts_with(&format!("{broken_file}: "))), "{broken}");
    assert_eq!(broken.as_object().unwrap().len(), 2);
    // No project is reached, so none is named.
    assert!(served.connects.lock().unwrap().is_empty());
    assert_eq!(served.ask("GET", "/workflows", &[]).status, Some(401));
}

/// A server whose workflow files are in dir's workflows directory, and whose projects are none it should reach.
fn editing(dir: &TempDir) -> Served {
    Served::fake_with(
        &Arc::new(ServeHost::on(None)),
        &[],
        Options { config: ConfigDir::at(&dir.path()), ..Options::default() },
    )
}

#[test]
fn a_workflow_reads_with_its_file_or_none_for_a_built_in_one() {
    let d = TempDir::new();
    std::fs::create_dir_all(d.join("workflows")).unwrap();
    let mine = "# default's steps, by name\n[[steps]]\nuse = \"spec\"\n\n[[steps]]\nuse = \"build\"\n";
    std::fs::write(d.join("workflows/mine.toml"), mine).unwrap();
    std::fs::write(d.join("workflows/broken.toml"), "not toml [").unwrap();
    let served = editing(&d);
    let reply = served.get("/workflow?name=default", &[]);
    assert_eq!(reply.status, Some(200), "{}", reply.body);
    let default = reply.json();
    assert_eq!((default["name"].clone(), default["built_in"].clone()), (Value::from("default"), Value::from(true)));
    assert_eq!((default["path"].clone(), default["text"].clone()), (Value::Null, Value::Null));
    assert_eq!(default["hand_written"], false);
    let definition = crate::workflow_file::workflow_definition(&crate::pipeline::DEFAULT_WORKFLOW);
    assert_eq!(default["definition"], definition);
    let reply = served.get("/workflow?name=mine", &[]);
    assert_eq!(reply.status, Some(200), "{}", reply.body);
    let read = reply.json();
    assert_eq!(read["built_in"], false);
    assert_eq!(
        (read["path"].clone(), read["text"].clone()),
        (Value::from(d.join("workflows/mine.toml")), Value::from(mine))
    );
    assert_eq!(read["hand_written"], true);
    assert_eq!(read["definition"]["steps"].as_array().map(Vec::len), Some(2), "{read}");
    let reply = served.get("/workflow?name=broken", &[]);
    let broken = d.join("workflows/broken.toml");
    assert_eq!(reply.status, Some(422), "{}", reply.body);
    assert_eq!(
        (reply.json()["path"].clone(), reply.json()["text"].clone()),
        (Value::from(broken.as_str()), Value::from("not toml ["))
    );
    assert!(reply.error().starts_with(&format!("{broken}: ")), "{}", reply.body);
    let reply = served.get("/workflow?name=nope", &[]);
    assert_eq!((reply.status, reply.error()), (Some(404), "no workflow nope".to_string()));
    assert_eq!(served.get("/workflow", &[]).status, Some(400));
    assert_eq!(served.ask("GET", "/workflow?name=default", &[]).status, Some(401));
    assert!(served.connects.lock().unwrap().is_empty(), "a project was reached");
}

#[test]
fn a_check_answers_the_problem_and_the_placeholders() {
    let d = TempDir::new();
    let served = editing(&d);
    let definition = crate::workflow_file::workflow_definition(&crate::pipeline::DEFAULT_WORKFLOW);
    let body = serde_json::json!({"name": "mine", "definition": definition}).to_string();
    let reply = served.post("/workflow/check", &body);
    assert_eq!(reply.status, Some(200), "{}", reply.body);
    let ids = crate::pipeline::DEFAULT_WORKFLOW.steps().iter().map(|st| st.id.as_str());
    let placeholders = Value::from(crate::pipeline::prompt_placeholders(ids));
    assert_eq!(reply.json(), serde_json::json!({"problem": null, "placeholders": placeholders}));
    let body = serde_json::json!({"name": "default", "definition": definition}).to_string();
    let checked = served.post("/workflow/check", &body).json();
    assert_eq!(checked["problem"], "workflow default is built in, so no file may take its name");
    assert_eq!(checked["placeholders"], placeholders);
    for bad in [
        "not json".to_string(),
        r#"{"name": "mine"}"#.to_string(),
        serde_json::json!({"name": "mine", "definition": []}).to_string(),
        serde_json::json!({"name": "mine", "definition": definition, "base": null}).to_string(),
    ] {
        let reply = served.post("/workflow/check", &bad);
        assert_eq!(reply.status, Some(400), "{bad}: {}", reply.body);
        assert!(reply.error().starts_with("the body must be {"), "{}", reply.body);
    }
    assert!(served.connects.lock().unwrap().is_empty(), "a project was reached");
}

#[test]
fn a_save_writes_the_file_once_it_is_still_what_the_editor_opened() {
    let d = TempDir::new();
    let served = editing(&d);
    let definition = crate::workflow_file::workflow_definition(&crate::pipeline::DEFAULT_WORKFLOW);
    let save = |base: Value| serde_json::json!({"name": "mine", "definition": definition, "base": base}).to_string();
    let path = d.join("workflows/mine.toml");
    let reply = served.post("/workflow/save", &save(Value::Null));
    assert_eq!((reply.status, reply.json()), (Some(201), serde_json::json!({"path": path})), "{}", reply.body);
    let saved = std::fs::read_to_string(&path).unwrap();
    // A new workflow under a name a file has, and a base that is not the file.
    let reply = served.post("/workflow/save", &save(Value::Null));
    assert_eq!((reply.status, reply.error()), (Some(409), format!("{path} already exists")));
    let reply = served.post("/workflow/save", &save(Value::from("# another text\n")));
    assert_eq!((reply.status, reply.error()), (Some(409), format!("{path} changed since it was opened")));
    assert_eq!(served.post("/workflow/save", &save(Value::from(saved.as_str()))).status, Some(201));
    // A problem, and bodies of another shape: base missing or not a string.
    let invalid = serde_json::json!({"name": "mine", "definition": {"steps": []}, "base": saved}).to_string();
    let reply = served.post("/workflow/save", &invalid);
    assert_eq!(reply.status, Some(422), "{}", reply.body);
    assert_eq!(reply.json().as_object().map(|o| o.len()), Some(1), "{}", reply.body);
    let missing = serde_json::json!({"name": "mine", "definition": definition}).to_string();
    assert_eq!(served.post("/workflow/save", &missing).status, Some(400));
    assert_eq!(served.post("/workflow/save", &save(Value::from(1))).status, Some(400));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), saved);
    assert!(served.connects.lock().unwrap().is_empty(), "a project was reached");
}

#[test]
fn a_delete_removes_the_file_the_editor_opened() {
    let d = TempDir::new();
    std::fs::create_dir_all(d.join("workflows")).unwrap();
    let text = "[[steps]]\nuse = \"build\"\n";
    let path = d.join("workflows/mine.toml");
    std::fs::write(&path, text).unwrap();
    let served = editing(&d);
    let delete = |name: &str, base: &str| serde_json::json!({"name": name, "base": base}).to_string();
    let reply = served.post("/workflow/delete", &delete("mine", "# another text\n"));
    assert_eq!((reply.status, reply.error()), (Some(409), format!("{path} changed since it was opened")));
    let reply = served.post("/workflow/delete", &delete("mine", text));
    assert_eq!((reply.status, reply.json()), (Some(200), serde_json::json!({"path": path})), "{}", reply.body);
    assert!(!std::path::Path::new(&path).exists());
    let reply = served.post("/workflow/delete", &delete("mine", text));
    assert_eq!((reply.status, reply.error()), (Some(404), "no workflow file mine".to_string()));
    assert_eq!(served.post("/workflow/delete", &delete("default", "")).status, Some(400));
    assert_eq!(served.post("/workflow/delete", r#"{"name": "mine", "base": null}"#).status, Some(400));
    assert!(served.connects.lock().unwrap().is_empty(), "a project was reached");
}

#[test]
fn a_workflow_post_needs_the_token_and_a_body_within_bounds() {
    let d = TempDir::new();
    let served = editing(&d);
    let body = r#"{"name": "mine", "base": "x"}"#;
    let length = body.len().to_string();
    for target in ["/workflow/check", "/workflow/save", "/workflow/delete"] {
        assert_eq!(
            served.ask_with_body("POST", target, &[("Content-Length", &length)], body).status,
            Some(401),
            "{target}"
        );
        let long = format!(r#"{{"name": "{}"}}"#, "x".repeat(MAX_BODY_BYTES));
        assert_eq!(served.post(target, &long).status, Some(413), "{target}");
    }
    assert!(!std::path::Path::new(&d.join("workflows")).exists());
    assert!(served.connects.lock().unwrap().is_empty(), "a project was reached");
}

// -- Events, here and on a machine -------------------------------------------------------------------------------

/// An event stream as an EventSource reads it, with the token in the URL, its events parsed on a thread as they
/// arrive. It ends when the server stops.
struct EventStream {
    /// The response's status line and headers.
    head: String,
    /// Each event as (kind, payload, the count of quiet polls before it).
    events: mpsc::Receiver<(String, Value, usize)>,
    /// The count of quiet polls so far, sent at each: polls that found nothing to tell, each of which the server tells
    /// by a keep-alive comment.
    quiet_counts: mpsc::Receiver<usize>,
    /// Gets one message when the response has ended.
    ended: mpsc::Receiver<()>,
    /// The connection, which the stream's reader shares, closed when the stream is dropped, as a client that goes.
    conn: TcpStream,
}

impl Drop for EventStream {
    fn drop(&mut self) {
        let _ = self.conn.shutdown(std::net::Shutdown::Both);
    }
}

impl EventStream {
    /// The stream of query, a path with its query, asked with extra_headers, each a line with its line end.
    fn open(served: &Served, query: &str, extra_headers: &str) -> EventStream {
        let mut conn = TcpStream::connect(("127.0.0.1", served.port)).unwrap();
        let target = format!("{query}&token={}", served.token());
        write!(conn, "GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n{extra_headers}\r\n", served.port).unwrap();
        let mut reader = BufReader::new(conn.try_clone().expect("the connection is shared"));
        let mut head = String::new();
        while !head.ends_with("\r\n\r\n") {
            assert_ne!(reader.read_line(&mut head).expect("the head"), 0, "the response ended in its head: {head}");
        }
        let (send, events) = mpsc::channel();
        let (send_quiet, quiet_counts) = mpsc::channel();
        let (end, ended) = mpsc::channel();
        std::thread::spawn(move || {
            let (mut kind, mut payload, mut quiet_polls) = (None, None, 0);
            each_chunked_line(reader, |line| {
                if line == ":" {
                    quiet_polls += 1;
                    let _ = send_quiet.send(quiet_polls);
                } else if let Some(k) = line.strip_prefix("event: ") {
                    kind = Some(k.to_string());
                } else if let Some(d) = line.strip_prefix("data: ") {
                    payload = Some(serde_json::from_str::<Value>(d).expect("JSON data"));
                } else if let (true, Some(k), Some(p)) = (line.is_empty(), kind.take(), payload.take()) {
                    let _ = send.send((k, p, quiet_polls));
                }
            });
            let _ = end.send(());
        });
        EventStream { head, events, quiet_counts, ended, conn }
    }

    /// The next event, as (kind, payload); fails the test when none comes within timeout.
    fn next(&self, timeout: Duration) -> (String, Value) {
        let (kind, payload, _) = self.next_counted(timeout);
        (kind, payload)
    }

    /// The next event, as (kind, payload, the count of quiet polls before it); fails the test when none comes within
    /// timeout.
    fn next_counted(&self, timeout: Duration) -> (String, Value, usize) {
        self.events.recv_timeout(timeout).unwrap_or_else(|_| panic!("no event within {timeout:?}"))
    }

    /// The count of quiet polls, once one arrives after this call, the latest the stream's reader has passed on: the
    /// server has then finished that poll and is waiting for the next, unless the reader has fallen a poll interval
    /// behind it, when the count lags. Fails the test when none comes within timeout.
    fn next_quiet_poll(&self, timeout: Duration) -> usize {
        while self.quiet_counts.try_recv().is_ok() {}
        let n = self.quiet_counts.recv_timeout(timeout).unwrap_or_else(|_| panic!("no quiet poll within {timeout:?}"));
        self.quiet_counts.try_iter().last().unwrap_or(n)
    }

    fn kinds(&self, n: usize) -> Vec<String> {
        (0..n).map(|_| self.next(Duration::from_secs(2)).0).collect()
    }
}

/// Call on_line with each line of a chunked body as it arrives, without its line end, until its last chunk or the
/// connection's end.
fn each_chunked_line(mut reader: BufReader<TcpStream>, mut on_line: impl FnMut(&str)) {
    let mut body = Vec::new();
    loop {
        let mut size = String::new();
        if reader.read_line(&mut size).unwrap_or(0) == 0 {
            return;
        }
        let n = usize::from_str_radix(size.trim(), 16).expect("a chunk's size");
        if n == 0 {
            return;
        }
        let mut chunk = vec![0; n + 2];
        if reader.read_exact(&mut chunk).is_err() {
            return;
        }
        assert_eq!(&chunk[n..], b"\r\n");
        body.extend_from_slice(&chunk[..n]);
        while let Some(i) = body.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = body.drain(..=i).collect();
            on_line(&String::from_utf8_lossy(&line[..i]));
        }
    }
}

#[test]
fn the_stream() {
    let d = Project::new();
    d.save(SERVE_ID, &[]);
    d.write(&format!("{RUNS_DIR}/{SERVE_ID}/spec.md"), "# Spec\n");
    let served = Served::real();
    let stream = EventStream::open(&served, &d.query("/events"), "");
    assert!(stream.head.starts_with("HTTP/1.1 200 OK\r\n"), "{}", stream.head);
    assert!(stream.head.contains("\r\nContent-Type: text/event-stream\r\n"), "{}", stream.head);
    assert!(stream.head.contains("\r\nCache-Control: no-store\r\n"), "{}", stream.head);
    let (kind, record) = stream.next(Duration::from_secs(2));
    assert_eq!((kind.as_str(), record.get("run")), ("record", Some(&Value::from(SERVE_ID))));
    assert_eq!(record.get("record").and_then(|r| r.get("phase")), Some(&Value::from("build")));
    let (kind, files) = stream.next(Duration::from_secs(2));
    assert_eq!(kind, "files");
    let names = |files: &Value| -> Vec<String> {
        files.get("files").and_then(Value::as_array).unwrap().iter().map(|f| f.get("name").unwrap().text()).collect()
    };
    assert_eq!(names(&files), ["spec.md"]);

    // A change is told by the first poll that starts after it, which is counted here in polls, not seconds, as a busy
    // host stretches each poll (one took 1.4 s beside the other tests, so that an event came 2.4 s after its change;
    // GitHub issue #335). Each change is made just after a quiet poll, while the server waits SERVE_POLL.local. The
    // next poll may still start before the change is done, when this thread is slower than that wait, and find nothing:
    // so one quiet poll before the event is allowed, and a second means a poll that started after the change missed it.
    // A change told one poll late passes here; RunWatch's own tests (serve/runs/tests.rs) catch that. A correct server
    // fails the bound only when this thread, or the stream's reader, falls about a whole interval behind. The timeout
    // only ends a test whose server hangs.
    let timeout = SERVE_POLL.local * 10;
    let quiet_poll = || stream.next_quiet_poll(timeout);
    // The next event, which must be of expected_kind for run and come at most one quiet poll after the count `before`.
    let soon = |before: usize, expected_kind: &str, run: &str| {
        let (kind, payload, quiet_polls) = stream.next_counted(timeout);
        assert_eq!((kind.as_str(), payload.get("run")), (expected_kind, Some(&Value::from(run))));
        assert!(quiet_polls <= before + 1, "{kind} after {} quiet polls", quiet_polls - before);
        payload
    };
    let before = quiet_poll();
    d.save(SERVE_ID, &[("phase", Value::from("review"))]);
    let record = soon(before, "record", SERVE_ID);
    assert_eq!(record.get("record").and_then(|r| r.get("phase")), Some(&Value::from("review")));

    // A second later, so that the new file sorts after spec.md by its modification time.
    std::thread::sleep(Duration::from_millis(1100));
    let before = quiet_poll();
    d.write(&format!("{RUNS_DIR}/{SERVE_ID}/build-1.md"), "built");
    let files = soon(before, "files", SERVE_ID);
    assert_eq!(names(&files), ["spec.md", "build-1.md"]);

    let before = quiet_poll();
    d.save("20261024-130000-a11ce0", &[]);
    soon(before, "record", "20261024-130000-a11ce0");
    assert_eq!(soon(before, "files", "20261024-130000-a11ce0").get("files"), Some(&Value::Array(vec![])));

    let before = quiet_poll();
    std::fs::remove_dir_all(format!("{}/{RUNS_DIR}/{SERVE_ID}", d.root())).unwrap();
    assert_eq!(soon(before, "record", SERVE_ID).get("record"), Some(&Value::Null));
}

#[test]
fn nothing_while_nothing_changes() {
    let served = Served::fake(&host_with_run(None), &[]);
    let stream = EventStream::open(&served, &Served::query("/events", &[]), "");
    stream.kinds(2);
    assert!(stream.events.recv_timeout(Duration::from_millis(2500)).is_err());
}

#[test]
fn a_state_json_that_is_not_json_is_the_record_of_an_unreadable_run() {
    // Not a problem of the whole project, which would hide every other run (cross-review 194 F1).
    let d = Project::new();
    d.write(&format!("{RUNS_DIR}/{SERVE_ID}/state.json"), "{not json");
    let served = Served::real();
    let stream = EventStream::open(&served, &d.query("/events"), "");
    let (kind, payload) = stream.next(Duration::from_secs(2));
    assert_eq!(kind, "record");
    assert_eq!(payload.get("run").unwrap().text(), SERVE_ID);
    let error = payload.get("record").unwrap().get("error").unwrap().text();
    assert!(error.contains(&format!("{SERVE_ID}/state.json is not JSON (")), "{payload:?}");
    let started = Instant::now();
    d.save(SERVE_ID, &[]);
    // The poll that finds the run readable also tells its files, in either order.
    let payload = std::iter::repeat_with(|| stream.next(Duration::from_secs(3)))
        .find_map(|(kind, payload)| (kind == "record").then_some(payload))
        .unwrap();
    assert_eq!(payload.get("record").unwrap().get("error"), Some(&Value::Null), "{payload:?}");
    assert!(started.elapsed() <= Duration::from_secs(2), "{:?}", started.elapsed());
}

#[test]
fn the_stream_ends_when_the_server_stops() {
    let served = Served::fake(&Arc::new(ServeHost::on(None)), &[]);
    let stream = EventStream::open(&served, &Served::query("/events", &[]), "");
    drop(served);
    assert!(stream.ended.recv_timeout(Duration::from_secs(3)).is_ok());
}

#[test]
fn a_stream_to_an_http_1_0_client_ends_with_its_connection() {
    let served = Served::fake(&Arc::new(ServeHost::on(None)), &[]);
    let raw = format!(
        "GET {}&token={} HTTP/1.0\r\nHost: 127.0.0.1:{}\r\n\r\n",
        Served::query("/events", &[]),
        served.token(),
        served.port
    );
    let port = served.port;
    let reader = std::thread::spawn(move || exchange(port, raw.as_bytes()));
    std::thread::sleep(Duration::from_millis(300));
    drop(served);
    let reply = reader.join().unwrap();
    assert_eq!(reply.status, Some(200));
    assert_eq!(reply.header("Transfer-Encoding"), None);
    assert_eq!(reply.body, ":\n\n");
}

#[test]
fn a_client_that_goes_ends_its_stream() {
    // The stream's thread finds the client gone at a write, and polls no more.
    let host = host_with_run(None);
    let fast = Polls { local: Duration::from_millis(20), remote: Duration::from_millis(20) };
    let served = Served::fake_polling(&host, &[], fast);
    let stream = EventStream::open(&served, &Served::query("/events", &[]), "");
    stream.kinds(2);
    drop(stream);
    // A write or two may still succeed before the server sees the connection reset.
    std::thread::sleep(Duration::from_millis(500));
    let polls = host.state().run_states_inside_calls;
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(host.state().run_states_inside_calls, polls);
}

#[test]
fn changes_are_polled_on_a_machine() {
    let host = host_with_run(Some("remote-host"));
    let polls = Polls { local: Duration::from_secs(60), remote: Duration::from_millis(100) };
    let served = Served::fake_polling(&host, &[], polls);
    let stream = EventStream::open(&served, &Served::query("/events", &[("machine", "remote")]), "");
    assert_eq!(stream.kinds(2), ["record", "files"]);
    host.state().files.insert(format!("{}/review-1.md", run_dir()), "VERDICT: APPROVE\n".to_string());
    let (kind, payload) = stream.next(Duration::from_secs(2));
    assert_eq!(kind, "files");
    assert_eq!(payload.to_string(), format!(r#"{{"run":"{SERVE_ID}","files":[{{"name":"review-1.md","mtime":0}}]}}"#));
    host.state().files.remove(&format!("{}/state.json", run_dir()));
    let gone = object([("run", Value::from(SERVE_ID)), ("record", Value::Null)]);
    assert_eq!(stream.next(Duration::from_secs(2)), ("record".to_string(), gone));
}

#[test]
fn a_machine_is_polled_less_often() {
    let host = host_with_run(Some("remote-host"));
    let polls = Polls { local: Duration::from_millis(100), remote: Duration::from_secs(60) };
    let served = Served::fake_polling(&host, &[], polls);
    let stream = EventStream::open(&served, &Served::query("/events", &[("machine", "remote")]), "");
    // The first poll, whenever the handler gets to it, sends the run; only a change after it waits for the next.
    assert_eq!(stream.kinds(2), ["record", "files"]);
    host.state()
        .files
        .insert(format!("{}/state.json", run_dir()), serve_state(SERVE_ID, &[("phase", Value::from("review"))]));
    assert!(stream.events.recv_timeout(Duration::from_secs(1)).is_err());
}

#[test]
fn the_stream_allows_the_webview_origin() {
    let served = Served::fake(&host_with_run(None), &[TAURI_ORIGIN]);
    let stream = EventStream::open(&served, &Served::query("/events", &[]), &format!("Origin: {TAURI_ORIGIN}\r\n"));
    assert!(
        stream.head.contains(&format!("\r\nAccess-Control-Allow-Origin: {TAURI_ORIGIN}\r\nVary: Origin\r\n")),
        "{}",
        stream.head
    );
    assert_eq!(stream.kinds(2), ["record", "files"]);
}

#[test]
fn a_stream_is_refused_as_any_request() {
    let served = Served::fake(&host_with_run(None), &[TAURI_ORIGIN]);
    let events = Served::query("/events", &[]);
    assert_eq!(served.ask("GET", &events, &[]).status, Some(401));
    assert_eq!(served.ask("GET", &format!("{events}&token=x"), &[]).status, Some(401));
    let with_token = format!("{events}&token={}", served.token());
    assert_eq!(served.ask("GET", &with_token, &[("Origin", "http://evil.example")]).status, Some(403));
    assert_eq!(served.ask("GET", &with_token, &[("Host", "evil.example")]).status, Some(403));
}

// -- serve_command's in-process cases ----------------------------------------------------------------------------

/// A writer that hands each write to a channel, for a line printed while serve still runs.
struct Lines(mpsc::Sender<Vec<u8>>);

impl Write for Lines {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let _ = self.0.send(buf.to_vec());
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn in_process() {
    let host = Arc::new(ServeHost::on(None));
    let connect = move |_: &str, _: &str, _: &Interrupt| Ok(reached(&host, &Arc::default()));
    let (lines, printed) = mpsc::channel();
    let (stop, stopped) = mpsc::channel::<()>();
    let serving = std::thread::spawn(move || {
        let mut err = vec![];
        let status = serve(
            RunServer::start(&[], Box::new(connect), unchecked(), ConfigDir::at("/nowhere/config")),
            &mut Lines(lines),
            &mut err,
            || stopped.recv().map_err(io::Error::other),
        );
        (status, err)
    });
    let mut line = vec![];
    while !line.contains(&b'\n') {
        line.extend(printed.recv_timeout(Duration::from_secs(10)).expect("serve prints its line"));
    }
    let line = String::from_utf8(line).unwrap();
    assert!(line.ends_with('\n') && line.matches('\n').count() == 1, "{line:?}");
    let first = serde_json::from_str::<Value>(&line).unwrap();
    assert_eq!(first.as_object().unwrap().keys().collect::<Vec<_>>(), ["url", "token"]);
    let port = port_of(&first.get("url").unwrap().text());
    let token = first.get("token").unwrap().text();
    let raw = format!("GET /runs?cwd=x&token={token} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    let reply = exchange(port, raw.as_bytes());
    assert_eq!((reply.status, reply.body.as_str()), (Some(200), r#"{"version":1,"runs":[]}"#));
    stop.send(()).unwrap();
    let (status, err) = serving.join().unwrap();
    assert_eq!((status, err), (0, vec![]));
    assert_eq!(status_at(port), None, "the stopped server still answers");
}

/// A writer whose every write fails as a pipe whose reader has gone.
struct BrokenPipe;

impl Write for BrokenPipe {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::from_raw_os_error(32))
    }

    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::from_raw_os_error(32))
    }
}

#[test]
fn an_unprintable_line_ends_it() {
    let connect = |_: &str, _: &str, _: &Interrupt| Err(OrchestratorError::new("not asked"));
    let started = RunServer::start(&[], Box::new(connect), unchecked(), ConfigDir::at("/nowhere/config")).unwrap();
    let port = port_of(started.url());
    let mut err = vec![];
    let status = serve(Ok(started), &mut BrokenPipe, &mut err, || -> io::Result<()> { panic!("serve waited") });
    assert_eq!(status, EXIT_ERROR);
    assert_eq!(String::from_utf8(err).unwrap(), "error: cannot print the server's address: Broken pipe\n");
    assert_eq!(status_at(port), None, "the stopped server still answers");
}

#[test]
fn no_port() {
    let mut err = vec![];
    let unbound = Err(io::Error::from_raw_os_error(98));
    let status = serve(unbound, &mut Vec::new(), &mut err, || -> io::Result<()> { panic!("serve waited") });
    assert_eq!(status, EXIT_ERROR);
    assert_eq!(String::from_utf8(err).unwrap(), "error: cannot serve on 127.0.0.1: Address already in use\n");
}

#[test]
fn a_wait_for_the_signals_that_fails_ends_it() {
    let connect = |_: &str, _: &str, _: &Interrupt| Err(OrchestratorError::new("not asked"));
    let started = RunServer::start(&[], Box::new(connect), unchecked(), ConfigDir::at("/nowhere/config")).unwrap();
    let port = port_of(started.url());
    let (mut out, mut err) = (vec![], vec![]);
    let status = serve(Ok(started), &mut out, &mut err, || Err(io::Error::from_raw_os_error(22)));
    assert_eq!(status, EXIT_ERROR);
    assert!(String::from_utf8(out).unwrap().starts_with(r#"{"url":"http://127.0.0.1:"#));
    assert_eq!(String::from_utf8(err).unwrap(), "error: cannot wait for SIGINT and SIGTERM: Invalid argument\n");
    assert_eq!(status_at(port), None, "the stopped server still answers");
}

#[test]
fn a_server_that_stops_accepting_is_started_again_on_the_port() {
    // Not faithful to an accept error: unblock makes the server's recv fail while the RunServer is not stopping, but
    // leaves its accept thread running, which a real accept error ends. That is why dispatch must drop the first server
    // before the new one accepts on the port.
    let served = Served::fake(&Arc::new(ServeHost::on(None)), &[]);
    // Weak: a strong reference here would keep the first server alive, and its accept thread with it, past the restart.
    let first = Arc::downgrade(&lock(&served.server.listening).clone().expect("it listens"));
    first.upgrade().expect("it is alive").unblock();
    let started = Instant::now();
    while lock(&served.server.listening).as_ref().is_none_or(|http| std::ptr::eq(first.as_ptr(), Arc::as_ptr(http))) {
        assert!(started.elapsed() < Duration::from_secs(5), "no server was started again");
        std::thread::sleep(Duration::from_millis(10));
    }
    // Not at once: a lasting accept error would make each new server stop at once, and the restarts spin.
    assert!(started.elapsed() >= Duration::from_millis(100), "started again after {:?}", started.elapsed());
    assert!(first.upgrade().is_none(), "the first server was not dropped before its successor started");
    assert_eq!(served.get(&Served::query("/runs", &[]), &[]).status, Some(200));
    let port = served.port;
    drop(served);
    assert_eq!(status_at(port), None, "the stopped server still answers");
}

#[test]
fn a_request_names_its_target_flags() {
    assert_eq!(target_args("", "~/proj"), ["--cwd", "~/proj"]);
    assert_eq!(target_args("box", "/p"), ["--machine", "box", "--cwd", "/p"]);
    assert_eq!(target_args("", ""), Vec::<String>::new());
}
