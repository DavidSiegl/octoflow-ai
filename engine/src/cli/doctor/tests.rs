//! check_setup and the doctor command, on FakeTools: the commands of a machine as ProcessHost and ProcessHerdr start
//! them, played by the runner each is built with for tests.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::sync::{Arc, Mutex};

use super::*;
use crate::acp_adapter::Installs;
use crate::cli::{ended_status, herdrless_command, parse_args, Local};
use crate::constants::HARNESS_NAMES;
use crate::env::ConfigDir;
use crate::harness::Adapter;
use crate::host::ProcessError;
use crate::interrupt::Interrupt;
use crate::test_support::TempDir;

/// What `herdr integration status` lists with every harness's integration current.
fn integrations_current() -> String {
    let current: String = HARNESSES
        .iter()
        .map(|h| format!("{0}: current (v3) (/home/u/.{0}/herdr-agent-state.sh)\n", h.integration))
        .collect();
    format!("{current}letta (experimental): not installed (/l)\n")
}

/// How a command FakeTools plays ends.
#[derive(Clone, Debug)]
enum Answer {
    /// It exits with the status, having printed stdout and stderr.
    Ran(i32, String, String),
    /// It runs out of time.
    TimesOut,
    /// Its program is not on PATH, so it cannot be started.
    NotFound,
}

fn completed(stdout: &str) -> Answer {
    Answer::Ran(0, stdout.to_string(), String::new())
}

fn failing(stderr: &str, status: i32) -> Answer {
    Answer::Ran(status, String::new(), stderr.to_string())
}

/// A test's own answers, given a command's SSH target (None here) and argv; None leaves the command to the rest.
type AnswerWith = Box<dyn Fn(Option<&str>, &[String]) -> Option<Answer> + Send>;

/// What FakeTools holds and records.
struct Tools {
    /// Each tool on PATH, by name, with its path.
    paths: BTreeMap<String, String>,
    /// What each command answers, by its argv.
    answers: HashMap<Vec<String>, Answer>,
    /// The SSH targets that answer.
    targets: Vec<String>,
    /// The directories Host::resolve_dir finds.
    dirs: Vec<String>,
    /// The text of each file Host::read finds, by its path; it finds no other.
    files: BTreeMap<String, String>,
    /// (SSH target or None, argv) of each command, in order.
    calls: Vec<(Option<String>, Vec<String>)>,
    /// The environment each command was started in, in order.
    envs: Vec<Vec<(OsString, OsString)>>,
    /// Answers in place of answers when it gives one, given the target and argv: a test's own.
    answer_with: Option<AnswerWith>,
    /// What the login shell an SSH command runs in answers for a command, by its name, that is not on PATH.
    missing_over_ssh: fn(&str) -> Answer,
    /// What crate::herdr::pane_shell's script answers: $SHELL, the path of herdr's config.toml and its text.
    pane_shell: Answer,
}

/// A POSIX login shell's answer for a command not on PATH.
fn sh_not_found(command: &str) -> Answer {
    failing(&format!("sh: 1: {command}: not found\n"), 127)
}

/// A csh or tcsh login shell's answer for a command not on PATH.
fn csh_not_found(command: &str) -> Answer {
    failing(&format!("{command}: Command not found.\n"), 1)
}

/// A machine's commands with every tool doctor checks present; a test changes paths or answers to take one away.
/// Over SSH it answers for targets, and records each command's target in calls.
#[derive(Clone)]
struct FakeTools(Arc<Mutex<Tools>>);

impl FakeTools {
    fn new() -> FakeTools {
        FakeTools::with_targets(&["me@box"])
    }

    fn with_targets(targets: &[&str]) -> FakeTools {
        let names = HARNESS_NAMES.iter().copied().chain(["herdr", "git", "gh"]);
        let paths = names.map(|n| (n.to_string(), format!("/usr/bin/{n}"))).collect();
        let argv = |words: &str| words.split(' ').map(String::from).collect::<Vec<_>>();
        let machines = r#"[{"label": "box", "id": "m1", "enabled": true, "target": "me@box"}, {"label": "off", "id": "m2", "enabled": false, "target": "me@off"}]"#;
        let answers = [
            ("herdr --version", completed("herdr 0.9.1\n")),
            ("herdr integration status", completed(&integrations_current())),
            ("git --version", completed("git version 2.47.3\n")),
            ("gh auth status", completed("github.com\n  ✓ Logged in to github.com\n")),
            ("gitleaks git --help", completed("Usage: gitleaks git [flags]\n")),
            ("sh -c", completed("")),
            ("git -C /proj rev-parse --show-toplevel", completed("/proj\n")),
            ("git -C /proj remote get-url origin", completed("git@github.com:me/proj.git\n")),
            ("git -C /home/u/proj rev-parse --show-toplevel", completed("/home/u/proj\n")),
            ("git -C /home/u/proj remote get-url origin", completed("git@github.com:me/p.git\n")),
            ("herdr machine list --json", completed(machines)),
        ]
        .into_iter()
        .map(|(words, answer)| (argv(words), answer))
        .collect();
        FakeTools(Arc::new(Mutex::new(Tools {
            paths,
            answers,
            targets: targets.iter().map(|t| t.to_string()).collect(),
            dirs: vec!["/proj".into(), "/home/u/proj".into()],
            files: BTreeMap::new(),
            calls: vec![],
            envs: vec![],
            answer_with: None,
            missing_over_ssh: sh_not_found,
            pane_shell: completed("/bin/bash\n/home/u/.config/herdr/config.toml\n"),
        })))
    }

    fn tools(&self) -> std::sync::MutexGuard<'_, Tools> {
        self.0.lock().unwrap()
    }

    /// Take a tool off PATH.
    fn remove(&self, name: &str) {
        self.tools().paths.remove(name);
    }

    /// Have the command of words, split at spaces, answer so.
    fn answer(&self, words: &str, answer: Answer) {
        self.tools().answers.insert(words.split(' ').map(String::from).collect(), answer);
    }

    /// Whether the command of words, split at spaces, ran on target.
    fn ran(&self, target: Option<&str>, words: &str) -> bool {
        let argv: Vec<String> = words.split(' ').map(String::from).collect();
        self.tools().calls.iter().any(|(t, a)| t.as_deref() == target && *a == argv)
    }

    /// Play one command, as ProcessHost's or ProcessHerdr's runner, started in env.
    fn call(&self, argv: &[String], env: &[(OsString, OsString)]) -> Result<(i32, Vec<u8>, Vec<u8>), ProcessError> {
        let mut t = self.tools();
        let ran = |status: i32, stdout: &str, stderr: &str| {
            Ok((status, stdout.as_bytes().to_vec(), stderr.as_bytes().to_vec()))
        };
        let (target, argv) = if argv[0] == "ssh" {
            let target = argv[argv.len() - 2].clone();
            if !t.targets.contains(&target) {
                return ran(255, "", &format!("ssh: connect to host {target}: Connection refused"));
            }
            (Some(target), shlex_split(&argv[argv.len() - 1]))
        } else {
            (None, argv.to_vec())
        };
        t.calls.push((target.clone(), argv.clone()));
        t.envs.push(env.to_vec());
        if let Some(answer) = t.answer_with.as_ref().and_then(|with| with(target.as_deref(), &argv)) {
            return played(&answer);
        }
        if argv == ["true"] {
            return ran(0, "", "");
        }
        // local_gate::preflight's script, answered as `sh -c` is, the script left out.
        if is_preflight(&argv) {
            let answer = t.answers[&vec!["sh".to_string(), "-c".to_string()]].clone();
            return played(&answer);
        }
        if argv[0] == "sh" && argv[2] == crate::herdr::PANE_SHELL_SCRIPT {
            return played(&t.pane_shell);
        }
        if argv[0] == "sh" && argv[2] == "command -v -- \"$1\"" {
            return match t.paths.get(argv.last().unwrap()) {
                Some(path) => ran(0, &format!("{path}\n"), ""),
                None => ran(1, "", ""),
            };
        }
        if argv[0] == "sh" && argv[2].starts_with("[ -f \"$1\" ] || exit") {
            // Host::read.
            return match t.files.get(argv.last().unwrap()) {
                Some(text) => ran(0, text, ""),
                None => ran(crate::host::MISSING_FILE_STATUS, "", ""),
            };
        }
        if argv[0] == "sh" && argv[2].ends_with("&& pwd") {
            // Host::resolve_dir, home being /home/u.
            let home = if argv[2].contains("$HOME") { "/home/u" } else { "" };
            let path = format!("{home}{}", argv.last().unwrap());
            return match t.dirs.contains(&path) {
                true => ran(0, &format!("{path}\n"), ""),
                false => ran(2, "", "cd: no such dir"),
            };
        }
        if !t.paths.contains_key(&argv[0]) && argv[0] != "gitleaks" {
            // The remote login shell answers for a command it cannot find; locally it cannot be started at all.
            if target.is_some() {
                return played(&(t.missing_over_ssh)(&argv[0]));
            }
            return Err(ProcessError::Start(std::io::ErrorKind::NotFound.into()));
        }
        let answer = t.answers.get(&argv).cloned().unwrap_or_else(|| panic!("FakeTools got an unscripted {argv:?}"));
        played(&answer)
    }

    /// A Host on this machine whose commands these tools play, in an environment of its own with set added.
    fn host(&self, set: &[(&str, &str)]) -> ProcessHost {
        let tools = self.clone();
        let vars =
            set.iter().map(|(k, v)| (OsString::from(k), OsString::from(v))).chain([("PATH".into(), "/usr/bin".into())]);
        ProcessHost::with_runner(vars.collect::<Vec<_>>().into_iter(), move |argv, _, _, env| tools.call(argv, env))
    }

    /// The herdr here, forwarded to machine, whose commands these tools play.
    fn herdr(&self, machine: Option<&str>) -> ProcessHerdr {
        let tools = self.clone();
        ProcessHerdr::with_runner(machine, move |argv, _| tools.call(argv, &[]))
    }
}

/// How answer plays out for the runner.
fn played(answer: &Answer) -> Result<(i32, Vec<u8>, Vec<u8>), ProcessError> {
    match answer {
        Answer::Ran(status, stdout, stderr) => Ok((*status, stdout.as_bytes().to_vec(), stderr.as_bytes().to_vec())),
        Answer::TimesOut => Err(ProcessError::Timeout),
        Answer::NotFound => Err(ProcessError::Start(std::io::ErrorKind::NotFound.into())),
    }
}

/// The words of a line of sh as shlex_join writes them, for what the fake ssh is given.
fn shlex_split(line: &str) -> Vec<String> {
    let (mut words, mut word, mut in_word) = (vec![], String::new(), false);
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                word.extend(chars.by_ref().take_while(|&c| c != '\''));
            }
            '"' => {
                in_word = true;
                word.extend(chars.by_ref().take_while(|&c| c != '"'));
            }
            ' ' if in_word => {
                words.push(std::mem::take(&mut word));
                in_word = false;
            }
            ' ' => {}
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    words
}

/// The checks of check_setup on tools, the agents on machine, the project at cwd, from a herdr pane, by name.
fn setup(tools: &FakeTools, machine: Option<&str>, cwd: Option<&str>) -> BTreeMap<String, Check> {
    check_in_pane(tools, machine, cwd, true)
}

fn check_in_pane(
    tools: &FakeTools,
    machine: Option<&str>,
    cwd: Option<&str>,
    in_pane: bool,
) -> BTreeMap<String, Check> {
    let checks = check_setup(&tools.host(&[]), &tools.herdr(machine), cwd, in_pane);
    checks.into_iter().map(|c| (c.name.clone(), c)).collect()
}

/// The names of check_setup's checks on tools, in order.
fn names(tools: &FakeTools, cwd: Option<&str>) -> Vec<String> {
    check_setup(&tools.host(&[]), &tools.herdr(None), cwd, true).into_iter().map(|c| c.name).collect()
}

/// What report_checks prints for checks, and its exit status.
fn reported(checks: &BTreeMap<String, Check>, order: &[String]) -> (String, i32) {
    let checks: Vec<Check> = order.iter().filter_map(|n| checks.get(n).cloned()).collect();
    report_list(&checks)
}

fn report_list(checks: &[Check]) -> (String, i32) {
    let out = RefCell::new(String::new());
    let status = report_checks(checks, &|line| out.borrow_mut().push_str(&format!("{line}\n")));
    (out.into_inner(), status)
}

fn failed(checks: &BTreeMap<String, Check>, order: &[String]) -> Vec<String> {
    order.iter().filter(|n| checks.get(*n).is_some_and(|c| !c.ok)).cloned().collect()
}

fn check_of(name: &str, level: Level, ok: bool, detail: &str, fix: &str) -> Check {
    Check { name: name.into(), level, ok, detail: detail.into(), fix: fix.into() }
}

#[test]
fn all_present() {
    let tools = FakeTools::new();
    let order = names(&tools, Some("/proj"));
    let mut want: Vec<String> = vec!["herdr".into(), "reaching herdr".into(), "pane shell".into()];
    for kind in HARNESS_NAMES {
        want.extend([format!("{kind} CLI"), format!("{kind} integration")]);
    }
    want.extend(["git", "gh", "gitleaks", "git checkout", "origin", "local quality gate"].map(String::from));
    assert_eq!(order, want);
    let checks = setup(&tools, None, Some("/proj"));
    assert_eq!(failed(&checks, &order), Vec::<String>::new());
    assert_eq!(checks["herdr"].detail, "herdr 0.9.1");
    assert_eq!(checks["pane shell"].detail, "/bin/bash ($SHELL)");
    assert_eq!(checks["claude integration"].detail, "current (v3)");
    assert_eq!(checks["origin"].detail, "git@github.com:me/proj.git");
    let (out, status) = reported(&checks, &order);
    assert_eq!(status, 0);
    assert!(out.lines().all(|l| l.starts_with("ok      ")), "{out}");
    assert!(out.contains("ok      herdr: herdr 0.9.1\n"), "{out}");
}

#[test]
fn levels() {
    let checks = setup(&FakeTools::new(), None, Some("/proj"));
    let levels: BTreeMap<&str, &str> = checks
        .iter()
        .filter(|(_, c)| c.level != Level::Optional)
        .map(|(n, c)| (n.as_str(), c.level.as_str()))
        .collect();
    let want: BTreeMap<&str, &str> = [
        ("herdr", "required"),
        ("reaching herdr", "required"),
        ("pane shell", "required"),
        ("claude CLI", "required"),
        ("claude integration", "required"),
        ("git", "required"),
        ("gh", "needed-for-pr"),
        ("gitleaks", "needed-for-pr"),
        ("git checkout", "needed-for-pr"),
        ("origin", "needed-for-pr"),
    ]
    .into_iter()
    .collect();
    assert_eq!(levels, want);
}

#[test]
fn pane_shell_that_runs_cannot_type_into() {
    let tools = FakeTools::new();
    tools.tools().pane_shell = completed("/usr/bin/nu\n/home/u/.config/herdr/config.toml\n");
    let checks = setup(&tools, None, None);
    let fix = "set [terminal] default_shell in herdr's config.toml to a POSIX shell or fish, then run `herdr server \
               reload-config`";
    let detail = "herdr's panes start /usr/bin/nu ($SHELL), which cannot run the lines a run types: they need a POSIX \
                  shell (sh, bash, zsh, dash, ksh) or fish";
    assert_eq!(checks["pane shell"], check_of("pane shell", Level::Required, false, detail, fix));
}

#[test]
fn pane_shell_of_herdrs_config_wins_over_shell() {
    let tools = FakeTools::new();
    let config = "[terminal]\ndefault_shell = \"/usr/bin/fish\"\n";
    tools.tools().pane_shell = completed(&format!("/usr/bin/nu\n/home/u/.config/herdr/config.toml\n{config}"));
    let checks = setup(&tools, None, None);
    assert!(checks["pane shell"].ok, "{:?}", checks["pane shell"]);
    assert_eq!(
        checks["pane shell"].detail,
        "/usr/bin/fish ([terminal] default_shell in /home/u/.config/herdr/config.toml)"
    );
}

#[test]
fn pane_shell_on_the_machine() {
    let tools = FakeTools::new();
    tools.tools().answer_with = Some(Box::new(|target, argv| {
        let nu = target == Some("me@box") && argv[0] == "sh" && argv[2] == crate::herdr::PANE_SHELL_SCRIPT;
        nu.then(|| completed("/usr/bin/nu\n/home/me/.config/herdr/config.toml\n"))
    }));
    assert!(setup(&tools, None, None)["pane shell"].ok);
    let checks = setup(&tools, Some("box"), None);
    assert!(
        checks["pane shell"].detail.starts_with("herdr's panes start /usr/bin/nu ($SHELL)"),
        "{:?}",
        checks["pane shell"]
    );
}

#[test]
fn without_herdr() {
    let tools = FakeTools::new();
    tools.remove("herdr");
    let order = names(&tools, None);
    let checks = setup(&tools, None, None);
    let install = "install herdr 0.9 or later, or update it with `herdr update`";
    assert_eq!(checks["herdr"], check_of("herdr", Level::Required, false, "herdr is not on PATH", install));
    assert_eq!(checks["claude integration"].detail, "skipped: needs herdr");
    assert_eq!(checks["codex integration"].detail, "skipped: needs herdr; needed only for a role that runs in codex");
    assert!(checks["git"].ok && checks["gh"].ok && checks["claude CLI"].ok);
    assert!(!tools.ran(None, "herdr integration status"));
    let (out, status) = reported(&checks, &order);
    assert_eq!(status, EXIT_ERROR);
    assert!(out.contains(&format!("missing herdr: herdr is not on PATH\n        fix: {install}\n")), "{out}");
    assert!(
        out.contains(
            "missing claude integration: skipped: needs herdr\n        fix: fix herdr first\nok      codex CLI"
        ),
        "{out}"
    );
}

#[test]
fn old_herdr() {
    let tools = FakeTools::new();
    tools.answer("herdr --version", completed("herdr 0.8.4\n"));
    let herdr = &setup(&tools, None, None)["herdr"];
    assert_eq!((herdr.ok, herdr.detail.as_str()), (false, "herdr 0.8.4 is older than 0.9"));
    for (said, ok) in [("herdr 0.9\n", true), ("herdr 0.10.0\n", true), ("herdr 1.0.0\n", true), ("herdr dev\n", false)]
    {
        tools.answer("herdr --version", completed(said));
        assert_eq!(setup(&tools, None, None)["herdr"].ok, ok, "{said}");
    }
}

#[test]
fn integrations_not_installed() {
    let tools = FakeTools::new();
    let listing = integrations_current()
        .replace("claude: current (v3)", "claude: not installed")
        .replace("codex: current (v3)", "codex: not installed")
        .replace("antigravity-cli: current (v3)", "antigravity-cli: outdated (v1)");
    tools.answer("herdr integration status", completed(&listing));
    let order = names(&tools, Some("/proj"));
    let checks = setup(&tools, None, Some("/proj"));
    let claude =
        check_of("claude integration", Level::Required, false, "not installed", "herdr integration install claude");
    assert_eq!(checks["claude integration"], claude);
    let codex = check_of(
        "codex integration",
        Level::Optional,
        false,
        "not installed; needed only for a role that runs in codex",
        "herdr integration install codex",
    );
    assert_eq!(checks["codex integration"], codex);
    // agy's integration is listed by its own name, not the kind's.
    let agy = check_of(
        "agy integration",
        Level::Optional,
        false,
        "outdated (v1); needed only for a role that runs in agy",
        "herdr integration install antigravity-cli",
    );
    assert_eq!(checks["agy integration"], agy);
    assert_eq!(failed(&checks, &order), ["agy integration", "claude integration", "codex integration"]);
    let asked =
        tools.tools().calls.iter().filter(|(t, a)| t.is_none() && *a == ["herdr", "integration", "status"]).count();
    // Once for each check_setup: names' and check's.
    assert_eq!(asked, 2);
    let (out, status) = reported(&checks, &order);
    assert_eq!(status, EXIT_ERROR);
    assert!(
        out.contains("missing claude integration: not installed\n        fix: herdr integration install claude\n"),
        "{out}"
    );
    assert!(out.contains("warn    codex integration: not installed;"), "{out}");
}

#[test]
fn optional_failures_alone_pass() {
    let tools = FakeTools::new();
    tools.remove("codex");
    tools.answer("herdr integration status", completed(&integrations_current().replace("pi: current", "pi: x")));
    let order = names(&tools, None);
    let checks = setup(&tools, None, None);
    assert_eq!(checks["codex CLI"].fix, "install the codex CLI where the agents run");
    assert_eq!(reported(&checks, &order).1, 0);
}

#[test]
fn integration_unlisted_or_unasked() {
    let tools = FakeTools::new();
    tools.answer("herdr integration status", completed("claude: current (v3) (/c)\n"));
    assert_eq!(
        setup(&tools, None, None)["pi integration"].detail,
        "herdr integration status does not list pi; needed only for a role that runs in pi"
    );
    tools.answer("herdr integration status", failing("unknown command", 2));
    let checks = setup(&tools, None, None);
    assert!(checks["claude integration"].detail.contains("herdr integration status failed: unknown command"));
    for kind in HARNESS_NAMES {
        assert_eq!(
            checks[&format!("{kind} integration")].fix,
            "update herdr with `herdr update`, then check that `herdr integration status` runs"
        );
    }
}

#[test]
fn integration_states_are_read_from_the_listing() {
    let listing = "pi: current (v9) (/home/u/.pi/a.ts)\n\
                   codex: not installed (/home/u/.codex/a.sh)\n\
                   letta (experimental): not installed (/home/u/.letta/a.sh)\n\
                   antigravity-cli: outdated (v2, current is v3) (/g)\n\
                   \n\
                   noise\n";
    let pairs = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Vec<_>>();
    assert_eq!(
        integration_states(listing),
        pairs(&[
            ("pi", "current (v9)"),
            ("codex", "not installed"),
            ("letta", "not installed"),
            ("antigravity-cli", "outdated (v2, current is v3)")
        ])
    );
    // The pattern's corners: a colon in the name, an empty state, nested parentheses.
    let cases = [
        ("a:b: x", Some(("a:b", "x"))),
        ("k:  (p)", Some(("k", ""))),
        ("k: x (y (z))", Some(("k", "x (y (z))"))),
        ("k: x(p)", Some(("k", "x(p)"))),
        ("k:x", None),
        ("k: a (b) (c)", Some(("k", "a (b)"))),
    ];
    for (line, want) in cases {
        assert_eq!(integration_line(line), want.map(|(k, s)| (k.to_string(), s.to_string())), "{line}");
    }
}

#[test]
fn git() {
    let tools = FakeTools::new();
    for (said, ok) in [
        ("git version 2.36.0\n", true),
        ("git version 2.39.3 (Apple Git-145)\n", true),
        ("git version 2.35.9\n", false),
    ] {
        tools.answer("git --version", completed(said));
        assert_eq!(setup(&tools, None, None)["git"].ok, ok, "{said}");
    }
    let git = setup(&tools, None, None)["git"].clone();
    assert_eq!((git.detail.as_str(), git.fix.as_str()), ("git 2.35.9 is older than 2.36", "install git 2.36 or later"));
    tools.remove("git");
    let checks = setup(&tools, None, Some("/proj"));
    assert_eq!(checks["git"].detail, "git is not on PATH");
    assert_eq!(checks["git checkout"].detail, "skipped: needs git");
    assert_eq!(checks["origin"].detail, "skipped: needs git checkout");
}

#[test]
fn gh_logged_out() {
    let tools = FakeTools::new();
    let logged_out = "You are not logged into any GitHub hosts. To log in, run: gh auth login\n";
    tools.answer("gh auth status", failing(logged_out, 1));
    let order = names(&tools, None);
    let checks = setup(&tools, None, None);
    let want = check_of(
        "gh",
        Level::NeededForPr,
        false,
        &format!("gh auth status fails: {}", logged_out.trim()),
        "gh auth login",
    );
    assert_eq!(checks["gh"], want);
    assert!(checks["gitleaks"].ok);
    assert_eq!(reported(&checks, &order).1, EXIT_ERROR);
    let said = "github.com\n  X Failed to log in to github.com using token (GH_TOKEN)\n  - The token in GH_TOKEN is invalid.\n";
    tools.answer("gh auth status", Answer::Ran(1, said.into(), String::new()));
    assert_eq!(
        setup(&tools, None, None)["gh"].detail,
        "gh auth status fails: github.com X Failed to log in to github.com using token (GH_TOKEN) - The token in \
         GH_TOKEN is invalid."
    );
    tools.remove("gh");
    let gh = setup(&tools, None, None)["gh"].clone();
    assert_eq!((gh.detail.as_str(), gh.fix.as_str()), ("gh is not on PATH", "install gh (https://cli.github.com)"));
}

#[test]
fn a_missing_git_or_gh_is_looked_up_through_sh_whatever_the_login_shell_and_not_run() {
    let lookup = |tool: &str| ["sh", "-c", "command -v -- \"$1\"", "_", tool].map(String::from).to_vec();
    for (machine, target) in [(None, None), (Some("box"), Some("me@box"))] {
        for login_shell in [sh_not_found as fn(&str) -> Answer, csh_not_found] {
            for tool in ["git", "gh"] {
                let tools = FakeTools::new();
                tools.tools().missing_over_ssh = login_shell;
                tools.remove(tool);
                assert_eq!(setup(&tools, machine, None)[tool].detail, format!("{tool} is not on PATH"), "{machine:?}");
                let asked: Vec<Vec<String>> = tools
                    .tools()
                    .calls
                    .iter()
                    .filter(|(t, a)| t.as_deref() == target && (a[0] == tool || a.last().unwrap() == tool))
                    .map(|(_, a)| a.clone())
                    .collect();
                assert_eq!(asked, [lookup(tool)], "{machine:?} {tool}");
            }
        }
    }
}

#[test]
fn a_tool_on_path_that_exits_127_itself_fails_rather_than_being_missing() {
    for machine in [None, Some("box")] {
        let tools = FakeTools::new();
        tools.answer("gh auth status", failing("gh: exec: /opt/gh/bin/gh: not found", 127));
        tools.answer("git --version", failing("git: shim target gone", 127));
        let checks = setup(&tools, machine, None);
        assert_eq!(checks["gh"].detail, "gh auth status fails: gh: exec: /opt/gh/bin/gh: not found", "{machine:?}");
        assert!(checks["git"].detail.starts_with("git --version failed"), "{machine:?}: {}", checks["git"].detail);
    }
}

#[test]
fn gh_times_out() {
    let tools = FakeTools::new();
    tools.answer("gh auth status", Answer::TimesOut);
    let checks = setup(&tools, None, None);
    assert!(!checks["gh"].ok);
    assert!(checks["gh"].detail.contains("timed out"), "{}", checks["gh"].detail);
    assert!(checks["gitleaks"].ok);
}

#[test]
fn gitleaks() {
    let tools = FakeTools::new();
    tools.answer("gitleaks git --help", failing("unknown command \"git\"", 1));
    let order = names(&tools, None);
    let checks = setup(&tools, None, None);
    assert_eq!(
        checks["gitleaks"],
        check_of("gitleaks", Level::NeededForPr, false, "`gitleaks git` does not run", GITLEAKS_INSTALL)
    );
    assert!(checks["gitleaks"].fix.contains("https://github.com/gitleaks/gitleaks#installing"));
    assert_eq!(reported(&checks, &order).1, EXIT_ERROR);
    tools.answer("gitleaks git --help", Answer::NotFound);
    let detail = setup(&tools, None, None)["gitleaks"].detail.clone();
    assert!(detail.starts_with("gitleaks cannot be started ("), "{detail}");
}

#[test]
fn the_local_gate_is_checked_by_the_runs_preflight_and_is_optional() {
    let tools = FakeTools::new();
    let gate = setup(&tools, None, None)["local quality gate"].clone();
    assert_eq!((gate.level, gate.ok), (Level::Optional, true));
    assert!(ran_preflight(&tools, None), "the run's preflight ran");
    tools.answer("sh -c", failing("curl is not on PATH", 1));
    let order = names(&tools, None);
    let checks = setup(&tools, None, None);
    let gate = &checks["local quality gate"];
    let detail = format!(
        "the local quality gate fetches octoflow-gate and runs it on the project's machine: curl is not on PATH; {}; \
         needed only for --quality-gate local",
        built_in_judges()
    );
    assert_eq!((gate.ok, gate.detail.as_str()), (false, detail.as_str()));
    assert!(gate.fix.contains("The local gate"), "{}", gate.fix);
    // Optional: a machine that cannot run the local gate still runs everything else.
    assert_eq!(reported(&checks, &order).1, 0);
}

#[test]
fn the_local_gate_check_needs_gitleaks_as_a_gated_run_does() {
    let tools = FakeTools::new();
    tools.answer("gitleaks git --help", failing("unknown command \"git\"", 1));
    let gate = setup(&tools, None, None)["local quality gate"].clone();
    // In doctor's words, not a run's, with the install text once, as the fix.
    let detail = format!(
        "`gitleaks git` does not run, and the gate scans each snapshot for secrets with it; {}; needed only for \
         --quality-gate local",
        built_in_judges()
    );
    assert_eq!(gate, check_of("local quality gate", Level::Optional, false, &detail, GITLEAKS_INSTALL));
}

/// What the local quality gate check says of a project not given: the built-in release judges it.
fn built_in_judges() -> String {
    let (built_in, _) = local_gate::parse_pin(local_gate::BUILT_IN_PIN).unwrap();
    format!("a project is judged by octoflow-gate {built_in}, built in, unless its ci/gate.pin pins another")
}

#[test]
fn the_local_gate_check_names_the_release_even_when_the_machine_cannot_run_it() {
    let tools = FakeTools::new();
    tools.tools().files.insert("/proj/ci/gate.pin".into(), format!("0.7.0 {}\n", "0".repeat(64)));
    tools.answer("gitleaks git --help", failing("unknown command \"git\"", 1));
    let gate = setup(&tools, None, Some("/proj"))["local quality gate"].clone();
    assert!(!gate.ok && gate.detail.contains("ci/gate.pin holds its runs at octoflow-gate 0.7.0"), "{gate:?}");
}

#[test]
fn the_local_gate_check_names_the_release_that_judges_the_project() {
    let (built_in, _) = local_gate::parse_pin(local_gate::BUILT_IN_PIN).unwrap();
    let ready = "gitleaks, and all the gate's preflight asks for, are ready; ";
    let tools = FakeTools::new();
    let gate = |cwd| setup(&tools, None, cwd)["local quality gate"].clone();
    assert_eq!((gate(None).ok, gate(None).detail), (true, format!("{ready}{}", built_in_judges())));
    let detail =
        format!("{ready}the checkout has no ci/gate.pin, so octoflow-gate {built_in}, built in, judges its runs");
    assert_eq!(gate(Some("/proj")).detail, detail);

    let pin = |version: &str| {
        let line = format!("# held\n{version} {}\n", "0".repeat(64));
        tools.tools().files.insert("/proj/ci/gate.pin".into(), line);
    };
    // A project set up before setup-gate pinned only when asked learns how to follow again.
    pin("0.7.0");
    let older = gate(Some("/proj"));
    assert!(older.ok, "{older:?}");
    let detail = format!(
        "{ready}the checkout's ci/gate.pin holds its runs at octoflow-gate 0.7.0, older than the built-in {built_in}: \
         delete ci/gate.pin, and commit that, to follow it"
    );
    assert_eq!(older.detail, detail);
    pin(&built_in);
    let same = gate(Some("/proj"));
    let held = "the checkout's ci/gate.pin holds its runs at octoflow-gate";
    let detail = format!("{ready}{held} {built_in} (built in: {built_in})");
    assert_eq!((same.ok, same.detail), (true, detail));

    pin("99.0.0");
    let newer = gate(Some("/proj"));
    let detail = format!(
        "the checkout's ci/gate.pin holds its runs at octoflow-gate 99.0.0, newer than the built-in {built_in}, whose \
         report this octoflow may not read; needed only for --quality-gate local"
    );
    let fix = "update octoflow, or delete ci/gate.pin, and commit that, to follow it";
    assert_eq!(newer, check_of("local quality gate", Level::Optional, false, &detail, fix));

    // A pin every gated run refuses, by its line or by its release, is not ok, with how to mend it.
    let mend = "write a `<version> <sha256>` line of 0.7.0 or later, or delete ci/gate.pin to follow the built-in \
                release, and commit that";
    tools.tools().files.insert("/proj/ci/gate.pin".into(), "0.9.0\n".into());
    let malformed = gate(Some("/proj"));
    assert!(!malformed.ok && malformed.detail.starts_with("ci/gate.pin: its first line '0.9.0'"), "{malformed:?}");
    assert_eq!(malformed.fix, mend);
    pin("0.6.3");
    let before_gate = gate(Some("/proj"));
    assert!(!before_gate.ok && before_gate.detail.contains("0.6.3 ships no gate"), "{before_gate:?}");
    assert_eq!(before_gate.fix, mend);

    // A pin that cannot be read may be right: the fix is the check's own, not the pin's.
    tools.tools().answer_with = Some(Box::new(|_, argv| {
        let reading = argv[0] == "sh" && argv[2].starts_with("[ -f \"$1\" ]");
        reading.then(|| failing("cat: ci/gate.pin: Permission denied", 1))
    }));
    let unread = gate(Some("/proj"));
    assert!(!unread.ok && unread.detail.contains("Permission denied"), "{unread:?}");
    assert!(unread.fix.contains("The local gate"), "{unread:?}");
}

#[test]
fn the_local_gate_check_does_not_need_the_checkout() {
    let tools = FakeTools::new();
    tools.answer("git -C /proj rev-parse --show-toplevel", failing("fatal: not a git repository", 128));
    let gate = setup(&tools, None, Some("/proj"))["local quality gate"].clone();
    assert!(gate.ok && gate.detail.ends_with("unless its ci/gate.pin pins another"), "{gate:?}");
}

#[test]
fn the_local_gate_is_checked_where_the_agents_run() {
    let tools = FakeTools::new();
    assert!(setup(&tools, Some("box"), Some("/proj"))["local quality gate"].ok);
    assert!(ran_preflight(&tools, Some("me@box")));
    assert!(!ran_preflight(&tools, None));
}

/// Whether argv is local_gate::preflight's command: sh running the script that makes the gate's cache.
fn is_preflight(argv: &[String]) -> bool {
    argv.len() > 2 && argv[0] == "sh" && argv[1] == "-c" && argv[2].contains("octoflow/gate")
}

/// Whether local_gate::preflight ran on target.
fn ran_preflight(tools: &FakeTools, target: Option<&str>) -> bool {
    tools.tools().calls.iter().any(|(t, a)| t.as_deref() == target && is_preflight(a))
}

/// What doctor_command prints for argv, a doctor command line, on tools from a herdr pane, and its exit status or
/// error.
fn doctor_printed(tools: &FakeTools, argv: &[&str]) -> (Result<i32, OrchestratorError>, String) {
    let config = ConfigDir::at("/nowhere");
    let parsed = parse_args(argv.iter().map(OsString::from), &config);
    let Ok(crate::cli::Command::Doctor { target, setup_gate, pin_gate }) = parsed else {
        panic!("{argv:?} is no doctor command")
    };
    let out = RefCell::new(String::new());
    let host = tools.host(&[]);
    let herdr = |machine: Option<&str>| tools.herdr(machine);
    let (interrupt, print) = (Interrupt::new(), |t: &str| out.borrow_mut().push_str(t));
    let cwd = || -> Result<String, OrchestratorError> { panic!("doctor read the current directory") };
    let local = Local {
        host: &host,
        herdr: &herdr,
        in_herdr_pane: true,
        cwd: &cwd,
        config: &config,
        stdout: &print,
        stderr: &print,
        interrupt: &interrupt,
    };
    let ended = doctor_command(&local, &target, setup_gate, pin_gate.as_deref());
    (ended, out.into_inner())
}

/// Have tools play local_gate::set_up's look and write on target (None: here), home being /home/u: the look finds the
/// directory it is given to be a checkout's top level, holding no file it looks for; the write keeps each path from
/// the top level that there names and writes the others.
fn play_files(tools: &FakeTools, target: Option<&'static str>, there: &'static [&'static str]) {
    tools.tools().answer_with = Some(Box::new(move |t, argv| {
        if t != target || argv[0] != "sh" {
            return None;
        }
        if argv[2].ends_with(crate::local_gate::SET_UP_LOOK) {
            let home = if argv[2].contains("$HOME") { "/home/u" } else { "" };
            return Some(completed(&format!("{home}{}\0\0", argv[4])));
        }
        (argv[2] == crate::local_gate::SET_UP_WRITE).then(|| {
            let top = &argv[4];
            let said: String = argv[5..]
                .iter()
                .step_by(2)
                .map(|path| if there.contains(&format!("{top}/{path}").as_str()) { "kept\n" } else { "written\n" })
                .collect();
            completed(&said)
        })
    }));
}

/// The top level of each checkout local_gate::set_up wrote files into on target, in order.
fn set_up_writes(tools: &FakeTools, target: Option<&str>) -> Vec<String> {
    let calls = tools.tools().calls.clone();
    calls
        .into_iter()
        .filter(|(t, a)| t.as_deref() == target && a[0] == "sh" && a[2] == crate::local_gate::SET_UP_WRITE)
        .map(|(_, a)| a[4].clone())
        .collect()
}

#[test]
fn doctor_writes_nothing_unless_asked_to_set_up_the_gate() {
    let tools = FakeTools::new();
    play_files(&tools, None, &[]);
    let (status, out) = doctor_printed(&tools, &["doctor", "--cwd", "/proj"]);
    assert_eq!(status.unwrap(), 0);
    assert!(out.contains("ok      local quality gate: "), "{out}");
    assert_eq!(set_up_writes(&tools, None), Vec::<String>::new());
}

#[test]
fn setup_gate_writes_the_missing_files_where_the_agents_run_and_keeps_the_rest() {
    let tools = FakeTools::new();
    play_files(&tools, Some("me@box"), &["/home/u/proj/gate.toml"]);
    let (status, out) = doctor_printed(&tools, &["doctor", "--machine", "box", "--cwd", "~/proj", "--setup-gate"]);
    assert_eq!(status.unwrap(), 0);
    assert_eq!(set_up_writes(&tools, Some("me@box")), ["/home/u/proj"]);
    assert_eq!(set_up_writes(&tools, None), Vec::<String>::new());
    let tail = "kept    gate.toml: already there\n\
                --quality-gate local reads these from a run's base commit: commit them for a run to be judged by them.\n";
    assert!(out.ends_with(tail), "{out}");
}

#[test]
fn setup_gate_pins_the_release_pin_gate_names() {
    let tools = FakeTools::new();
    play_files(&tools, None, &[]);
    let (version, _) = local_gate::parse_pin(local_gate::BUILT_IN_PIN).unwrap();
    let (status, out) = doctor_printed(&tools, &["doctor", "--cwd", "/proj", "--setup-gate", "--pin-gate", &version]);
    assert_eq!(status.unwrap(), 0);
    assert!(out.contains("wrote   gate.toml\nwrote   ci/gate.pin\n"), "{out}");
    let calls = tools.tools().calls.clone();
    let pinned = calls.iter().any(|(_, a)| a[0] == "sh" && a[2] == local_gate::SET_UP_WRITE && a[7] == "ci/gate.pin");
    assert!(pinned, "the write was given the pin");
}

#[test]
fn pin_gate_needs_setup_gate() {
    let config = ConfigDir::at("/nowhere");
    let argv = ["doctor", "--pin-gate", "0.15.2"];
    let e = parse_args(argv.iter().map(OsString::from), &config).unwrap_err();
    assert_eq!(e.exit_code(), 2);
    assert!(e.to_string().contains("--setup-gate"), "{e}");
}

#[test]
fn setup_gate_outside_a_checkout_is_an_error_after_the_checks() {
    let tools = FakeTools::new();
    tools.answer("git -C /proj rev-parse --show-toplevel", failing("fatal: not a git repository", 128));
    tools.tools().answer_with = Some(Box::new(|_, argv| {
        let look = argv[0] == "sh" && argv[2].ends_with(crate::local_gate::SET_UP_LOOK);
        look.then(|| failing("fatal: not a git repository", 128))
    }));
    let (status, out) = doctor_printed(&tools, &["doctor", "--cwd", "/proj", "--setup-gate"]);
    let e = status.unwrap_err().to_string();
    assert_eq!(e, "setting up the local quality gate: looking at /proj: fatal: not a git repository");
    assert!(out.contains("missing git checkout: /proj is not in a git checkout"), "{out}");
    assert_eq!(set_up_writes(&tools, None), Vec::<String>::new());
}

#[test]
fn not_a_checkout() {
    let tools = FakeTools::new();
    let stderr = "fatal: not a git repository (or any of the parent directories): .git\nmore\n";
    tools.answer("git -C /proj rev-parse --show-toplevel", failing(stderr, 128));
    let checks = setup(&tools, None, Some("/proj"));
    assert_eq!(
        checks["git checkout"].detail,
        "/proj is not in a git checkout: fatal: not a git repository (or any of the parent directories): .git"
    );
    // Worded for the app's setup dialog too, where the project is chosen in the project bar.
    let fix = "choose a git checkout as the project (--cwd), or make one with git clone or git init";
    assert_eq!(checks["git checkout"].fix, fix);
    let origin = check_of("origin", Level::NeededForPr, false, "skipped: needs git checkout", "fix git checkout first");
    assert_eq!(checks["origin"], origin);
    assert!(!tools.ran(None, "git -C /proj remote get-url origin"));
}

#[test]
fn no_origin() {
    let tools = FakeTools::new();
    tools.answer("git -C /proj remote get-url origin", failing("error: No such remote 'origin'", 2));
    let origin = setup(&tools, None, Some("/proj"))["origin"].clone();
    assert!(!origin.ok);
    assert_eq!(origin.fix, "git remote add origin <url>");
}

#[test]
fn cwd_resolved_as_a_run_resolves_it() {
    let tools = FakeTools::new();
    let checks = setup(&tools, Some("box"), Some("~/proj"));
    assert_eq!(
        (checks["git checkout"].detail.as_str(), checks["origin"].detail.as_str()),
        ("/home/u/proj", "git@github.com:me/p.git")
    );
    let resolve = ["sh", "-c", "cd -- \"$HOME$1\" && pwd", "_", "/proj"].map(String::from).to_vec();
    assert!(tools.tools().calls.contains(&(Some("me@box".into()), resolve)));
    let checkout = setup(&tools, None, Some("/gone"))["git checkout"].clone();
    assert_eq!((checkout.ok, checkout.detail.split(':').next()), (false, Some("/gone is not a directory")));
}

#[test]
fn without_cwd_the_project_is_not_checked() {
    assert!(!setup(&FakeTools::new(), None, None).contains_key("git checkout"));
}

#[test]
fn outside_a_herdr_pane() {
    let tools = FakeTools::new();
    let order = names(&tools, None);
    let checks = check_in_pane(&tools, None, None, false);
    let reach = check_of(
        "reaching herdr",
        Level::Required,
        false,
        "not inside a herdr pane",
        "run from a herdr pane, or choose a herdr machine (--machine)",
    );
    assert_eq!(checks["reaching herdr"], reach);
    assert_eq!(failed(&checks, &order), ["reaching herdr"]);
    let (out, status) = reported(&checks, &order);
    assert_eq!(status, EXIT_ERROR);
    assert!(out.contains("missing reaching herdr: not inside a herdr pane\n        fix: run from a herdr pane, or choose a herdr machine (--machine)\n"), "{out}");
}

#[test]
fn on_a_machine() {
    let tools = FakeTools::new();
    let checks = check_in_pane(&tools, Some("box"), Some("/proj"), false);
    assert!(checks.values().all(|c| c.ok), "{checks:?}");
    assert_eq!(checks["reaching herdr"].detail, "herdr machine box, over SSH to me@box");
    assert_eq!(
        (checks["herdr"].detail.as_str(), checks["herdr on box"].detail.as_str()),
        ("herdr 0.9.1", "herdr 0.9.1")
    );
    // Only the herdr a run drives is checked here; everything else goes over SSH to the machine.
    let calls = tools.tools().calls.clone();
    let local: Vec<Vec<String>> = calls.iter().filter(|(t, _)| t.is_none()).map(|(_, a)| a.clone()).collect();
    let words = |w: &str| w.split(' ').map(String::from).collect::<Vec<_>>();
    let lookup = |tool: &str| ["sh", "-c", "command -v -- \"$1\"", "_", tool].map(String::from).to_vec();
    assert_eq!(local, [lookup("herdr"), words("herdr --version"), words("herdr machine list --json")]);
    let remote: Vec<Vec<String>> =
        calls.iter().filter(|(t, _)| t.as_deref() == Some("me@box")).map(|(_, a)| a.clone()).collect();
    // Every tool run for its version or login is looked up first, as the agent CLIs are.
    let mut looked_up: Vec<String> = remote
        .iter()
        .filter(|a| a.len() > 3 && a[..3] == ["sh", "-c", "command -v -- \"$1\""])
        .map(|a| a.last().unwrap().clone())
        .collect();
    looked_up.sort();
    let mut tools_run: Vec<String> =
        HARNESS_NAMES.iter().copied().chain(["gh", "git", "herdr"]).map(String::from).collect();
    tools_run.sort();
    assert_eq!(looked_up, tools_run);
    for argv in [
        "herdr --version",
        "herdr integration status",
        "git --version",
        "gh auth status",
        "gitleaks git --help",
        "git -C /proj rev-parse --show-toplevel",
    ] {
        assert!(remote.contains(&words(argv)), "{argv}");
    }
    assert!(remote.contains(&lookup("codex")));
}

#[test]
fn machine_not_saved_disabled_or_unreachable() {
    let tools = FakeTools::with_targets(&[]);
    for (machine, detail) in [
        ("nowhere", "no saved herdr machine named nowhere"),
        ("off", "herdr machine off is disabled"),
        ("box", "true failed on me@box: ssh: connect to host me@box: Connection refused"),
    ] {
        let checks = setup(&tools, Some(machine), Some("/proj"));
        let reach = &checks["reaching herdr"];
        assert_eq!((reach.ok, reach.detail.as_str()), (false, detail));
        assert!(reach.fix.contains(machine));
        assert!(checks["herdr"].ok);
        assert_eq!(checks[&format!("herdr on {machine}")].detail, "skipped: needs reaching herdr");
        assert_eq!(checks["gh"].detail, "skipped: needs reaching herdr");
        assert_eq!(checks["origin"].detail, "skipped: needs git checkout");
        let order: Vec<String> = check_setup(&tools.host(&[]), &tools.herdr(Some(machine)), Some("/proj"), true)
            .into_iter()
            .map(|c| c.name)
            .collect();
        let (out, status) = reported(&checks, &order);
        assert_eq!(status, EXIT_ERROR);
        assert!(out.contains(&format!("missing reaching herdr: {detail}\n")), "{out}");
    }
}

#[test]
fn machine_without_local_herdr() {
    let tools = FakeTools::new();
    tools.remove("herdr");
    let checks = setup(&tools, Some("box"), None);
    let herdr = &checks["herdr"];
    let install = "install herdr 0.9 or later, or update it with `herdr update`";
    assert_eq!((herdr.detail.as_str(), herdr.fix.as_str()), ("herdr is not on PATH", install));
    let reach = &checks["reaching herdr"];
    assert_eq!((reach.detail.as_str(), reach.fix.as_str()), ("skipped: needs herdr", "fix herdr first"));
    assert!(!tools.ran(None, "herdr machine list --json"));
}

#[test]
fn machine_herdr_versions_checked_apart() {
    // The herdr here drives the run and the one on the machine hosts its agents; each must be recent enough.
    for (local, remote, old) in [("0.8.4", "0.9.1", "herdr"), ("0.9.1", "0.8.4", "herdr on box")] {
        let tools = FakeTools::new();
        let (local, remote) = (format!("herdr {local}\n"), format!("herdr {remote}\n"));
        tools.tools().answer_with = Some(Box::new(move |target, argv| {
            (argv == ["herdr", "--version"]).then(|| completed(if target.is_none() { &local } else { &remote }))
        }));
        let checks = setup(&tools, Some("box"), None);
        assert_eq!(checks[old].detail, "herdr 0.8.4 is older than 0.9");
        let oks: Vec<bool> = ["herdr", "reaching herdr", "herdr on box"].iter().map(|n| checks[*n].ok).collect();
        assert_eq!(oks, if old == "herdr" { [false, false, false] } else { [true, true, false] });
    }
}

#[test]
fn commands_get_the_child_env() {
    let tools = FakeTools::new();
    let host = tools.host(&[("GIT_LITERAL_PATHSPECS", "1")]);
    check_setup(&host, &tools.herdr(Some("box")), Some("/proj"), true);
    let envs = tools.tools().envs.clone();
    // The herdr runner's commands are not given theirs here; ProcessHerdr's own tests hold it to child_env.
    assert!(envs.iter().any(|env| !env.is_empty()));
    for env in envs {
        assert!(!env.iter().any(|(k, _)| k == "GIT_LITERAL_PATHSPECS"), "{env:?}");
    }
}

// -- The doctor command ----------------------------------------------------------------------------------------------

#[test]
fn needs_no_herdr_pane() {
    let tools = FakeTools::new();
    let argv = ["doctor", "--machine", "box", "--cwd", "~/proj"];
    let config = ConfigDir::at("/nowhere");
    let command = parse_args(argv.iter().map(OsString::from), &config).unwrap();
    let out = RefCell::new(String::new());
    let host = tools.host(&[]);
    let machines = RefCell::new(vec![]);
    let herdr = |machine: Option<&str>| {
        machines.borrow_mut().push(machine.map(String::from));
        tools.herdr(machine)
    };
    let (interrupt, print) = (Interrupt::new(), |t: &str| out.borrow_mut().push_str(t));
    let cwd = || -> Result<String, OrchestratorError> { panic!("doctor read the current directory") };
    let local = Local {
        host: &host,
        herdr: &herdr,
        in_herdr_pane: false,
        cwd: &cwd,
        config: &config,
        stdout: &print,
        stderr: &print,
        interrupt: &interrupt,
    };
    let ended = herdrless_command(&command, &local).expect("doctor needs no herdr pane, and is not connected");
    assert_eq!(ended_status(ended, &command, &print), 0);
    assert_eq!(*machines.borrow(), [Some("box".to_string())]);
    assert!(out.borrow().contains("ok      git checkout: /home/u/proj\n"), "{}", out.borrow());
    assert!(out.borrow().lines().all(|l| l.starts_with("ok      ")), "{}", out.borrow());
    // An interrupted doctor prints nothing and ends interrupted.
    out.borrow_mut().clear();
    interrupt.raise();
    assert!(matches!(herdrless_command(&command, &local), Some(Err(crate::cli::Stop::Interrupted(None)))));
    assert_eq!(*out.borrow(), "");
}

#[test]
fn machine_needs_cwd() {
    let config = ConfigDir::at("/nowhere");
    let parse = |argv: &[&str]| parse_args(argv.iter().map(OsString::from), &config);
    assert_eq!(parse(&["doctor", "--machine", "box"]).unwrap_err().exit_code(), 2);
    assert_eq!(
        parse(&["doctor"]).unwrap(),
        crate::cli::Command::Doctor { target: crate::cli::Target::default(), setup_gate: false, pin_gate: None }
    );
}

#[test]
fn the_report() {
    let checks = [
        check_of("a", Level::Required, true, "fine", ""),
        check_of("b", Level::Optional, false, "gone", "get b"),
        check_of("c", Level::NeededForPr, false, "skipped: needs a", "fix a first"),
    ];
    let (out, status) = report_list(&checks);
    assert_eq!(status, EXIT_ERROR);
    assert_eq!(
        out,
        "ok      a: fine\nwarn    b: gone\n        fix: get b\nmissing c: skipped: needs a\n        fix: fix a first\n"
    );
    assert_eq!(report_list(&checks[..2]).1, 0);
}

#[test]
fn a_version_number_is_the_first_run_of_dotted_digits() {
    let cases = [
        ("herdr 0.9.1", Some("0.9.1")),
        ("git version 2.39.3 (Apple Git-145)", Some("2.39.3")),
        ("v12 then 1.2.", Some("1.2")),
        ("herdr dev", None),
        ("7", None),
    ];
    for (text, want) in cases {
        assert_eq!(version_number(text), want, "{text}");
    }
}

/// check_acp_setup's checks on tools and the adapters installed in env, by name, with their order, each harness
/// answered as answers says; the harnesses answers was asked about, in order.
fn acp_setup(
    tools: &FakeTools,
    env: &[(OsString, OsString)],
    answers: impl Fn(&str) -> Result<SelfReport, OrchestratorError>,
) -> (BTreeMap<String, Check>, Vec<String>, Vec<String>) {
    let asked = RefCell::new(vec![]);
    let answer = |h: &'static Harness| {
        asked.borrow_mut().push(h.kind.to_string());
        answers(h.kind)
    };
    let checks = check_acp_setup(&tools.host(&[]), env, Some("/proj"), &answer);
    let order = checks.iter().map(|c| c.name.clone()).collect();
    (checks.into_iter().map(|c| (c.name.clone(), c)).collect(), order, asked.into_inner())
}

/// An environment whose data directory is data_home, its HOME a directory with no codex login, and extra.
fn acp_env(data_home: &TempDir, extra: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
    let base = [("XDG_DATA_HOME", data_home.path()), ("HOME", data_home.join("home"))];
    let extra = extra.iter().map(|(k, v)| (*k, v.to_string()));
    base.into_iter().chain(extra).map(|(k, v)| (OsString::from(k), OsString::from(v))).collect()
}

/// adapter's installs in env, as check_acp_setup finds them.
fn installs_in(adapter: Adapter, env: &[(OsString, OsString)]) -> Installs {
    Installs::in_environment(adapter, env.to_vec())
}

#[test]
fn claude_s_adapter_installed_but_not_answering_is_told_how_it_starts() {
    let tools = FakeTools::new();
    let data_home = TempDir::new();
    let env = acp_env(&data_home, &[]);
    let installed = installs_in(Adapter::Claude, &env).install_fake("");
    let silent = |kind: &str| match kind {
        "claude" => Err(OrchestratorError::new("claude initialize: no answer within 30s")),
        kind => acp_answers(kind),
    };
    let (checks, _, _) = acp_setup(&tools, &env, silent);

    let fix = format!("check that `{} {}` starts", installed.node, installed.script);
    let silent = check_of("claude over ACP", Level::Required, false, "claude initialize: no answer within 30s", &fix);
    assert_eq!(checks["claude over ACP"], silent);
}

/// What the ACP harnesses answer `initialize` with, and why the others cannot answer, as crate::acp::probe says:
/// codex's agent reports a login, as codex-acp does whatever store holds it, the others none.
fn acp_answers(kind: &str) -> Result<SelfReport, OrchestratorError> {
    let identity = match kind {
        "agy" | "opencode" => format!("{kind} 1.0"),
        "claude" => "@agentclientprotocol/claude-agent-acp 0.101.0".to_string(),
        "codex" => "@agentclientprotocol/codex-acp 2.5.0".to_string(),
        _ => return Err(OrchestratorError(format!("{kind} has no ACP launch"))),
    };
    let login = (kind == "codex").then(|| "account".to_string());
    Ok(SelfReport { identity, login })
}

#[test]
fn the_app_s_acp_setup_checks_each_harness_over_acp_and_nothing_of_herdr() {
    let tools = FakeTools::new();
    let data_home = TempDir::new();
    // codex's login is in the OS keyring, so its home has no auth.json: its agent reports it (review 264).
    let env = acp_env(&data_home, &[]);
    let installed = installs_in(Adapter::Claude, &env).install_fake("");
    let codex_installed = installs_in(Adapter::Codex, &env).install_fake("");
    let (checks, order, asked) = acp_setup(&tools, &env, acp_answers);

    let mut want: Vec<String> = HARNESS_NAMES.iter().map(|kind| format!("{kind} over ACP")).collect();
    want.extend(["git", "gh", "gitleaks", "git checkout", "origin", "local quality gate"].map(String::from));
    assert_eq!(order, want);
    assert_eq!(asked, HARNESS_NAMES);
    assert_eq!(checks["agy over ACP"], check_of("agy over ACP", Level::Optional, true, "/usr/bin/agy: agy 1.0", ""));
    assert!(checks["opencode over ACP"].ok);
    let adapter = format!("{}: @agentclientprotocol/claude-agent-acp 0.101.0", installed.script);
    assert_eq!(checks["claude over ACP"], check_of("claude over ACP", Level::Required, true, &adapter, ""));
    let codex = format!("{}: @agentclientprotocol/codex-acp 2.5.0", codex_installed.script);
    assert_eq!(checks["codex over ACP"], check_of("codex over ACP", Level::Optional, true, &codex, ""));
    let pi = check_of(
        "pi over ACP",
        Level::Optional,
        false,
        "pi has no ACP launch; needed only for a role that runs in pi",
        "run its roles in another harness: the app does not run pi yet",
    );
    assert_eq!(checks["pi over ACP"], pi);
    assert!(checks["origin"].ok && checks["git"].ok, "{checks:?}");
    let herdr_calls = tools.tools().calls.iter().filter(|(_, argv)| argv.iter().any(|a| a == "herdr")).count();
    assert_eq!(herdr_calls, 0, "{:?}", tools.tools().calls);
}

#[test]
fn a_harness_off_path_is_missing_without_being_started_and_one_that_does_not_answer_is_told_how_to_check_it() {
    let tools = FakeTools::new();
    tools.remove("agy");
    let data_home = TempDir::new();
    let silent = |kind: &str| match kind {
        "opencode" => Err(OrchestratorError::new("opencode initialize: no answer within 30s")),
        kind => acp_answers(kind),
    };
    let (checks, _, asked) = acp_setup(&tools, &acp_env(&data_home, &[]), silent);

    let off_path = check_of(
        "agy over ACP",
        Level::Optional,
        false,
        "agy is not on PATH; needed only for a role that runs in agy",
        "install the agy CLI",
    );
    assert_eq!(checks["agy over ACP"], off_path);
    let unasked = ["agy", "claude", "codex"].map(String::from);
    assert!(unasked.iter().all(|kind| !asked.contains(kind)), "{asked:?}");
    // Whether or not the claude CLI is on PATH: the adapter brings its own.
    let claude = check_of(
        "claude over ACP",
        Level::Required,
        false,
        "claude's ACP adapter is not installed",
        "install claude's ACP adapter with this dialog's button",
    );
    assert_eq!(checks["claude over ACP"], claude);
    let silent = check_of(
        "opencode over ACP",
        Level::Optional,
        false,
        "opencode initialize: no answer within 30s; needed only for a role that runs in opencode",
        "update the opencode CLI, or check that `opencode acp` starts",
    );
    assert_eq!(checks["opencode over ACP"], silent);
}

#[test]
fn codex_installed_and_answering_but_not_logged_in_is_told_how_to_log_in() {
    let tools = FakeTools::new();
    let data_home = TempDir::new();
    let env = acp_env(&data_home, &[]);
    let installs = installs_in(Adapter::Codex, &env);
    installs.install_fake("");
    let logged_out = |kind: &str| match kind {
        "codex" => acp_answers(kind).map(|p| SelfReport { login: Some("none".to_string()), ..p }),
        kind => acp_answers(kind),
    };
    let (checks, _, asked) = acp_setup(&tools, &env, logged_out);
    let keyed = acp_env(&data_home, &[("OPENAI_API_KEY", "sk-test")]);
    let (with_key, _, _) = acp_setup(&tools, &keyed, logged_out);

    assert!(asked.contains(&"codex".to_string()), "{asked:?}");
    let not_logged_in = installs.logged_in(Some("none")).unwrap_err();
    let detail = format!("{}; needed only for a role that runs in codex", not_logged_in.detail);
    let codex = check_of("codex over ACP", Level::Optional, false, &detail, &not_logged_in.fix);
    assert_eq!(checks["codex over ACP"], codex);
    assert!(with_key["codex over ACP"].ok, "an API key is a login its agent makes: {:?}", with_key["codex over ACP"]);
}
