//! The prompts each workflow sends, held to recorded texts and golden files, and the prompt files that word them,
//! as far as a Workflow is needed. A case of a
//! prompt's text alone asks RunView::step_prompt, on a run state and the files a FakeHost holds; the cases that run a
//! Workflow show what it sends besides (a session's ContinuePrompt), what it saves with the prompts, and that its
//! turns are prompted with step_prompt's texts.
//!
//! Not here: bad prompt files stopping a run before any agent, and `workflows`, are the CLI's (crate::cli's tests and
//! crate::cli::workflows'); a bad file stopping drain before any run is drain's; a saved state that replaces an
//! unknown prompt is resumable_state's (crate::cli, cli/start/tests.rs); reading prompt files is crate::prompts' and
//! crate::workflow_file's. The cases here that need a new run's state run on the state new_state would give, built
//! by hand (new_state is the CLI's).

use super::*;
use crate::env::ConfigDir;
use crate::paths::file_name;
use crate::pipeline::Pipeline;
use crate::prompts::load_prompts;
use crate::test_support::{quick, TempDir};
use crate::workflow::fixture::Outcome;
use crate::workflow::step_prompt::RunView;
use crate::workflow_file::{find_workflow, WORKFLOWS};

const D: &str = "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3";

// The default workflow's prompts as commit d6bb100 sent them, recorded by running these scenarios on that commit's
// code: since then, SPEC_PROMPT's "Separate Claude Code sessions" became "Separate agent sessions", FIX_PROMPT's "The
// Reviewer requested changes" became "Changes were requested", every prompt the Builder gets ends with
// BLOCKED_NOTE and WAITING_NOTE, and in a gated run with QUALITY_FOLLOWS_NOTE before them.

pub(super) const FRESH_BUILD_2: &str = concat!(
    "You are the Builder, the second of three roles (Spec Collector -> Builder -> Reviewer). The spec in ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/spec.md was agreed with the human by a separate ",
    "session; it is your contract.\n\nImplement it in /proj, following the conventions of the ",
    "surrounding code. Verify the change the way the spec's Verification section says, and run the ",
    "tests. Do not commit, push or switch branches; leave the changes in the working tree for the ",
    "Reviewer. If the spec is wrong or cannot be met, do not deviate silently: say so in your ",
    "report.\n\nAs your last step, write a report to ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/build-2.md in a single write; it hands the work to ",
    "the Reviewer: the files you changed and why, how you verified the change (commands and a summary of ",
    "their results), and any acceptance criterion you did not meet, with the reason.\n\nThis is round 2, ",
    "and you are a fresh session. An earlier Builder session did the earlier turns; its changes are ",
    "already in the working tree, and its reports are ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/build-1.md.\n\nChanges were requested; the ",
    "findings are in /proj/.octoflow-ai/runs/20260929-120000-a1b2c3/review-1.md. Fix each finding, or ",
    "explain in your report why it is wrong. Re-run the verification. Do not commit, push or switch ",
    "branches. As your last step, write a new report to ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/build-2.md in a single write, in the same shape as ",
    "before, answering each finding by its number.\n\nIf you cannot continue without a decision from the ",
    "human, write the report with a first line `BLOCKED: <your question>` instead.",
    "\n\nIf you must wait for something outside this run before you can go on, such as another pull ",
    "request merging, write what you are waiting for to ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/build-2.waiting and delete that file when you ",
    "go back to work, as the turn's time limit stands still while it is there.",
);

const FRESH_REVIEW_2: &str = concat!(
    "You are the Reviewer, the last of three roles (Spec Collector -> Builder -> Reviewer). You did not ",
    "write this change. Judge it only against the spec in ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/spec.md and the code itself.\n\nThe change: `git ",
    "diff --no-relative abc123` in /proj, plus the untracked files `git status --porcelain --untracked-files=all` lists\nThe Builder's ",
    "report is in /proj/.octoflow-ai/runs/20260929-120000-a1b2c3/build-2.md. Treat its claims as ",
    "unverified: check them, and run the verification yourself. Do not modify any file other than your ",
    "review.\n\nAs your last step, write your review to ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/review-2.md in a single write. Its first line must ",
    "be exactly `VERDICT: APPROVE` or `VERDICT: CHANGES_REQUESTED`. Then list numbered findings, each ",
    "with file:line, what is wrong, and which acceptance criterion it violates or what failure it ",
    "causes. Request changes only for defects: an unmet acceptance criterion, a bug, a broken test. ",
    "Style preferences are not defects.\n\nThis is round 2, and you are a fresh session. The earlier ",
    "reviews of this change are /proj/.octoflow-ai/runs/20260929-120000-a1b2c3/review-1.md; check that ",
    "the Builder has answered each of their findings.",
);

const CONTINUE_BUILD_2: &str = concat!(
    "Your session was restarted in the middle of this turn. Continue where you left off; the turn still ",
    "ends when you write /proj/.octoflow-ai/runs/20260929-120000-a1b2c3/build-2.md in a single write.",
);

const SPEC_INTERVIEW: &str = concat!(
    "You are the Spec Collector, the first of three roles (Spec Collector -> Builder -> Reviewer). ",
    "Separate agent sessions play the Builder and the Reviewer; they will know only what you write ",
    "down. A human is at this terminal and answers you directly.\n\nTask from the human:\nadd a rate ",
    "limiter\n\nInterview the human until the requirements are unambiguous: the goal, what is in and ",
    "out of scope, testable acceptance criteria, constraints, and how the result will be verified. ",
    "Read the code in /proj first so your questions are grounded and you can cite the files the ",
    "change touches. Ask a few questions at a time. Do not write or change any code.\n\nWhen the human ",
    "approves the spec, write it in a single write to ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/spec.md as Markdown. Its first line is a `# ` ",
    "heading: a short imperative title for the change, under 70 characters; it becomes the commit ",
    "subject and the pull request title. Then these `##` sections: Goal, Scope, Non-goals, ",
    "Acceptance criteria (a numbered list, each one checkable), Relevant code (file:line), ",
    "Verification. Writing that file hands the work to the Builder, so write it only after the human ",
    "approves it.",
);

const BUILD_1: &str = concat!(
    "You are the Builder, the second of three roles (Spec Collector -> Builder -> Reviewer). The ",
    "spec in /proj/.octoflow-ai/runs/20260929-120000-a1b2c3/spec.md was agreed with the human by a ",
    "separate session; it is your contract.\n\nImplement it in /proj, following the conventions of the ",
    "surrounding code. Verify the change the way the spec's Verification section says, and run the ",
    "tests. Do not commit, push or switch branches; leave the changes in the working tree for the ",
    "Reviewer. If the spec is wrong or cannot be met, do not deviate silently: say so in your ",
    "report.\n\nAs your last step, write a report to ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/build-1.md in a single write; it hands the work ",
    "to the Reviewer: the files you changed and why, how you verified the change (commands and a ",
    "summary of their results), and any acceptance criterion you did not meet, with the reason.\n\n",
    "Once your report is written, the orchestrator runs the quality gate on your change, so do not run the ",
    "gate yourself: run the tests your change touches. This turn may take at most 1800 seconds; write the ",
    "report before then.\n\nIf ",
    "you cannot continue without a decision from the human, write the report with a first line ",
    "`BLOCKED: <your question>` instead.",
    "\n\nIf you must wait for something outside this run before you can go on, such as another pull ",
    "request merging, write what you are waiting for to ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/build-1.waiting and delete that file when you ",
    "go back to work, as the turn's time limit stands still while it is there.",
);

const QUALITY_FIX_1_1: &str = concat!(
    "The quality gate did not pass on your change; the findings are in ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/quality-1-1.md. Fix each numbered issue, the ",
    "failed conditions, the failing tests and a failed build, or explain in your report why one ",
    "should stand. Answer each numbered finding to weigh the same way; those never fail the gate, ",
    "but the Reviewer reads your answers. Re-run the verification. Do not commit, push or switch ",
    "branches. As your last step, write a new report to ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/build-1-q1.md in a single write, in the same ",
    "shape as before, answering each numbered issue and finding to weigh by its number.\n\n",
    "Once your report is written, the orchestrator runs the quality gate on your change, so do not run the ",
    "gate yourself: run the tests your change touches. This turn may take at most 1800 seconds; write the ",
    "report before then.\n\nIf you ",
    "cannot continue without a decision from the human, write the report with a first line `BLOCKED: ",
    "<your question>` instead.",
    "\n\nIf you must wait for something outside this run before you can go on, such as another pull ",
    "request merging, write what you are waiting for to ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/build-1-q1.waiting and delete that file when ",
    "you go back to work, as the turn's time limit stands still while it is there.",
);

const REVIEW_1_PASSED: &str = concat!(
    "You are the Reviewer, the last of three roles (Spec Collector -> Builder -> Reviewer). You did ",
    "not write this change. Judge it only against the spec in ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/spec.md and the code itself.\n\nThe change: `git ",
    "diff --no-relative abc123` in /proj, plus the untracked files `git status --porcelain --untracked-files=all` lists\nThe Builder's ",
    "report is in /proj/.octoflow-ai/runs/20260929-120000-a1b2c3/build-1-q1.md. Treat its claims as ",
    "unverified: check them, and run the verification yourself. Do not modify any file other than ",
    "your review.\n\nAs your last step, write your review to ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/review-1.md in a single write. Its first line ",
    "must be exactly `VERDICT: APPROVE` or `VERDICT: CHANGES_REQUESTED`. Then list numbered ",
    "findings, each with file:line, what is wrong, and which acceptance criterion it violates or ",
    "what failure it causes. Request changes only for defects: an unmet acceptance criterion, a bug, ",
    "a broken test. Style preferences are not defects.\n\nThe quality gate passed on this change; its ",
    "report is in /proj/.octoflow-ai/runs/20260929-120000-a1b2c3/quality-1-2.md. If ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/quality-1-2.md lists, under Build configuration, an edit the ",
    "change makes to lint or clippy configuration, judge each such edit as a finding: say whether the spec ",
    "justifies the loosening, and request changes when it only hides a finding.",
);

const FIX_2: &str = concat!(
    "Changes were requested; the findings are in ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/review-1.md. Fix each finding, or explain in ",
    "your report why it is wrong. Re-run the verification. Do not commit, push or switch branches. ",
    "As your last step, write a new report to ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/build-2.md in a single write, in the same shape ",
    "as before, answering each finding by its number.\n\n",
    "Once your report is written, the orchestrator runs the quality gate on your change, so do not run the ",
    "gate yourself: run the tests your change touches. This turn may take at most 1800 seconds; write the ",
    "report before then.\n\nIf you cannot continue without a decision ",
    "from the human, write the report with a first line `BLOCKED: <your question>` instead.",
    "\n\nIf you must wait for something outside this run before you can go on, such as another pull ",
    "request merging, write what you are waiting for to ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/build-2.waiting and delete that file when you ",
    "go back to work, as the turn's time limit stands still while it is there.",
);

const QUALITY_FIX_2_1: &str = concat!(
    "The quality gate did not pass on your change; the findings are in ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/quality-2-1.md. Fix each numbered issue, the ",
    "failed conditions, the failing tests and a failed build, or explain in your report why one ",
    "should stand. Answer each numbered finding to weigh the same way; those never fail the gate, ",
    "but the Reviewer reads your answers. Re-run the verification. Do not commit, push or switch ",
    "branches. As your last step, write a new report to ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/build-2-q1.md in a single write, in the same ",
    "shape as before, answering each numbered issue and finding to weigh by its number.\n\n",
    "Once your report is written, the orchestrator runs the quality gate on your change, so do not run the ",
    "gate yourself: run the tests your change touches. This turn may take at most 1800 seconds; write the ",
    "report before then.\n\nIf you ",
    "cannot continue without a decision from the human, write the report with a first line `BLOCKED: ",
    "<your question>` instead.",
    "\n\nIf you must wait for something outside this run before you can go on, such as another pull ",
    "request merging, write what you are waiting for to ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/build-2-q1.waiting and delete that file when ",
    "you go back to work, as the turn's time limit stands still while it is there.",
);

const RECHECK_2_UNRESOLVED: &str = concat!(
    "The Builder has answered your review; the new report is in ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/build-2-q1.md. Review the change again (`git ",
    "diff --no-relative abc123` in /proj, plus the untracked files `git status --porcelain --untracked-files=all` lists) against the ",
    "spec in /proj/.octoflow-ai/runs/20260929-120000-a1b2c3/spec.md and your previous findings, ",
    "checking the Builder's claims rather than trusting them. As your last step, write the review to ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/review-2.md in a single write, with the same ",
    "first-line verdict and numbered findings as before.\n\nThe quality gate still did not pass after ",
    "the Builder's last quality round; its unresolved findings are in ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/quality-2-2.md. Weigh them as you would your ",
    "own. If ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/quality-2-2.md lists, under Build configuration, an edit the ",
    "change makes to lint or clippy configuration, judge each such edit as a finding: say whether the spec ",
    "justifies the loosening, and request changes when it only hides a finding.",
);

const REVIEW_1_UNRESOLVED: &str = concat!(
    "You are the Reviewer, the last of three roles (Spec Collector -> Builder -> Reviewer). You did ",
    "not write this change. Judge it only against the spec in ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/spec.md and the code itself.\n\nThe change: `git ",
    "diff --no-relative abc123` in /proj, plus the untracked files `git status --porcelain --untracked-files=all` lists\nThe Builder's ",
    "report is in /proj/.octoflow-ai/runs/20260929-120000-a1b2c3/build-1-q1.md. Treat its claims as ",
    "unverified: check them, and run the verification yourself. Do not modify any file other than ",
    "your review.\n\nAs your last step, write your review to ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/review-1.md in a single write. Its first line ",
    "must be exactly `VERDICT: APPROVE` or `VERDICT: CHANGES_REQUESTED`. Then list numbered ",
    "findings, each with file:line, what is wrong, and which acceptance criterion it violates or ",
    "what failure it causes. Request changes only for defects: an unmet acceptance criterion, a bug, ",
    "a broken test. Style preferences are not defects.\n\nThe quality gate still did not pass after ",
    "the Builder's last quality round; its unresolved findings are in ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/quality-1-2.md. Weigh them as you would your ",
    "own. If /proj/.octoflow-ai/runs/20260929-120000-a1b2c3/quality-1-2.md lists, under Build configuration, an ",
    "edit the change makes to lint or clippy configuration, judge each such edit as a finding: say whether the spec ",
    "justifies the loosening, and request changes when it only hides a finding.",
);

const RECHECK_2_PASSED: &str = concat!(
    "The Builder has answered your review; the new report is in ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/build-2.md. Review the change again (`git diff --no-relative ",
    "abc123` in /proj, plus the untracked files `git status --porcelain --untracked-files=all` lists) against the spec in ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/spec.md and your previous findings, checking ",
    "the Builder's claims rather than trusting them. As your last step, write the review to ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/review-2.md in a single write, with the same ",
    "first-line verdict and numbered findings as before.\n\nThe quality gate passed on this change; ",
    "its report is in /proj/.octoflow-ai/runs/20260929-120000-a1b2c3/quality-2-1.md. If ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/quality-2-1.md lists, under Build configuration, an edit the ",
    "change makes to lint or clippy configuration, judge each such edit as a finding: say whether the spec justifies ",
    "the loosening, and request changes when it only hides a finding.",
);

const FRESH_BUILD_2_AFTER_QUALITY_ROUNDS: &str = concat!(
    "You are the Builder, the second of three roles (Spec Collector -> Builder -> Reviewer). The ",
    "spec in /proj/.octoflow-ai/runs/20260929-120000-a1b2c3/spec.md was agreed with the human by a ",
    "separate session; it is your contract.\n\nImplement it in /proj, following the conventions of the ",
    "surrounding code. Verify the change the way the spec's Verification section says, and run the ",
    "tests. Do not commit, push or switch branches; leave the changes in the working tree for the ",
    "Reviewer. If the spec is wrong or cannot be met, do not deviate silently: say so in your ",
    "report.\n\nAs your last step, write a report to ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/build-2.md in a single write; it hands the work ",
    "to the Reviewer: the files you changed and why, how you verified the change (commands and a ",
    "summary of their results), and any acceptance criterion you did not meet, with the reason.\n\n",
    "This is round 2, and you are a fresh session. An earlier Builder session did the earlier turns; ",
    "its changes are already in the working tree, and its reports are ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/build-1.md, ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/build-1-q1.md, ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/build-1-q2.md.\n\nChanges were requested; ",
    "the findings are in /proj/.octoflow-ai/runs/20260929-120000-a1b2c3/review-1.md. Fix each ",
    "finding, or explain in your report why it is wrong. Re-run the verification. Do not commit, ",
    "push or switch branches. As your last step, write a new report to ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/build-2.md in a single write, in the same shape ",
    "as before, answering each finding by its number.\n\n",
    "Once your report is written, the orchestrator runs the quality gate on your change, so do not run the ",
    "gate yourself: run the tests your change touches. This turn may take at most 1800 seconds; write the ",
    "report before then.\n\nIf you cannot continue without a decision ",
    "from the human, write the report with a first line `BLOCKED: <your question>` instead.",
    "\n\nIf you must wait for something outside this run before you can go on, such as another pull ",
    "request merging, write what you are waiting for to ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/build-2.waiting and delete that file when you ",
    "go back to work, as the turn's time limit stands still while it is there.",
);

const FRESH_QUALITY_FIX_2_1: &str = concat!(
    "You are the Builder, the second of three roles (Spec Collector -> Builder -> Reviewer). The ",
    "spec in /proj/.octoflow-ai/runs/20260929-120000-a1b2c3/spec.md was agreed with the human by a ",
    "separate session; it is your contract.\n\nImplement it in /proj, following the conventions of the ",
    "surrounding code. Verify the change the way the spec's Verification section says, and run the ",
    "tests. Do not commit, push or switch branches; leave the changes in the working tree for the ",
    "Reviewer. If the spec is wrong or cannot be met, do not deviate silently: say so in your ",
    "report.\n\nAs your last step, write a report to ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/build-2-q1.md in a single write; it hands the ",
    "work to the Reviewer: the files you changed and why, how you verified the change (commands and ",
    "a summary of their results), and any acceptance criterion you did not meet, with the reason.\n\n",
    "This is round 2, and you are a fresh session. An earlier Builder session did the earlier turns; ",
    "its changes are already in the working tree, and its reports are ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/build-1.md, ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/build-1-q1.md, ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/build-2.md.\n\nThe quality gate did not pass on ",
    "your change; the findings are in ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/quality-2-1.md. Fix each numbered issue, the ",
    "failed conditions, the failing tests and a failed build, or explain in your report why one ",
    "should stand. Answer each numbered finding to weigh the same way; those never fail the gate, ",
    "but the Reviewer reads your answers. Re-run the verification. Do not commit, push or switch ",
    "branches. As your last step, write a new report to ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/build-2-q1.md in a single write, in the same ",
    "shape as before, answering each numbered issue and finding to weigh by its number.\n\n",
    "Once your report is written, the orchestrator runs the quality gate on your change, so do not run the ",
    "gate yourself: run the tests your change touches. This turn may take at most 1800 seconds; write the ",
    "report before then.\n\nIf you ",
    "cannot continue without a decision from the human, write the report with a first line `BLOCKED: ",
    "<your question>` instead.",
    "\n\nIf you must wait for something outside this run before you can go on, such as another pull ",
    "request merging, write what you are waiting for to ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/build-2-q1.waiting and delete that file when ",
    "you go back to work, as the turn's time limit stands still while it is there.",
);

const FRESH_REVIEW_2_PASSED: &str = concat!(
    "You are the Reviewer, the last of three roles (Spec Collector -> Builder -> Reviewer). You did ",
    "not write this change. Judge it only against the spec in ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/spec.md and the code itself.\n\nThe change: `git ",
    "diff --no-relative abc123` in /proj, plus the untracked files `git status --porcelain --untracked-files=all` lists\nThe Builder's ",
    "report is in /proj/.octoflow-ai/runs/20260929-120000-a1b2c3/build-2.md. Treat its claims as ",
    "unverified: check them, and run the verification yourself. Do not modify any file other than ",
    "your review.\n\nAs your last step, write your review to ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/review-2.md in a single write. Its first line ",
    "must be exactly `VERDICT: APPROVE` or `VERDICT: CHANGES_REQUESTED`. Then list numbered ",
    "findings, each with file:line, what is wrong, and which acceptance criterion it violates or ",
    "what failure it causes. Request changes only for defects: an unmet acceptance criterion, a bug, ",
    "a broken test. Style preferences are not defects.\n\nThe quality gate passed on this change; its ",
    "report is in /proj/.octoflow-ai/runs/20260929-120000-a1b2c3/quality-2-1.md. If ",
    "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3/quality-2-1.md lists, under Build configuration, an edit the ",
    "change makes to lint or clippy configuration, judge each such edit as a finding: say whether the spec ",
    "justifies the loosening, and request changes when it only hides a finding.\n\nThis is round 2, and you are a ",
    "fresh session. The earlier reviews of this change are /proj/.octoflow-ai/runs/20260929-120000-a1b2c3/review-1.md; ",
    "check that the Builder has answered each of their findings.",
);

/// The pull request body, but for "<built-in gate>", which stands for fixture::built_in_gate.
const GOLDEN_PR_BODY: &str = concat!(
    "Opened by Octoflow AI run `20260929-120000-a1b2c3`. Reviewer verdict after 2 rounds: ",
    "**APPROVE**.\n\n| Commit trailer | Value |\n| --- | --- |\n| Octoflow-Run | ",
    "`20260929-120000-a1b2c3` |\n| Octoflow-Workflow | `default` |\n| Octoflow-Role | ",
    "`spec=claude/default` |\n| Octoflow-Role | `build=claude/default` |\n| Octoflow-Role | ",
    "`review=claude/default` |\n| Octoflow-Gate | `ERROR after 2 quality rounds` |\n\n<details ",
    "open>\n<summary>Spec</summary>\n\n# Add a token-bucket rate limiter\n\n## Goal\nlimit ",
    "requests\n\n</details>\n\n<details>\n<summary>Builder report (round 2)</summary>\n\nreport 2 ",
    "q1\n\n</details>\n\n<details>\n<summary>Quality gate (round 2)</summary>\n\nGATE: ",
    "ERROR\n\n<built-in gate>, run locally, judged snapshot `snap4` (round 2, quality round 2 of ",
    "2).\n\n`gate` judged the working tree.\n\n## Issues on changed lines\n\n1. ",
    "`limiter.py:9` error mypy arg-type: Argument 1 has incompatible ",
    "type\n\n</details>\n\n<details>\n<summary>Review (round 2)</summary>\n\n**VERDICT: APPROVE**\n1. ",
    "finding\n\n</details>\n\n<details>\n<summary>Run timings</summary>\n\n| step | role | round | ",
    "harness | model | start (UTC) | duration | file |\n| --- | --- | --- | --- | --- | --- | --- | --- ",
    "|\n| spec (human-paced) | spec | 0 | claude | default | 2026-10-03 04:00:00 | 0s | spec.md |\n| ",
    "build | build | 1 | claude | default | 2026-10-03 04:00:00 | 0s | build-1.md |\n| quality | quality ",
    "| 1 q1 | - | - | 2026-10-03 04:00:00 | 10s | quality-1-1.md |\n| build | build | 1 q1 | claude | ",
    "default | 2026-10-03 04:00:10 | 0s | build-1-q1.md |\n| quality | quality | 1 q2 | - | - | ",
    "2026-10-03 04:00:10 | 10s | quality-1-2.md |\n| review | review | 1 | claude | default | 2026-10-03 ",
    "04:00:20 | 0s | review-1.md |\n| build | build | 2 | claude | default | 2026-10-03 04:00:20 | 0s | ",
    "build-2.md |\n| quality | quality | 2 q1 | - | - | 2026-10-03 04:00:20 | 10s | quality-2-1.md |\n| ",
    "build | build | 2 q1 | claude | default | 2026-10-03 04:00:30 | 0s | build-2-q1.md |\n| quality | ",
    "quality | 2 q2 | - | - | 2026-10-03 04:00:30 | 10s | quality-2-2.md |\n| review | review | 2 | ",
    "claude | default | 2026-10-03 04:00:40 | 0s | review-2.md |\n\n</details>\n",
);

/// The prompts of the step with id step in round n of the run in state, of pipeline, its files those host holds:
/// RunView::step_prompt's, the running session's and a fresh one's.
fn step_prompt_in(
    pipeline: &Pipeline,
    state: &RunState,
    host: &FakeHost,
    step: &str,
    n: u32,
) -> (String, Option<String>) {
    let prompts = state.prompts().unwrap();
    let view = RunView { state, pipeline, prompts: &prompts, host };
    view.step_prompt(pipeline.step(step).expect("a step of the workflow"), n).unwrap()
}

/// step_prompt_in the run's own workflow (RunState::pipeline), as a resume takes it.
fn step_prompt(state: &RunState, host: &FakeHost, step: &str, n: u32) -> (String, Option<String>) {
    let pipeline = state.pipeline().unwrap();
    step_prompt_in(&pipeline, state, host, step, n)
}

/// A FakeHost holding each (path, text) of files.
fn holding(files: &[(String, &str)]) -> FakeHost {
    let host = FakeHost::default();
    for (path, text) in files {
        host.state().files.insert(path.to_string(), text.to_string());
    }
    host
}

#[test]
fn fresh_builder_in_round_two() {
    let (_, fresh) = step_prompt(&saved_run("build", 2, FULL), &FakeHost::default(), "build", 2);
    assert_eq!(fresh.unwrap(), FRESH_BUILD_2);
}

#[test]
fn fresh_reviewer_in_round_two() {
    let state = saved_run("review", 2, FULL);
    let host = holding(&[(state.build_path(2), "report 2")]);
    assert_eq!(step_prompt(&state, &host, "review", 2).1.unwrap(), FRESH_REVIEW_2);
}

#[test]
fn continue_after_a_resumed_session() {
    let seen = seen();
    let mut state = saved_run("build", 2, FULL);
    state.prompted = Some("build-2.md".into());
    let script = vec![("build", vec![recording(build_turn(2), &seen)]), ("review", vec![review_turn(2, APPROVE)])];
    let f = Fixture::resume(Rc::new(FakeHost::default()), state, script, &["review"], &[("build", "s-build")]);
    f.workflow(f.as_saved()).run().unwrap();
    assert_eq!(*seen.borrow(), [CONTINUE_BUILD_2]);
}

/// A run with the gate and two quality rounds a round, its Builder taking the turns in build and its Reviewer those in
/// review, run to APPROVE: its fixture, every prompt in order, and the (phase, round, quality round) state.json was
/// saved with, each once, in the order first saved.
fn two_gated_rounds(
    outcomes: Vec<Outcome>,
    build: Vec<Turn>,
    review: Vec<Turn>,
) -> (Fixture, Vec<String>, Vec<(String, i64, i64)>) {
    let seen = seen();
    let f = Fixture::new(vec![
        ("spec", vec![recording(spec_turn(), &seen)]),
        ("build", build.into_iter().map(|b| recording(b, &seen)).collect()),
        ("review", review.into_iter().map(|r| recording(r, &seen)).collect()),
    ])
    .gated();
    f.judging(outcomes);
    let phases = Rc::new(std::cell::RefCell::new(Vec::new()));
    let saved = Rc::clone(&phases);
    f.host.state().before_write = Some(Box::new(move |path, text| {
        if path.ends_with("state.json") {
            let Ok(Value::Object(state)) = serde_json::from_str::<Value>(text) else {
                panic!("state.json is no object")
            };
            let int = |key: &str| state.get(key).and_then(Value::as_i64).expect("a saved count");
            let phase = (text_of(&state, "phase"), int("round"), int("quality_round"));
            if !saved.borrow().contains(&phase) {
                saved.borrow_mut().push(phase);
            }
        }
        Ok(())
    }));
    assert_eq!(f.workflow(RunSettings { max_quality_rounds: 2, ..RunSettings::default() }).run(), verdict(APPROVE));
    let seen = seen.borrow().clone();
    let phases = phases.borrow().clone();
    (f, seen, phases)
}

fn text_of(saved: &Object, key: &str) -> String {
    saved.get(key).and_then(Value::as_str).expect("a saved text").to_string()
}

#[test]
fn gate_passes_after_a_fix_then_runs_out() {
    let (f, seen, phases) = two_gated_rounds(
        vec![red(), green(), red(), red2()],
        vec![build_turn(1), fix_turn(1, 1), build_turn(2), fix_turn(2, 1)],
        vec![review_turn(1, CHANGES_REQUESTED), review_turn(2, APPROVE)],
    );

    let golden =
        [SPEC_INTERVIEW, BUILD_1, QUALITY_FIX_1_1, REVIEW_1_PASSED, FIX_2, QUALITY_FIX_2_1, RECHECK_2_UNRESOLVED];
    assert_eq!(seen, golden);
    let expected: Vec<(String, i64, i64)> = [
        ("spec", 0, 0),
        ("build", 1, 0),
        ("quality", 1, 1),
        ("build", 1, 1),
        ("quality", 1, 2),
        ("review", 1, 2),
        ("build", 2, 0),
        ("quality", 2, 1),
        ("build", 2, 1),
        ("quality", 2, 2),
        ("review", 2, 2),
        ("publish", 2, 2),
        ("done", 2, 2),
    ]
    .iter()
    .map(|(p, n, q)| (p.to_string(), *n, *q))
    .collect();
    assert_eq!(phases, expected);
    let written: Vec<String> = f
        .host
        .state()
        .writes
        .iter()
        // The run directory's own files: the local gate's are in directories of their own.
        .filter(|p| p.strip_prefix(D).is_some_and(|name| !name.trim_start_matches('/').contains('/')))
        .filter(|p| !p.ends_with("state.json"))
        .map(|p| file_name(p).to_string())
        .collect();
    assert_eq!(
        written,
        [
            "spec.md",
            "build-1.md",
            "quality-1-1.md",
            "build-1-q1.md",
            "quality-1-2.md",
            "review-1.md",
            "build-2.md",
            "quality-2-1.md",
            "build-2-q1.md",
            "quality-2-2.md",
            "review-2.md",
            "summary.md",
            "metrics.json"
        ]
    );
    let layout: Vec<Call> = f
        .herdr
        .state()
        .calls
        .iter()
        .filter(|c| matches!(c, Call::Rename { .. } | Call::Split { .. }))
        .cloned()
        .collect();
    let rename = |pane: &str, label: &str| Call::Rename { pane: pane.into(), label: label.into() };
    assert_eq!(
        layout,
        [
            rename("w1:p1", "Spec Collector"),
            split("w1:p1", Direction::Right),
            rename("w1:p2", "Builder"),
            split("w1:p2", Direction::Down),
            rename("w1:p3", "Reviewer"),
        ]
    );
    let pr = f.host.state().prs[0].clone();
    assert_eq!(pr.title, "Add a token-bucket rate limiter");
    assert_eq!(pr.body, GOLDEN_PR_BODY.replace("<built-in gate>", &built_in_gate()));
}

#[test]
fn gate_runs_out_then_passes() {
    let (_, seen, _) = two_gated_rounds(
        vec![red(), red2(), green()],
        vec![build_turn(1), fix_turn(1, 1), build_turn(2)],
        vec![review_turn(1, CHANGES_REQUESTED), review_turn(2, APPROVE)],
    );
    assert_eq!(seen, [SPEC_INTERVIEW, BUILD_1, QUALITY_FIX_1_1, REVIEW_1_UNRESOLVED, FIX_2, RECHECK_2_PASSED]);
}

/// saved_run of the default workflow's three roles with the gate's job.
fn gated_run(phase: &str, rnd: u32) -> RunState {
    let mut state = saved_run(phase, rnd, FULL);
    state.quality_job = Some(crate::local_gate::LOCAL_GATE.to_string());
    state
}

/// A human-paced gated step's turn has no deadline, so its prompt names none: it gets no QUALITY_FOLLOWS_NOTE, nor,
/// as a human-paced step cannot block, BLOCKED_NOTE.
#[test]
fn a_human_paced_gated_step_hears_no_time_limit() {
    let mut steps = crate::test_support::impl_().steps().to_vec();
    steps[0].human_paced = true;
    let roles = crate::test_support::roles(&[("impl", "Implementer"), ("review", "Reviewer")]);
    let pipeline = Pipeline::new("impl", roles, steps).unwrap();
    let mut state = new_run(&pipeline);
    state.quality_job = Some(crate::local_gate::LOCAL_GATE.to_string());
    let (prompt, _) = step_prompt_in(&pipeline, &state, &FakeHost::default(), "impl", 1);
    assert_eq!(prompt, format!("Implement {}; report to {D}/impl-1.md.", state.task));
}

#[test]
fn fresh_builder_in_round_two_after_quality_rounds() {
    let state = gated_run("build", 2);
    let host = holding(&[
        (state.build_path(1), "r"),
        (state.quality_build_path(1, 1), "r"),
        (state.quality_build_path(1, 2), "r"),
    ]);
    assert_eq!(step_prompt(&state, &host, "build", 2).1.unwrap(), FRESH_BUILD_2_AFTER_QUALITY_ROUNDS);
}

#[test]
fn fresh_builder_answering_a_quality_round_in_round_two() {
    let mut state = gated_run("build", 2);
    state.quality_round = 1;
    let host = holding(&[
        (state.build_path(1), "r"),
        (state.quality_build_path(1, 1), "r"),
        (state.build_path(2), "r"),
        (state.quality_path(2, 1), "GATE: ERROR\n"),
    ]);
    assert_eq!(step_prompt(&state, &host, "build", 2).1.unwrap(), FRESH_QUALITY_FIX_2_1);
}

#[test]
fn fresh_reviewer_in_round_two_with_the_gate() {
    let mut state = gated_run("review", 2);
    state.quality_round = 1;
    let host =
        holding(&[(state.build_path(1), "r"), (state.build_path(2), "r"), (state.quality_path(2, 1), "GATE: OK\n")]);
    assert_eq!(step_prompt(&state, &host, "review", 2).1.unwrap(), FRESH_REVIEW_2_PASSED);
}

// -- Each step's prompts in each built-in workflow, as Workflow renders them
// for a new run on the fakes, held to the golden file tests/golden/prompts_<name>.txt: round 1, and for the steps
// the verdict step loops back over, round 2 as the running session and as a fresh one hears it. --------------------

/// The prompts of pipeline's steps in a new run, past its base, as the golden file has them.
fn rendered(pipeline: &Pipeline) -> String {
    let mut state = new_run(pipeline);
    state.base = Some("abc123".into());
    let host = FakeHost::default();
    let steps = pipeline.steps();
    // The steps a later round runs: from the earliest loop target to the last verdict step.
    let at = |id: &str| steps.iter().position(|st| st.id == id).unwrap();
    let first_target = pipeline.verdict_steps().filter_map(|v| v.loop_to.as_deref()).map(at).min();
    let looped = match (first_target, pipeline.verdict_steps().next_back()) {
        (Some(target), Some(last)) => &steps[target..=at(&last.id)],
        _ => &[],
    };
    let mut parts = vec![];
    for step in steps {
        let (text, _) = step_prompt_in(pipeline, &state, &host, &step.id, 1);
        parts.push(format!("## {}, round 1\n\n{text}", step.id));
    }
    for step in looped {
        let (text, fresh) = step_prompt_in(pipeline, &state, &host, &step.id, 2);
        let fresh = fresh.unwrap_or_else(|| "None".into());
        parts.push(format!("## {}, round 2\n\n{text}", step.id));
        parts.push(format!("## {}, round 2, a fresh session\n\n{fresh}", step.id));
    }
    parts.join("\n\n") + "\n"
}

/// Fail unless text is tests/golden/<name>.txt, byte for byte.
fn assert_golden(name: &str, text: &str) {
    let path = format!("{}/../tests/golden/{name}.txt", env!("CARGO_MANIFEST_DIR"));
    let want = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("no golden file {path}: {e}"));
    assert!(text == want, "{path} differs from the text written now:\n{text}");
}

#[test]
fn each_built_in_workflow() {
    assert!(!WORKFLOWS.is_empty());
    for name in WORKFLOWS {
        assert_golden(&format!("prompts_{name}"), &rendered(&find_workflow(name, &Default::default()).unwrap()));
    }
}

// -- Prompt files in the config directory, in place of the built-in prompts. ---------------------------------------

/// A configuration directory whose prompt files are files, each (name, text) written with the newline an editor
/// ends it with.
fn config_with(files: &[(&str, &str)]) -> (TempDir, ConfigDir) {
    let dir = TempDir::new();
    let config = ConfigDir::at(&dir.path());
    std::fs::create_dir_all(config.prompts_dir()).unwrap();
    for (name, text) in files {
        std::fs::write(format!("{}/{name}", config.prompts_dir()), format!("{text}\n")).unwrap();
    }
    (dir, config)
}

/// A new run's state and workflow as `run task` makes them with config's prompt files: new_state's.
fn started(config: &ConfigDir) -> (RunState, Pipeline) {
    let prompts = load_prompts(config).unwrap();
    let pipeline = find_workflow("default", &prompts).unwrap();
    let mut state = RunState::new(crate::workflow::fixture::RUN_ID, "task", "/proj");
    (state.phase, state.workflow) = (pipeline.steps()[0].id.clone(), pipeline.name().to_string());
    state.prompt_overrides = prompts.overrides();
    (state, pipeline)
}

fn no_pr_of(pipeline: Pipeline) -> RunSettings {
    RunSettings { pipeline: Some(pipeline), pull_request: false, ..RunSettings::default() }
}

#[test]
fn a_build_prompt_file_words_the_builders_first_turn() {
    let build = "Build {spec_path} in {cwd}; report to {build_path}.";
    let (_dir, config) = config_with(&[("BUILD_PROMPT.md", build)]);
    let (state, pipeline) = started(&config);
    let seen = seen();
    let f = Fixture::on(
        Rc::new(FakeHost::default()),
        state,
        vec![
            ("spec", vec![spec_turn()]),
            ("build", vec![recording(build_turn(1), &seen)]),
            ("review", vec![review_turn(1, APPROVE)]),
        ],
    );
    let mut wf = f.workflow(no_pr_of(pipeline));
    assert_eq!(wf.run(), verdict(APPROVE));
    assert_eq!(
        *seen.borrow(),
        [format!("Build {D}/spec.md in /proj; report to {D}/build-1.md.{}", note(&format!("{D}/build-1.md")))]
    );
    let overrides: Vec<(&str, &String)> = wf.state().prompt_overrides.iter().map(|(k, v)| (k.as_str(), v)).collect();
    assert_eq!(overrides, [("BUILD_PROMPT", &build.to_string())]);
    // The saved definition holds the overridden prompt, so a resume asks the same of the Builder.
    let definition = wf.state().workflow_definition.as_ref().expect("a run saves its definition");
    let build_step = definition["steps"].as_array().unwrap().iter().find(|s| s["id"] == "build").unwrap();
    assert_eq!(build_step["prompt"], build);
}

#[test]
fn a_follow_up_file_reaches_the_turn_that_uses_it() {
    let (_dir, config) = config_with(&[("FIX_PROMPT.md", "Fix {findings}; report to {build_path}.")]);
    let (mut state, pipeline) = started(&config);
    (state.phase, state.round) = ("build".into(), 2);
    let (text, _) = step_prompt_in(&pipeline, &state, &FakeHost::default(), "build", 2);
    assert_eq!(text, format!("Fix {D}/review-1.md; report to {D}/build-2.md.{}", note(&format!("{D}/build-2.md"))));
}

#[test]
fn a_note_file_reaches_the_turn_that_uses_it() {
    let (_dir, config) = config_with(&[("QUALITY_PASSED_NOTE.md", "Gate passed: {quality_path}.")]);
    let mut state = quality_state(1);
    state.phase = "review".into();
    state.prompt_overrides = load_prompts(&config).unwrap().overrides();
    let host = holding(&[(state.build_path(1), "report 1"), (state.quality_path(1, 1), "GATE: OK\n")]);

    let (review, _) = step_prompt(&state, &host, "review", 1);
    assert!(review.ends_with(&format!("\n\nGate passed: {D}/quality-1-1.md.")), "{review}");
    assert!(!review.contains("The quality gate passed"));
}

#[test]
fn a_note_file_reaches_every_workflow() {
    let (_dir, config) = config_with(&[("BLOCKED_NOTE.md", "Stuck? Write `BLOCKED: <question>` {{and stop}}.")]);
    let quick = quick();
    let mut state = new_run(&quick);
    state.prompt_overrides = load_prompts(&config).unwrap().overrides();
    let (text, _) = step_prompt_in(&quick, &state, &FakeHost::default(), "build", 1);
    let stuck = "Stuck? Write `BLOCKED: <question>` {and stop}.";
    let expected = format!("Build add a rate limiter in /proj; report to {D}/build-1.md.\n\n{stuck}");
    assert!(text.starts_with(&format!("{expected}\n\nIf you must wait ")), "{text}");
}

#[test]
fn a_resume_keeps_the_prompts_the_run_started_with() {
    for edit in ["edited", "deleted"] {
        let (_dir, config) = config_with(&[("FIX_PROMPT.md", "Fix {findings}; report to {build_path}.")]);
        let path = format!("{}/FIX_PROMPT.md", config.prompts_dir());
        let (state, pipeline) = started(&config);
        let f = Fixture::on(
            Rc::new(FakeHost::default()),
            state,
            vec![("spec", vec![spec_turn()]), ("build", vec![build_turn(1)])],
        );
        f.play("review", vec![then_interrupted(&f.interrupt, idle())]);
        assert_eq!(f.workflow(no_pr_of(pipeline)).run(), Err(Stopped::Interrupted));
        if edit == "edited" {
            std::fs::write(&path, "Changed {findings}; report to {build_path}.\n").unwrap();
        } else {
            std::fs::remove_file(&path).unwrap();
        }
        let seen = seen();
        let saved = RunState::from_saved(&f.saved()).unwrap();
        let f2 = Fixture::resume(
            Rc::clone(&f.host),
            saved,
            vec![
                ("review", vec![review_turn(1, CHANGES_REQUESTED), review_turn(2, APPROVE)]),
                ("build", vec![recording(build_turn(2), &seen)]),
            ],
            &[],
            &[],
        );
        assert_eq!(f2.workflow(f2.as_saved()).run(), verdict(APPROVE), "{edit}");
        assert_eq!(seen.borrow().len(), 1, "{edit}");
        assert!(seen.borrow()[0].contains(&format!("Fix {D}/review-1.md; report to {D}/build-2.md.")), "{edit}");
        assert!(!seen.borrow()[0].contains("Changed"), "{edit}");
    }
}

#[test]
fn a_run_an_earlier_release_started_with_its_fix_prompt_file_is_resumable_and_words_its_fix() {
    // Started before 0.58.0 with a FIX_PROMPT.md that named {prev_review_path}, which a prompt file read today may
    // not (crate::prompts' EARLIER_PLACEHOLDERS), and saved in round 1's review, before RunState::verdicts. The
    // resume itself is verdict_steps.rs's and a_resume_keeps_the_prompts_the_run_started_with's: this checks that
    // the saved state passes resume's checks and that its fix prompt fills.
    let mut state = saved_run("review", 1, &["spec", "build", "review"]);
    state
        .prompt_overrides
        .insert("FIX_PROMPT".to_string(), "Fix {prev_review_path}; report to {build_path}.".to_string());
    let Value::Object(mut saved) = state.to_json() else { unreachable!("a state is saved as an object") };
    saved.shift_remove("verdicts");
    crate::test_support::resumable(&[], &saved).unwrap();
    // Its review then requested changes, and the Builder is to fix them.
    let mut state = RunState::from_saved(&saved).unwrap();
    (state.phase, state.round) = ("build".into(), 2);
    let (fix, _) = step_prompt(&state, &FakeHost::default(), "build", 2);
    assert!(fix.contains(&format!("Fix {D}/review-1.md; report to {D}/build-2.md.")), "{fix}");
}
