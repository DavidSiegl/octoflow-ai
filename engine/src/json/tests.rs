use super::*;

fn read(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

#[test]
fn a_value_is_set_unless_it_is_null_false_zero_or_empty() {
    for unset in ["null", "false", "0", "0.0", "\"\"", "[]", "{}"] {
        assert!(!read(unset).is_set(), "{unset}");
    }
    for set in ["true", "1", "0.5", "\"x\"", "[0]", "{\"a\": null}"] {
        assert!(read(set).is_set(), "{set}");
    }
}

#[test]
fn text_is_a_string_as_it_is_and_anything_else_as_compact_json() {
    assert_eq!(read(r#""plain""#).text(), "plain");
    assert_eq!(read(r#"[null, true, 2.5, "it's", {"k": []}]"#).text(), r#"[null,true,2.5,"it's",{"k":[]}]"#);
}

#[test]
fn objects_keep_their_keys_in_order_and_a_repeated_key_its_first_place() {
    assert_eq!(read(r#"{"b": 1, "a": 2, "b": 3}"#).to_string(), r#"{"b":3,"a":2}"#);
}

#[test]
fn kind_names_the_json_type() {
    let kinds: Vec<_> = ["null", "true", "1", "\"s\"", "[]", "{}"].iter().map(|t| read(t).kind()).collect();
    assert_eq!(kinds, ["null", "boolean", "number", "string", "array", "object"]);
}
