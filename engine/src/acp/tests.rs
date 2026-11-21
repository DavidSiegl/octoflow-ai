//! The ACP backend against a fake agent: a shell script that speaks enough of the protocol for each job, put in a
//! test's bin directory under a harness's program name, first on the agents' PATH (AcpAgents::in_environment).

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use super::*;
use crate::agents::Activity;
use crate::harness::Adapter;
use crate::host::tests::{assert_scrubbed, environment, put, read, tool, with_pathspec_switches};
use crate::test_support::{ended, eventually, TempDir};

/// The fake ACP agent. It writes its environment to `env` beside it, and appends to `log` beside it its argv
/// (`argv …`) and each message it gets (`<method> <line>`; a reply to its own request as ` <line>`). It answers
/// `initialize` with the agentCapabilities in `caps.json` beside it (`{}` without one) and the result fields in
/// `info.json` (after a comma; none without one), or not at all where `mute` is beside it; and `session/new`, `session/load` and
/// `session/resume` with session `s-1` plus the result fields in `opened.json` (each after a comma; none without
/// one); `session/load` and `session/resume` with an error where `refuse` is beside it. A prompt's text decides its
/// answer: ASK asks a permission (request 900, offering allow and deny) and answers end_turn once it is given,
/// UPDATE sends a session update (an agent_message_chunk) and answers end_turn, READ asks to read a file
/// (request 901) and answers end_turn once refused, FAIL answers with an error, EXIT exits, HANG never answers, FORK
/// starts a `sleep` that holds its output, its pid in `forked` beside it, and answers end_turn; CANCELLED, MAXTOKENS
/// and REFUSAL answer with that stop reason, PARTIAL with a usage of no kind the engine counts, NOCACHE with input and
/// output tokens alone (1 and 2); anything else answers end_turn with usage 10/20/30/40.
const FAKE_AGENT: &str = r#"#!/bin/sh
here=$(dirname "$0")
log="$here/log"
echo "argv $*" >> "$log"
env > "$here/env"
caps=$(cat "$here/caps.json" 2>/dev/null || echo '{}')
opened=$(cat "$here/opened.json" 2>/dev/null)
info=$(cat "$here/info.json" 2>/dev/null)
reply() { printf '{"jsonrpc":"2.0","id":%s,"result":%s}\n' "$1" "$2"; }
refuse() { printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32000,"message":"%s"}}\n' "$1" "$2"; }
# Read messages until the reply to this agent's request $1.
await() {
  while IFS= read -r answer; do
    echo " $answer" >> "$log"
    case "$answer" in *"\"id\":$1,"*) return ;; esac
  done
}
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/^{"jsonrpc":"2.0","id":\([0-9]*\),.*/\1/p')
  method=$(printf '%s' "$line" | sed -n 's/^{[^{]*"method":"\([^"]*\)".*/\1/p')
  echo "$method $line" >> "$log"
  case "$method" in
  initialize) [ -e "$here/mute" ] || reply "$id" "{\"protocolVersion\":1,\"agentCapabilities\":$caps$info}" ;;
  session/new) reply "$id" "{\"sessionId\":\"s-1\"$opened}" ;;
  session/load|session/resume)
    if [ -e "$here/refuse" ]; then refuse "$id" "no such session"; else reply "$id" "{\"x\":0$opened}"; fi ;;
  session/set_mode|session/set_model) reply "$id" '{}' ;;
  session/set_config_option) reply "$id" '{"configOptions":[]}' ;;
  session/prompt)
    case "$line" in
    *ASK*)
      printf '{"jsonrpc":"2.0","id":900,"method":"session/request_permission","params":{"sessionId":"s-1","toolCall":{"toolCallId":"t1"},"options":[{"optionId":"allow","name":"Allow","kind":"allow_once"},{"optionId":"deny","name":"Deny","kind":"reject_once"}]}}\n'
      await 900
      reply "$id" '{"stopReason":"end_turn"}' ;;
    *UPDATE*)
      printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hi"}}}}\n'
      reply "$id" '{"stopReason":"end_turn"}' ;;
    *READ*)
      printf '{"jsonrpc":"2.0","id":901,"method":"fs/read_text_file","params":{"path":"/etc/hosts"}}\n'
      await 901
      reply "$id" '{"stopReason":"end_turn"}' ;;
    *FAIL*) refuse "$id" "not logged in" ;;
    *EXIT*) exit 0 ;;
    *HANG*) ;;
    *FORK*)
      sleep 30 &
      echo $! > "$here/forked"
      reply "$id" '{"stopReason":"end_turn"}' ;;
    *CANCELLED*) reply "$id" '{"stopReason":"cancelled"}' ;;
    *MAXTOKENS*) reply "$id" '{"stopReason":"max_tokens"}' ;;
    *REFUSAL*) reply "$id" '{"stopReason":"refusal"}' ;;
    *PARTIAL*) reply "$id" '{"stopReason":"end_turn","usage":{"totalTokens":5}}' ;;
    *NOCACHE*) reply "$id" '{"stopReason":"end_turn","usage":{"inputTokens":1,"outputTokens":2}}' ;;
    *) reply "$id" '{"stopReason":"end_turn","usage":{"inputTokens":10,"outputTokens":20,"cachedReadTokens":30,"cachedWriteTokens":40,"totalTokens":100}}' ;;
    esac ;;
  esac
done
"#;

/// The log lines a human was told.
#[derive(Default)]
struct Heard(Mutex<Vec<String>>);

impl Human for Heard {
    fn notify(&self, _title: &str, _body: &str) {}
    fn log(&self, line: &str) {
        self.0.lock().unwrap().push(line.to_string());
    }
}

impl Heard {
    fn lines(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

/// A test's ACP run, its run directory made, with the fake agent as agy and as opencode in its bin directory.
struct Fixture {
    tmp: TempDir,
    run: RunState,
    /// The run's Ctrl-C, which a `stop` on its socket raises.
    interrupt: Interrupt,
}

impl Fixture {
    fn new() -> Fixture {
        let tmp = TempDir::new();
        let mut run = RunState::new("20261120-000000-abcd", "a task", &tmp.join("project"));
        run.backend = Backend::Acp;
        std::fs::create_dir_all(run.dir()).expect("the run directory");
        for program in ["agy", "opencode"] {
            tool(&tmp.join("bin"), program, FAKE_AGENT);
        }
        Fixture { tmp, run, interrupt: Interrupt::new() }
    }

    fn bin(&self) -> String {
        self.tmp.join("bin")
    }

    /// The run's agents, started in this process's environment with the bin directory first on PATH and every git
    /// pathspec switch set, besides KEEP_ME (with_pathspec_switches).
    fn agents<'h>(&self, human: &'h dyn Human) -> AcpAgents<'h> {
        AcpAgents::in_environment(
            human,
            &self.interrupt,
            environment(Some(&self.bin()), &with_pathspec_switches()).into_iter(),
        )
    }

    /// Give the fake agent's file of this name the text.
    fn put(&self, name: &str, text: &str) {
        put(&format!("{}/{name}", self.bin()), text);
    }

    /// What the fake agents logged.
    fn log(&self) -> String {
        read(&format!("{}/log", self.bin()))
    }

    /// The methods the fake agents were asked, in order.
    fn methods(&self) -> Vec<String> {
        let log = self.log();
        let methods = log.lines().filter_map(|l| l.split_once(" {").map(|(m, _)| m.to_string()));
        methods.filter(|m| !m.is_empty()).collect()
    }

    /// How many times the fake agents were asked method.
    fn asked(&self, method: &str) -> usize {
        self.methods().iter().filter(|m| *m == method).count()
    }

    /// Whether the run's usage file has been written.
    fn usage_written(&self) -> bool {
        std::path::Path::new(&usage_file(&self.run)).exists()
    }
}

fn launch(harness: &str) -> AgentLaunch<'_> {
    AgentLaunch { label: "Builder", harness, permission_mode: None, model: None, session: None, extra_dir: None }
}

fn activity(agents: &AcpAgents, run: &RunState) -> Option<Activity> {
    agents.find(run, "builder").expect("find").map(|seen| seen.activity)
}

/// The time, in seconds since the epoch.
fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).expect("a time after the epoch").as_secs_f64()
}

#[test]
fn a_start_initializes_the_agent_and_opens_a_session_whose_id_find_tells_before_any_prompt() {
    let f = Fixture::new();
    let human = Heard::default();
    let agents = f.agents(&human);

    let started = agents.start(&f.run, "builder", &launch("agy")).expect("start");

    let no_default = "agy's session offers no mode default, its equivalent of --permission-mode default; starting it \
                      without one";
    assert_eq!(started, Started { asking: false, unpassed: vec![no_default.to_string()] });
    assert_eq!(f.methods(), ["initialize", "session/new"]);
    let cwd = format!(r#""params":{{"cwd":"{}","#, f.run.work_dir());
    assert!(f.log().contains(&cwd), "{}", f.log());
    let seen = agents.find(&f.run, "builder").expect("find").expect("an agent");
    assert_eq!(seen, Seen { activity: Activity::Idle, session: Some("s-1".to_string()) });
    assert!(agents.wait_ready(&f.run, "builder", 0).expect("wait_ready"));
}

#[test]
fn a_prompt_is_taken_as_sent_works_until_answered_and_its_usage_is_recorded_as_of_its_sending() {
    let f = Fixture::new();
    let human = Heard::default();
    let agents = f.agents(&human);
    agents.start(&f.run, "builder", &launch("agy")).expect("start");

    agents.prompt(&f.run, "builder", "HANG").expect("prompt");
    assert!(agents.wait_taken(&f.run, "builder", 0).expect("wait_taken"));
    assert_eq!(activity(&agents, &f.run), Some(Activity::Working));
    let before = now();
    agents.prompt(&f.run, "builder", "build it").expect("prompt");
    assert!(eventually(|| f.usage_written()));
    let after = now();

    let usage = session_usage(&read(&usage_file(&f.run)), "s-1");
    assert_eq!(usage.len(), 1, "{usage:?}");
    assert_eq!(usage[0].tokens, Tokens { input: 10, output: 20, cache_read: 30, cache_write: 40 });
    assert!(usage[0].when >= before && usage[0].when <= after, "{usage:?} not in {before}..{after}");
    assert!(session_usage(&read(&usage_file(&f.run)), "s-2").is_empty());
    // The prompt that hangs still keeps it working.
    assert_eq!(activity(&agents, &f.run), Some(Activity::Working));
}

#[test]
fn an_answered_prompt_leaves_the_agent_idle() {
    let f = Fixture::new();
    let human = Heard::default();
    let agents = f.agents(&human);
    agents.start(&f.run, "builder", &launch("agy")).expect("start");

    agents.prompt(&f.run, "builder", "build it").expect("prompt");

    assert!(eventually(|| activity(&agents, &f.run) == Some(Activity::Idle) && f.usage_written()));
}

#[test]
fn a_permission_the_agent_asks_for_blocks_it_until_answered() {
    let f = Fixture::new();
    let human = Heard::default();
    let agents = f.agents(&human);
    agents.start(&f.run, "builder", &launch("agy")).expect("start");
    assert_eq!(agents.answer("builder", 1, "allow"), Err(CommandError::NoRequest), "no permission is asked yet");

    agents.prompt(&f.run, "builder", "ASK").expect("prompt");

    assert!(eventually(|| activity(&agents, &f.run) == Some(Activity::Blocked)));
    assert!(!agents.wait_ready(&f.run, "builder", 0).expect("wait_ready"));
    agents.answer("builder", 1, "allow").expect("answer");
    assert!(eventually(|| activity(&agents, &f.run) == Some(Activity::Idle)));
    let answer = r#" {"jsonrpc":"2.0","id":900,"result":{"outcome":{"outcome":"selected","optionId":"allow"}}}"#;
    assert!(f.log().contains(answer), "{}", f.log());
    assert!(agents.wait_ready(&f.run, "builder", 0).expect("wait_ready"));
}

#[test]
fn a_request_the_engine_does_not_offer_is_refused_as_no_such_method() {
    let f = Fixture::new();
    let human = Heard::default();
    let agents = f.agents(&human);
    agents.start(&f.run, "builder", &launch("agy")).expect("start");

    agents.prompt(&f.run, "builder", "READ").expect("prompt");

    assert!(eventually(|| activity(&agents, &f.run) == Some(Activity::Idle)));
    assert!(f.log().contains(r#""id":901,"error":{"code":-32601,"#), "{}", f.log());
}

#[test]
fn an_agent_that_exits_is_found_gone() {
    let f = Fixture::new();
    let human = Heard::default();
    let agents = f.agents(&human);
    agents.start(&f.run, "builder", &launch("agy")).expect("start");

    agents.prompt(&f.run, "builder", "EXIT").expect("prompt");

    assert!(eventually(|| activity(&agents, &f.run).is_none()));
    assert!(!agents.wait_taken(&f.run, "builder", 0).expect("wait_taken"));
    assert_eq!(agents.place(&f.run).expect("place"), None);
}

#[test]
fn an_agent_that_answers_a_prompt_with_an_error_is_told_to_the_human_and_found_gone() {
    let f = Fixture::new();
    let human = Heard::default();
    let agents = f.agents(&human);
    agents.start(&f.run, "builder", &launch("agy")).expect("start");

    agents.prompt(&f.run, "builder", "FAIL").expect("prompt");

    // The find that tells the error finds the agent gone, not idle.
    assert!(eventually(|| {
        let seen = activity(&agents, &f.run);
        !human.lines().is_empty() && seen.is_none()
    }));
    assert_eq!(human.lines(), ["[builder] agy failed: not logged in"]);
}

#[test]
fn a_saved_session_is_loaded_where_the_agent_can_load_one() {
    let f = Fixture::new();
    f.put("caps.json", r#"{"loadSession":true,"sessionCapabilities":{"resume":{}}}"#);
    let human = Heard::default();
    let agents = f.agents(&human);

    agents.start(&f.run, "builder", &AgentLaunch { session: Some("s-9"), ..launch("agy") }).expect("start");

    assert_eq!(f.methods(), ["initialize", "session/load"]);
    assert!(f.log().contains(r#""mcpServers":[],"sessionId":"s-9"}"#), "{}", f.log());
    assert_eq!(agents.find(&f.run, "builder").expect("find").and_then(|s| s.session), Some("s-9".to_string()));
}

#[test]
fn a_saved_session_is_resumed_where_the_agent_can_resume_but_not_load_one() {
    let f = Fixture::new();
    f.put("caps.json", r#"{"loadSession":false,"sessionCapabilities":{"resume":{}}}"#);
    let human = Heard::default();
    let agents = f.agents(&human);

    agents.start(&f.run, "builder", &AgentLaunch { session: Some("s-9"), ..launch("agy") }).expect("start");

    assert_eq!(f.methods(), ["initialize", "session/resume"]);
}

#[test]
fn a_saved_session_is_refused_where_the_agent_can_neither_load_nor_resume_one_or_refuses_it() {
    let f = Fixture::new();
    let human = Heard::default();
    let agents = f.agents(&human);
    let resumed = AgentLaunch { session: Some("s-9"), ..launch("agy") };

    let neither = agents.start(&f.run, "builder", &resumed);
    f.put("caps.json", r#"{"loadSession":true}"#);
    f.put("refuse", "");
    let refused = agents.start(&f.run, "builder", &resumed);

    assert!(
        matches!(&neither, Err(AgentError::Refused(e)) if e.0 == "agy can neither load nor resume a session over ACP"),
        "{neither:?}"
    );
    assert!(
        matches!(&refused, Err(AgentError::Refused(e)) if e.0 == "agy session/load: no such session"),
        "{refused:?}"
    );
}

#[test]
fn a_permission_mode_the_session_offers_is_set_once_it_is_open_unless_it_is_already_current() {
    let f = Fixture::new();
    let modes = r#","modes":{"currentModeId":"default","availableModes":[{"id":"default"},{"id":"accept-edits"}]}"#;
    f.put("opened.json", modes);
    let human = Heard::default();
    let agents = f.agents(&human);
    let accept_edits = AgentLaunch { permission_mode: Some("acceptEdits"), ..launch("agy") };

    let default = agents.start(&f.run, "builder", &launch("agy")).expect("start");
    let set = agents.start(&f.run, "builder", &accept_edits).expect("start");

    assert_eq!((default.unpassed, set.unpassed), (vec![], vec![]));
    assert_eq!(f.methods(), ["initialize", "session/new", "initialize", "session/new", "session/set_mode"]);
    assert!(f.log().contains(r#""params":{"sessionId":"s-1","modeId":"accept-edits"}"#), "{}", f.log());
}

#[test]
fn bypass_is_passed_at_launch_and_a_mode_the_session_or_harness_lacks_is_reported_unpassed() {
    let f = Fixture::new();
    let human = Heard::default();
    let agents = f.agents(&human);
    let mode = |harness, mode| AgentLaunch { permission_mode: Some(mode), ..launch(harness) };

    let bypass = agents.start(&f.run, "builder", &mode("agy", "bypassPermissions")).expect("start");
    let not_offered = agents.start(&f.run, "builder", &mode("agy", "plan")).expect("start");
    let no_equivalent = agents.start(&f.run, "builder", &mode("opencode", "acceptEdits")).expect("start");

    assert_eq!(bypass.unpassed, Vec::<String>::new());
    assert!(f.log().contains("argv acp --dangerously-skip-permissions\n"), "{}", f.log());
    let no_plan =
        "agy's session offers no mode plan, its equivalent of --permission-mode plan; starting it without one";
    assert_eq!(not_offered.unpassed, [no_plan]);
    assert_eq!(no_equivalent.unpassed, [harness("opencode").unwrap().without_mode("acceptEdits")]);
    assert_eq!(f.asked("session/set_mode"), 0, "{:?}", f.methods());
}

#[test]
fn a_model_the_launch_cannot_pass_is_set_where_the_session_offers_it_and_reported_unpassed_where_not() {
    let f = Fixture::new();
    let models = r#""models":{"currentModelId":"a","availableModels":[{"modelId":"a"},{"modelId":"b"}]}"#;
    f.put(
        "opened.json",
        &format!(r#","modes":{{"currentModeId":"default","availableModes":[{{"id":"default"}}]}},{models}"#),
    );
    let human = Heard::default();
    let agents = f.agents(&human);
    let model = |model| AgentLaunch { model: Some(model), ..launch("agy") };

    let offered = agents.start(&f.run, "builder", &model("b")).expect("start");
    let not_offered = agents.start(&f.run, "builder", &model("z")).expect("start");

    assert_eq!(offered.unpassed, Vec::<String>::new());
    assert!(f.log().contains(r#""params":{"sessionId":"s-1","modelId":"b"}"#), "{}", f.log());
    assert_eq!(not_offered.unpassed, ["agy offers no model z; starting it on its own"]);
    assert_eq!(f.asked("session/set_model"), 1, "{:?}", f.methods());
}

#[test]
fn a_model_is_set_by_the_session_s_model_config_option_where_it_has_one() {
    let f = Fixture::new();
    let options = r#"[{"group":"g","name":"G","options":[{"value":"a","name":"A"},{"value":"b","name":"B"}]}]"#;
    let model_option =
        format!(r#"{{"id":"m","category":"model","type":"select","currentValue":"a","options":{options}}}"#);
    let effort = r#"{"id":"effort","category":"thought_level","type":"select","currentValue":"b","options":[]}"#;
    f.put("opened.json", &format!(r#","configOptions":[{effort},{model_option}]"#));
    let human = Heard::default();
    let agents = f.agents(&human);
    let model = |model| AgentLaunch { model: Some(model), ..launch("opencode") };

    let current = agents.start(&f.run, "builder", &model("a")).expect("start");
    let offered = agents.start(&f.run, "builder", &model("b")).expect("start");
    let not_offered = agents.start(&f.run, "builder", &model("z")).expect("start");

    let without_mode = harness("opencode").unwrap().without_mode("default");
    assert_eq!((&current.unpassed, &offered.unpassed), (&vec![without_mode.clone()], &vec![without_mode.clone()]));
    assert_eq!(not_offered.unpassed, [without_mode, "opencode offers no model z; starting it on its own".to_string()]);
    assert!(f.log().contains(r#""params":{"sessionId":"s-1","configId":"m","value":"b"}"#), "{}", f.log());
    assert_eq!(f.asked("session/set_config_option"), 1, "{:?}", f.methods());
    assert_eq!(f.asked("session/set_model"), 0, "{:?}", f.methods());
}

#[test]
fn opencode_s_grant_is_a_root_of_its_session_and_agy_s_a_flag() {
    let f = Fixture::new();
    f.put("caps.json", r#"{"sessionCapabilities":{"additionalDirectories":{}}}"#);
    let human = Heard::default();
    let agents = f.agents(&human);
    let granted = |harness| AgentLaunch { extra_dir: Some("/runs"), ..launch(harness) };

    let opencode = agents.start(&f.run, "builder", &granted("opencode")).expect("start");
    agents.start(&f.run, "builder", &granted("agy")).expect("start");

    let log = f.log();
    let roots = |line: &&str| line.contains(r#""additionalDirectories":["/runs"]"#);
    let news: Vec<&str> = log.lines().filter(|l| l.starts_with("session/new ")).collect();
    assert_eq!(news.iter().map(roots).collect::<Vec<_>>(), [true, false], "{log}");
    assert!(log.contains("argv acp\n"), "{log}");
    assert!(log.contains("argv acp --add-dir /runs\n"), "{log}");
    // opencode has no equivalent of any mode yet, `default` included, which a run without one runs in.
    assert_eq!(opencode.unpassed, [harness("opencode").unwrap().without_mode("default")]);
}

#[test]
fn a_session_root_is_granted_again_to_a_resumed_session_and_reported_unpassed_where_the_agent_takes_none() {
    let f = Fixture::new();
    let human = Heard::default();
    let agents = f.agents(&human);
    let granted = AgentLaunch { extra_dir: Some("/runs"), ..launch("opencode") };

    let unoffered = agents.start(&f.run, "builder", &granted).expect("start");
    f.put("caps.json", r#"{"loadSession":true,"sessionCapabilities":{"additionalDirectories":{}}}"#);
    agents.start(&f.run, "builder", &AgentLaunch { session: Some("s-0"), ..granted }).expect("start");

    let log = f.log();
    let new = log.lines().find(|l| l.starts_with("session/new ")).expect("a session/new");
    assert!(!new.contains("additionalDirectories"), "{log}");
    let load = r#""params":{"cwd":"#;
    let loaded = log.lines().find(|l| l.starts_with("session/load ")).expect("a session/load");
    assert!(
        loaded.contains(load) && loaded.contains(r#""additionalDirectories":["/runs"],"sessionId":"s-0"}"#),
        "{log}"
    );
    let ungranted = "opencode cannot be granted /runs; it may ask before each write there";
    assert_eq!(unoffered.unpassed[0], ungranted);
}

#[test]
fn a_harness_without_an_acp_launch_is_refused_saying_the_app_does_not_offer_it_yet() {
    let f = Fixture::new();
    let human = Heard::default();
    let agents = f.agents(&human);

    let started = agents.start(&f.run, "builder", &launch("pi"));

    assert!(
        matches!(&started, Err(AgentError::Failed(e))
            if e.0 == "pi cannot run in a run the app starts: the app does not offer it yet"),
        "{started:?}"
    );
}

#[test]
fn a_new_start_of_a_role_kills_its_agent_before_and_end_kills_them_all() {
    let f = Fixture::new();
    let human = Heard::default();
    let agents = f.agents(&human);
    agents.start(&f.run, "builder", &launch("agy")).expect("start");
    agents.start(&f.run, "builder", &launch("agy")).expect("start");
    agents.start(&f.run, "reviewer", &launch("agy")).expect("start");

    assert_eq!(agents.place(&f.run).expect("place"), Some("2 ACP agents".to_string()));
    agents.end(&f.run).expect("end");

    assert_eq!(agents.place(&f.run).expect("place"), None);
    assert!(agents.find(&f.run, "builder").expect("find").is_none());
}

#[test]
fn a_run_binds_to_acp_and_a_herdr_run_is_not_opened() {
    let mut f = Fixture::new();
    let human = Heard::default();
    let agents = f.agents(&human);
    let mut herdr = RunState::new("20261120-000000-abcd", "a task", &f.tmp.join("project"));

    let refused = agents.open(&mut herdr, &human);
    let opened = agents.open(&mut f.run, &human);
    agents.bind(&mut herdr);

    assert!(matches!(refused, Err(AgentError::Failed(_))), "{refused:?}");
    assert!(matches!(opened, Ok(false)), "{opened:?}");
    assert_eq!(herdr.backend, Backend::Acp);
}

#[test]
fn session_usage_passes_over_lines_that_do_not_read() {
    let log = [
        r#"{"session":"s","when":1.5,"tokens":{"input":1,"output":2,"cache_read":3,"cache_write":4}}"#,
        "not json",
        r#"{"session":"s","when":2.0,"tokens":{"input":1}}"#,
        r#"{"session":"t","when":3.0,"tokens":{"input":1,"output":2,"cache_read":3,"cache_write":4}}"#,
    ]
    .join("\n");

    let usage = session_usage(&log, "s");

    assert_eq!(usage, [Usage { when: 1.5, tokens: Tokens { input: 1, output: 2, cache_read: 3, cache_write: 4 } }]);
}

#[test]
fn an_agent_starts_without_the_git_pathspec_switches_and_hears_the_signals_the_engine_blocks() {
    let f = Fixture::new();
    let human = Heard::default();
    let agents = f.agents(&human);
    let terminate: nix::sys::signal::SigSet = crate::interrupt::SIGNALS.into_iter().collect();

    // agy logs the signals it was started with blocked, as /proc has them, and then runs the fake agent: a shell
    // clears its mask as it starts, so the fake agent cannot tell.
    tool(&f.bin(), "fake-agent", FAKE_AGENT);
    let bin = f.bin();
    let logs_mask = format!(
        "#!/usr/bin/env -S awk -f\nBEGIN {{\n  while ((getline l < \"/proc/self/status\") > 0) if (l ~ /^SigBlk:/) print l >> \"{bin}/log\"\n  close(\"{bin}/log\")\n  exit system(\"exec {bin}/fake-agent\")\n}}\n"
    );
    tool(&bin, "agy", &logs_mask);
    // Started from a thread that blocks the signals, as every thread of the engine does (crate::interrupt::by_signals).
    terminate.thread_block().expect("blocking the signals");
    let started = agents.start(&f.run, "builder", &launch("agy"));
    terminate.thread_unblock().expect("unblocking the signals");

    started.expect("start");
    assert_scrubbed(&read(&format!("{}/env", f.bin())));
    let blocked = f.log().lines().find_map(|l| l.strip_prefix("SigBlk:").map(|mask| mask.trim().to_string()));
    let blocked = u64::from_str_radix(blocked.as_deref().expect("a SigBlk line"), 16).expect("a hex mask");
    for signal in crate::interrupt::SIGNALS {
        assert_eq!(blocked & (1 << (signal as u64 - 1)), 0, "{signal} is blocked: {blocked:x}");
    }
}

#[test]
fn ending_an_agent_kills_what_it_started_too() {
    let f = Fixture::new();
    let human = Heard::default();
    let agents = f.agents(&human);
    agents.start(&f.run, "builder", &launch("agy")).expect("start");
    agents.prompt(&f.run, "builder", "FORK").expect("prompt");
    assert!(eventually(|| activity(&agents, &f.run) == Some(Activity::Idle)));

    agents.end(&f.run).expect("end");

    assert!(ended(&format!("{}/forked", f.bin())), "the agent's sleep outlived it");
}

#[test]
fn an_agent_that_stops_for_another_reason_than_end_turn_is_idle_and_not_failed() {
    let f = Fixture::new();
    let human = Heard::default();
    let agents = f.agents(&human);
    agents.start(&f.run, "builder", &launch("agy")).expect("start");

    for reason in ["CANCELLED", "MAXTOKENS", "REFUSAL"] {
        agents.prompt(&f.run, "builder", reason).expect("prompt");
        assert!(
            eventually(|| f.log().contains(reason) && activity(&agents, &f.run) == Some(Activity::Idle)),
            "{reason}"
        );
    }

    assert_eq!(human.lines(), Vec::<String>::new());
}

#[test]
fn a_usage_without_input_or_output_tokens_is_not_recorded_and_one_without_cache_counts_counts_them_0() {
    let f = Fixture::new();
    let human = Heard::default();
    let agents = f.agents(&human);
    agents.start(&f.run, "builder", &launch("agy")).expect("start");

    agents.prompt(&f.run, "builder", "PARTIAL").expect("prompt");
    agents.prompt(&f.run, "builder", "NOCACHE").expect("prompt");

    assert!(eventually(|| f.usage_written() && activity(&agents, &f.run) == Some(Activity::Idle)));
    let usage = session_usage(&read(&usage_file(&f.run)), "s-1");
    let tokens: Vec<Tokens> = usage.into_iter().map(|u| u.tokens).collect();
    assert_eq!(tokens, [Tokens { input: 1, output: 2, cache_read: 0, cache_write: 0 }]);
}

/// probe of harness kind's fake agent, in the fixture's environment, given timeout and stopping.
fn probed(f: &Fixture, kind: &str, timeout: Duration, stopping: &Interrupt) -> Result<String, OrchestratorError> {
    let data_home = f.tmp.join("data");
    let env =
        environment(Some(&f.bin()), &[with_pathspec_switches().as_slice(), &[("XDG_DATA_HOME", &data_home)]].concat());
    probe(harness(kind).unwrap(), &env, &f.tmp.join("project"), timeout, stopping)
}

#[test]
fn a_probe_reports_what_the_agent_says_of_itself() {
    let f = Fixture::new();
    std::fs::create_dir_all(f.tmp.join("project")).expect("the project");
    let long = Duration::from_secs(30);

    let unnamed = probed(&f, "opencode", long, &Interrupt::new()).expect("an answer");
    f.put("info.json", r#","agentInfo":{"name":"agy","version":"1.5.11"}"#);
    let named = probed(&f, "agy", long, &Interrupt::new()).expect("an answer");
    f.put("info.json", r#","agentInfo":{"name":"agy"}"#);
    let unversioned = probed(&f, "agy", long, &Interrupt::new()).expect("an answer");
    let refused = probed(&f, "pi", long, &Interrupt::new()).expect_err("pi has no ACP launch");
    let uninstalled = probed(&f, "codex", long, &Interrupt::new()).expect_err("codex's adapter is not installed");

    assert_eq!((unnamed.as_str(), named.as_str()), ("answers over ACP", "agy 1.5.11"));
    assert_eq!(unversioned, "agy");
    assert_eq!(f.methods(), ["initialize", "initialize", "initialize"]);
    assert!(f.log().contains("argv acp\n"), "{}", f.log());
    assert_eq!(refused.0, "pi has no ACP launch");
    let not_installed = "codex's ACP adapter is not installed: install it from the app's setup dialog (Check setup…)";
    assert_eq!(uninstalled.0, not_installed);
}

#[test]
fn an_adapter_s_agent_is_its_installed_adapter_run_by_its_node() {
    for adapter in Adapter::ALL {
        let f = Fixture::new();
        std::fs::create_dir_all(f.tmp.join("project")).expect("the project");
        let installs = Installs::in_environment(adapter, vec![("XDG_DATA_HOME".into(), f.tmp.join("data").into())]);
        // A Node that is the fake agent, run from the bin directory, where it logs.
        let installed = installs.install_fake(&format!("#!/bin/sh\nexec {}/agy \"$@\"\n", f.bin()));

        let answer = probed(&f, adapter.harness(), Duration::from_secs(30), &Interrupt::new()).expect("an answer");

        assert_eq!(answer, "answers over ACP");
        assert!(f.log().contains(&format!("argv {}\n", installed.script)), "{adapter:?}: {}", f.log());
    }
}

#[test]
fn codex_s_adapter_starts_without_the_variables_that_would_choose_its_codex_mode_or_login() {
    let f = Fixture::new();
    std::fs::create_dir_all(f.tmp.join("project")).expect("the project");
    let data_home = f.tmp.join("data");
    let installs = Installs::in_environment(Adapter::Codex, vec![("XDG_DATA_HOME".into(), data_home.clone().into())]);
    installs.install_fake(&format!("#!/bin/sh\nexec {}/agy \"$@\"\n", f.bin()));
    let withheld = ["CODEX_PATH", "INITIAL_AGENT_MODE", "DEFAULT_AUTH_REQUEST"];
    let user = [("XDG_DATA_HOME", data_home.as_str()), ("CODEX_HOME", "/codex-home")];
    let set: Vec<(&str, &str)> = withheld.iter().map(|w| (*w, "set")).chain(user).collect();
    let env = environment(Some(&f.bin()), &set);

    probe(harness("codex").unwrap(), &env, &f.tmp.join("project"), Duration::from_secs(30), &Interrupt::new())
        .expect("an answer");

    let started = read(&format!("{}/env", f.bin()));
    for name in withheld {
        assert!(!started.lines().any(|l| l.starts_with(&format!("{name}="))), "{name} reached it:\n{started}");
    }
    assert!(started.lines().any(|l| l == "CODEX_HOME=/codex-home"), "{started}");
}

#[test]
fn a_probe_fails_for_an_agent_that_is_missing_exits_or_does_not_answer_and_stops_when_interrupted() {
    let f = Fixture::new();
    std::fs::create_dir_all(f.tmp.join("project")).expect("the project");
    f.put("mute", "");

    let silent = probed(&f, "agy", Duration::from_secs(1), &Interrupt::new()).expect_err("no answer");
    let stopping = Interrupt::new();
    let raiser = stopping.clone();
    let raise = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        raiser.raise();
    });
    let started = std::time::Instant::now();
    let interrupted = probed(&f, "agy", Duration::from_secs(30), &stopping).expect_err("interrupted");
    raise.join().unwrap();
    tool(&f.bin(), "opencode", "#!/bin/sh\nexit 0\n");
    let exited = probed(&f, "opencode", Duration::from_secs(30), &Interrupt::new()).expect_err("exited");
    // The bin directory alone on PATH, so that no agy installed on this machine is found.
    let only_bin = [(OsString::from("PATH"), OsString::from(f.bin()))];
    std::fs::remove_file(format!("{}/agy", f.bin())).expect("agy removed");
    let missing =
        probe(harness("agy").unwrap(), &only_bin, &f.tmp.join("project"), Duration::from_secs(30), &Interrupt::new())
            .expect_err("missing");

    assert_eq!(silent.0, "agy initialize: no answer within 1s");
    assert_eq!(interrupted.0, "interrupted");
    assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
    assert_eq!(exited.0, "opencode initialize: it exited");
    assert!(missing.0.starts_with("cannot start agy: "), "{}", missing.0);
}

#[test]
fn a_request_of_an_agent_that_has_exited_fails_as_exited_also_when_it_cannot_be_written_to() {
    let tmp = TempDir::new();
    let records = Records {
        stderr: "/dev/null",
        usage: "/dev/null",
        group: "/dev/null",
        journal: Arc::new(Journal::open("/dev/null").expect("a journal")),
    };
    let connection = Connection::spawn(
        "/bin/sh",
        &["-c".to_string(), "exit 0".to_string()],
        std::iter::empty(),
        &tmp.path(),
        records,
    )
    .expect("sh starts");
    // Gone before the request is written, so the write meets a closed pipe.
    assert!(eventually(|| connection.activity().is_none()), "sh exited");

    let failure = connection.request("initialize", json!({}), Duration::from_secs(5)).expect_err("no answer");

    assert!(matches!(failure, Failure::Exited), "{failure:?}");
}

/// The run's journal of the builder, each line read, with `at` taken out after it is checked to be a time.
fn journal(f: &Fixture) -> Vec<Value> {
    let text = read(&format!("{}/builder.events.jsonl", f.run.dir()));
    let lines = text.lines().map(|l| serde_json::from_str::<Value>(l).expect("a JSON line"));
    lines
        .map(|mut line| {
            if let Some(at) = line.as_object_mut().and_then(|o| o.remove("at")) {
                assert!(crate::summary::epoch_time(at.as_str().expect("a text")).is_ok(), "{at}");
            }
            line
        })
        .collect()
}

#[test]
fn the_journal_records_the_session_prompts_updates_permissions_answers_turn_ends_and_the_exit() {
    let f = Fixture::new();
    let human = Heard::default();
    let agents = f.agents(&human);
    agents.start(&f.run, "builder", &launch("agy")).expect("start");

    agents.prompt(&f.run, "builder", "UPDATE").expect("prompt");
    assert!(eventually(|| journal(&f).len() == 5));
    agents.prompt(&f.run, "builder", "ASK").expect("prompt");
    assert!(eventually(|| activity(&agents, &f.run) == Some(Activity::Blocked)));
    agents.answer("builder", 1, "deny").expect("answer");
    assert!(eventually(|| activity(&agents, &f.run) == Some(Activity::Idle)));
    agents.prompt(&f.run, "builder", "FAIL").expect("prompt");
    assert!(eventually(|| activity(&agents, &f.run).is_none()));
    agents.end(&f.run).expect("end");

    let options = json!([
        {"optionId": "allow", "name": "Allow", "kind": "allow_once"},
        {"optionId": "deny", "name": "Deny", "kind": "reject_once"},
    ]);
    let update = json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "hi"}});
    assert_eq!(
        journal(&f),
        [
            json!({"journal": 1}),
            json!({"seq": 1, "kind": "session", "session": "s-1", "harness": "agy", "resumed": false}),
            json!({"seq": 2, "kind": "prompt", "text": "UPDATE", "from": "orchestrator"}),
            json!({"seq": 3, "kind": "update", "update": update}),
            json!({"seq": 4, "kind": "turn_end", "stopReason": "end_turn"}),
            json!({"seq": 5, "kind": "prompt", "text": "ASK", "from": "orchestrator"}),
            json!({"seq": 6, "kind": "permission", "request": 1, "toolCall": {"toolCallId": "t1"}, "options": options}),
            json!({"seq": 7, "kind": "answered", "request": 1, "option": "deny", "by": "human"}),
            json!({"seq": 8, "kind": "turn_end", "stopReason": "end_turn"}),
            json!({"seq": 9, "kind": "prompt", "text": "FAIL", "from": "orchestrator"}),
            json!({"seq": 10, "kind": "exited", "error": "not logged in"}),
        ]
    );
}

#[test]
fn a_resumed_role_s_journal_goes_on_numbering_and_its_reopened_session_is_journaled_before_the_agent_answers() {
    let f = Fixture::new();
    f.put("caps.json", r#"{"loadSession":true}"#);
    let human = Heard::default();
    let agents = f.agents(&human);
    agents.start(&f.run, "builder", &launch("agy")).expect("start");
    agents.prompt(&f.run, "builder", "ASK").expect("prompt");
    assert!(eventually(|| activity(&agents, &f.run) == Some(Activity::Blocked)));
    drop(agents);

    let agents = f.agents(&human);
    agents.start(&f.run, "builder", &AgentLaunch { session: Some("s-1"), ..launch("agy") }).expect("start");
    agents.prompt(&f.run, "builder", "ASK").expect("prompt");
    assert!(eventually(|| activity(&agents, &f.run) == Some(Activity::Blocked)));

    let lines = journal(&f);
    let kinds: Vec<&str> = lines.iter().filter_map(|l| l["kind"].as_str()).collect();
    assert_eq!(kinds, ["session", "prompt", "permission", "exited", "session", "prompt", "permission"]);
    let seqs: Vec<u64> = lines.iter().filter_map(|l| l["seq"].as_u64()).collect();
    assert_eq!(seqs, [1, 2, 3, 4, 5, 6, 7]);
    assert_eq!(lines[5]["resumed"], true);
    assert_eq!(lines[7]["request"], 2);
    // Request 1 was the session before's: this connection never had it.
    assert_eq!(agents.answer("builder", 1, "allow"), Err(CommandError::NoRequest));
}

#[test]
fn the_first_answer_to_a_permission_wins_and_an_option_it_does_not_offer_is_refused() {
    let f = Fixture::new();
    let human = Heard::default();
    let agents = f.agents(&human);
    agents.start(&f.run, "builder", &launch("agy")).expect("start");
    agents.prompt(&f.run, "builder", "ASK").expect("prompt");
    assert!(eventually(|| activity(&agents, &f.run) == Some(Activity::Blocked)));

    let bad = agents.answer("builder", 1, "maybe");
    let first = agents.answer("builder", 1, "allow");
    let second = agents.answer("builder", 1, "deny");
    let nobody = agents.answer("reviewer", 1, "allow");

    assert_eq!(bad, Err(CommandError::BadOption));
    assert_eq!(first, Ok(()));
    assert_eq!(second, Err(CommandError::AlreadyAnswered { option: "allow".into(), by: "human".into() }));
    assert_eq!(nobody, Err(CommandError::NoAgent));
    assert!(eventually(|| activity(&agents, &f.run) == Some(Activity::Idle)));
    assert_eq!(f.log().matches(r#""id":900,"result""#).count(), 1, "{}", f.log());
}

#[test]
fn stop_cancels_each_permission_and_each_prompt_pending_then_raises_the_interrupt() {
    let f = Fixture::new();
    let human = Heard::default();
    let agents = f.agents(&human);
    agents.start(&f.run, "builder", &launch("agy")).expect("start");
    agents.start(&f.run, "reviewer", &launch("agy")).expect("start");
    agents.prompt(&f.run, "builder", "ASK").expect("prompt");
    agents.prompt(&f.run, "reviewer", "HANG").expect("prompt");
    assert!(eventually(|| activity(&agents, &f.run) == Some(Activity::Blocked)));

    agents.stop();

    assert!(f.interrupt.is_raised());
    let cancelled = r#" {"jsonrpc":"2.0","id":900,"result":{"outcome":{"outcome":"cancelled"}}}"#;
    assert!(eventually(|| f.log().contains(cancelled)), "{}", f.log());
    let cancel = r#"session/cancel {"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":"s-1"}}"#;
    assert!(eventually(|| f.log().contains(cancel)), "{}", f.log());
    let answered = journal(&f).into_iter().find(|l| l["kind"] == "answered").expect("an answered line");
    assert_eq!((&answered["option"], &answered["by"]), (&json!("cancelled"), &json!("stop")));
    assert_eq!(
        agents.answer("builder", 1, "allow"),
        Err(CommandError::AlreadyAnswered { option: "cancelled".into(), by: "stop".into() })
    );
}

#[test]
fn an_open_run_listens_on_its_socket_whose_stop_raises_the_interrupt_and_closes_it_when_dropped() {
    let mut f = Fixture::new();
    let human = Heard::default();
    let agents = f.agents(&human);
    agents.open(&mut f.run, &human).expect("open");
    agents.start(&f.run, "builder", &launch("agy")).expect("start");
    agents.prompt(&f.run, "builder", "ASK").expect("prompt");
    assert!(eventually(|| activity(&agents, &f.run) == Some(Activity::Blocked)));

    let answered = ask(&f.run.dir(), &json!({"answer": {"role": "builder", "request": 1, "option": "allow"}}));
    let stopped = ask(&f.run.dir(), &json!({"stop": {}}));

    assert_eq!((answered, stopped), (Ok(json!({})), Ok(json!({}))));
    assert!(eventually(|| f.interrupt.is_raised()));
    drop(agents);
    assert!(matches!(ask(&f.run.dir(), &json!({"stop": {}})), Err(SocketError::Unreachable(_))));
}

#[test]
fn an_agent_s_group_is_recorded_and_one_an_earlier_orchestrator_left_is_ended_before_the_role_starts_again() {
    let f = Fixture::new();
    let human = Heard::default();
    let agents = f.agents(&human);
    agents.start(&f.run, "builder", &launch("agy")).expect("start");
    agents.prompt(&f.run, "builder", "FORK").expect("prompt");
    assert!(eventually(|| activity(&agents, &f.run) == Some(Activity::Idle)));
    let record: Value = serde_json::from_str(&read(&format!("{}/builder.acp.group", f.run.dir()))).expect("JSON");
    assert!(record["pgid"].as_i64().is_some() && record["started"].as_str().is_some(), "{record}");
    // As a SIGKILLed orchestrator leaves it: the agent and its group run on.
    std::mem::forget(agents);

    let agents = f.agents(&human);
    agents.start(&f.run, "builder", &launch("agy")).expect("start");

    assert!(ended(&format!("{}/forked", f.bin())), "the earlier agent's sleep outlived the new start");
    assert_eq!(human.lines(), ["[builder] ended the agent an earlier orchestrator of the run left"]);
}

#[test]
fn a_message_is_refused_until_the_run_waits_on_the_human_and_then_sent_once_as_the_human_s_prompt() {
    let mut f = Fixture::new();
    let human = Heard::default();
    let agents = f.agents(&human);
    agents.open(&mut f.run, &human).expect("open");
    agents.start(&f.run, "builder", &launch("agy")).expect("start");
    agents.prompt(&f.run, "builder", "HANG").expect("prompt");

    // Working, and then idle but not waited on: no message reaches it.
    agents.await_human(&f.run, "builder", Waiting::Interview);
    let working = agents.message("builder", "use 10");
    let nobody = agents.message("reviewer", "use 10");
    assert_eq!((working, nobody), (Err(CommandError::NotWaiting), Err(CommandError::NoAgent)));
    agents.start(&f.run, "builder", &launch("agy")).expect("start again");
    assert_eq!(agents.message("builder", "use 10"), Err(CommandError::NotWaiting));

    agents.await_human(&f.run, "builder", Waiting::Blocked);
    agents.await_human(&f.run, "builder", Waiting::Blocked);
    let sent = ask(&f.run.dir(), &json!({"message": {"role": "builder", "text": "use 10"}}));
    assert_eq!(sent, Ok(json!({})));
    assert!(eventually(|| activity(&agents, &f.run) == Some(Activity::Idle) && f.usage_written()));
    let again = ask(&f.run.dir(), &json!({"message": {"role": "builder", "text": "use 10"}}));
    assert!(matches!(&again, Err(SocketError::Refused { code, .. }) if code == "not_waiting"), "{again:?}");

    let lines = journal(&f);
    let waiting: Vec<&Value> = lines.iter().filter(|l| l["kind"] == "waiting").collect();
    assert_eq!(waiting.len(), 1, "one waiting line until the next prompt: {lines:?}");
    assert_eq!(waiting[0]["why"], "blocked");
    let last_prompt = lines.iter().rev().find(|l| l["kind"] == "prompt").expect("a prompt");
    assert_eq!((&last_prompt["text"], &last_prompt["from"]), (&json!("use 10"), &json!("human")));
    assert_eq!(f.log().matches("use 10").count(), 1, "{}", f.log());
}
