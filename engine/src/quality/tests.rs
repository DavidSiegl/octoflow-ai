use super::*;
use serde_json::Value;
use crate::json::Object;
use crate::test_support::{oracle, Rng};
use crate::json::object;

fn case_text(case: &Value, key: &str) -> String {
    case.get(key).and_then(Value::as_str).unwrap_or_else(|| panic!("no {key}")).to_string()
}

/// The quality file of quality.json named from.
fn quality_file(from: &str) -> String {
    let quality = oracle("quality.json");
    let files = quality.get("files").and_then(Value::as_array).unwrap();
    let file = files.iter().find(|f| f.get("from").and_then(Value::as_str) == Some(from)).unwrap_or_else(|| panic!("no {from}"));
    case_text(file, "text")
}

fn fingerprints_json(failures: &GateFailures) -> String {
    serde_json::to_string(&failures.fingerprints).unwrap()
}

#[test]
fn every_quality_file_reads_as_quality_json_records() {
    let quality = oracle("quality.json");
    let files = quality.get("files").and_then(Value::as_array).unwrap();
    assert!(files.len() >= 25, "quality.json holds {} files", files.len());
    for file in files {
        let (from, text) = (case_text(file, "from"), case_text(file, "text"));
        assert_eq!(parse_gate(&text), file.get("gate").and_then(Value::as_str), "{from}");
        let failures = gate_failures(&text);
        let conditions = Value::Array(failures.conditions.iter().map(|c| Value::from(c.as_str())).collect());
        assert_eq!(conditions.to_string(), file.get("conditions").unwrap().to_string(), "{from}");
        assert_eq!(fingerprints_json(&failures), file.get("fingerprints").unwrap().to_string(), "{from}");
    }
}

fn leak(value: &Value) -> Leak {
    let field = |key: &str| case_text(value, key);
    Leak {
        rule: field("rule"),
        file: field("file"),
        line: value.get("line").and_then(Value::as_i64).unwrap() as u64,
        commit: field("commit"),
        fingerprint: field("fingerprint"),
    }
}

#[test]
fn the_engines_quality_files_are_written_as_quality_json_records() {
    let quality = oracle("quality.json");
    for case in quality.get("leak_reports").and_then(Value::as_array).unwrap() {
        let leaks: Vec<Leak> = case.get("leaks").and_then(Value::as_array).unwrap().iter().map(leak).collect();
        assert_eq!(leak_report(&leaks, &case_text(case, "intro")), case_text(case, "text"));
    }
    for case in quality.get("one_line").and_then(Value::as_array).unwrap() {
        let pair = case.as_array().unwrap();
        assert_eq!(one_line(pair[0].as_str().unwrap()), pair[1].as_str().unwrap());
    }
    for case in quality.get("fingerprints").and_then(Value::as_array).unwrap() {
        let c = case.as_array().unwrap();
        assert_eq!(finding_fingerprint(c[0].as_str(), c[1].as_str().unwrap(), c[2].as_str().unwrap()), c[3].as_str().unwrap());
    }
}

/// A leak gitleaks reports, with changes.
fn a_leak(change: impl FnOnce(&mut Leak)) -> Leak {
    let mut leak = Leak {
        rule: "github-pat".into(),
        file: "limiter.py".into(),
        line: 3,
        commit: "c0ffee".repeat(6) + "c0ff",
        fingerprint: "c0ffee:limiter.py:github-pat:3".into(),
    };
    change(&mut leak);
    leak
}

#[test]
fn reads_the_secrets_of_a_leak_file_whichever_snapshot_and_line() {
    let report = |leaks: &[Leak]| gate_failures(&leak_report(leaks, "found."));
    let first = report(&[a_leak(|l| l.commit = "snap1".into())]);
    let moved = a_leak(|l| {
        l.line = 9;
        l.commit = "snap2".into();
        l.fingerprint = "snap2:limiter.py:github-pat:9".into();
    });
    let pat = finding_fingerprint(Some("limiter.py"), "gitleaks github-pat", "");
    assert_eq!(fingerprints_json(&first), format!("{{\"gitleaks\":[\"{pat}-1\"]}}"));
    assert!(report(std::slice::from_ref(&moved)).repeats(&first));
    let other_rule = Leak { rule: "aws-access-token".into(), ..moved.clone() };
    assert!(!report(&[other_rule]).repeats(&first));
    // Of a file's two keys of a kind, one removed is progress, not the same again.
    let two = report(&[a_leak(|l| l.commit = "snap1".into()), moved.clone()]);
    assert!(!report(std::slice::from_ref(&moved)).repeats(&two));
    assert!(report(&[moved, a_leak(|l| l.commit = "snap3".into())]).repeats(&two));
}

#[test]
fn a_coverage_condition_repeated_is_stuck_until_its_words_change() {
    let fifty = gate_failures(&quality_file("coverage 50.0"));
    let sixty = gate_failures(&quality_file("coverage 60.0"));
    assert!(fifty.repeats(&gate_failures(&quality_file("coverage 50.0"))));
    assert!(!sixty.repeats(&fifty));
    assert!(fifty.fingerprints.is_empty());
}

#[test]
fn reads_failing_findings_and_tests_and_leaves_findings_to_weigh_out() {
    let failures = gate_failures(&quality_file("findings, tests and findings to weigh"));
    assert_eq!(
        failures.conditions,
        ["1 tests fail; each is listed below.", "1 findings of the `diff-budget` check; each is listed below."]
    );
    let fp = |path: Option<&str>, rule: &str, message: &str| Value::Array(vec![Value::from(finding_fingerprint(path, rule, message))]);
    let expected: Object = object([
        ("ruff", fp(Some("a.py"), "ruff F401", "`os` unused")),
        ("mypy", fp(Some("b.py"), "mypy", "bad: 2 errors")),
        ("tests", fp(Some("tests.test_a.T.test_x"), "tests", "")),
        ("diff-budget", fp(None, "diff-budget", "too big: 900 lines")),
    ]);
    assert_eq!(serde_json::from_str::<Value>(&fingerprints_json(&failures)).unwrap(), Value::Object(expected));
}

#[test]
fn a_fingerprint_ignores_digits_and_keeps_paths_and_rules_apart() {
    let fp = finding_fingerprint;
    assert_eq!(fp(Some("a.py"), "ruff C901", "too complex (16 > 15)"), fp(Some("a.py"), "ruff C901", "too complex (17 > 15)"));
    assert_ne!(fp(Some("a.py"), "ruff C901", "m"), fp(Some("b.py"), "ruff C901", "m"));
    assert_ne!(fp(Some("a.py"), "ruff C901", "m"), fp(Some("a.py"), "ruff E501", "m"));
    assert_eq!(fp(None, "x", "y").len(), 16);
}

#[test]
fn nothing_failed_repeats_nothing() {
    let ok = gate_failures(&quality_file("ok"));
    assert_eq!(ok, GateFailures::default());
    assert!(!ok.repeats(&ok));
}

#[test]
fn read_back_the_gate_of_a_could_not_judge_report() {
    let mut rng = Rng::new(20);
    for _ in 0..200 {
        let (intro, said) = (rng.text(20, ""), rng.text(20, ""));
        let report = could_not_judge_report(&intro, &said);
        assert_eq!(parse_gate(&report), Some(QUALITY_BUILD_FAILED), "{report:?}");
    }
}

#[test]
fn lines_that_only_look_like_findings_are_not_read_as_them() {
    for line in [
        "1. `a.py:3` error", "1. `a.py:x` error r: m", "x. `a.py:3` error r: m", "1. `` error r: m",
        "1. `a.py:3` error : m", "1.`a.py:3` error r: m", "1. `a.py:3`  error r: m",
    ] {
        assert!(numbered_finding(line).is_none(), "{line}");
    }
    let found = numbered_finding("12. `a`b.py:3` (the whole file)` error ruff X: y: z").unwrap();
    // The shortest path the rest of the line allows.
    assert_eq!((found.path, found.rule, found.message), (Some("a`b.py"), "whole file)` error ruff X", "y: z"));
    assert_eq!(failing_test("- `a`: b`: c"), Some("a`: b"));
    assert_eq!(failing_test("- ``: c"), None);
    assert_eq!(secret_found("1. `a:b.py:3` r in c (fingerprint `x`)"), Some(("a:b.py", "r")));
    for line in ["1. `a.py:3` r in c (fingerprint `x`", "1. `a.py:` r in c (fingerprint `x`)", "1. `a.py:3` r in  (fingerprint `x`)"] {
        assert_eq!(secret_found(line), None, "{line}");
    }
}

/// The vendored golden file name.ext of the gate release ci/gate.pin names (engine/tests/data/quality/README.md):
/// the quality file (txt) or quality.json (json) the gate writes in one case of its tests.
fn gate_golden(name: &str, ext: &str) -> String {
    let path = format!("{}/tests/data/quality/{name}.{ext}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// The cases of the vendored golden files, each a quality file and its quality.json.
const GOLDEN_REPORTS: [&str; 7] = [
    "gate_error",
    "gate_ok",
    "quality_report_error",
    "quality_report_error_over_limit",
    "quality_report_error_to_weigh_cut",
    "quality_report_ok",
    "quality_report_ok_to_weigh",
];

#[test]
fn the_engine_reads_what_the_gate_writes() {
    let error = gate_golden("gate_error", "txt");
    assert_eq!(parse_gate(&error), Some(QUALITY_ERROR));
    let failures = gate_failures(&error);
    assert_eq!(
        failures.conditions,
        [
            "1 findings on lines the change added; each is listed below.",
            "1 findings of the `test-integrity` check; each is listed below.",
            "1 tests fail; each is listed below.",
            "Coverage of new code is 50.0%; the gate wants at least 80% once a change has 20 or more new lines to cover.",
        ]
    );
    // The two issues and the failing test; the finding to weigh is left out.
    let mut want = Map::new();
    want.insert("ruff", vec![finding_fingerprint(Some("f.py"), "ruff B023", "Function definition does not bind loop variable `x`")]);
    want.insert(
        "test-integrity",
        vec![finding_fingerprint(
            Some("tests/test_f.py"),
            "test-integrity skip",
            "`pytest.mark.skip` added: it skips a test or expects it to fail.",
        )],
    );
    want.insert("tests", vec![finding_fingerprint(Some("tests.T.test_limit"), "tests", "")]);
    assert_eq!(failures.fingerprints, want);
    assert_eq!(read_report(&gate_golden("gate_error", "json")).unwrap(), Judgement { gate: QUALITY_ERROR, failures });

    let ok = gate_golden("gate_ok", "txt");
    assert_eq!(parse_gate(&ok), Some(QUALITY_OK));
    assert_eq!(gate_failures(&ok), GateFailures::default());
}

#[test]
fn the_engine_reads_a_cut_list_as_the_gate_cuts_it() {
    // quality_report lists QUALITY_ISSUE_LIMIT issues, here all of them `f.py:<n>` with rule r, and counts the rest.
    let failures = gate_failures(&gate_golden("quality_report_error_over_limit", "txt"));
    let issues = failures.fingerprints.get("r").expect("the issues, under their rule");
    assert_eq!(issues.len(), QUALITY_FINDINGS_LIMIT);
    assert_eq!(issues[0], finding_fingerprint(Some("f.py"), "r", "m1"));
}

#[test]
fn quality_json_reads_as_its_quality_file_but_for_what_the_file_cuts() {
    for name in GOLDEN_REPORTS {
        let markdown = gate_golden(name, "txt");
        let (from_file, judged) = (gate_failures(&markdown), read_report(&gate_golden(name, "json")).unwrap());
        assert_eq!(Some(judged.gate), parse_gate(&markdown), "{name}");
        assert_eq!(judged.failures.conditions, from_file.conditions, "{name}");
        // The file lists at most QUALITY_FINDINGS_LIMIT findings and as many failing tests; quality.json all of them,
        // in the same order.
        let checks: Vec<&str> = judged.failures.fingerprints.keys().collect();
        assert_eq!(checks, from_file.fingerprints.keys().collect::<Vec<_>>(), "{name}");
        for (check, listed) in from_file.fingerprints.iter() {
            let all = judged.failures.fingerprints.get(check).unwrap();
            assert!(all.starts_with(listed), "{name}: {check}");
            if all.len() != listed.len() {
                assert_eq!(listed.len(), QUALITY_FINDINGS_LIMIT, "{name}: {check}");
            }
        }
    }
    let cut = read_report(&gate_golden("quality_report_error_over_limit", "json")).unwrap();
    assert!(cut.failures.fingerprints.get("r").unwrap().len() > QUALITY_FINDINGS_LIMIT);
}

/// The golden quality.json of name, as a JSON object with change applied.
fn golden_json(name: &str, change: impl FnOnce(&mut Object)) -> String {
    let mut report: Object = serde_json::from_str(&gate_golden(name, "json")).unwrap();
    change(&mut report);
    serde_json::to_string(&report).unwrap()
}

#[test]
fn an_unknown_schema_version_is_an_error_that_names_it() {
    let newer = golden_json("gate_ok", |r| {
        r.insert("schema_version".into(), Value::from(2));
    });
    let e = read_report(&newer).unwrap_err().0;
    assert!(e.contains("schema_version 2"), "{e}");
    let none = golden_json("gate_ok", |r| {
        r.remove("schema_version");
    });
    let e = read_report(&none).unwrap_err().0;
    assert!(e.contains("schema_version"), "{e}");
}

#[test]
fn a_report_not_of_the_schema_is_an_error() {
    assert!(read_report("GATE: OK\n").is_err());
    let gate = golden_json("gate_ok", |r| {
        r.insert("gate".into(), Value::from("BUILD_FAILED"));
    });
    assert!(read_report(&gate).unwrap_err().0.contains("BUILD_FAILED"));
    let finding = golden_json("quality_report_error", |r| {
        r["findings"][0].as_object_mut().unwrap().remove("fails");
    });
    assert!(read_report(&finding).is_err());
}

#[test]
fn takes_each_finding_s_fingerprint_as_the_report_gives_it() {
    let report = golden_json("quality_report_error", |r| {
        r["findings"][0]["fingerprint"] = Value::from("0123456789abcdef");
    });
    let judged = read_report(&report).unwrap();
    assert_eq!(judged.failures.fingerprints.get("r").unwrap()[0], "0123456789abcdef");
}

#[test]
fn a_failing_test_is_fingerprinted_by_its_name_as_the_quality_file_writes_it() {
    let report = golden_json("gate_error", |r| {
        r["failing_tests"] = serde_json::json!([{"name": "t.T.test_a\nb", "detail": "x"}]);
    });
    let judged = read_report(&report).unwrap();
    let want = finding_fingerprint(Some("t.T.test_a\\x0ab"), "tests", "");
    assert_eq!(judged.failures.fingerprints.get("tests").unwrap(), &vec![want]);
}
