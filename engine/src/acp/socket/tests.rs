use std::sync::Mutex;

use super::*;
use crate::test_support::{eventually, TempDir};

/// Commands that record what they were asked, and answer request 1 with option `allow` alone, once.
#[derive(Default)]
struct Asked(Mutex<Vec<String>>);

impl Commands for Asked {
    fn answer(&self, role: &str, request: u64, option: &str) -> Result<(), CommandError> {
        let mut asked = self.0.lock().unwrap();
        asked.push(format!("answer {role} {request} {option}"));
        match (role, request, option) {
            ("builder", 1, "allow") if asked.len() == 1 => Ok(()),
            ("builder", 1, "allow") => {
                Err(CommandError::AlreadyAnswered { option: "allow".into(), by: "human".into() })
            }
            ("builder", 1, _) => Err(CommandError::BadOption),
            ("builder", _, _) => Err(CommandError::NoRequest),
            _ => Err(CommandError::NoAgent),
        }
    }

    /// Takes a message to the builder alone.
    fn message(&self, role: &str, text: &str) -> Result<(), CommandError> {
        self.0.lock().unwrap().push(format!("message {role} {text}"));
        match role {
            "builder" => Ok(()),
            "reviewer" => Err(CommandError::NotWaiting),
            _ => Err(CommandError::NoAgent),
        }
    }

    fn stop(&self) {
        self.0.lock().unwrap().push("stop".to_string());
    }
}

/// The run key a test's socket is bound for: its process's own, so that two tests at once whose run directories are
/// too long for a socket address (as on Jenkins) do not share the socket that /tmp then holds for the key.
fn key() -> String {
    format!("t{}", std::process::id())
}

/// A run directory in tmp with a listener on its socket, answering by asked.
fn listening(tmp: &TempDir, asked: &Arc<Asked>) -> (String, Listener) {
    let dir = tmp.join("run");
    std::fs::create_dir_all(&dir).unwrap();
    let commands: Arc<dyn Commands> = asked.clone();
    let listener = Listener::bind(&dir, "20261120-000000-abcd", &key(), commands).expect("bound");
    (dir, listener)
}

/// A connection to the socket dir's record names, and the lines it answers sent, in order.
fn talk(dir: &str, sent: &[&str]) -> Vec<Value> {
    let path = std::fs::read_to_string(format!("{dir}/{SOCKET_RECORD}")).unwrap();
    let stream = UnixStream::connect(path.trim_end()).expect("connected");
    let mut out = stream.try_clone().unwrap();
    let mut lines = BufReader::new(stream);
    let mut answers = vec![];
    for line in sent {
        // A connection the socket closed refuses the write, or ends the read.
        let mut answer = String::new();
        if out.write_all(format!("{line}\n").as_bytes()).is_err() || lines.read_line(&mut answer).unwrap_or(0) == 0 {
            break;
        }
        answers.push(serde_json::from_str(&answer).expect("JSON"));
    }
    answers
}

#[test]
fn the_socket_is_in_the_run_directory_when_it_fits_and_else_in_the_user_s_tmp_directory() {
    let short = "/home/u/p/.octoflow-ai/runs/20261120-000000-abcd";
    let long = format!("/home/u/{}/.octoflow-ai/runs/20261120-000000-abcd", "p".repeat(60));

    assert_eq!(socket_path(short, "abcd", 1000), format!("{short}/sock/run.sock"));
    assert_eq!(socket_path(&long, "abcd", 1000), "/tmp/octoflow-1000/abcd.sock");
    let fits = format!("/{}", "d".repeat(SUN_PATH_MAX - "/sock/run.sock".len() - 1));
    assert_eq!(socket_path(&fits, "k", 1).len(), SUN_PATH_MAX);
    assert_eq!(socket_path(&format!("{fits}x"), "k", 1), "/tmp/octoflow-1/k.sock");
}

#[test]
fn a_bound_socket_is_0600_in_a_0700_directory_recorded_and_removed_with_its_record_when_closed() {
    let tmp = TempDir::new();
    let asked = Arc::new(Asked::default());
    let (dir, listener) = listening(&tmp, &asked);

    // Where the socket is depends on how long the temporary directory's path is (socket_path), as on Jenkins.
    let path = socket_path(&dir, &key(), nix::unistd::getuid().as_raw());
    let parent = path.rsplit_once('/').unwrap().0.to_string();
    let mode = |p: &str| std::fs::symlink_metadata(p).unwrap().mode() & 0o777;
    assert_eq!((mode(&path), mode(&parent)), (0o600, 0o700));
    assert_eq!(std::fs::read_to_string(format!("{dir}/{SOCKET_RECORD}")).unwrap(), format!("{path}\n"));
    drop(listener);

    assert!(!std::path::Path::new(&path).exists());
    assert!(!std::path::Path::new(&format!("{dir}/{SOCKET_RECORD}")).exists());
}

#[test]
fn a_socket_left_by_a_killed_orchestrator_is_replaced_and_a_record_of_another_socket_is_kept() {
    let tmp = TempDir::new();
    let asked = Arc::new(Asked::default());
    let (dir, first) = listening(&tmp, &asked);
    // As a SIGKILLed orchestrator leaves it: the socket file stays.
    std::mem::forget(first);

    let commands: Arc<dyn Commands> = asked.clone();
    let second = Listener::bind(&dir, "20261120-000000-abcd", &key(), commands).expect("bound again");
    std::fs::write(format!("{dir}/{SOCKET_RECORD}"), "/elsewhere.sock\n").unwrap();
    drop(second);

    assert_eq!(std::fs::read_to_string(format!("{dir}/{SOCKET_RECORD}")).unwrap(), "/elsewhere.sock\n");
}

#[test]
fn a_run_dir_too_long_for_a_socket_address_gets_its_socket_in_the_user_s_tmp_directory() {
    let tmp = TempDir::new();
    let dir = tmp.join(&"d".repeat(SUN_PATH_MAX));
    std::fs::create_dir_all(&dir).unwrap();
    let key = format!("test-{}", std::process::id());
    let asked = Arc::new(Asked::default());
    let commands: Arc<dyn Commands> = asked.clone();

    let listener = Listener::bind(&dir, "20261120-000000-abcd", &key, commands).expect("bound");

    let uid = nix::unistd::getuid().as_raw();
    let path = format!("/tmp/octoflow-{uid}/{key}.sock");
    assert_eq!(std::fs::read_to_string(format!("{dir}/{SOCKET_RECORD}")).unwrap(), format!("{path}\n"));
    let meta = std::fs::symlink_metadata(format!("/tmp/octoflow-{uid}")).unwrap();
    assert_eq!((meta.mode() & 0o777, meta.uid()), (0o700, uid));
    assert_eq!(ask(&dir, &json!({"stop": {}})), Ok(json!({})));
    drop(listener);
    assert!(!std::path::Path::new(&path).exists());
}

#[test]
fn a_tmp_directory_that_is_a_link_is_refused() {
    let tmp = TempDir::new();
    let target = tmp.join("target");
    std::fs::create_dir(&target).unwrap();
    let link = tmp.join("link");
    std::os::unix::fs::symlink(&target, &link).unwrap();

    let refused = private_dir(&link, nix::unistd::getuid().as_raw()).expect_err("a link");
    let other = private_dir(&target, nix::unistd::getuid().as_raw() + 1).expect_err("another user's");

    assert!(refused.0.contains("is not a directory of user"), "{}", refused.0);
    assert!(other.0.contains("is not a directory of user"), "{}", other.0);
}

#[test]
fn the_handshake_tells_the_run_and_a_client_of_another_protocol_is_told_so_and_closed() {
    let tmp = TempDir::new();
    let asked = Arc::new(Asked::default());
    let (dir, _listener) = listening(&tmp, &asked);
    let hello = json!({"protocol": PROTOCOL, "engine": env!("CARGO_PKG_VERSION"), "run": "20261120-000000-abcd"});

    let same = talk(&dir, &[r#"{"hello":{"protocol":1}}"#]);
    let other = talk(&dir, &[r#"{"hello":{"protocol":99}}"#, r#"{"stop":{}}"#]);

    assert_eq!(same, [json!({"hello": hello})]);
    assert_eq!(other.len(), 1, "closed after the mismatch: {other:?}");
    assert_eq!(other[0]["hello"], hello);
    assert_eq!(other[0]["error"]["code"], "protocol");
    assert!(asked.0.lock().unwrap().is_empty());
}

#[test]
fn ask_answers_a_permission_and_reports_each_refusal_by_its_code() {
    let tmp = TempDir::new();
    let asked = Arc::new(Asked::default());
    let (dir, _listener) = listening(&tmp, &asked);
    let answer = |role: &str, request: u64, option: &str| {
        ask(&dir, &json!({"answer": {"role": role, "request": request, "option": option}}))
    };
    let code = |r: Result<Value, SocketError>| match r {
        Err(SocketError::Refused { code, .. }) => code,
        other => format!("{other:?}"),
    };

    assert_eq!(answer("builder", 1, "allow"), Ok(json!({})));
    assert_eq!(code(answer("builder", 1, "allow")), "already_answered");
    assert_eq!(code(answer("builder", 1, "deny")), "bad_option");
    assert_eq!(code(answer("builder", 2, "allow")), "no_request");
    assert_eq!(code(answer("reviewer", 1, "allow")), "no_agent");
    assert_eq!(code(ask(&dir, &json!({"answer": {"role": "builder"}}))), "bad_request");
    assert_eq!(code(ask(&dir, &json!({"dance": {}}))), "bad_request");
}

#[test]
fn ask_sends_a_message_and_reports_an_agent_that_does_not_wait_by_its_code() {
    let tmp = TempDir::new();
    let asked = Arc::new(Asked::default());
    let (dir, _listener) = listening(&tmp, &asked);
    let message = |role: &str| ask(&dir, &json!({"message": {"role": role, "text": "use 10"}}));
    let code = |r: Result<Value, SocketError>| match r {
        Err(SocketError::Refused { code, .. }) => code,
        other => format!("{other:?}"),
    };

    assert_eq!(message("builder"), Ok(json!({})));
    assert_eq!(code(message("reviewer")), "not_waiting");
    assert_eq!(code(message("tester")), "no_agent");
    assert_eq!(code(ask(&dir, &json!({"message": {"role": "builder"}}))), "bad_request");
    assert_eq!(asked.0.lock().unwrap()[0], "message builder use 10");
}

#[test]
fn already_answered_names_the_answer_that_won() {
    let tmp = TempDir::new();
    let asked = Arc::new(Asked::default());
    let (dir, _listener) = listening(&tmp, &asked);
    let line = r#"{"answer":{"role":"builder","request":1,"option":"allow"}}"#;

    let answers = talk(&dir, &[r#"{"hello":{"protocol":1}}"#, line, line]);

    assert_eq!(answers[1], json!({"ok": {}}));
    assert_eq!(answers[2]["error"]["code"], "already_answered");
    assert_eq!((&answers[2]["error"]["option"], &answers[2]["error"]["by"]), (&json!("allow"), &json!("human")));
}

#[test]
fn stop_is_answered_before_it_is_done() {
    let tmp = TempDir::new();
    let asked = Arc::new(Asked::default());
    let (dir, _listener) = listening(&tmp, &asked);

    let answers = talk(&dir, &[r#"{"hello":{"protocol":1}}"#, r#"{"stop":{}}"#]);

    assert_eq!(answers[1], json!({"ok": {}}));
    assert!(eventually(|| asked.0.lock().unwrap().as_slice() == ["stop"]));
}

#[test]
fn a_line_too_long_is_refused_and_closes_the_connection() {
    let tmp = TempDir::new();
    let asked = Arc::new(Asked::default());
    let (dir, _listener) = listening(&tmp, &asked);
    let long = format!(r#"{{"hello":{{"protocol":1,"pad":"{}"}}}}"#, "x".repeat(MAX_LINE));

    let answers = talk(&dir, &[&long, r#"{"hello":{"protocol":1}}"#]);

    assert_eq!(answers.len(), 1, "{answers:?}");
    assert_eq!(answers[0]["error"]["code"], "too_long");
}

#[test]
fn a_run_without_a_socket_is_unreachable() {
    let tmp = TempDir::new();

    let unreachable = ask(&tmp.path(), &json!({"stop": {}}));

    assert!(matches!(unreachable, Err(SocketError::Unreachable(_))), "{unreachable:?}");
}
