use super::*;

fn values(name: &str) -> Option<String> {
    ["task", "spec_path", "prev_my-step", "n"].contains(&name).then(|| format!("<{name}>"))
}

#[test]
fn fill_replaces_each_placeholder_and_undoubles_braces() {
    assert_eq!(fill("Do {task} per {spec_path}; {task} again.", &values).unwrap(), "Do <task> per <spec_path>; <task> again.");
    assert_eq!(fill("{{x}} {{{n}}} }}{{", &values).unwrap(), "{x} {<n>} }{");
    assert_eq!(fill("read {prev_my-step}", &values).unwrap(), "read <prev_my-step>");
    assert_eq!(fill("", &values).unwrap(), "");
}

#[test]
fn placeholders_lists_each_name_in_order() {
    assert_eq!(placeholders("{a} {b} {a} {{d}}").unwrap(), ["a", "b", "a"]);
    assert_eq!(
        parse("x {a}{{y").unwrap(),
        [Piece::Text("x ".into()), Piece::Placeholder("a".into()), Piece::Text("{y".into())]
    );
}

#[test]
fn each_refused_brace_names_the_placeholder_and_where_it_is() {
    for (template, message) in [
        ("Write {task!r}.", "placeholder {task!r} at line 1, column 7 has a conversion; a placeholder takes none"),
        ("{task:>10}", "placeholder {task:>10} at line 1, column 1 has a format spec; a placeholder takes none"),
        ("a\nb {task:{n}}", "placeholder {task:{n} at line 2, column 3 has a format spec; a placeholder takes none"),
        ("{diff.upper}", "placeholder {diff.upper} at line 1, column 1 has an attribute; a placeholder takes none"),
        ("{diff[0]}", "placeholder {diff[0]} at line 1, column 1 has an index; a placeholder takes none"),
        ("Ask {}.", "placeholder {} at line 1, column 5 is a positional field; a placeholder is a name"),
        ("{0}", "placeholder {0} at line 1, column 1 is a positional field; a placeholder is a name"),
        ("{0!r}", "placeholder {0!r} at line 1, column 1 is a positional field; a placeholder is a name"),
        ("{ task }", "placeholder { task } at line 1, column 1 is not a name; a name may use only letters, digits, - and _"),
        ("é {a{b}", "placeholder {a{b} at line 1, column 3 is not a name; a name may use only letters, digits, - and _"),
        ("x\n\n  {task", "{ at line 3, column 3 is not closed; write a literal brace as {{"),
        ("{task}}", "} at line 1, column 7 closes no placeholder; write a literal brace as }}"),
        ("Write {nope}.", "placeholder {nope} at line 1, column 7 has no value"),
    ] {
        assert_eq!(fill(template, &values).unwrap_err().to_string(), message, "{template:?}");
    }
}

#[test]
fn an_unreadable_placeholder_is_refused_before_a_missing_value_is() {
    let e = fill("{nope} {task!r}", &values).unwrap_err();
    assert!(e.to_string().starts_with("placeholder {task!r}"), "{e}");
}
