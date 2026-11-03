//! What the port still needs of Python's own semantics: TOML's values as tomllib reads them (toml) and the parts of a
//! URL (url).

pub mod toml;
pub mod url;

/// What engine/tests/oracle/python.py recorded from Python under key, which the tests of this module replay.
#[cfg(test)]
pub(crate) fn recorded(key: &str) -> serde_json::Value {
    let all: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/oracle/python.json")).expect("python.json is JSON");
    all.get(key).cloned().unwrap_or_else(|| panic!("python.json has no {key}"))
}

/// The cases engine/tests/oracle/python.py recorded from Python for one kind of call.
#[cfg(test)]
pub(crate) fn oracle(kind: &str) -> Vec<serde_json::Value> {
    recorded(kind).as_array().cloned().unwrap_or_else(|| panic!("python.json's {kind} is no list"))
}
