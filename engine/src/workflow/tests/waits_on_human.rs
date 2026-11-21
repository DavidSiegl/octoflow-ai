//! When a turn lets the human's message reach its agent (Agents::await_human): while the agent is idle in an
//! interview, and once a non-interview agent has stalled; and when a run lets its agents settle (Agents::settle).

use std::cell::RefCell;

use super::*;
use crate::agents::{AgentError, AgentLaunch, Agents, RunPlaces, Seen, Started, Waiting};
use crate::herdr::fake::FakeHerdr;
use crate::human::Human;
use crate::workflow::{Collaborators, Workflow};

/// The fixture's FakeHerdr as the run's Agents, with each Agents::await_human and Agents::settle it is asked
/// recorded.
struct Awaited<'f> {
    herdr: &'f FakeHerdr,
    /// Each await_human's role and why, in order.
    asked: RefCell<Vec<(String, Waiting)>>,
    /// For each settle, in order, whether any turn of the run had its tokens counted by then.
    settled: RefCell<Vec<bool>>,
}

impl<'f> Awaited<'f> {
    /// herdr as the run's Agents, nothing asked yet.
    fn new(herdr: &'f FakeHerdr) -> Awaited<'f> {
        Awaited { herdr, asked: RefCell::default(), settled: RefCell::default() }
    }
}

impl RunPlaces for Awaited<'_> {
    fn place(&self, run: &RunState) -> Result<Option<String>, AgentError> {
        self.herdr.place(run)
    }
    fn end(&self, run: &RunState) -> Result<(), AgentError> {
        self.herdr.end(run)
    }
}

impl Agents for Awaited<'_> {
    fn bind(&self, run: &mut RunState) {
        self.herdr.bind(run)
    }
    fn open(&self, run: &mut RunState, human: &dyn Human) -> Result<bool, AgentError> {
        self.herdr.open(run, human)
    }
    fn close(&self, run: &RunState, human: &dyn Human) {
        self.herdr.close(run, human)
    }
    fn next_record(&self, run: &RunState, role: &str, order: &[String], kind: &str) -> Result<AgentRecord, AgentError> {
        self.herdr.next_record(run, role, order, kind)
    }
    fn start(&self, run: &RunState, role: &str, launch: &AgentLaunch) -> Result<Started, AgentError> {
        self.herdr.start(run, role, launch)
    }
    fn wait_ready(&self, run: &RunState, role: &str, seconds: u64) -> Result<bool, AgentError> {
        self.herdr.wait_ready(run, role, seconds)
    }
    fn find(&self, run: &RunState, role: &str) -> Result<Option<Seen>, AgentError> {
        self.herdr.find(run, role)
    }
    fn prompt(&self, run: &RunState, role: &str, text: &str) -> Result<(), AgentError> {
        self.herdr.prompt(run, role, text)
    }
    fn wait_taken(&self, run: &RunState, role: &str, seconds: u64) -> Result<bool, AgentError> {
        self.herdr.wait_taken(run, role, seconds)
    }
    fn show(&self, run: &RunState, role: &str) -> Result<(), AgentError> {
        self.herdr.show(run, role)
    }
    fn await_human(&self, _run: &RunState, role: &str, why: Waiting) {
        self.asked.borrow_mut().push((role.to_string(), why));
    }
    fn whereabouts(&self, run: &RunState, role: &str) -> String {
        self.herdr.whereabouts(run, role)
    }
    fn settle(&self, run: &RunState) {
        self.settled.borrow_mut().push(run.turns.iter().any(|t| t.tokens.is_set()));
        self.herdr.settle(run)
    }
}

#[test]
fn an_idle_interview_and_a_stalled_agent_wait_on_the_human_and_a_working_one_does_not() {
    let f = Fixture::new(vec![("spec", vec![idle()]), ("build", vec![idle()]), ("review", vec![showing("working")])]);
    let (host, spec, build, review) =
        (Rc::clone(&f.host), f.state.spec_path(), f.state.build_path(1), f.state.review_path(1));
    // Each file written well after its agent went quiet: the build's past STALL_SECONDS.
    f.clock.hook(move |now| {
        let write = |path: &str, text: &str| {
            if host.read(path).unwrap().is_none() {
                host.write(path, text, false).unwrap();
            }
        };
        if now > 100.0 {
            write(&spec, "spec");
        }
        if now > (STALL_SECONDS + 200) as f64 {
            write(&build, "report");
        }
        if now > (STALL_SECONDS + 400) as f64 {
            write(&review, &format!("VERDICT: {APPROVE}\n"));
        }
    });
    let agents = Awaited::new(&f.herdr);

    let ran = Workflow::new(Collaborators { agents: &agents, ..f.collaborators() }, f.state.clone(), no_pr())
        .expect("Workflow::new takes the settings")
        .run();

    assert_eq!(ran, verdict(APPROVE));
    let mut asked = agents.asked.into_inner();
    asked.dedup();
    assert_eq!(asked, [("spec".to_string(), Waiting::Interview), ("build".to_string(), Waiting::Idle)]);
}

#[test]
fn an_agent_that_reports_a_wait_waits_on_the_human_before_it_would_stall() {
    let f = Fixture::new(vec![
        ("spec", vec![spec_turn()]),
        ("build", vec![idle()]),
        ("review", vec![review_turn(1, APPROVE)]),
    ]);
    let (host, spec, build) = (Rc::clone(&f.host), f.state.spec_path(), f.state.build_path(1));
    let waiting = build.replace("build-1.md", "build-1.waiting");
    // The report written well before STALL_SECONDS.
    f.clock.hook(move |now| {
        if host.read(&spec).unwrap().is_none() || host.read(&build).unwrap().is_some() {
            return;
        }
        if host.read(&waiting).unwrap().is_none() {
            host.write(&waiting, "the merge of pull request #526", false).unwrap();
        } else if now > 60.0 {
            host.write(&build, "report", false).unwrap();
        }
    });
    let agents = Awaited::new(&f.herdr);

    let ran = Workflow::new(Collaborators { agents: &agents, ..f.collaborators() }, f.state.clone(), no_pr())
        .expect("Workflow::new takes the settings")
        .run();

    assert_eq!(ran, verdict(APPROVE));
    assert!(agents.asked.into_inner().contains(&("build".to_string(), Waiting::Idle)));
}

/// A run's agents settle once its turns are over, before its tokens are counted: an ACP agent records a turn's usage
/// only when it answers the turn's prompt (cross-review 257 F7).
#[test]
fn a_finished_run_s_agents_settle_before_its_tokens_are_counted() {
    let f = Fixture::new(vec![
        ("spec", vec![spec_turn()]),
        ("build", vec![build_turn(1)]),
        ("review", vec![review_turn(1, APPROVE)]),
    ]);
    let agents = Awaited::new(&f.herdr);

    let ran = Workflow::new(Collaborators { agents: &agents, ..f.collaborators() }, f.state.clone(), no_pr())
        .expect("Workflow::new takes the settings")
        .run();

    assert_eq!(ran, verdict(APPROVE));
    assert_eq!(agents.settled.into_inner(), [false]);
}
