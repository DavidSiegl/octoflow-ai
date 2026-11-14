//! PyPI as the new-dependencies check asks it, through a FakePyPI: the URL asked, the answers read, and how an answer
//! of another shape is worded.

use std::sync::Mutex;

use super::fake::{FakePyPI, ProjectJson};
use super::*;
use crate::http::Response;

fn project(answer: Result<Response, HttpError>) -> Result<Option<IndexedPackage>, IndexError> {
    struct Once(Mutex<Option<Result<Response, HttpError>>>);
    impl Http for Once {
        fn send(&self, request: &Request) -> Result<Response, HttpError> {
            assert_eq!(request.url, "https://pypi.org/pypi/limits/json");
            self.0.lock().unwrap().take().expect("one request")
        }
    }
    PyPI::new(Arc::new(Once(Mutex::new(Some(answer))))).project("limits")
}

fn body(text: &str) -> Result<Response, HttpError> {
    Ok(Response::ok(text))
}

#[test]
fn reads_dates_version_and_license() {
    let fake = FakePyPI::with(&[("limits", ProjectJson::default())]);
    let found = PyPI::new(fake.clone()).project("limits").unwrap().unwrap();
    assert_eq!(found.first_release, Some(Upload { at: 1_420_279_200.123456, date: "2015-01-03".to_string() }));
    assert_eq!(found.latest_release, Some(Upload { at: 1_719_050_400.123456, date: "2024-06-22".to_string() }));
    assert_eq!((found.version.as_str(), found.license.as_deref()), ("3.13.0", Some("MIT")));
    assert_eq!(fake.state().asked, ["limits"]);
}

#[test]
fn a_license_is_the_expression_then_the_field_then_the_classifier() {
    let long = format!("BSD {}", "x".repeat(100));
    let cases: Vec<(ProjectJson, Option<String>)> = vec![
        (ProjectJson { expression: Some("Apache-2.0"), ..ProjectJson::default() }, Some("Apache-2.0".to_string())),
        (
            ProjectJson { license: Some("BSD\n\nlong text of the licence"), ..ProjectJson::default() },
            Some("BSD".to_string()),
        ),
        (
            ProjectJson {
                license: Some(""),
                classifiers: vec!["Topic :: X", "License :: OSI Approved :: MIT License"],
                ..ProjectJson::default()
            },
            Some("MIT License".to_string()),
        ),
        (ProjectJson { license: None, ..ProjectJson::default() }, None),
        (
            ProjectJson { license: Some(Box::leak(long.clone().into_boxed_str())), ..ProjectJson::default() },
            Some(format!("{}…", &long[..79])),
        ),
    ];
    for (json, license) in cases {
        assert_eq!(project(body(&json.json().to_string())).unwrap().unwrap().license, license);
    }
}

#[test]
fn a_project_without_files_has_no_release_dates() {
    let found =
        project(body(r#"{"info": {"version": "0.0.1"}, "releases": {"0.0.1": []}, "urls": []}"#)).unwrap().unwrap();
    assert_eq!((found.first_release, found.latest_release), (None, None));
    let bare = project(body(r#"{"info": {"version": "0.0.1"}, "releases": null}"#)).unwrap().unwrap();
    assert_eq!((bare.first_release, bare.license), (None, None));
}

#[test]
fn an_unknown_project_is_none() {
    assert_eq!(project(Err(HttpError::Status(404))), Ok(None));
    assert_eq!(PyPI::new(FakePyPI::with(&[])).project("limitz"), Ok(None));
}

#[test]
fn any_other_failure_raises() {
    let url = "https://pypi.org/pypi/limits/json";
    let cases = [
        (Err(HttpError::Status(503)), format!("GET {url}: HTTP 503")),
        (
            Err(HttpError::NoAnswer("[Errno -3] name resolution".to_string())),
            format!("GET {url}: [Errno -3] name resolution"),
        ),
        (Err(HttpError::NoAnswer("timed out".to_string())), format!("GET {url}: timed out")),
        (body("<html>"), format!("GET {url}: the answer is not PyPI's JSON")),
        (body("[]"), format!("GET {url}: the answer is not PyPI's JSON")),
        (body(r#"{"info": {}}"#), format!("GET {url}: the answer is not PyPI's JSON")),
    ];
    for (answer, message) in cases {
        let e = project(answer).unwrap_err();
        assert!(e.to_string().starts_with(&message), "{e}");
    }
}

#[test]
fn json_of_another_shape_says_what_was_found_where() {
    let info = r#""info": {"version": "1"}"#;
    let cases = [
        (r#"{"x": 1}"#.to_string(), "\"info\""),
        (r#"[]"#.to_string(), "it has no info.version"),
        (format!(r#"{{{info}, "releases": [1]}}"#), "releases is array, not an object"),
        (format!(r#"{{{info}, "releases": {{"1": 5}}}}"#), "number where an array belongs"),
        (format!(r#"{{{info}, "urls": [3]}}"#), "a file is number, not an object"),
        (format!(r#"{{{info}, "urls": [{{}}]}}"#), "\"upload_time_iso_8601\""),
        (format!(r#"{{{info}, "urls": [{{"upload_time_iso_8601": 1}}]}}"#), "upload_time_iso_8601 is not a string"),
        (format!(r#"{{{info}, "urls": [{{"upload_time_iso_8601": "yesterday"}}]}}"#), ""),
        (r#"{"info": {"version": "1", "license_expression": 7}}"#.to_string(), "license_expression is number, not a string"),
        (r#"{"info": {"version": "1", "classifiers": [null]}}"#.to_string(), "a classifier is null, not a string"),
    ];
    let licensed = |classifiers: &str| format!(r#"{{"info": {{"version": "1", "classifiers": {classifiers}}}}}"#);
    assert_eq!(project(body(&licensed(r#""License :: MIT""#))).unwrap().unwrap().license, None);
    assert_eq!(
        project(body(&licensed(r#"{"License :: OSI :: BSD": 1}"#))).unwrap().unwrap().license.as_deref(),
        Some("BSD")
    );
    let late = project(body(&licensed(r#"["License :: MIT", 5]"#))).unwrap_err().to_string();
    assert!(late.ends_with("a classifier is number, not a string"), "{late}");
    for (text, why) in cases {
        let e = project(body(&text)).unwrap_err().to_string();
        assert!(
            e.starts_with(&format!("GET https://pypi.org/pypi/limits/json: the answer is not PyPI's JSON: {why}")),
            "{text}: {e}"
        );
    }
    assert!(project(Ok(Response::ok(b"\xff".to_vec())))
        .unwrap_err()
        .to_string()
        .ends_with("invalid utf-8 sequence of 1 bytes from index 0"));
}

#[test]
fn the_earliest_upload_wins_by_its_fraction_of_a_second_and_its_offset() {
    let text = r#"{"info": {"version": "1"}, "urls": [{"upload_time_iso_8601": "2024-06-22T10:00:00.5Z"},
        {"upload_time_iso_8601": "2024-06-22T10:00:00.25Z"},
        {"upload_time_iso_8601": "2024-06-22T11:00:00.1+02:00"}]}"#;
    let latest = project(body(text)).unwrap().unwrap().latest_release.unwrap();
    assert_eq!(latest, Upload { at: 1_719_046_800.1, date: "2024-06-22".to_string() });
}

#[test]
fn an_upload_time_is_an_rfc_3339_time_with_its_offset() {
    let cases = [
        ("2026-10-28T12:00:00Z", 1_793_188_800.0, "2026-10-28"),
        ("2026-10-28T23:00:00-02:00", 1_793_235_600.0, "2026-10-28"),
        ("2026-10-28T12:00:00.5Z", 1_793_188_800.5, "2026-10-28"),
    ];
    for (time, at, date) in cases {
        assert_eq!(Upload::from_iso(time), Ok(Upload { at, date: date.to_string() }), "{time}");
    }
    for bad in ["20261003", "2026-10-28T12:00:00", "yesterday"] {
        assert!(Upload::from_iso(bad).is_err(), "{bad}");
    }
}

#[test]
fn a_name_is_quoted_into_its_url() {
    let fake = FakePyPI::with(&[]);
    PyPI::new(fake.clone()).project("a b").unwrap();
    assert_eq!(fake.state().asked, ["a%20b"]);
    fake.state().down = true;
    assert_eq!(
        PyPI::new(fake).project("x").unwrap_err(),
        IndexError("GET https://pypi.org/pypi/x/json: timed out".to_string())
    );
}
