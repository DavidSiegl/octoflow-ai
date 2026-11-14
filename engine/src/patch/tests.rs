use super::*;
use serde_json::Value;
use crate::test_support::oracle;


/// added_lines as host.json records it: each line's number as a string, its text as a string.
fn as_recorded(added: &Map<AddedLines>) -> Value {
    Value::Object(
        added
            .iter()
            .map(|(path, lines)| {
                let lines = lines
                    .iter()
                    .map(|(n, text)| (n.to_string(), Value::from(std::str::from_utf8(text).expect("UTF-8"))))
                    .collect();
                (path.to_string(), Value::Object(lines))
            })
            .collect(),
    )
}

#[test]
fn added_lines_reads_each_diff_as_host_json_records() {
    for case in oracle("host.json").get("added_lines").cloned().unwrap().as_array().unwrap() {
        let case = case.as_array().unwrap();
        let diff = case[0].as_str().unwrap();
        let added = added_lines(diff.as_bytes()).expect("a diff of UTF-8 paths");
        assert_eq!(as_recorded(&added), case[1], "{diff:?}");
        let order: Vec<&str> = added.keys().collect();
        let recorded: Vec<&str> = case[1].as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(order, recorded, "{diff:?}");
    }
}

#[test]
fn an_added_line_keeps_bytes_that_are_not_utf8() {
    let added = added_lines(b"diff --git a/l.txt b/l.txt\n+++ b/l.txt\n@@ -0,0 +1 @@\n+caf\xe9\r\n").unwrap();
    assert_eq!(added.get("l.txt").unwrap().get(&1).unwrap(), b"caf\xe9\r");
}

#[test]
fn a_path_that_is_not_utf8_is_refused() {
    for diff in [
        &b"diff --git a/\xe9 b/\xe9\n"[..],
        b"diff --git \"a/\\351\" \"b/\\351\"\n",
        b"+++ b/\xe9\n",
        b"rename to \xe9\n",
    ] {
        let e = added_lines(diff).unwrap_err();
        assert!(e.0.starts_with("the diff names a path that is not UTF-8: \""), "{diff:?}: {e}");
        assert!(e.0.ends_with("\u{fffd}\""), "{diff:?}: {e}");
    }
}

#[test]
fn a_quoted_name_reads_as_git_writes_it() {
    // A multibyte character after a backslash stands for itself, and a backslash before a line end is no escape.
    assert_eq!(unquote("\"\\é\\\"\"".as_bytes()), "é\"".as_bytes());
    assert_eq!(quoted_len(b"\"a\\\nb\""), None);
    assert_eq!(quoted_len(b"\"open"), None);
    assert_eq!(quoted_pair(b"\"a\" \"b\" x"), None);
    assert_eq!(unquote(b"\"\\9\""), b"9");
    assert_eq!(unquote(b"\""), b"\"");
}
