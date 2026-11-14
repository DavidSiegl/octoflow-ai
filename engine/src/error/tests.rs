use super::*;

#[test]
fn an_error_shows_its_message() {
    let e = OrchestratorError::new("it failed");
    assert_eq!(e.to_string(), "it failed");
    assert_eq!(e, OrchestratorError("it failed".into()));
    let boxed: Box<dyn std::error::Error> = Box::new(e);
    assert_eq!(boxed.to_string(), "it failed");
}

#[test]
fn strerror_leaves_out_the_os_error_number() {
    let missing = std::fs::read("/nonexistent/file").unwrap_err();
    assert_eq!(strerror(&missing), "No such file or directory");
    assert_eq!(os_error(&missing, Some("/nonexistent/file")), "/nonexistent/file: No such file or directory");
    assert_eq!(os_error(&missing, Some("/my file")), "\"/my file\": No such file or directory");
    assert_eq!(os_error(&missing, Some("/a\nb")), "\"/a\\nb\": No such file or directory");
    assert_eq!(os_error(&missing, None), "No such file or directory");
    assert_eq!(os_error(&std::io::Error::other("odd"), Some("p")), "odd");
    let custom = std::io::Error::other("no luck (os error 2)");
    assert_eq!(strerror(&custom), "no luck (os error 2)");
}

#[test]
fn a_long_text_is_cut_to_n_characters_with_an_ellipsis() {
    assert_eq!(shortened("abc", 3), "abc");
    assert_eq!(shortened("abcd", 3), "ab…");
    assert_eq!(shortened("äöüß", 2), "ä…");
}

#[test]
fn the_closest_name_is_the_fewest_edits_away_within_half_its_length() {
    let names = ["spec", "build", "review", "tests"];
    assert_eq!(closest_name("reviw", names), Some("review"));
    assert_eq!(closest_name("sepc", names), Some("spec"));
    assert_eq!(closest_name("tset", ["test"]), Some("test"), "a swap is two edits");
    assert_eq!(closest_name("deploy", names), None);
    assert_eq!(closest_name("x", ["y"]), None);
    assert_eq!(closest_name("ab", ["ax", "ay"]), Some("ax"), "the first of a tie");
    assert_eq!(closest_name("x", []), None);
}

#[test]
fn text_that_is_not_toml_says_where() {
    let why = |text: &str| toml_syntax_error(text, &text.parse::<toml::Table>().unwrap_err());
    assert_eq!(why("a = 1\n[b\n").split(" (at ").nth(1), Some("line 2, column 3)"));
    assert!(why("a = 1\nb").ends_with("(at line 2, column 2)"), "{}", why("a = 1\nb"));
    // The column counts characters, not bytes.
    assert!(why("\"é\" = @").ends_with("(at line 1, column 7)"), "{}", why("\"é\" = @"));
}
