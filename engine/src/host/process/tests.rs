use super::*;
use crate::test_support::TempDir;

#[test]
fn no_control_directory_where_ssh_could_not_use_one() {
    let d = TempDir::new();
    // Missing, too long for a socket's path, or with what ssh's option parser takes apart.
    let long = format!("/tmp/{}", "x".repeat(80));
    for parent in [d.join("missing"), long, "/tmp/a b".into(), "/tmp/%h".into(), String::new()] {
        assert_eq!(new_ssh_control_dir(&parent), None, "{parent}");
    }
    let made = new_ssh_control_dir("/tmp").expect("a directory in /tmp");
    assert!(made.starts_with("/tmp/octoflow-ssh-") && made.len() == "/tmp/octoflow-ssh-".len() + 8, "{made}");
    assert!(private_dir(&made));
    std::fs::remove_dir(&made).unwrap();
    assert!(!private_dir(&made));
}

#[test]
fn names_differ_from_call_to_call() {
    let names: std::collections::HashSet<String> = (0..50).map(unguessable_name).collect();
    assert_eq!(names.len(), 50);
    let mkdtemp_like = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_';
    assert!(names.iter().all(|n| n.len() == 8 && n.bytes().all(mkdtemp_like)));
}

#[test]
fn without_a_usable_control_path_a_command_runs_on_a_connection_of_its_own() {
    // OpenSSH 10 fails a command whose ControlPath it cannot bind rather than connect without sharing, so then no
    // option is passed.
    assert_eq!(ssh_argv("remote-host", vec![], &["true"]), ["ssh", "-o", "BatchMode=yes", "remote-host", "true"]);
    let options = vec!["-o".to_string(), "ControlMaster=auto".to_string()];
    let argv = ssh_argv("remote-host", options, &["sh", "-c", "a b"]);
    assert_eq!(argv, ["ssh", "-o", "BatchMode=yes", "-o", "ControlMaster=auto", "remote-host", "sh -c 'a b'"]);
}

#[test]
fn a_host_over_ssh_says_where() {
    let host = ProcessHost::local(&Interrupt::new());
    assert_eq!(host.ssh_target(), None);
    assert_eq!(host.over_ssh("me@box").ssh_target(), Some("me@box"));
    // An empty target is this machine.
    assert_eq!(host.over_ssh("").ssh_target(), None);
    let secret =
        ProcessHost::in_environment([("GH_TOKEN".into(), "token-value".into())].into_iter(), &Interrupt::new());
    let shown = format!("{:?}", secret.over_ssh("box"));
    assert!(shown.contains("box") && !shown.contains("token-value"), "{shown}");
    let vars = [("A".into(), "1".into()), ("A".into(), "2".into())];
    assert_eq!(ProcessHost::in_environment(vars.into_iter(), &Interrupt::new()).var("A").as_deref(), Some("2"));
}

#[test]
fn a_detached_process_is_not_a_child_runs_in_a_session_of_its_own_and_writes_to_its_log() {
    let tmp = TempDir::new();
    let (pid_file, log) = (tmp.join("pid"), tmp.join("run.log"));
    let script = "echo $$ > \"$1\"; echo out; echo err >&2; exec sleep 30";
    let argv: Vec<String> = ["sh", "-c", script, "sh", &pid_file].map(String::from).to_vec();

    let detached = spawn_detached(&argv, &tmp.path(), &log).expect("started");

    assert!(crate::test_support::eventually(|| std::fs::read_to_string(&log).is_ok_and(|l| l.contains("err"))));
    assert_eq!(std::fs::read_to_string(&pid_file).unwrap().trim(), detached.pid.to_string());
    assert_eq!(std::fs::read_to_string(&log).unwrap(), "out\nerr\n");
    let session = nix::unistd::getsid(Some(Pid::from_raw(detached.pid))).expect("a session");
    assert_ne!(session, nix::unistd::getsid(None).expect("our session"));
    let waited = nix::sys::wait::waitpid(Pid::from_raw(detached.pid), Some(nix::sys::wait::WaitPidFlag::WNOHANG));
    assert_eq!(waited, Err(nix::errno::Errno::ECHILD), "it is our child");
    assert!(!detached.exited());
    nix::sys::signal::kill(Pid::from_raw(detached.pid), Signal::SIGKILL).expect("killed");
    assert!(crate::test_support::eventually(|| detached.exited()));
}

#[test]
fn a_detached_program_that_cannot_start_is_an_error_naming_it() {
    let tmp = TempDir::new();
    let argv = vec!["/no/such/program".to_string()];

    let refused = spawn_detached(&argv, &tmp.path(), &tmp.join("run.log")).err().expect("refused");

    assert!(refused.0.starts_with("cannot start /no/such/program: "), "{}", refused.0);
}
