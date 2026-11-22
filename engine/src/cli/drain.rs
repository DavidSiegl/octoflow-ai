//! `drain`: run each ready spec in a directory, or each ready ticket of a ready spec that has tickets, one after
//! another, each as run --spec runs it, recording each run and how it ended in its header. First it follows the pull
//! requests and runs that earlier drains left behind (refreshed_headers). The specs and tickets, and their headers, are
//! crate::cli::drain::queue's.

use std::collections::HashMap;

use super::args::DrainArgs;
use super::refresh::{has_conflict, refresh_conflict};
use super::start::{flagged_workflow, new_state};
use super::{finish, hint_commands, prune, record_in_state, resume_command, Connected, Ended, Stop};
use crate::constants::{EXIT_ERROR, EXIT_WAITING, RUNS_DIR, SUCCEEDED};
use crate::error::OrchestratorError;
use crate::prompts::load_prompts;
use crate::runs::RUNNING;
use crate::state::{RunState, SavedRun};
use crate::workflow::Stopped;

pub mod queue;

use queue::{entry, merge, read_queue, status_word, Entries, Kind, Queued, PR_KEY, RUN_KEY, STATUS_KEY};

/// Where drain looks for specs when given no DIR, relative to the current directory.
pub const DRAIN_DIR: &str = ".scratch";
/// The values of a spec's Status: line that drain acts on. A spec moves from SPEC_READY to SPEC_IN_REVIEW, its pull
/// request open, to SPEC_DONE, merged; or to SPEC_FOR_HUMAN, for a run that failed, did not succeed, conflicts, or
/// opened no pull request (RunState::nothing_to_publish), which drain leaves only for SPEC_IN_REVIEW, once a succeeded
/// run's conflict is cleared (refreshed_headers). Drain leaves a spec with any other value, or none, alone.
pub const SPEC_READY: &str = "ready-for-agent";
pub const SPEC_IN_REVIEW: &str = "in-review";
pub const SPEC_DONE: &str = "done";
pub const SPEC_FOR_HUMAN: &str = "ready-for-human";
/// A ticket moves as a spec does, through the same values, but to TICKET_RESOLVED, written `resolved (PR #<n>
/// merged)`, in place of SPEC_DONE (pull_request_header). Its spec turns SPEC_DONE once each of its tickets is
/// TICKET_RESOLVED or TICKET_WONTFIX (Kind::Ticketed).
pub const TICKET_RESOLVED: &str = "resolved";
pub const TICKET_WONTFIX: &str = "wontfix";

/// Run the specs and tickets in args.dir that are ready (queue::read_queue), one after another, recording each in its
/// header, in world on the project at cwd, and say how drain ended: with exit status 0 once none is SPEC_READY,
/// EXIT_WAITING when those left are blocked or gone (drain_end), or the status of the run that stopped the drain. With
/// args.dry_run, print what it would change and run, and change nothing. Prints the header changes it made, however it
/// ended.
///
/// Errors (Stop::Failed), before any run starts: the workflow cannot take a spec (Pipeline::seeded_start), and as
/// read_queue and refreshed_headers fail, and a taken-up run's state that changed since the pass read it or cannot be
/// written (forget_cleared); at any point, a spec header that cannot be written, or a run whose state cannot be made
/// (new_state). A Ctrl-C ends it interrupted, with the run it stopped.
pub(super) fn drain_command(args: &DrainArgs, world: &Connected, cwd: &str) -> Result<Ended, Stop> {
    let interrupt = world.with.interrupt;
    let stopped = |e: OrchestratorError| super::stop(e, interrupt, None);
    let prompts = load_prompts(world.config).map_err(stopped)?;
    flagged_workflow(&args.run, &prompts, world).and_then(|p| p.seeded_start().map(drop)).map_err(stopped)?;
    let mut queue = read_queue(&args.dir).map_err(stopped)?;
    let target = args.run.target.flags();
    let refreshed = refreshed_headers(&queue, world, cwd, &target, args.dry_run).map_err(stopped)?;
    let mut unwritten = vec![];
    for (i, entries) in refreshed.headers {
        if !queue[i].update(&entries, !args.dry_run, world.with.human).map_err(stopped)? {
            unwritten.push(i);
        }
    }
    // Only for a spec whose header took it up, or had nothing to change, so a drain that fails before, or a spec found
    // gone, keeps the record for the next drain.
    if !args.dry_run {
        let taken: Vec<RunState> =
            refreshed.taken.into_iter().filter(|(i, _)| !unwritten.contains(i)).map(|(_, state)| state).collect();
        forget_cleared(world, cwd, &taken).map_err(stopped)?;
    }
    let by_slug: HashMap<String, usize> = queue.iter().enumerate().map(|(i, s)| (s.slug.clone(), i)).collect();
    if args.dry_run {
        print_changes(&queue, world);
        let order: Vec<&str> =
            queue.iter().filter(|s| startable(s, &queue, &by_slug)).map(|s| s.slug.as_str()).collect();
        world.say(&if order.is_empty() {
            "would run no spec".to_string()
        } else {
            format!("would run, in order: {}", order.join(", "))
        });
        return if interrupt.is_raised() { Err(Stop::Interrupted(None)) } else { Ok(Ended::Status(0)) };
    }
    prune::sweep_branches(world, cwd).map_err(stopped)?;
    let drained = drain_ready(args, world, cwd, &mut queue, &by_slug);
    // The changes are printed whether the drain ended or failed.
    print_changes(&queue, world);
    let status = match drained? {
        Some(status) => status,
        None => drain_end(&queue, &by_slug, world),
    };
    if interrupt.is_raised() {
        return Err(Stop::Interrupted(None));
    }
    Ok(Ended::Status(status))
}

/// Run each of queue that is startable, in turn, until none is (None) or one stops the drain (its exit status), as
/// drain_one runs it; by_slug indexes queue by slug. Errors: as drain_one's, and the interrupt raised between runs
/// (Stop::Interrupted, no run named).
fn drain_ready(
    args: &DrainArgs,
    world: &Connected,
    cwd: &str,
    queue: &mut [Queued],
    by_slug: &HashMap<String, usize>,
) -> Result<Option<i32>, Stop> {
    loop {
        // A Ctrl-C between runs, which no run took.
        if world.with.interrupt.is_raised() {
            return Err(Stop::Interrupted(None));
        }
        let Some(ready) = next_startable(queue, by_slug, world) else { return Ok(None) };
        if let Some(status) = drain_one(args, world, cwd, &mut queue[ready])? {
            return Ok(Some(status));
        }
    }
}

/// Whether drain runs it: a spec it runs whole, or a ticket whose spec is still SPEC_READY, that is SPEC_READY and
/// waits for nothing unfinished among queue, by by_slug (waits_for, Queued::finished).
fn startable(item: &Queued, queue: &[Queued], by_slug: &HashMap<String, usize>) -> bool {
    let runs = match &item.kind {
        Kind::Spec => true,
        Kind::Ticketed => false,
        Kind::Ticket(spec) => queue[by_slug[spec]].status().as_deref() == Some(SPEC_READY),
    };
    runs && item.status().as_deref() == Some(SPEC_READY)
        && waits_for(item, queue, by_slug).into_iter().all(|b| queue[by_slug[b]].finished())
}

/// The slugs, among queue by by_slug, of what must be finished before it starts: its blockers, then a ticket's spec's.
fn waits_for<'q>(item: &'q Queued, queue: &'q [Queued], by_slug: &HashMap<String, usize>) -> Vec<&'q str> {
    let of_spec = match &item.kind {
        Kind::Ticket(spec) => queue[by_slug[spec]].blockers.as_slice(),
        _ => &[],
    };
    item.blockers.iter().chain(of_spec).map(String::as_str).collect()
}

/// The index of the first of queue drain runs next (startable), each read again from disk first, so a status the
/// human changed while an earlier one ran counts; None when there is none. Logs each that has become gone.
fn next_startable(queue: &mut [Queued], by_slug: &HashMap<String, usize>, world: &Connected) -> Option<usize> {
    for spec in queue.iter_mut() {
        let was_gone = spec.gone.is_some();
        if !spec.reread() && !was_gone {
            let what = if spec.status_as_read().as_deref() == Some(SPEC_READY) { "not running it" } else { "gone" };
            let gone = spec.gone.as_deref().unwrap_or_default();
            world.with.human.log(&format!("{}: {gone}; {what}", spec.slug));
        }
    }
    queue.iter().position(|s| startable(s, queue, by_slug))
}

/// Run the spec or ticket as run --spec would with args's flags, and record the run and how it ended in its header.
///
/// Returns None when the run succeeded without conflicts, and the spec is SPEC_IN_REVIEW, or SPEC_FOR_HUMAN for a run
/// that opened no pull request (SPEC_READY's doc), or gone; or when the spec is found gone before the run starts,
/// which it then does not; otherwise the exit status that stops the drain, having
/// printed why on standard error. A header write to a gone spec is logged instead (Queued::update). Errors: the
/// run's state cannot be made (new_state), or the spec cannot be written (Stop::Failed); the run was interrupted
/// (Stop::Interrupted, with its id).
fn drain_one(args: &DrainArgs, world: &Connected, cwd: &str, spec: &mut Queued) -> Result<Option<i32>, Stop> {
    let interrupt = world.with.interrupt;
    let human = world.with.human;
    let given = spec.file.clone();
    let (state, pipeline) =
        new_state(&args.run, cwd, Some(&given), world).map_err(|e| super::stop(e, interrupt, None))?;
    let run_id = state.run_id.clone();
    let stopped = |e: OrchestratorError| super::stop(e, interrupt, Some(&run_id));
    if !spec.update(&vec![(RUN_KEY, run_id.clone())], true, human).map_err(stopped)? {
        return Ok(None);
    }
    match (world.workflow)(world, state, pipeline, Some(given), &hint_commands(&args.run.target.flags())) {
        Err(Stopped::TakenOver(e)) => {
            // Another orchestrator drives the run now; the next drain finds it by the Run: line and records its end.
            world.say_err(&format!("error: {e}"));
            world.say_err(&format!("{} stays {SPEC_READY}; run drain again once that run has ended", spec.slug));
            Ok(Some(EXIT_ERROR))
        }
        Err(Stopped::Failed(_)) | Err(Stopped::Interrupted) if interrupt.is_raised() => {
            Err(Stop::Interrupted(Some(run_id)))
        }
        Err(Stopped::Interrupted) => Err(Stop::Interrupted(Some(run_id))),
        Err(Stopped::Failed(e)) => {
            // Printed first, so the run's error and its resume command survive a header write that fails too.
            world.say_err(&format!("error: {e}"));
            world.say_err(&format!("resume its run with: {}", resume_command(&run_id, &args.run.target.flags())));
            if spec.update(&vec![(STATUS_KEY, SPEC_FOR_HUMAN.to_string())], true, human).map_err(stopped)? {
                world.say_err(&format!("{} is {SPEC_FOR_HUMAN}", spec.slug));
            }
            Ok(Some(EXIT_ERROR))
        }
        Ok(ran) => {
            let status = finish(ran.verdict.as_deref(), &ran.state, &ran.pipeline, world.stdout);
            let pr = ran.state.pr_url.as_ref().filter(|u| !u.is_empty());
            // In review only through a pull request (SPEC_READY's doc).
            let in_review = status == 0 && pr.is_some();
            let mut entries: Entries =
                vec![(STATUS_KEY, if in_review { SPEC_IN_REVIEW } else { SPEC_FOR_HUMAN }.into())];
            if let Some(pr) = pr {
                entries.push((PR_KEY, pr.clone()));
            }
            spec.update(&entries, true, human).map_err(stopped)?;
            Ok((status != 0).then_some(status))
        }
    }
}

/// Print the change_line of each of queue that drain changed, in their order.
fn print_changes(queue: &[Queued], world: &Connected) {
    for spec in queue.iter().filter(|s| s.changed) {
        world.say(&spec.change_line());
    }
}

/// Print what is left once drain can start nothing, and return its exit status: 0 when no spec it runs whole, and no
/// ticket, was SPEC_READY as drain last read it, else EXIT_WAITING. Such a one is left blocked, or gone; a ticket also
/// waits while its spec is no longer SPEC_READY.
fn drain_end(queue: &[Queued], by_slug: &HashMap<String, usize>, world: &Connected) -> i32 {
    let left: Vec<&Queued> = queue
        .iter()
        .filter(|s| s.kind != Kind::Ticketed && s.status_as_read().as_deref() == Some(SPEC_READY))
        .collect();
    if left.is_empty() {
        world.say("all specs cleared");
        return 0;
    }
    // A blocker's line: its status and pull request, or why it is gone.
    let say_blocker = |slug: &str| {
        let blocker = &queue[by_slug[slug]];
        if let Some(gone) = &blocker.gone {
            world.say(&format!("    {slug}: gone ({gone})"));
        } else {
            let status = queue::status_text(blocker.status());
            let pr = blocker.pr().filter(|p| !p.is_empty()).map(|p| format!("  {p}")).unwrap_or_default();
            world.say(&format!("    {slug}: {status}{pr}"));
        }
    };
    for item in left {
        if let Some(gone) = &item.gone {
            world.say(&format!("{}: gone ({gone})", item.slug));
            continue;
        }
        world.say(&format!("{} waits on:", item.slug));
        if let Kind::Ticket(spec) = &item.kind {
            if queue[by_slug[spec]].status().as_deref() != Some(SPEC_READY) {
                say_blocker(spec);
            }
        }
        for b in waits_for(item, queue, by_slug) {
            say_blocker(b);
        }
    }
    EXIT_WAITING
}

/// What has become of the pull requests and runs that earlier drains left behind (Refreshed): the header lines to set,
/// each of queue followed to where it settles in this pass (settled_header), so that a pull request already merged or
/// closed frees its dependants in this drain, and a Ticketed spec SPEC_DONE once its tickets are (Kind::Ticketed); and
/// the runs whose cleared conflict this pass took up. It
/// refreshes a run's conflict and logs what refresh_conflict did; with dry_run, it does nothing of that, and gives the
/// header lines as though it had. It changes no run's conflict_cleared: the caller does, with forget_cleared.
///
/// cwd is the project, where gh asks about pull requests and the runs are, on world's host; target is the --machine
/// and --cwd flags as given, for a resume command. Errors: as Host::pull_request, refresh_conflict and
/// finished_run_header.
pub fn refreshed_headers(
    queue: &[Queued],
    world: &Connected,
    cwd: &str,
    target: &[String],
    dry_run: bool,
) -> Result<Refreshed, OrchestratorError> {
    let statuses = [SPEC_READY, SPEC_IN_REVIEW, SPEC_FOR_HUMAN];
    // Read once, and only when a spec's run is to be looked at.
    let looked_at = |s: &Queued| {
        s.run().is_some_and(|r| !r.is_empty()) && s.status().is_some_and(|st| statuses.contains(&st.as_str()))
    };
    let runs = if queue.iter().any(looked_at) { world.with.host.run_states(cwd)? } else { vec![] };
    let mut prs = PullRequestStates { world, cwd, states: HashMap::new() };
    let mut refreshed = Refreshed { headers: vec![], taken: vec![] };
    for (i, spec) in queue.iter().enumerate() {
        let mut taken = vec![];
        let entries = settled_header(spec, &runs, &mut prs, &mut taken, target, dry_run)?;
        refreshed.taken.extend(taken.into_iter().map(|state| (i, state)));
        if !entries.is_empty() {
            refreshed.headers.push((i, entries));
        }
    }
    // Each ticket's status as this pass leaves it.
    let settled = |i: usize| {
        let set = refreshed.headers.iter().find(|(at, _)| *at == i).and_then(|(_, e)| entry(e, STATUS_KEY));
        set.map(|s| status_word(s).to_string()).or_else(|| queue[i].status())
    };
    let mut done = vec![];
    for (i, spec) in queue.iter().enumerate() {
        if spec.kind != Kind::Ticketed || spec.status().as_deref() != Some(SPEC_READY) {
            continue;
        }
        let ticket = Kind::Ticket(spec.slug.clone());
        let mut tickets = (0..queue.len()).filter(|&t| queue[t].kind == ticket);
        if tickets.all(|t| settled(t).is_some_and(|s| [TICKET_RESOLVED, TICKET_WONTFIX].contains(&s.as_str()))) {
            done.push((i, vec![(STATUS_KEY, SPEC_DONE.to_string())]));
        }
    }
    refreshed.headers.extend(done);
    Ok(refreshed)
}

/// What refreshed_headers found in one pass.
pub struct Refreshed {
    /// The header lines to set, by the index of their spec or ticket among the queue, one pair for each that has any.
    pub headers: Vec<(usize, Entries)>,
    /// The saved states, as read, of the runs whose conflict_cleared the pass took up (cleared_conflict), each by the
    /// index of the spec that took it up, whose header lines, if any, are in headers.
    pub taken: Vec<(usize, RunState)>,
}

/// Remove conflict_cleared from the state of each run of taken, as refreshed_headers read it, so that no later drain
/// takes it up again (RunState::conflict_cleared); for the caller to do once the headers that took them up are
/// written, for the runs of specs whose header was written or had nothing to change. cwd is the project, on world's host. Errors, naming the run: as record_in_state.
pub fn forget_cleared(world: &Connected, cwd: &str, taken: &[RunState]) -> Result<(), OrchestratorError> {
    for state in taken {
        let run_id = &state.run_id;
        record_in_state(
            world.with.host,
            cwd,
            state,
            |s| s.conflict_cleared = None,
            "drain took up its cleared conflict",
        )
        .map_err(|e| OrchestratorError(format!("run {run_id}: {e}")))?;
    }
    Ok(())
}

/// The states gh gives pull requests in one refreshed_headers pass, OPEN, CLOSED or MERGED by URL: each asked of gh
/// once at most, or taken from what a conflict refresh was told (learn).
struct PullRequestStates<'w, 'a> {
    /// Whom to ask, and in which project, as refreshed_headers says.
    world: &'w Connected<'a>,
    cwd: &'w str,
    /// By URL, each state heard in this pass.
    states: HashMap<String, String>,
}

impl PullRequestStates<'_, '_> {
    /// Take state as the pull request's at url for the rest of the pass, as gh gave it another way.
    fn learn(&mut self, url: &str, state: &str) {
        self.states.insert(url.to_string(), state.to_string());
    }

    /// The pull request's state at url, if this pass has heard it; None, asking no one, if not.
    fn known(&self, url: &str) -> Option<&str> {
        self.states.get(url).map(String::as_str)
    }

    /// The pull request's state at url, asking gh unless this pass has heard it. Errors: as Host::pull_request.
    fn state(&mut self, url: &str) -> Result<String, OrchestratorError> {
        if let Some(state) = self.states.get(url) {
            return Ok(state.clone());
        }
        let state = self.world.with.host.pull_request(self.cwd, url)?.state;
        self.states.insert(url.to_string(), state.clone());
        Ok(state)
    }
}

/// The header lines to set for spec, empty for none, from these steps in this order, each on the status the one
/// before it left:
///
/// 1. a SPEC_IN_REVIEW or SPEC_FOR_HUMAN spec's run, its conflict refreshed, or found cleared by a refresh before
///    this drain (conflict_header), which can make a SPEC_FOR_HUMAN spec SPEC_IN_REVIEW with its pull request;
/// 2. a SPEC_READY spec's finished run on its Run: line, which an interrupted drain leaves (finished_run_header):
///    SPEC_IN_REVIEW with its pull request, or SPEC_FOR_HUMAN; a conflict a refresh cleared is taken up so, its run
///    added to taken;
/// 3. a spec now SPEC_IN_REVIEW with a pull request, or SPEC_FOR_HUMAN with one whose state step 1 heard, which it
///    hears only of a run whose conflict is refreshed or was cleared: SPEC_DONE once it is merged, SPEC_FOR_HUMAN
///    once it is closed (pull_request_header).
///
/// gh hears of each pull request once a pass (prs). runs are Host::run_states's; taken is Refreshed::taken, so far; the
/// rest is as refreshed_headers says. Errors: as those steps.
fn settled_header(
    spec: &Queued,
    runs: &[SavedRun],
    prs: &mut PullRequestStates,
    taken: &mut Vec<RunState>,
    target: &[String],
    dry_run: bool,
) -> Result<Entries, OrchestratorError> {
    let mut entries: Entries = vec![];
    let status = spec.status();
    let run = spec.run().filter(|r| !r.is_empty());
    match (status.as_deref(), run) {
        (Some(SPEC_IN_REVIEW | SPEC_FOR_HUMAN), _) => {
            merge(&mut entries, conflict_header(spec, runs, prs, taken, dry_run)?)
        }
        (Some(SPEC_READY), Some(run)) => {
            merge(&mut entries, finished_run_header(spec, &run, runs, prs, target)?);
            let state = saved_run(runs, &run).and_then(|r| r.state().ok());
            if let Some(state) = state.filter(|s| cleared_conflict(s).is_some()) {
                taken.push(state.clone());
            }
        }
        _ => {}
    }
    let status = entry(&entries, STATUS_KEY).map(String::from).or(status);
    let pr = entry(&entries, PR_KEY).map(String::from).or_else(|| spec.pr()).filter(|p| !p.is_empty());
    if let Some(pr) = pr {
        let follow = status.as_deref() == Some(SPEC_IN_REVIEW)
            || (status.as_deref() == Some(SPEC_FOR_HUMAN) && prs.known(&pr).is_some());
        if follow {
            let settled = pull_request_header(spec, &prs.state(&pr)?, &pr);
            if entry(&settled, STATUS_KEY).map(|s| status_word(s).to_string()).or_else(|| status.clone()) != status {
                merge(&mut entries, settled);
            }
        }
    }
    Ok(entries)
}

/// The run of runs whose id is run_id, readable or not; None when there is none.
fn saved_run<'r>(runs: &'r [SavedRun], run_id: &str) -> Option<&'r SavedRun> {
    runs.iter().find(|r| r.shown().run_id == run_id)
}

/// Refresh the run on the spec's Run: line, if it has_conflict, as refreshed_headers says, telling prs the state gh
/// gave its pull request, and return the header lines that sets: SPEC_IN_REVIEW and the pull request for a
/// SPEC_FOR_HUMAN spec whose succeeded run's pull request is open and no longer conflicts; none otherwise, and for a
/// run not among runs, Host::run_states's, or one whose state cannot be read, which is logged with why.
///
/// A run whose conflict a refresh before this drain cleared (cleared_conflict) counts as one this refresh cleared, its
/// pull request's state asked of prs, and is added to taken once gh has answered. If gh cannot answer, that is logged
/// and the spec left as it is, the record kept for the next drain: each drain asks again, and none fails for it.
fn conflict_header(
    spec: &Queued,
    runs: &[SavedRun],
    prs: &mut PullRequestStates,
    taken: &mut Vec<RunState>,
    dry_run: bool,
) -> Result<Entries, OrchestratorError> {
    let Some(run) = spec.run().and_then(|run| saved_run(runs, &run)) else { return Ok(vec![]) };
    let state = match run.state() {
        Ok(state) => state,
        Err(e) => {
            prs.world.with.human.log(&format!("{}: {e}; {} is left as it is", run.shown().run_id, spec.slug));
            return Ok(vec![]);
        }
    };
    // The pull request whose conflict is cleared now, while it is open; None when there is none.
    let cleared_open = if has_conflict(state) {
        let check = refresh_conflict(prs.world, prs.cwd, state, dry_run, false)?;
        if !dry_run {
            prs.world.with.human.log(&check.line());
        }
        prs.learn(&check.url, &check.pr_state);
        (check.pr_state == "OPEN" && check.clears()).then_some(check.url)
    } else if let Some(url) = cleared_conflict(state) {
        match prs.state(&url) {
            Ok(pr_state) => {
                taken.push(state.clone());
                (pr_state == "OPEN").then_some(url)
            }
            Err(e) => {
                let run_id = &state.run_id;
                prs.world.with.human.log(&format!(
                    "{run_id}: cannot ask gh about {url}, whose conflict refresh cleared ({e}); {} is left as it is \
                     until a drain can",
                    spec.slug
                ));
                None
            }
        }
    } else {
        None
    };
    let succeeded = state.verdict.as_deref().is_some_and(|v| SUCCEEDED.contains(&v));
    match cleared_open {
        Some(url) if spec.status().as_deref() == Some(SPEC_FOR_HUMAN) && succeeded => {
            Ok(vec![(STATUS_KEY, SPEC_IN_REVIEW.into()), (PR_KEY, url)])
        }
        _ => Ok(vec![]),
    }
}

/// The pull request whose conflict a refresh run by hand cleared for the run whose state is state, which no drain has
/// taken up yet (RunState::conflict_cleared); None for a run without one.
fn cleared_conflict(state: &RunState) -> Option<String> {
    state.conflict_cleared.clone().filter(|u| !u.is_empty())
}

/// The header lines to set for a spec or ticket whose pull request at url is in state, as gh gives it: for MERGED,
/// SPEC_DONE, or a ticket's TICKET_RESOLVED with the pull request's number; SPEC_FOR_HUMAN for CLOSED; none for OPEN.
fn pull_request_header(item: &Queued, state: &str, url: &str) -> Entries {
    match state {
        "MERGED" if matches!(item.kind, Kind::Ticket(_)) => {
            let number = url.trim_end_matches('/').rsplit('/').next().unwrap_or(url);
            vec![(STATUS_KEY, format!("{TICKET_RESOLVED} (PR #{number} merged)"))]
        }
        "MERGED" => vec![(STATUS_KEY, SPEC_DONE.into())],
        "CLOSED" => vec![(STATUS_KEY, SPEC_FOR_HUMAN.into())],
        _ => vec![],
    }
}

/// The header lines to set for the finished run run_id, on the spec's Run: line: SPEC_IN_REVIEW and its pull request
/// for one that succeeded without conflicts and opened one, SPEC_FOR_HUMAN otherwise (SPEC_READY's doc), with its
/// pull request if it has one.
///
/// runs are Host::run_states's. Errors: the run is not finished, being running, stale, failed, closed or not among
/// runs, or is done but cannot be read (SavedRun::problem); the message names the spec, the run and what to do about
/// it.
fn finished_run_header(
    spec: &Queued,
    run_id: &str,
    runs: &[SavedRun],
    prs: &PullRequestStates,
    target: &[String],
) -> Result<Entries, OrchestratorError> {
    let run = saved_run(runs, run_id);
    let Some(s) = run.and_then(|r| r.state().ok()).filter(|s| s.done()) else {
        let (why, remedy) = unfinished_run(spec, run_id, run, prs, target);
        return Err(OrchestratorError(format!(
            "spec {}: run {run_id}, on its {RUN_KEY}: line, {why}; {remedy}",
            spec.slug
        )));
    };
    let pr = s.pr_url.clone().filter(|p| !p.is_empty());
    let succeeded = s.verdict.as_deref().is_some_and(|v| SUCCEEDED.contains(&v));
    let conflicts = !s.conflicts.is_empty();
    match pr {
        Some(pr) if succeeded && !conflicts => Ok(vec![(STATUS_KEY, SPEC_IN_REVIEW.into()), (PR_KEY, pr)]),
        pr => Ok([(STATUS_KEY, SPEC_FOR_HUMAN.to_string())].into_iter().chain(pr.map(|pr| (PR_KEY, pr))).collect()),
    }
}

/// Why the run run_id on the spec's Run: line is not finished, and what the human can do about it, as two clauses for
/// finished_run_header's error: for a done run whose state cannot be read, why the human is to settle its spec instead.
/// run is the run as Host::run_states gives it, None when there is no such run; it must not be both readable and
/// done.
fn unfinished_run(
    spec: &Queued,
    run_id: &str,
    run: Option<&SavedRun>,
    prs: &PullRequestStates,
    target: &[String],
) -> (String, String) {
    let start_over = format!("remove the {RUN_KEY}: line from {} to start the spec over", spec.file.path);
    let Some(run) = run else {
        return (format!("is not under {}/{RUNS_DIR}", prs.cwd), start_over);
    };
    let s = run.shown();
    if let Some(closed) = s.closed_line() {
        return (format!("was closed ({closed})"), start_over);
    }
    let world = prs.world;
    let health = run.health(world.local_host, world.with.pid_alive);
    // A run still running is waited for whatever release drives it, since the remedy for a state this release cannot
    // read is removing its directory; resume refuses any other such run, and the problem says what to do instead.
    if let Some((RUNNING, why)) = health {
        return (format!("is {RUNNING} ({why})"), format!("wait for it to finish, or {start_over}"));
    }
    // Starting over would do a finished run's work twice, so its spec's header is the human's to set.
    if let Some(problem) = run.problem().filter(|_| s.done()) {
        let pr = s
            .pr_url
            .as_deref()
            .filter(|p| !p.is_empty())
            .map(|p| format!(" (its pull request: {p})"))
            .unwrap_or_default();
        return (
            format!("is done but cannot be read: {problem}"),
            format!("drain with that release, or set its {STATUS_KEY}: and {PR_KEY}: lines by hand{pr}"),
        );
    }
    if let Some(problem) = run.problem() {
        return (format!("cannot be resumed: {problem}"), start_over);
    }
    let resume = format!("resume it with `{}`, or {start_over}", resume_command(run_id, target));
    match health {
        // None for a run that is not done: it failed.
        None => (format!("failed: {}", s.error.as_deref().unwrap_or_default()), resume),
        Some((health, why)) => (format!("is {health} ({why})"), resume),
    }
}

#[cfg(test)]
mod tests;
