//! The proof step's proofs (Pipeline::proof_step, Pipeline::proof_command), run against the change: those of a
//! review that requests changes, then again each round until they pass, with what the review, the proof step and the
//! step it loops back to are told of them.

use super::{prompt_error, Outcome, Workflow};
use crate::constants::PROOF_COPIES_DIR;
use crate::error::OrchestratorError;
use crate::host::{checked, shlex_quote, DEFAULT_TIMEOUT, MISSING_FILE_STATUS};
use crate::paths::file_name;
use crate::pipeline::Step;
use crate::proofs::{
    proof_finding, proof_outcome, proof_output, proofs_section, ProofOutcome, ProofRun, PROOFS_HEADING, PROOF_TAIL_LINES,
    REMOVE_TREE_SCRIPT,
};
use crate::prompts::template::fill;
use crate::prompts::Prompt;

/// Runs "$2" with sh in the directory "$1", under GNU timeout's limit of "$3" seconds where there is a timeout: it runs
/// the proof in a process group of its own and kills all of it, so nothing a proof started goes on writing to the
/// tree; where there is none, Host::exec's limit stops only the shell's group. timeout exits 124 when its limit stops
/// the proof; the script exits 125 when it cannot enter "$1".
const PROOF_SCRIPT: &str =
    r#"cd -- "$1" || exit 125; command -v timeout >/dev/null 2>&1 && exec timeout -k 10 "$3" sh -c "$2"; eval "$2""#;

/// Empties the directory "$1" names (REMOVE_TREE_SCRIPT) and copies the directory "$3" into it as "$2". It exits with
/// MISSING_FILE_STATUS, copying nothing, when "$3" holds no file "$4": a proof file that is gone, as one the Builder
/// moved into the project is, is not run on a missing path. Emptied first: an orchestrator stopped during a proof
/// leaves its copy behind.
fn copy_script() -> String {
    format!(
        r#"[ -f "$3/$4" ] || exit {MISSING_FILE_STATUS}; {REMOVE_TREE_SCRIPT} && mkdir -p -- "$2" && cp -R -- "$3/." "$2""#
    )
}

impl Workflow<'_> {
    /// Run each proof the proof step wrote in round n with the workflow's proof_command, record which fail, and
    /// append their outcomes to its review, whose text is review. A resume runs none that were recorded, and appends
    /// them once. Errors: the proofs cannot be listed, saving or the review's write fails, or the Ctrl-C.
    pub(super) fn judge_proofs(&mut self, step: &Step, n: u32, review: &str) -> Outcome<()> {
        let Some(command) = self.pipeline.proof_command().map(String::from) else { return Ok(()) };
        let mut records: Vec<ProofRun> =
            self.proof_runs()?.into_iter().filter(|p| p.round == n && p.written == n).collect();
        if records.is_empty() {
            let mut files: Vec<String> = self
                .host
                .run_files(&self.state.proofs_dir(n))?
                .iter()
                .map(|f| file_name(&f.path).to_string())
                .collect();
            files.sort();
            for file in files {
                let ProofOutcome { demonstrated, said, moved } = self.run_proof(&command, n, &file)?;
                records.push(ProofRun { round: n, written: n, finding: proof_finding(&file), file, demonstrated, said, moved });
            }
            if records.is_empty() {
                return Ok(());
            }
            let failing = self.record_proof_runs(&records)?;
            self.human.log(&format!("[{}] {failing} of {} proofs fail against the change", step.role, records.len()));
        }
        if !review.split('\n').any(|line| line == PROOFS_HEADING) {
            let section = proofs_section(&command, self.state.work_dir(), review, &records);
            let judged = format!("{}\n\n{section}", review.trim_end_matches('\n'));
            self.host.write(&step.path(&self.state.dir(), n, 0), &judged, false)?;
        }
        Ok(())
    }

    /// Before the proof step's turn of round n, run each proof carried into it from an earlier round
    /// (carried_proofs) again against the change, and record the outcomes in RunState::proofs. A resume runs none
    /// once round n's reruns are recorded. Errors: as judge_proofs'.
    pub(super) fn rerun_proofs(&mut self, step: &Step, n: u32) -> Outcome<()> {
        let Some(command) = self.pipeline.proof_command().map(String::from) else { return Ok(()) };
        if !self.reruns(n)?.is_empty() {
            return Ok(());
        }
        let mut records = vec![];
        for p in self.carried_proofs(n)? {
            let ProofOutcome { demonstrated, said, moved } = self.run_proof(&command, p.written, &p.file)?;
            records.push(ProofRun { round: n, demonstrated, said, moved, ..p });
        }
        if records.is_empty() {
            return Ok(());
        }
        let failing = self.record_proof_runs(&records)?;
        self.human.log(&format!(
            "[{}] {failing} of {} proofs from earlier rounds still fail against the change",
            step.role,
            records.len()
        ));
        Ok(())
    }

    /// The latest run before round n of each proof carried into it, in the order the proofs first ran: one that
    /// failed when last run, or that a rerun found erroring rather than passing. A proof whose first run, in the round
    /// that wrote it, errored is not carried: it never demonstrated its finding. Nor is one whose last run found its
    /// file gone (ProofRun::moved): the Builder moved it into the project, which ends its carrying.
    fn carried_proofs(&self, n: u32) -> Result<Vec<ProofRun>, OrchestratorError> {
        let mut latest: Vec<ProofRun> = vec![];
        for p in self.proof_runs()?.into_iter().filter(|p| p.round < n) {
            match latest.iter_mut().find(|q| (q.written, &q.file) == (p.written, &p.file)) {
                Some(q) => *q = p,
                None => latest.push(p),
            }
        }
        Ok(latest.into_iter().filter(|p| !p.moved && (p.demonstrated || (p.written < p.round && !p.passed()))).collect())
    }

    /// The runs of the proofs carried into round n, run again before its proof step's turn.
    fn reruns(&self, n: u32) -> Result<Vec<ProofRun>, OrchestratorError> {
        Ok(self.proof_runs()?.into_iter().filter(|p| p.round == n && p.written < n).collect())
    }

    /// Where the proof of a run lies: in the proofs directory of the round that wrote it.
    fn proof_path(&self, proof: &ProofRun) -> String {
        format!("{}/{}", self.state.proofs_dir(proof.written), proof.file)
    }

    /// What the proof step hears in round n of the proofs from earlier rounds run again before its turn:
    /// CARRIED_PROOFS_NOTE after a blank line; "" when none was. A proof found gone in round n (ProofRun::moved) is
    /// listed as moved into the project, and only in round n, since carried_proofs carries it no further. Errors: a saved run is malformed (ProofRun::read), or
    /// the prompt cannot be filled in.
    pub(super) fn carried_proofs_note(&self, n: u32) -> Result<String, OrchestratorError> {
        let Some(command) = self.pipeline.proof_command() else { return Ok(String::new()) };
        let records = self.reruns(n)?;
        if records.is_empty() {
            return Ok(String::new());
        }
        let items: Vec<String> = records
            .iter()
            .map(|p| {
                if p.moved {
                    return format!(
                        "- `{}`: not run, as its file is gone from its directory, taken as moved into the project. Check that the \
                         change holds it as a test that passes; if it does not, the proof's finding stands.",
                        self.proof_path(p)
                    );
                }
                let passed = p.passed();
                let how = if p.demonstrated {
                    "still fails"
                } else if passed {
                    "now passes"
                } else {
                    "no longer fails but errors"
                };
                let (said, output) = if passed {
                    (String::new(), vec![])
                } else {
                    (format!(", {}", proof_outcome(&p.said)), proof_output(&p.said))
                };
                let mut lines = vec![format!("- `{}`: {how}{said}.", self.proof_path(p))];
                lines.extend(output);
                lines.join("\n")
            })
            .collect();
        let note = self.fill(Prompt::CarriedProofsNote, &[
            ("command", command),
            ("cwd", self.state.work_dir()),
            ("proofs", &items.join("\n\n")),
        ])?;
        Ok(format!("\n\n{note}"))
    }

    /// Whether the proof file that round written's proof step wrote fails against the working tree, as command's
    /// exit status 1 says, and what it printed last: its output's end, how else it exited, or why it could not run.
    /// A file gone from proofs_dir(written) is not run, and its outcome is moved (ProofRun::moved).
    ///
    /// The proof runs from a copy of proofs_dir(written), with the files beside it, at
    /// <work_dir>/PROOF_COPIES_DIR/<written>: command gets the copy's path, and runs in the working tree's top
    /// directory, for at most proof_timeout seconds. A tool that looks upwards from the proof for its configuration,
    /// as pytest does for its rootdir, conftest.py files and pythonpath, so meets the working tree's before any of the
    /// checkout's, which holds the run directory and encloses a run's worktree. The proof's output names the copy. The
    /// working tree's .orchestrator directory is made ignored first, so no commit holds the copy; PROOF_COPIES_DIR is
    /// emptied before the copy is made and removed after the proof, so what a proof writes beside itself goes with it.
    ///
    /// The working tree is left as the proof found it, ignored files aside (Host::tree_snapshot, Host::restore_tree),
    /// also when the proof fails, errors or is interrupted: what it writes elsewhere in the tree, such as a coverage
    /// file or the __pycache__ of a module it imports, never reaches the commit, and a file it edits is put back as it
    /// was. So is what anyone else changes in the tree while the proof runs, as a human may in an in-place run's
    /// checkout (TODO.md). When the proof ran and its tree cannot be put back, that is logged and the outcome says the
    /// proof did not run cleanly; when it did not run, or was interrupted, it is only logged. Nothing is put back
    /// outside a git repository, which no run commits; nor where the tree cannot be snapshotted, as with a nested
    /// repository without a commit, which `git add -A` and so publishing refuse too: that is logged, and the proof runs.
    ///
    /// Errors: the Ctrl-C, raised before the proof, which then does not run, or during it; command cannot be filled
    /// in, which Pipeline rules out. A proof that cannot run is no error, but its outcome.
    pub(super) fn run_proof(&self, command: &str, written: u32, file: &str) -> Outcome<ProofOutcome> {
        self.check_interrupt()?;
        let work_dir = self.state.work_dir();
        let copies = format!("{work_dir}/{PROOF_COPIES_DIR}");
        let copy = format!("{copies}/{written}");
        let proof = shlex_quote(&format!("{copy}/{file}"));
        let cmd = fill(command, &|name| (name == "proofs").then(|| proof.clone()))
            .map_err(|e| prompt_error("proof_command", &e))?;
        let limit = self.proof_timeout.to_string();
        // The tree as the proof finds it, put back after it; None puts nothing back.
        let mut before = None;
        let ran = self.ignore_orchestrator_dir(work_dir).and_then(|()| {
            before = self.host.tree_snapshot(work_dir).unwrap_or_else(|e| {
                self.human.log(&format!("cannot snapshot {work_dir}, so what the proof writes there stays: {e}"));
                None
            });
            let proofs_dir = self.state.proofs_dir(written);
            let argv = ["sh", "-c", &copy_script(), "_", &copies, &copy, &proofs_dir, file];
            let copied = self.host.run(&argv, None, DEFAULT_TIMEOUT)?;
            if copied.status == MISSING_FILE_STATUS {
                return Ok(None);
            }
            checked(&*self.host, &argv, copied)?;
            let argv = ["sh", "-c", PROOF_SCRIPT, "_", work_dir, &cmd, &limit];
            self.host.run_verbatim(&argv, None, self.proof_timeout + 30).map(Some)
        });
        // As Python's finally, which removed the copies also when the proof's run raised KeyboardInterrupt; the tree
        // is put back likewise.
        let put_back = self.interrupt.finally(|| {
            self.remove_proof_copies(&copies);
            let restored = before.as_deref().map_or(Ok(()), |before| self.host.restore_tree(work_dir, before));
            if let Err(e) = &restored {
                self.human.log(&format!("could not undo what a proof wrote in {work_dir}: {e}"));
            }
            restored
        });
        // As Python's KeyboardInterrupt, which the proof's run raises: nothing of an interrupted proof is recorded.
        self.check_interrupt()?;
        let did_not_run = |said: String| ProofOutcome { demonstrated: false, said, moved: false };
        let proc = match ran {
            Ok(Some(proc)) => proc,
            Ok(None) => return Ok(ProofOutcome { moved: true, ..did_not_run("it did not run: its file is gone".into()) }),
            Err(e) => return Ok(did_not_run(format!("it did not run: {e}"))),
        };
        if let Err(e) = put_back {
            return Ok(did_not_run(format!("it did not run cleanly: {e}")));
        }
        let output = String::from_utf8_lossy(&proc.stdout).into_owned() + &proc.stderr;
        let lines: Vec<&str> = output.trim_end().split('\n').collect();
        let tail = lines[lines.len().saturating_sub(PROOF_TAIL_LINES)..].join("\n");
        let how = match proc.status {
            0 => "it passes".to_string(),
            1 => "it fails".to_string(),
            124 => format!("it errors, running out of its {} s", self.proof_timeout),
            status => format!("it errors, with exit status {status}"),
        };
        let said = if tail.is_empty() { how } else { format!("{how}:\n{tail}") };
        Ok(ProofOutcome { demonstrated: proc.status == 1, said, moved: false })
    }

    /// Remove the directory copies and everything under it; a failure is only logged, since the next proof's run
    /// empties it again and git ignores it meanwhile.
    fn remove_proof_copies(&self, copies: &str) {
        if let Err(e) = self.host.check(&["sh", "-c", REMOVE_TREE_SCRIPT, "_", copies], None, DEFAULT_TIMEOUT) {
            self.human.log(&format!("could not remove the proof copies in {copies}: {e}"));
        }
    }

    /// What the step hears in round n of the proofs carried into it (carried_proofs): PROOFS_NOTE after a blank line;
    /// "" when it is not the step the proof step (Pipeline::proof_step) loops back to, or none is carried. Errors: as
    /// carried_proofs_note's.
    pub(super) fn proofs_note(&self, step: &Step, n: u32) -> Result<String, OrchestratorError> {
        let loops_here = self.pipeline.proof_step().is_some_and(|v| v.loop_to.as_deref() == Some(step.id.as_str()));
        if !loops_here {
            return Ok(String::new());
        }
        let failing: Vec<String> = self.carried_proofs(n)?.iter().map(|p| self.proof_path(p)).collect();
        if failing.is_empty() {
            return Ok(String::new());
        }
        Ok(format!("\n\n{}", self.fill(Prompt::ProofsNote, &[("proofs", &failing.join(", "))])?))
    }

    /// Add records to RunState::proofs and save it; how many of them failed. Errors: saving fails.
    fn record_proof_runs(&mut self, records: &[ProofRun]) -> Outcome<usize> {
        self.state.proofs.extend(records.iter().map(ProofRun::record));
        self.save()?;
        Ok(records.iter().filter(|p| p.demonstrated).count())
    }

    /// RunState::proofs, read. Errors: as ProofRun::read's.
    fn proof_runs(&self) -> Result<Vec<ProofRun>, OrchestratorError> {
        self.state.proofs.iter().map(ProofRun::read).collect()
    }
}
