//! Where a record first parts from its recording.

use serde_json::Value;

/// The longest part of a value a difference quotes, in characters.
const QUOTED: usize = 300;

/// Where got first differs from want, as a JSON path and the two sides there; None when they are equal, keys in the
/// same order. Two strings are told apart by the line on which they part.
pub fn first_difference(want: &Value, got: &Value, path: &str) -> Option<String> {
    if std::mem::discriminant(want) != std::mem::discriminant(got) {
        return Some(format!("{path}: recorded {}, got {}", cut(&want.to_string()), cut(&got.to_string())));
    }
    match (want, got) {
        (Value::Object(a), Value::Object(b)) => {
            for key in a.keys().chain(b.keys().filter(|k| !a.contains_key(k.as_str()))) {
                let (Some(x), Some(y)) = (a.get(key), b.get(key)) else {
                    let side = if a.contains_key(key) { "recorded" } else { "in this run" };
                    return Some(format!("{path}: key {key:?} only {side}"));
                };
                if let Some(found) = first_difference(x, y, &format!("{path}[{key:?}]")) {
                    return Some(found);
                }
            }
            let (ka, kb): (Vec<&str>, Vec<&str>) = (a.keys().map(String::as_str).collect(), b.keys().map(String::as_str).collect());
            (ka != kb).then(|| format!("{path}: keys in another order: recorded {ka:?}, got {kb:?}"))
        }
        (Value::Array(a), Value::Array(b)) => {
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                if let Some(found) = first_difference(x, y, &format!("{path}[{i}]")) {
                    return Some(found);
                }
            }
            (a.len() != b.len()).then(|| {
                let (side, extra) = if a.len() > b.len() { ("recorded", &a[b.len()]) } else { ("got", &b[a.len()]) };
                format!("{path}: recorded {} items, got {}; first unmatched ({side}): {}", a.len(), b.len(), cut(&extra.to_string()))
            })
        }
        (Value::String(a), Value::String(b)) if a != b => {
            let (la, lb): (Vec<&str>, Vec<&str>) = (a.split('\n').collect(), b.split('\n').collect());
            let i = la.iter().zip(&lb).position(|(x, y)| x != y).unwrap_or(la.len().min(lb.len()));
            let line = |lines: &[&str]| lines.get(i).map_or("(no line)".to_string(), |l| format!("{l:?}"));
            Some(format!("{path}, line {}:\n  recorded: {}\n  got:      {}", i + 1, line(&la), line(&lb)))
        }
        _ => (want != got).then(|| format!("{path}: recorded {}, got {}", want, got)),
    }
}

fn cut(text: &str) -> String {
    text.chars().take(QUOTED).collect()
}
