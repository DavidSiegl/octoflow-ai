//! A role's turn: getting its agent ready, prompting it, and waiting for its handoff file, with the turn's record in
//! RunState.turns.

use super::{How, Outcome, Stopped, Workflow};
use crate::agents::{Activity, AgentError, AgentLaunch, LaunchedModel, Seen, Waiting};
use crate::constants::{
    APPROVE, CHANGES_REQUESTED, HEARTBEAT_SECONDS, POLL_SECONDS, PROMPT_DELIVERY_SECONDS, STALL_SECONDS,
};
use crate::error::OrchestratorError;
use crate::handoff::{parse_blocked, parse_verdict};
use crate::harness::harness;
use crate::json::{object, Object, ValueExt};
use crate::paths::{file_name, split_extension};
use crate::prompts::Prompt;
use crate::state::{AgentRecord, AgentStart, RunState, StandingWait, TurnRecord};
use crate::timestamp::{epoch_seconds, iso_utc};
use serde_json::Value;

/// What a turn does once its agent is ready, before any prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Announce {
    Nothing,
    /// The human-paced step's: bring the agent before the human (Agents::show) and tell them it waits for them.
    Interview,
    /// A blocked turn's wait for the human's answer: bring the agent before the human and tell them its question.
    Blocked {
        question: String,
    },
}

impl Announce {
    /// What the turn waits on the human for all through: an interview's or a block's answer; None for neither.
    fn waiting(&self) -> Option<Waiting> {
        match self {
            Announce::Nothing => None,
            Announce::Interview => Some(Waiting::Interview),
            Announce::Blocked { .. } => Some(Waiting::Blocked),
        }
    }
}

/// The turn a step asks of a role.
pub(super) struct Turn {
    pub role: String,
    /// The handoff file that ends the turn.
    pub path: String,
    /// The prompt for an agent that has seen the role's earlier turns.
    pub text: String,
    /// The prompt for an agent in a new session, which has not; None (or empty) sends text.
    pub fresh_text: Option<String>,
    /// Seconds the turn may take; None for no limit.
    pub timeout: Option<u64>,
    /// Whether an agent idle for STALL_SECONDS without its file is told to the human.
    pub watch_stalls: bool,
    pub announce: Announce,
    pub judged: Judged,
}

/// How a turn judges the handoff file its role wrote, beyond refusing an empty one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Judged {
    /// Taken as written.
    AsWritten,
    /// A verdict step's: a file without a VERDICT first line is asked for once more (retry) before it ends the run.
    Verdict,
    /// A step that can block's (Step::can_block): a BLOCKED report waits for the human (block), as often as it
    /// comes.
    Blocking,
}

/// Why an agent could not be started: its backend refused (AgentError::Refused), which a resume into a saved session
/// gets past by starting a fresh one; or the run stops.
enum StartFailure {
    Refused(AgentError),
    Stopped(Stopped),
}

impl From<Stopped> for StartFailure {
    fn from(e: Stopped) -> StartFailure {
        StartFailure::Stopped(e)
    }
}

impl From<OrchestratorError> for StartFailure {
    fn from(e: OrchestratorError) -> StartFailure {
        StartFailure::Stopped(Stopped::Failed(e))
    }
}

impl From<AgentError> for StartFailure {
    fn from(e: AgentError) -> StartFailure {
        match e {
            AgentError::Refused(_) => StartFailure::Refused(e),
            AgentError::Failed(e) => StartFailure::Stopped(Stopped::Failed(e)),
        }
    }
}

impl From<StartFailure> for Stopped {
    fn from(e: StartFailure) -> Stopped {
        match e {
            StartFailure::Refused(e) => e.into(),
            StartFailure::Stopped(e) => e,
        }
    }
}

impl Workflow<'_> {
    /// The handoff file that ends the role's turn, prompting the role only if it still owes it.
    pub(super) fn turn(&mut self, t: Turn) -> Outcome<String> {
        if t.judged == Judged::Verdict {
            return self.verdict_turn(t);
        }
        let blocks = t.judged == Judged::Blocking;
        let file = file_name(&t.path).to_string();
        let block = self.state.blocked.clone().filter(|b| blocks && file_of(b) == Some(&file));
        let out = match block {
            // A resume in the middle of a block.
            Some(block) => {
                self.finish_block(&t.path, &block)?;
                self.unblocked(&t.role, &t.path, block)?
            }
            None => {
                let out = self.written(&t)?;
                match parse_blocked(&out).filter(|_| blocks) {
                    Some(question) => {
                        let block = self.block(&t.role, &t.path, &question)?;
                        self.unblocked(&t.role, &t.path, block)?
                    }
                    None => out,
                }
            }
        };
        if out.trim().is_empty() {
            let why = format!("was written empty by the {}", self.label(&t.role));
            return Err(self.reject(&t.path, &why));
        }
        Ok(out)
    }

    /// turn for a verdict step: its file, once it starts with a VERDICT line.
    fn verdict_turn(&mut self, mut t: Turn) -> Outcome<String> {
        let file = file_name(&t.path).to_string();
        if self.state.retrying.as_deref() == Some(&file) {
            t.text = self.retry_prompt(&t.path)?;
            t.fresh_text = Some(t.text.clone());
        }
        let out = loop {
            let out = self.written(&t)?;
            if parse_verdict(&out).is_some() {
                break out;
            }
            self.retry(&t.role, &t.path, &out)?;
            t.text = self.retry_prompt(&t.path)?;
            t.fresh_text = Some(t.text.clone());
        };
        if self.state.retrying.as_deref() == Some(&file) {
            self.state.retrying = None;
        }
        Ok(out)
    }

    /// The handoff file as the role wrote it, before turn judges it.
    fn written(&mut self, t: &Turn) -> Outcome<String> {
        let (role, path) = (t.role.as_str(), t.path.as_str());
        let file = file_name(path).to_string();
        // First, because the role may have written it while no orchestrator was watching.
        if let Some(out) = self.host.read(path)? {
            self.human.log(&format!("[{role}] {file} is already written"));
            if let Some(agent) = self.state.agents.get_mut(role).filter(|a| a.unprompted.is_some()) {
                agent.past_startup = true;
            }
            self.host.remove(&waiting_path(path))?;
            self.end_turn(role, path)?;
            return Ok(out);
        }
        let how = self.agent(role)?;
        // A wait is the session's that reported it (waiting_path): one that is not the session prompted for this turn
        // going on, as after its agent exited and was started again, knows nothing of it.
        let goes_on = matches!(how, How::Alive | How::Started(AgentStart::Resumed))
            && self.state.prompted.as_deref() == Some(file.as_str());
        if !goes_on {
            self.host.remove(&waiting_path(path))?;
            self.record_wait(&file, None)?;
        }
        let message = self.prompt_for(how, &file, &t.text, t.fresh_text.as_deref(), path)?;
        self.announce(role, how, message.is_some(), &t.announce)?;
        self.human.log(&format!("[{role}] working in {}", self.whereabouts(role)));
        if let Some(message) = message {
            // A resumed or restarted turn keeps its record and start. Read before the prompt goes out, so that no
            // agent can write its file before its turn started (its mtime still can: end_turn counts that as the
            // start).
            let started = if self.open_turn(&file).is_some() { None } else { Some(self.host.now()?) };
            self.deliver(role, path, &message)?;
            // Recorded only once the prompt is taken. Dying between sending and confirming costs one duplicate
            // prompt, which the role answers by writing the same file again.
            self.state.prompted = Some(file.clone());
            self.clear_unprompted(role);
            if let Some(started) = started {
                let record = self.turn_record(role, &file, Some(started));
                self.state.turns.push(record);
            }
            self.save()?;
        }
        let out = self.await_handoff(role, path, t)?;
        self.human.log(&format!("[{role}] wrote {file}"));
        // The turn's file withdraws its wait: a later turn that writes the same file, a retry's or a block's, must not
        // find it standing.
        self.host.remove(&waiting_path(path))?;
        self.end_turn(role, path)?;
        Ok(out)
    }

    /// Take the role's agent as prompted (AgentRecord::unprompted), unsaved; a role with no agent is left as it is.
    fn clear_unprompted(&mut self, role: &str) {
        if let Some(agent) = self.state.agents.get_mut(role) {
            (agent.unprompted, agent.past_startup) = (None, false);
        }
    }

    /// What the turn does once the role's agent is ready, as how says it came to be; prompting is true when this pass
    /// sends the agent a prompt.
    ///
    /// An interview is announced only with a prompt: an agent only waited for (alive, and already prompted for the
    /// turn's file) was announced by the orchestrator that prompted it, and a resume does not tell the human again.
    /// A block's wait is announced on every pass.
    fn announce(&mut self, role: &str, how: How, prompting: bool, announce: &Announce) -> Outcome<()> {
        match announce {
            Announce::Nothing => Ok(()),
            Announce::Interview if !prompting => Ok(()),
            Announce::Interview => {
                self.agents.show(&self.state, role)?;
                let place = self.whereabouts(role);
                let label = self.label(role);
                // One past its startup (AgentRecord::past_startup) went on from its restart with the human already.
                let past_startup = self.state.agents.get(role).is_some_and(|a| a.past_startup);
                let what = if how == How::Started(AgentStart::Restarted) && !past_startup {
                    "restarted; the interview starts over"
                } else {
                    "is waiting for you"
                };
                self.human.notify(&format!("{label} {what}"), &format!("{place}: {}", self.state.task));
                self.human.log(&format!("[{role}] answer the {label} in {place}"));
                Ok(())
            }
            Announce::Blocked { question } => {
                self.agents.show(&self.state, role)?;
                self.ask_human(role, &format!("is blocked: {question}"));
                Ok(())
            }
        }
    }

    /// The report of a blocked turn once it is no longer BLOCKED, block being the state's record of the block it is
    /// in (RunState::blocked); each that still is waits again (block).
    fn unblocked(&mut self, role: &str, path: &str, mut block: Object) -> Outcome<String> {
        let out = loop {
            let out = self.block_answer(role, path, &block)?;
            match parse_blocked(&out) {
                Some(question) => block = self.block(role, path, &question)?,
                None => break out,
            }
        };
        self.state.blocked = None;
        self.save()?;
        Ok(out)
    }

    /// Move a BLOCKED report aside to its next blocked name, and record the block, so that the turn waits for the
    /// human's answer and then for the report again. Returns the record, RunState::blocked.
    fn block(&mut self, role: &str, path: &str, question: &str) -> Outcome<Object> {
        let file = file_name(path).to_string();
        let s = &mut self.state;
        let k = match &s.blocked {
            Some(b) if file_of(b) == Some(&file) => block_number(b)? + 1,
            _ => 1,
        };
        let blocked = blocked_path(path, k);
        // Everything is saved before the rename, which a resume finishes (finish_block): dying at any point leaves the
        // report to be judged again, as block k once more, or the wait under way, never a report lost or counted
        // twice. No prompt counts as delivered, so even an agent that is alive gets BLOCKED_PROMPT.
        s.prompted = None;
        let record =
            object([("file", Value::from(file.as_str())), ("question", Value::from(question)), ("k", Value::from(k))]);
        s.blocked = Some(record.clone());
        // The turn's record, which written closed, goes with the file, as in retry; the wait opens a record of its
        // own, human-paced (turn_record).
        let moved = file_name(&blocked).to_string();
        if let Some(t) = s.turns.iter_mut().rev().find(|t| t.file == file) {
            t.file.clone_from(&moved);
        }
        self.save()?;
        self.host.rename(path, &blocked)?;
        self.human.log(&format!("[{role}] {file} is blocked: {question}; moved it to {moved}"));
        Ok(record)
    }

    /// Make the rename of the block, RunState::blocked, that the orchestrator saved but may have died before making.
    fn finish_block(&self, path: &str, block: &Object) -> Result<(), OrchestratorError> {
        let blocked = blocked_path(path, block_number(block)?);
        if self.host.read(&blocked)?.is_none() && self.host.read(path)?.is_some() {
            self.host.rename(path, &blocked)?;
        }
        Ok(())
    }

    /// The role's report once the human has answered the BLOCKED question of block, RunState::blocked, where it runs.
    ///
    /// As for the interview: no turn timeout and no stall notice, since the agent waits for the human. A resumed wait
    /// announces itself again, and prompts as prompt_for does, with BLOCKED_PROMPT in place of the step's.
    fn block_answer(&mut self, role: &str, path: &str, block: &Object) -> Outcome<String> {
        let blocked = blocked_path(path, block_number(block)?);
        let prompt = self.prompts.fill(Prompt::BlockedPrompt, &[("path", path), ("blocked_path", &blocked)])?;
        let question = block_field(block, "question")?.text();
        let t = Turn {
            role: role.to_string(),
            path: path.to_string(),
            text: prompt.clone(),
            fresh_text: Some(prompt),
            timeout: None,
            watch_stalls: false,
            announce: Announce::Blocked { question },
            judged: Judged::AsWritten,
        };
        self.written(&t)
    }

    /// Prompt the role's agent with message until it takes it (taken), sending it at most twice.
    ///
    /// Errors: the agent takes neither prompt, naming what a resume does about it, or exits meanwhile; its backend
    /// fails.
    fn deliver(&mut self, role: &str, path: &str, message: &str) -> Outcome<()> {
        self.agents.prompt(&self.state, role, message)?;
        if self.taken(role, path)? {
            return Ok(());
        }
        self.human
            .log(&format!("[{role}] did not take its prompt within {PROMPT_DELIVERY_SECONDS}s; sending it again"));
        self.agents.prompt(&self.state, role, message)?;
        if !self.taken(role, path)? {
            let (label, place) = (self.label(role), self.whereabouts(role));
            return Err(Stopped::Failed(OrchestratorError(format!(
                "the {label} did not take its prompt to write {path}, sent twice; see {place}, and resume to send it \
                 again"
            ))));
        }
        Ok(())
    }

    /// Whether the role's agent took the prompt just sent, within PROMPT_DELIVERY_SECONDS: its backend says so
    /// (Agents::wait_taken), or path, its handoff file, exists.
    ///
    /// Errors: the agent exits without writing path; its backend fails.
    fn taken(&self, role: &str, path: &str) -> Outcome<bool> {
        // First, for a turn so quick that the agent is done before the wait begins.
        if self.host.read(path)?.is_some() {
            return Ok(true);
        }
        if self.agents.wait_taken(&self.state, role, PROMPT_DELIVERY_SECONDS)? {
            return Ok(true);
        }
        if self.host.read(path)?.is_some() {
            return Ok(true);
        }
        if self.agents.find(&self.state, role)?.is_none() {
            let label = self.label(role);
            return Err(Stopped::Failed(OrchestratorError(format!("the {label} exited without writing {path}"))));
        }
        Ok(false)
    }

    /// Move a malformed verdict file aside so its role writes it again; end the run if its retry is used.
    fn retry(&mut self, role: &str, path: &str, out: &str) -> Outcome<()> {
        let file = file_name(path).to_string();
        let why = if out.trim().is_empty() {
            format!("was written empty by the {}", self.label(role))
        } else {
            "does not start with a VERDICT line".to_string()
        };
        if self.state.retried.contains(&file) {
            return Err(self.reject(path, &format!("{why}, and its one retry was already used")));
        }
        let rejected = rejected_path(path);
        // Saved around the rename, so dying at any point leaves the malformed file with its retry unused, no file and
        // no prompt counted as delivered, or the retry under way; never a used retry the role was not asked for.
        self.state.prompted = None;
        self.save()?;
        self.host.rename(path, &rejected)?;
        self.state.retried.push(file.clone());
        self.state.retrying = Some(file.clone());
        // The turn's record, which written closed, goes with the file, whose mtime the rename kept; the retry opens a
        // record of its own.
        let moved = file_name(&rejected).to_string();
        if let Some(record) = self.state.turns.iter_mut().rev().find(|t| t.file == file) {
            record.file.clone_from(&moved);
        }
        self.save()?;
        self.human.log(&format!("[{role}] {file} {why}; moved it to {moved}, prompting once more"));
        Ok(())
    }

    fn retry_prompt(&self, path: &str) -> Result<String, OrchestratorError> {
        let rejected = rejected_path(path);
        let values =
            [("path", path), ("rejected_path", &rejected), ("approve", APPROVE), ("changes", CHANGES_REQUESTED)];
        self.prompts.fill(Prompt::RetryVerdictPrompt, &values)
    }

    /// What to prompt an agent that is ready as how says with; None when it only has to be waited for.
    fn prompt_for(
        &self,
        how: How,
        file: &str,
        text: &str,
        fresh_text: Option<&str>,
        path: &str,
    ) -> Result<Option<String>, OrchestratorError> {
        if matches!(how, How::Started(AgentStart::New | AgentStart::Restarted)) {
            return Ok(Some(fresh_text.filter(|f| !f.is_empty()).unwrap_or(text).to_string()));
        }
        if self.state.prompted.as_deref() != Some(file) {
            return Ok(Some(text.to_string()));
        }
        if how == How::Started(AgentStart::Resumed) {
            return Ok(Some(self.prompts.fill(Prompt::ContinuePrompt, &[("path", path)])?));
        }
        // Alive and already prompted for this file.
        Ok(None)
    }

    /// Poll until the role writes path, t's handoff file, telling the human when it is blocked or stalled, and letting
    /// their message reach the agent while it is idle and the turn waits on them (Agents::await_human): all through
    /// an interview or a block's wait, and once it has stalled.
    ///
    /// While the role reports a wait on something outside the run (poll_wait), the turn's clock stands still
    /// (TurnClock), so the timeout counts only the time it works, no stall is told, and an idle agent is open to the
    /// human's message.
    fn await_handoff(&mut self, role: &str, path: &str, t: &Turn) -> Outcome<String> {
        let (watch_stalls, paced) = (t.watch_stalls, t.announce.waiting());
        let (label, file) = (self.label(role), file_name(path).to_string());
        let mut clock = TurnClock::new(t.timeout, self.clocks.monotonic());
        // Since when the agent has shown idle or done without the file; None while it works or waits.
        let mut quiet_since: Option<f64> = None;
        // What the human was last told about this turn.
        let mut told: Option<&'static str> = None;
        // How many polls in a row have found the role's wait standing; 0 for none.
        let mut waiting_polls = 0;
        loop {
            if let Some(out) = self.host.read(path)? {
                return Ok(out);
            }
            let Some(seen) = self.agents.find(&self.state, role)? else {
                return Err(Stopped::Failed(OrchestratorError(format!("the {label} exited without writing {path}"))));
            };
            self.note_session(role, &seen)?;
            let now = self.clocks.monotonic();
            let waiting = self.poll_wait(role, path, &mut waiting_polls)?;
            if let Some(timeout) = clock.run_out(now, waiting) {
                let place = self.whereabouts(role);
                return Err(Stopped::Failed(OrchestratorError(format!(
                    "the {label} did not write {path} within {timeout}s; see {place}"
                ))));
            }
            if waiting || seen.activity != Activity::Idle {
                quiet_since = None;
            } else if quiet_since.is_none() {
                quiet_since = Some(now);
            }
            let stalled = watch_stalls && quiet_since.is_some_and(|since| now - since >= STALL_SECONDS as f64);
            // A waiting agent is open to the human all through its wait, as a stalled one is: without a deadline that
            // runs out, a wait never withdrawn has no other way out but the human's message.
            self.open_to_human(role, seen.activity, paced, stalled || waiting);
            told = self.tell_human(role, seen.activity, stalled, told, &file);
            self.heartbeat()?;
            self.pause(POLL_SECONDS)?;
        }
    }

    /// Whether the role's wait on something outside the run stands at this poll (waiting_on), polls counting the polls
    /// in a row before it that found it standing, which this one updates. The human is told of a wait once, at its
    /// second poll, not its first: a poll can read the file between the agent's emptying it and its writing what it
    /// waits on. A rewritten reason is not told again; a withdrawal goes to the log. The turn's record follows the
    /// wait from that second poll too (record_wait). Errors: as waiting_on's and record_wait's.
    fn poll_wait(&mut self, role: &str, path: &str, polls: &mut u32) -> Outcome<bool> {
        let waiting = self.waiting_on(path)?;
        if waiting.is_none() || *polls > 0 {
            self.record_wait(file_name(path), waiting.as_deref())?;
        }
        match &waiting {
            Some(what) if *polls == 1 => self.ask_human(role, &format!("is waiting on {what}")),
            None if *polls > 0 => self.human.log(&format!("[{role}] is no longer waiting")),
            _ => {}
        }
        *polls = if waiting.is_some() { *polls + 1 } else { 0 };
        Ok(waiting.is_some())
    }

    /// Bring the record of the turn under way that writes file (TurnRecord::waiting_on) up to what its role waits on
    /// now, None for nothing, and save it if that changed it: a new wait starts at the host's now, a rewritten one
    /// takes its new reason and keeps its start, and a withdrawn one ends now. A file with no turn under way records
    /// nothing. Errors: the host's time cannot be read, a standing wait's start is not a time, or the state cannot
    /// be saved.
    fn record_wait(&mut self, file: &str, waiting: Option<&str>) -> Outcome<()> {
        let Some(i) = self.open_turn(file) else {
            return Ok(());
        };
        let standing = self.state.turns[i].waiting_on.as_ref();
        if waiting == standing.map(|w| w.on.as_str()) {
            return Ok(());
        }
        let since = standing.map(|w| w.since.clone());
        match waiting {
            Some(on) => {
                let since = match since {
                    Some(since) => since,
                    None => iso_utc(self.host.now()?),
                };
                self.state.turns[i].waiting_on = Some(StandingWait { on: on.to_string(), since });
            }
            None => {
                let now = self.host.now()?;
                self.state.turns[i].end_wait(now)?;
            }
        }
        self.save()
    }

    /// What the role reports its turn waits on, from the waiting file of its handoff file at path (waiting_path): the
    /// file's first line that is not blank; None while there is no such file. Errors: the file cannot be read.
    fn waiting_on(&self, path: &str) -> Result<Option<String>, OrchestratorError> {
        let text = self.host.read(&waiting_path(path))?;
        Ok(text.map(|t| {
            let what = t.lines().map(str::trim).find(|l| !l.is_empty());
            what.unwrap_or("something it did not name").to_string()
        }))
    }

    /// Let the human's message reach the role's agent while its activity is idle and the turn waits on them: through
    /// paced, the wait of an interview or a block, else once it has stalled or while it reports a wait, both given
    /// as stalled (Agents::await_human).
    fn open_to_human(&self, role: &str, activity: Activity, paced: Option<Waiting>, stalled: bool) {
        if activity != Activity::Idle {
            return;
        }
        if let Some(why) = paced.or(stalled.then_some(Waiting::Idle)) {
            self.agents.await_human(&self.state, role, why);
        }
    }

    /// Notify the human of a blocked or stalled agent once; what they have been told now.
    fn tell_human(
        &self,
        role: &str,
        activity: Activity,
        stalled: bool,
        told: Option<&'static str>,
        file: &str,
    ) -> Option<&'static str> {
        if activity == Activity::Blocked && told != Some("blocked") {
            self.ask_human(role, "needs your answer");
            return Some("blocked");
        }
        if stalled && told != Some("stalled") {
            self.ask_human(role, &format!("is idle without writing {file}"));
            return Some("stalled");
        }
        if activity == Activity::Working {
            None
        } else {
            told
        }
    }

    /// The error that ends the run over a handoff file the role wrote wrong.
    ///
    /// Resume does not judge a role's output; the human fixes the file or deletes it, and a deleted file is asked
    /// for again, so the prompt no longer counts as delivered, and neither does a retry or a block: the deleted file
    /// is asked for with the step's own prompt.
    pub(super) fn reject(&mut self, path: &str, why: &str) -> Stopped {
        self.state.prompted = None;
        self.state.retrying = None;
        self.state.blocked = None;
        Stopped::Failed(OrchestratorError(format!(
            "{path} {why}; fix it, or delete it to have it written again, then resume"
        )))
    }

    /// The index in RunState.turns of the record of the turn under way that writes file; None when there is none.
    pub(super) fn open_turn(&self, file: &str) -> Option<usize> {
        self.state.turns.iter().position(|t| t.file == file && t.under_way())
    }

    /// An open record of the current phase's turn that writes file: the current step's, or with no step of the phase,
    /// a quality analysis, which runs no agent. started is None when the turn's start is not known.
    pub(super) fn turn_record(&self, role: &str, file: &str, started: Option<i64>) -> TurnRecord {
        let s = &self.state;
        let step = self.pipeline.step(&s.phase);
        let (harness, model, unoffered_model, session) = match step {
            None => (None, None, None, None),
            Some(_) => {
                let agent = s.agents.get(role);
                let ran = s.model_ran_on(role);
                let (model, unoffered) = (ran.model.map(String::from), ran.unoffered.map(String::from));
                (Some(s.harness(role).to_string()), model, unoffered, agent.and_then(|a| a.session.clone()))
            }
        };
        let blocked = s.blocked.as_ref().is_some_and(|b| file_of(b) == Some(file));
        TurnRecord {
            step: s.phase.clone(),
            role: role.to_string(),
            round: s.round,
            quality_round: if step.is_none_or(|st| st.quality_gated) { s.quality_round } else { 0 },
            file: file.to_string(),
            harness,
            model,
            unoffered_model,
            session,
            tokens: Value::Null,
            human_paced: step.is_some_and(|st| st.human_paced) || blocked,
            started_at: started.map(iso_utc),
            ended_at: None,
            seconds: None,
            waited_seconds: None,
            outside_wait_seconds: None,
            waiting_on: None,
            unknown: Object::new(),
        }
    }

    /// Close the record of the turn that wrote path, found written, at the file's mtime: the turn ended then, even
    /// if no orchestrator was watching. A file whose turn has no record, as one written before turns were recorded,
    /// gets one with no start.
    ///
    /// An mtime before the turn's start counts as the start, so that a turn never ends before it started nor lasts
    /// less than 0s. The two do not come from one clock: the start is `date`'s reading, taken before the prompt went
    /// out, while Linux stamps an mtime from a coarser clock that trails it by up to a few milliseconds, so a file
    /// written that soon after a start read at the turn of a second is stamped in the second before.
    pub(super) fn end_turn(&mut self, role: &str, path: &str) -> Outcome<()> {
        let file = file_name(path);
        let i = match self.open_turn(file) {
            Some(i) => i,
            // Closed already, by an orchestrator that stopped before the run moved on.
            None if self.state.turns.iter().any(|t| t.file == file) => return Ok(()),
            None => {
                let record = self.turn_record(role, file, None);
                self.state.turns.push(record);
                self.state.turns.len() - 1
            }
        };
        let session = self.state.agents.get(role).and_then(|a| a.session.clone());
        let record = &mut self.state.turns[i];
        if record.session != session {
            record.session = None;
        }
        let started = record
            .started_at
            .as_deref()
            .map(epoch_seconds)
            .transpose()
            .map_err(|e| OrchestratorError(format!("the turn that wrote {file} started at no time: {e}")))?;
        let mtime = self.host.mtime(path)?;
        let ended = started.map_or(mtime, |started| mtime.max(started));
        let waited = match started {
            Some(started) if self.state.turns[i].harness.is_some() => self.waited_seconds(role, started, ended),
            _ => None,
        };
        let record = &mut self.state.turns[i];
        record.end_wait(ended)?;
        record.ended_at = Some(iso_utc(ended));
        record.seconds = started.map(|started| ended - started);
        record.waited_seconds = waited;
        self.save()
    }

    /// The role's agent's waits for a permission's answer from started to ended (since the epoch), as
    /// Agents::waited_seconds gives them: None also where its record cannot be read, which is logged.
    fn waited_seconds(&self, role: &str, started: i64, ended: i64) -> Option<i64> {
        self.agents.waited_seconds(&self.state, role, started, ended).unwrap_or_else(|e| {
            self.human.log(&format!("[{role}] could not read its journal for the turn's waits: {e}"));
            None
        })
    }

    /// Make sure the role's agent is running, and say how: as `alive` finds it, else started again.
    fn agent(&mut self, role: &str) -> Outcome<How> {
        let known = self.state.agents.get(role).cloned();
        if let Some(known) = &known {
            if let Some(how) = self.alive(role, known)? {
                return Ok(how);
            }
        }
        let next = self.next_harness(role)?;
        let order = self.shown_order(role);
        let record = self.agents.next_record(&self.state, role, &order, &next)?;
        let session = known.as_ref().and_then(|a| a.session.clone()).filter(|s| !s.is_empty());
        let ran = self.state.harness(role).to_string();
        if let Some(session) = &session {
            if ran != next {
                self.human.log(&format!(
                    "[{role}] session {session} is {ran}'s, and the role runs in {next} now; starting a fresh one"
                ));
            } else {
                // Keeps the conversation: for the Spec Collector that is the interview itself.
                match self.start(role, record.clone(), AgentStart::Resumed) {
                    Ok(true) => return Ok(How::Started(AgentStart::Resumed)),
                    Ok(false) => {}
                    Err(StartFailure::Refused(e)) => {
                        self.human.log(&format!("[{role}] could not resume session {session}: {e}"));
                    }
                    Err(StartFailure::Stopped(e)) => return Err(e),
                }
                self.human.log(&format!("[{role}] session {session} is gone; starting a fresh one"));
            }
        }
        // A role whose last agent never took a prompt, nor got past its startup, lost no conversation with it.
        let lost = known.is_some_and(|k| k.unprompted != Some(AgentStart::New) || k.past_startup);
        let start = if lost { AgentStart::Restarted } else { AgentStart::New };
        self.start(role, record, start)?;
        Ok(How::Started(start))
    }

    /// How the role's agent of the record known is ready, when its backend finds it alive; None when it has exited.
    ///
    /// An agent found alive has the session its backend reports for it saved, when that is not the one its record
    /// holds, so the turn recorded next is in it and its tokens can be counted. The record holds none when an
    /// orchestrator stopped between `start`'s two saves, and an older one when the agent moved to a new session (a
    /// human's `/clear` in its pane, say) while no orchestrator watched it, `await_handoff` noting sessions only while
    /// one does. An agent marked as not yet prompted (AgentRecord::unprompted) is prompted as the start that launched
    /// it would have, once the human has answered what it is blocked on (wait_out_block), unless it exits there; found
    /// working, it took a prompt the stopped orchestrator sent but did not record, and goes on as one found alive
    /// (waited on if prompted already names its file, else sent its prompt once more, the duplicate `written`
    /// accepts).
    fn alive(&mut self, role: &str, known: &AgentRecord) -> Outcome<Option<How>> {
        let mut found = self.agents.find(&self.state, role)?;
        let mut how = known.unprompted.map_or(How::Alive, How::Started);
        if known.unprompted.is_some() {
            match found.as_ref().map(|seen| seen.activity) {
                Some(Activity::Blocked) => {
                    self.wait_out_block(role)?;
                    found = self.agents.find(&self.state, role)?;
                }
                Some(Activity::Working) => {
                    how = How::Alive;
                    self.clear_unprompted(role);
                    self.save()?;
                }
                _ => {}
            }
        }
        let Some(seen) = found else { return Ok(None) };
        self.note_session(role, &seen)?;
        Ok(Some(how))
    }

    /// The roles in the order the human sees them (Agents::next_record), the first in the run's first place.
    ///
    /// That is the workflow's order, unless the run skipped its contract step (run --spec): then the first role to
    /// take a turn leads, and the contract's role, which may still have a later step, comes last. RunState.agents
    /// keeps the order the roles' agents first started in, and the role asking now is the first when none has.
    fn shown_order(&self, role: &str) -> Vec<String> {
        let roles: Vec<String> = self.pipeline.roles().keys().map(String::from).collect();
        let first = self.state.agents.keys().next().map_or(role, String::as_str).to_string();
        let Some(contract) = self.pipeline.contract().map(|c| c.role.clone()).filter(|c| *c != first) else {
            return roles;
        };
        let mut order = vec![first.clone()];
        order.extend(roles.into_iter().filter(|r| *r != first && *r != contract));
        order.push(contract);
        order
    }

    /// Start the role's agent, of record (Agents::next_record's), as start says: Resumed relaunches it into the session
    /// the role's saved record holds, which must hold one. False when it exited at once.
    ///
    /// The agent's record is saved, in the session it resumes and marked with start until its first prompt is taken
    /// (AgentRecord::unprompted), before its backend starts it; so an orchestrator stopped while the backend or the
    /// human is past a startup dialog leaves an agent the resume finds, waits for and prompts, never one unknown to
    /// the run.
    ///
    /// An agent relaunched into its session and asked for the same model again keeps the model it took in place of that
    /// one (AgentRecord::unoffered_model) when the reopened session does not say which it runs on; any other start
    /// records only a substitute its session names, and none when the session says nothing.
    fn start(&mut self, role: &str, mut record: AgentRecord, start: AgentStart) -> Result<bool, StartFailure> {
        let earlier = self.state.agents.get(role);
        let (session, unoffered_model) = match start {
            AgentStart::Resumed => {
                let asked = self.state.models.get(role);
                let unoffered = earlier.and_then(|a| a.unoffered_model.clone()).filter(|u| Some(&u.asked) == asked);
                (earlier.and_then(|a| a.session.clone()), unoffered)
            }
            AgentStart::New | AgentStart::Restarted => (None, None),
        };
        let h = harness(&self.next_harness(role)?)?;
        record.session.clone_from(&session);
        record.unprompted = Some(start);
        record.unoffered_model = unoffered_model;
        record.model = Some(self.state.models.get(role).cloned().unwrap_or_default());
        self.state.agents.insert(role.to_string(), record);
        self.save()?;
        let label = self.label(role);
        let s = &self.state;
        // The handoff files are outside a worktree run's working directory.
        let extra_dir = s.worktree.as_ref().map(|_| s.dir());
        let launch = AgentLaunch {
            label: &label,
            harness: h.kind,
            permission_mode: s.permission_mode.as_deref(),
            model: s.agents.get(role).and_then(|a| a.model.as_deref()).filter(|m| !m.is_empty()),
            session: session.as_deref(),
            extra_dir: extra_dir.as_deref(),
        };
        let started = self.agents.start(s, role, &launch)?;
        if let Some(agent) = self.state.agents.get_mut(role) {
            match started.model_said {
                LaunchedModel::Given => agent.unoffered_model = None,
                LaunchedModel::Unoffered(unoffered) => agent.unoffered_model = Some(unoffered),
                LaunchedModel::Unknown => {}
            }
        }
        for line in started.unpassed {
            if self.told_unpassed.insert((role.to_string(), line.clone())) {
                self.human.log(&format!("[{role}] {line}"));
            }
        }
        if started.asking {
            self.wait_out_block(role)?;
        }
        let Some(seen) = self.agents.find(&self.state, role)? else { return Ok(false) };
        self.note_session(role, &seen)?;
        Ok(true)
    }

    /// Ask the human to answer what the role's agent is blocked on, then wait until it is answered: the agent idle,
    /// or exited, as when the human refuses what it asks. The role must have an agent.
    ///
    /// The alert names a startup dialog, unless the agent is past its startup (AgentRecord::past_startup): then it is a
    /// question about what it was last given, by the run or the human. Both say where to answer (Agents::whereabouts).
    /// The human decides how long that takes, as with the interview, so there is no deadline; the wait is cut into
    /// steps so the heartbeat keeps the run from looking stale.
    fn wait_out_block(&mut self, role: &str) -> Result<(), StartFailure> {
        let agent = self.state.agents.get(role).ok_or_else(|| OrchestratorError(format!("no agent of {role}")))?;
        let place = self.whereabouts(role);
        if agent.past_startup {
            self.ask_human(role, "needs your answer");
            self.human.log(&format!(
                "[{role}] it is asking about what it was last given, or what was sent to it; answer it in {place}, and \
                 the run goes on"
            ));
        } else {
            self.ask_human(role, "needs your answer before it starts");
            self.human.log(&format!(
                "[{role}] a harness may ask something before it starts, such as Claude Code whether to trust a folder \
                 it has not run in; answer it in {place}, and the run goes on"
            ));
        }
        while !self.agents.wait_ready(&self.state, role, HEARTBEAT_SECONDS)? {
            self.heartbeat()?;
        }
        Ok(())
    }

    /// Save the session the role's agent's backend reports it in as soon as it does, so an agent that exits later can
    /// be resumed into it.
    fn note_session(&mut self, role: &str, seen: &Seen) -> Outcome<()> {
        let Some(session) = seen.session.as_deref() else { return Ok(()) };
        let agent = self.state.agents.get_mut(role).ok_or_else(|| OrchestratorError(format!("no agent of {role}")))?;
        if agent.session.as_deref() != Some(session) {
            agent.session = Some(session.to_string());
            self.save()?;
        }
        Ok(())
    }

    /// Tell the human the role's agent is in a state that needs them, saying where it is.
    pub(super) fn ask_human(&self, role: &str, what: &str) {
        let place = self.whereabouts(role);
        self.human.notify(&format!("{} {what}", self.label(role)), &place);
        self.human.log(&format!("[{role}] {what} ({place})"));
    }

    /// Where the human finds the role's agent (Agents::whereabouts).
    fn whereabouts(&self, role: &str) -> String {
        self.agents.whereabouts(&self.state, role)
    }

    /// The harness the role's next agent starts in, which a resume's --role-agent may have made another than the one
    /// its agent runs in (RunState::harness): it decides what starts, the other what ran. Errors: the state names
    /// none, which Workflow::new rules out.
    fn next_harness(&self, role: &str) -> Result<String, OrchestratorError> {
        let next = self.state.next_harnesses.get(role).cloned();
        next.ok_or_else(|| OrchestratorError(format!("no harness for role {role}")))
    }

    /// The role's label, as the workflow names it to the human.
    pub(super) fn label(&self, role: &str) -> String {
        self.pipeline.roles().get(role).cloned().unwrap_or_else(|| role.to_string())
    }
}

/// Whether the turn that writes the handoff file named file (a basename) has started, as state records it: its prompt
/// was delivered, it is being asked again for its verdict, or it waits on the human's answer to its BLOCKED report. A
/// turn with none of these has not been prompted, so has not yet seen the change.
pub(super) fn turn_started(state: &RunState, file: &str) -> bool {
    [state.prompted.as_deref(), state.retrying.as_deref(), state.blocked.as_ref().and_then(file_of)]
        .contains(&Some(file))
}

/// The file the record of a block (RunState::blocked) is of; None for a record without one.
fn file_of(record: &Object) -> Option<&str> {
    record.get("file").and_then(Value::as_str)
}

/// Where a BLOCKED report of the handoff file at path is moved the k-th time its turn blocks.
fn blocked_path(path: &str, k: i64) -> String {
    let (stem, ext) = split_extension(path);
    format!("{stem}.blocked-{k}{ext}")
}

/// The k of a block, RunState::blocked: how many times its turn has blocked, its report's blocked name's number.
/// Errors: as block_field's, and for a k that is no integer.
fn block_number(block: &Object) -> Result<i64, OrchestratorError> {
    let k = block_field(block, "k")?;
    k.as_i64().ok_or_else(|| OrchestratorError(format!("the block saved in state.json has k {k}, no number")))
}

/// The field key of a block, RunState::blocked. Errors: the record lacks it, as only a state.json edited by hand
/// does.
fn block_field<'b>(block: &'b Object, key: &str) -> Result<&'b Value, OrchestratorError> {
    block.get(key).ok_or_else(|| OrchestratorError(format!("the block saved in state.json has no {key}")))
}

/// A turn's time limit, read by Clocks::monotonic, which stands still while its role reports a wait on something
/// outside the run (Workflow::await_handoff).
struct TurnClock {
    /// The seconds the turn may take; None for no limit.
    timeout: Option<u64>,
    /// When the turn's time runs out; None for no limit.
    deadline: Option<f64>,
    /// The reading at the last poll, or at the start before any.
    last_poll: f64,
}

impl TurnClock {
    /// The clock of a turn of timeout seconds (None: no limit) that starts at now.
    fn new(timeout: Option<u64>, now: f64) -> TurnClock {
        TurnClock { timeout, deadline: timeout.map(|t| now + t as f64), last_poll: now }
    }

    /// Poll at now, with waiting saying whether the role's wait stood at this poll: the time since the last poll is
    /// then added to the deadline whole, though the wait may be younger, a poll's worth of the turn's time at most.
    /// The turn's timeout once its time has run out; None while it has not, or for a turn without a limit.
    fn run_out(&mut self, now: f64, waiting: bool) -> Option<u64> {
        if waiting {
            self.deadline = self.deadline.map(|d| d + (now - self.last_poll));
        }
        self.last_poll = now;
        self.timeout.filter(|_| self.deadline.is_some_and(|d| now > d))
    }
}

/// Where the role whose handoff file is at path reports, while its turn goes on, that it waits on something outside
/// the run (WAITING_NOTE): the file is the report, its text what the role waits on, and the role deletes it once it
/// goes back to work. The turn's end deletes one left behind (Workflow::written).
pub(super) fn waiting_path(path: &str) -> String {
    let (stem, _) = split_extension(path);
    format!("{stem}.waiting")
}

/// Where a malformed verdict file is moved, for its retry.
fn rejected_path(path: &str) -> String {
    let (stem, ext) = split_extension(path);
    format!("{stem}.rejected{ext}")
}
