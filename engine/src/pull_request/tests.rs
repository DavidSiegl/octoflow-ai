use super::*;
use serde_json::Value;
use crate::test_support::{helper_cases, TempDir};

#[test]
fn every_recorded_run_is_published_as_the_python_engine_does() {
    for case in helper_cases() {
        let expected = |key: &str| case.expected.get(key).and_then(Value::as_str).map(String::from);
        let place = format!("{} {} {}", expected("recording").unwrap(), expected("run_dir").unwrap(), case.expected.get("conflicts").unwrap());
        let s = &case.state;
        assert_eq!(spec_title(&case.spec), expected("spec_title"), "{place}");
        let title = spec_title(&case.spec).unwrap_or(s.task.clone());
        assert_eq!(Some(branch_name(&title, &s.run_id)), expected("branch_name"), "{place}");
        assert_eq!(Some(verdict_line(s)), expected("verdict_line"), "{place}");
        assert_eq!(Some(commit_note(s)), expected("commit_note"), "{place}");
        let trailers = Value::Array(provenance(s, &case.evidence).into_iter().map(|(k, v)| Value::Array(vec![Value::from(k), Value::from(v)])).collect());
        assert_eq!(Some(&trailers), case.expected.get("provenance"), "{place}");
        let body = pr_body(s, &case.spec, &case.report, &one(&case.review), &case.evidence);
        assert_eq!(Some(body.clone()), expected("pr_body"), "{place}");
        if !s.conflicts.is_empty() {
            let mut clean = s.clone();
            clean.conflicts.clear();
            let without = without_conflict_warning(&body, s.base_branch.as_deref(), &s.conflicts);
            assert_eq!(without, pr_body(&clean, &case.spec, &case.report, &one(&case.review), &case.evidence), "{place}");
        }
    }
}

/// The reviews of a run whose last round has one verdict file, review-1.md, of text review.
fn one(review: &str) -> Vec<(String, String)> {
    vec![("review-1.md".to_string(), review.to_string())]
}

#[test]
fn a_run_judged_by_several_verdict_steps_names_each_verdict_of_its_last_round() {
    let mut s = state(|s| s.round = 2);
    s.record_verdict(1, "review", APPROVE);
    s.record_verdict(1, "accept", CHANGES);
    s.record_verdict(2, "review", APPROVE);
    s.record_verdict(2, "accept", APPROVE);
    assert_eq!(verdict_line(&s), "Verdicts after 2 rounds: review **APPROVE**, accept **APPROVE**.");
    let reviews = [("review-2.md", "VERDICT: APPROVE\nfine"), ("accept-2.md", "VERDICT: APPROVE\naccepted")]
        .map(|(name, text)| (name.to_string(), text.to_string()));
    let body = pr_body(&s, "spec", "r", &reviews, &GateEvidence::default());
    assert!(body.contains("<details>\n<summary>Review (review-2.md)</summary>\n\nVERDICT: APPROVE\nfine"), "{body}");
    assert!(body.contains("<details>\n<summary>Review (accept-2.md)</summary>\n\nVERDICT: APPROVE\naccepted"), "{body}");
    // The second requests changes in the last round: its file opens, the first's stays closed.
    s.record_verdict(2, "accept", CHANGES);
    s.verdict = Some(CHANGES.into());
    assert_eq!(
        verdict_line(&s),
        "Verdicts after 2 rounds: review **APPROVE**, accept **CHANGES_REQUESTED**.\n\naccept still requested changes after \
         the last round, so this is a draft."
    );
    let reviews = [("review-2.md", "VERDICT: APPROVE\nfine"), ("accept-2.md", "VERDICT: CHANGES_REQUESTED\nno")]
        .map(|(name, text)| (name.to_string(), text.to_string()));
    let body = pr_body(&s, "spec", "r", &reviews, &GateEvidence::default());
    assert!(body.contains("<details>\n<summary>Review (review-2.md)</summary>"), "{body}");
    assert!(body.contains("<details open>\n<summary>Review (accept-2.md)</summary>"), "{body}");
    assert_eq!(commit_note(&s), "Orchestrator run r: CHANGES_REQUESTED after 2 review round(s).");
    // A last round the review ended: its one verdict file is still named by its file, as the line names its step.
    s.verdicts.retain(|v| (v.round, v.step.as_str()) != (2, "accept"));
    s.record_verdict(2, "review", CHANGES);
    assert_eq!(
        verdict_line(&s),
        "Verdicts after 2 rounds: review **CHANGES_REQUESTED**.\n\nreview still requested changes after the last round, \
         so this is a draft."
    );
    let reviews = [("review-2.md".to_string(), "VERDICT: CHANGES_REQUESTED\nbug".to_string())];
    let body = pr_body(&s, "spec", "r", &reviews, &GateEvidence::default());
    assert!(body.contains("<details open>\n<summary>Review (review-2.md)</summary>"), "{body}");
}

#[test]
fn a_run_judged_by_one_verdict_step_or_saved_without_the_record_names_the_runs_verdict() {
    let mut s = state(|s| s.round = 2);
    let expected = "Reviewer verdict after 2 rounds: **APPROVE**.";
    assert_eq!(verdict_line(&s), expected);
    s.record_verdict(1, "review", CHANGES);
    s.record_verdict(2, "review", APPROVE);
    assert_eq!(verdict_line(&s), expected);
}

fn state(change: impl FnOnce(&mut RunState)) -> RunState {
    let mut s = RunState::new("r", "t", "/proj");
    s.verdict = Some(APPROVE.into());
    s.round = 1;
    change(&mut s);
    s
}

#[test]
fn spec_titles() {
    assert_eq!(spec_title("\n# Add a limiter \n## Goal").as_deref(), Some("Add a limiter"));
    assert_eq!(spec_title("## Goal\n# Late title"), None);
    assert_eq!(spec_title("#Title"), None);
    // A \x1c is no blank, so the # is not followed by one.
    assert_eq!(spec_title("  #\u{1c} Title\u{a0}"), None);
    assert_eq!(spec_title("  #\t\u{3000}Title\u{a0}"), Some("Title".into()));
    assert_eq!(spec_title("# \n"), None);
    assert_eq!(spec_title(""), None);
}

#[test]
fn branch_names() {
    assert_eq!(branch_name("Add a Token-Bucket limiter!", "20260929-120000-a1b2c3"), "orchestrator/add-a-token-bucket-limiter-a1b2c3");
    assert_eq!(branch_name("???", "20260929-120000-a1b2c3"), "orchestrator/a1b2c3");
    assert_eq!(branch_name("Ünïcode İs here — and a very long title that goes past forty", "r-x"), "orchestrator/n-code-i-s-here-and-a-very-long-title-t-x");
}

#[test]
fn the_conflict_warning_comes_first() {
    let s = state(|s| {
        s.base_branch = Some("main".into());
        s.conflicts = vec!["a.py".into(), "docs/b.md".into()];
    });
    let body = pr_body(&s, "spec", "r", &one("VERDICT: APPROVE"), &GateEvidence::default());
    assert_eq!(
        body.split("\n\n").next().unwrap(),
        "> [!WARNING]\n> This branch conflicts with `main`, so this is a draft. Merge `main` into it and resolve the conflicts in:\n> - `a.py`\n> - `docs/b.md`"
    );
}

#[test]
fn the_conflict_warning_is_removed_and_nothing_else() {
    let files = ["a.py".to_string(), "docs/b.md".to_string()];
    let warning = conflict_warning(Some("main"), &files);
    let cases: Vec<(String, Option<&str>)> = vec![
        (format!("{warning}\n\nOpened by run r.\n"), Some("Opened by run r.\n")),
        (format!("A note.\n\n{warning}\n\nOpened by run r.\n"), Some("A note.\n\nOpened by run r.\n")),
        (format!("Opened by run r.\n\n{warning}\n"), Some("Opened by run r.\n")),
        (format!("{warning}\n\nOpened by run r.\n").replace('\n', "\r\n"), Some("Opened by run r.\r\n")),
        (warning.clone(), Some("")),
        ("Opened by run r.\n".into(), Some("Opened by run r.\n")),
        // Not the run's own warning: another base, other files, or edited.
        (format!("{}\n\nx", conflict_warning(Some("dev"), &files)), None),
        (format!("{}\n\nx", conflict_warning(Some("main"), &files[..1])), None),
        (format!("{warning} Fixed it.\n\nx"), None),
        (format!("Text{warning}\n\nx"), None),
    ];
    for (body, without) in cases {
        assert_eq!(without_conflict_warning(&body, Some("main"), &files), without.map_or(body.clone(), String::from), "{body:?}");
    }
}

#[test]
fn no_conflicts_no_warning_and_long_sections_are_truncated() {
    let s = state(|s| s.base_branch = Some("main".into()));
    assert!(pr_body(&s, "spec", "r", &one("VERDICT: APPROVE"), &GateEvidence::default()).starts_with("Opened by"));
    let body = pr_body(&s, &"x".repeat(PR_SECTION_LIMIT * 2), "r", &one("VERDICT: APPROVE"), &GateEvidence::default());
    assert!(body.chars().count() < 65536 && body.contains("truncated"));
}

fn evidence(round: u32, q: u32, text: &str, unjudged: bool) -> GateEvidence {
    GateEvidence { last: Some(QualityFile { round, quality_round: q, text: text.into() }), unjudged }
}

#[test]
fn the_gate_trailer_says_how_the_last_quality_round_ended() {
    let s = state(|s| s.agents.insert("build", [("name", "build-r".to_string()), ("pane", "p1".to_string())].into_iter().collect()));
    let last = |e: &GateEvidence| provenance(&s, e).last().cloned().unwrap();
    assert_eq!(last(&evidence(2, 1, "GATE: OK\n", false)), ("Orchestrator-Gate".into(), "OK after 1 quality round".into()));
    assert_eq!(last(&evidence(2, 3, "garbled", false)), ("Orchestrator-Gate".into(), "? after 3 quality rounds".into()));
    assert_eq!(last(&evidence(1, 1, "GATE: ERROR\n", true)), ("Orchestrator-Gate".into(), "ERROR after 1 quality round, then turned off".into()));
    let keys: Vec<String> = provenance(&s, &GateEvidence { last: None, unjudged: true }).into_iter().map(|(k, _)| k).collect();
    assert_eq!(keys, ["Orchestrator-Run", "Orchestrator-Workflow", "Orchestrator-Role"]);
}

#[test]
fn the_notes_of_a_run_without_a_review_or_a_gate_turned_off() {
    let finished = state(|s| s.verdict = Some(FINISHED.into()));
    assert_eq!(commit_note(&finished), "Orchestrator run r: FINISHED, unreviewed.");
    let body = pr_body(&finished, "spec", "report", &one("never read"), &GateEvidence { last: None, unjudged: true });
    assert!(body.starts_with("> [!NOTE]\n> The quality gate was turned off (`resume --no-quality-gate`) before any analysis finished"));
    assert!(!body.contains("never read"));
    let failed = state(|s| s.verdict = Some(QUALITY_GATE_FAILED.into()));
    assert!(verdict_line(&failed).ends_with("so this is a draft."));
    let unjudged = pr_body(&finished, "spec", "report", &one(""), &evidence(2, 1, "GATE: OK\n", true));
    assert!(unjudged.contains("after its last analysis, and the Builder changed the code since"));
    assert!(unjudged.contains("<summary>Quality gate (round 2, before the last change)</summary>"));
    let changes = state(|s| { s.verdict = Some(CHANGES.into()); s.round = 2; });
    assert_eq!(verdict_line(&changes), "Reviewer verdict after 2 rounds: **CHANGES_REQUESTED**.\n\nThe Reviewer still requested changes after the last round, so this is a draft.");
    assert_eq!(commit_note(&changes), "Orchestrator run r: CHANGES_REQUESTED after 2 review round(s).");
    assert!(pr_body(&changes, "s", "r", &one("VERDICT: CHANGES_REQUESTED"), &GateEvidence::default()).contains("<details open>\n<summary>Review (round 2)</summary>"));
    let unset = state(|s| s.verdict = None);
    assert_eq!(commit_note(&unset), "Orchestrator run r: None after 1 review round(s).");
    assert_eq!(conflict_warning(None, &["a".into()]).lines().nth(1), Some("> This branch conflicts with `None`, so this is a draft. Merge `None` into it and resolve the conflicts in:"));
}

const CHANGES: &str = crate::constants::CHANGES_REQUESTED;

/// git, run in dir with no configuration of the user's, its standard output; panics when it fails.
fn git(dir: &str, args: &[&str], stdin: Option<&str>) -> String {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("git runs");
    child.stdin.take().unwrap().write_all(stdin.unwrap_or("").as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "git {args:?} failed");
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn git_reads_them_as_trailers() {
    let dir = TempDir::new();
    let d = dir.join("");
    git(&d, &["init", "--quiet", "."], None);
    for (key, value) in [("user.name", "t"), ("user.email", "t@example.com"), ("commit.gpgsign", "false")] {
        git(&d, &["config", key, value], None);
    }
    let s = state(|s| {
        s.run_id = "20260929-120000-a1b2c3".into();
        for role in ["spec", "build", "review"] {
            s.agents.insert(role, [("name", format!("{role}-a1b2c3")), ("pane", "p".into())].into_iter().collect());
        }
        s.agents.get_mut("review").unwrap().insert("kind", "codex".into());
        s.models.insert("review", "gpt-5".to_string());
        s.quality_round = 2;
    });
    let trailers: Vec<String> = provenance(&s, &evidence(1, 2, "GATE: OK\n", false))
        .into_iter()
        .flat_map(|(k, v)| ["--trailer".to_string(), format!("{k}: {v}")])
        .collect();
    let note = commit_note(&s);
    let mut args = vec!["commit", "--quiet", "--allow-empty", "-m", "Add a limiter", "-m", &note];
    args.extend(trailers.iter().map(String::as_str));
    git(&d, &args, None);
    let message = git(&d, &["log", "-1", "--format=%B"], None);
    assert_eq!(
        git(&d, &["interpret-trailers", "--parse"], Some(&message)),
        "Orchestrator-Run: 20260929-120000-a1b2c3\nOrchestrator-Workflow: default\nOrchestrator-Role: spec=claude/default\n\
         Orchestrator-Role: build=claude/default\nOrchestrator-Role: review=codex/gpt-5\nOrchestrator-Gate: OK after 2 quality rounds\n"
    );
    assert!(message.contains(&note));
}
