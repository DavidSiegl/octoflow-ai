//! JSON as the engine reads and writes it: serde_json's Value, an object keeping its keys in the order they came
//! (serde_json's preserve_order), and what callers ask of a value read from a file they do not control, such as a
//! state.json an earlier step wrote or an answer from PyPI.

use serde_json::Value;

/// A JSON object, its keys in the order they were read or inserted.
pub type Object = serde_json::Map<String, Value>;

/// The object of pairs, in their order, a key repeated keeping its first place and its last value.
pub fn object<'a>(pairs: impl IntoIterator<Item = (&'a str, Value)>) -> Object {
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

/// What a value read from a file is taken to say, beyond serde_json's own accessors.
pub trait ValueExt {
    /// Whether the value is set: not null, false, 0, an empty string, an empty array or an empty object. A record's
    /// optional field is read through it, so that a field written as null, "" or [] counts as absent.
    fn is_set(&self) -> bool;

    /// The value as a message shows it: a string as it is, anything else as compact JSON (`null`, `true`, `2.5`,
    /// `["a",1]`).
    fn text(&self) -> String;

    /// The JSON type's name, for a message that says what a value is instead of what it should be: null, boolean,
    /// number, string, array or object.
    fn kind(&self) -> &'static str;
}

impl ValueExt for Value {
    fn is_set(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Number(n) => n.as_f64() != Some(0.0),
            Value::String(s) => !s.is_empty(),
            Value::Array(items) => !items.is_empty(),
            Value::Object(map) => !map.is_empty(),
        }
    }

    fn text(&self) -> String {
        match self {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        }
    }
}

#[cfg(test)]
mod tests;
