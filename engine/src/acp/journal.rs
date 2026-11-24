//! A role's journal, `<role>.events.jsonl` in its run directory: what its agents did, for the app to show and replay
//! (the format is design-03-run-socket.md's, in `.scratch/herdr-gui-decoupling/`). The orchestrator alone appends
//! to it; every client reads the file. One JSON object a line: first `{"journal": PROTOCOL}`, then each event with
//! `seq` (1 up, per file, no gaps), `at` (RFC 3339, to the second) and `kind`, then the kind's own fields:
//!
//! | kind | fields |
//! |---|---|
//! | `session` | `session`, `harness`, `resumed` |
//! | `prompt` | `text`, `from` (`orchestrator` or `human`) |
//! | `update` | `update`: ACP's `session/update` `params.update`, verbatim |
//! | `permission` | `request` (Journal::permission's number), `toolCall`, `options`, verbatim from ACP |
//! | `answered` | `request`, `option` (an optionId, or `cancelled`), `by` (`human` or `stop`) |
//! | `turn_end` | `stopReason` |
//! | `waiting` | `why` (crate::agents::Waiting::name): the run waits on the human's message, until the next `prompt` or `exited` |
//! | `exited` | `error`, absent when the agent's process ended without one |
//!
//! A file holds every session its role ran, in order; a client replays the current one, from its last `session` line.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use crate::error::OrchestratorError;
use crate::summary::{epoch_seconds, iso_utc};

/// The version of the journal's lines and of the run socket's messages, one number for both: raised by a change an
/// older reader would get wrong (a field renamed or removed, a meaning changed), not by a field or kind added, which
/// every reader skips.
pub const PROTOCOL: u64 = 1;

/// A role's journal's current session, as replay reads it.
#[derive(Debug, PartialEq)]
pub struct Replay {
    /// The `seq` of the current session's `session` line; None when the journal has none yet.
    pub session_start: Option<u64>,
    /// The current session's events after the one replay was given, oldest first, each the object its line holds.
    pub events: Vec<Value>,
}

/// The current session of the journal whose text is text, from its last `session` line on (from its first event
/// when it has none), each event of it whose `seq` is above after: what a client that has seen the session up to
/// after still has to show. A line that is not a JSON object with a `seq` is passed over, as a line cut short by a
/// crash is; an empty text, a journal not yet begun, has no events. Errors: the journal's version, from its first
/// line (None for a first line that names none), when it is not PROTOCOL.
pub fn replay(text: &str, after: u64) -> Result<Replay, Option<u64>> {
    let (mut session_start, mut events) = (None, vec![]);
    if text.is_empty() {
        return Ok(Replay { session_start, events });
    }
    let mut lines = text.lines();
    let version = lines.next().and_then(|l| serde_json::from_str::<Value>(l).ok()).and_then(|h| h["journal"].as_u64());
    if version != Some(PROTOCOL) {
        return Err(version);
    }
    for line in lines {
        let Ok(event) = serde_json::from_str::<Value>(line) else { continue };
        let Some(seq) = event.get("seq").and_then(Value::as_u64) else { continue };
        if event.get("kind").and_then(Value::as_str) == Some("session") {
            session_start = Some(seq);
            events.clear();
        }
        if seq > after {
            events.push(event);
        }
    }
    Ok(Replay { session_start, events })
}

/// How many of the seconds from `from` to `to` (seconds since the epoch) the journal whose text is text has an agent
/// waiting for the answer to a permission it asked for: from each `permission` line to the `answered` line of its
/// `request`, or with none, to the next `exited` or `session` line, or to `to`. Waits that overlap count once. A line
/// that does not read, or whose `at` is not a time, is passed over; an empty text has no waits.
pub fn waited_seconds(text: &str, from: i64, to: i64) -> i64 {
    let mut asked: Vec<(u64, i64)> = vec![];
    let mut waits: Vec<(i64, i64)> = vec![];
    for line in text.lines() {
        let Ok(event) = serde_json::from_str::<Value>(line) else { continue };
        let Some(at) = event.get("at").and_then(Value::as_str).and_then(|at| epoch_seconds(at).ok()) else { continue };
        let request = event.get("request").and_then(Value::as_u64);
        match event.get("kind").and_then(Value::as_str) {
            Some("permission") => asked.extend(request.map(|r| (r, at))),
            Some("answered") => {
                if let Some(i) = asked.iter().position(|&(r, _)| Some(r) == request) {
                    waits.push((asked.swap_remove(i).1, at));
                }
            }
            Some("exited" | "session") => waits.extend(asked.drain(..).map(|(_, start)| (start, at))),
            _ => {}
        }
    }
    waits.extend(asked.into_iter().map(|(_, start)| (start, to)));
    // The waits clipped to [from, to], then merged in order of their start, so that an overlap counts once.
    let mut clipped: Vec<(i64, i64)> =
        waits.into_iter().map(|(a, b)| (a.max(from), b.min(to))).filter(|(a, b)| a < b).collect();
    clipped.sort_unstable();
    let (mut total, mut covered) = (0, from);
    for (a, b) in clipped {
        let a = a.max(covered);
        if a < b {
            total += b - a;
            covered = b;
        }
    }
    total
}

/// A role's journal file, open for appending.
pub(super) struct Journal {
    lines: Mutex<Lines>,
}

/// The file and the numbers its next lines take.
struct Lines {
    file: File,
    /// The `seq` of the last line written; 0 before any event.
    seq: u64,
    /// The `request` of the last `permission` written; 0 before any.
    request: u64,
}

impl Journal {
    /// The journal at path, created with its first line where it does not exist or is empty, and otherwise continued:
    /// its next event takes the `seq` after the file's last, and its next permission the `request` after the file's
    /// last. Errors: the file cannot be read, created or written.
    pub(super) fn open(path: &str) -> Result<Journal, OrchestratorError> {
        let cannot = |what: &str, e: std::io::Error| OrchestratorError(format!("cannot {what} {path}: {e}"));
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(cannot("read", e)),
        };
        let mut file = OpenOptions::new().create(true).append(true).open(path).map_err(|e| cannot("open", e))?;
        let (mut seq, mut request) = (0, 0);
        for line in text.lines() {
            let Ok(event) = serde_json::from_str::<Value>(line) else { continue };
            seq = event.get("seq").and_then(Value::as_u64).unwrap_or(seq).max(seq);
            if event.get("kind").and_then(Value::as_str) == Some("permission") {
                request = event.get("request").and_then(Value::as_u64).unwrap_or(request).max(request);
            }
        }
        if text.is_empty() {
            file.write_all(format!("{}\n", json!({"journal": PROTOCOL})).as_bytes()).map_err(|e| cannot("write", e))?;
        }
        Ok(Journal { lines: Mutex::new(Lines { file, seq, request }) })
    }

    /// Append an event of kind with fields, a JSON object's.
    pub(super) fn write(&self, kind: &str, fields: Value) {
        self.lines().append(kind, fields);
    }

    /// Append the agent's request for a permission, as ACP's `session/request_permission` params has it, and return
    /// the number the journal gives it.
    pub(super) fn permission(&self, params: &Value) -> u64 {
        let mut lines = self.lines();
        lines.request += 1;
        let request = lines.request;
        let field = |key: &str| params.get(key).cloned().unwrap_or(Value::Null);
        lines.append(
            "permission",
            json!({"request": request, "toolCall": field("toolCall"), "options": field("options")}),
        );
        request
    }

    /// The lines, whatever thread panicked holding them: every change to them is whole.
    fn lines(&self) -> std::sync::MutexGuard<'_, Lines> {
        self.lines.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Lines {
    fn append(&mut self, kind: &str, fields: Value) {
        self.seq += 1;
        let at = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let mut event = Map::new();
        event.insert("seq".to_string(), self.seq.into());
        event.insert("at".to_string(), iso_utc(i64::try_from(at).unwrap_or(i64::MAX)).into());
        event.insert("kind".to_string(), kind.into());
        if let Value::Object(fields) = fields {
            event.extend(fields);
        }
        // A line that cannot be written is lost to the app's view of the agent alone: the journal is what clients
        // show, never what the run decides by, so its failure does not stop the run. One write call a line, so that a
        // reader never sees half of one and a later line still starts on a line of its own.
        let _ = self.file.write_all(format!("{}\n", Value::Object(event)).as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    /// A journal's text of the lines given as (seconds since the epoch, kind, the kind's fields), numbered in order.
    fn journal(lines: &[(i64, &str, Value)]) -> String {
        let mut text = format!("{}\n", json!({"journal": PROTOCOL}));
        for (i, (at, kind, fields)) in lines.iter().enumerate() {
            let mut event = json!({"seq": i + 1, "at": iso_utc(*at), "kind": kind});
            event.as_object_mut().unwrap().extend(fields.as_object().unwrap().clone());
            text.push_str(&format!("{event}\n"));
        }
        text
    }

    #[test]
    fn waited_seconds_sums_each_permission_until_its_answer() {
        let text = journal(&[
            (100, "session", json!({"session": "s", "harness": "claude", "resumed": false})),
            (110, "permission", json!({"request": 1})),
            (150, "answered", json!({"request": 1, "option": "allow", "by": "human"})),
            (160, "permission", json!({"request": 2})),
            (163, "answered", json!({"request": 2, "option": "allow", "by": "human"})),
        ]);

        assert_eq!(waited_seconds(&text, 100, 200), 43);
    }

    #[test]
    fn waited_seconds_counts_overlapping_waits_once() {
        let text = journal(&[
            (110, "permission", json!({"request": 1})),
            (120, "permission", json!({"request": 2})),
            (130, "answered", json!({"request": 1, "option": "allow", "by": "human"})),
            (140, "answered", json!({"request": 2, "option": "allow", "by": "human"})),
        ]);

        assert_eq!(waited_seconds(&text, 100, 200), 30);
    }

    #[test]
    fn waited_seconds_counts_only_the_part_of_a_wait_between_from_and_to() {
        let text = journal(&[
            (90, "permission", json!({"request": 1})),
            (110, "answered", json!({"request": 1, "option": "allow", "by": "human"})),
            (190, "permission", json!({"request": 2})),
            (220, "answered", json!({"request": 2, "option": "allow", "by": "human"})),
        ]);

        assert_eq!(waited_seconds(&text, 100, 200), 20);
    }

    #[test]
    fn an_unanswered_permission_waits_until_its_session_ends_or_until_to() {
        let text = journal(&[
            (110, "permission", json!({"request": 1})),
            (120, "exited", json!({})),
            (130, "session", json!({"session": "t", "harness": "claude", "resumed": true})),
            (140, "permission", json!({"request": 2})),
        ]);

        assert_eq!(waited_seconds(&text, 100, 200), 10 + 60);
    }

    #[test]
    fn waited_seconds_passes_over_a_line_that_does_not_read() {
        let mut text = journal(&[(110, "permission", json!({"request": 1}))]);
        text.push_str(
            "{\"seq\": 2, \"at\": \"not a time\", \"kind\": \"answered\", \"request\": 1}\n{\"seq\": 3, \"at\n",
        );
        text.push_str(&format!("{}\n", json!({"seq": 4, "at": iso_utc(115), "kind": "answered", "request": 1})));

        assert_eq!(waited_seconds(&text, 100, 200), 5);
        assert_eq!(waited_seconds("", 100, 200), 0);
    }

    #[test]
    fn a_journal_that_cannot_be_read_is_an_error_not_a_new_file() {
        let tmp = TempDir::new();

        let refused = Journal::open(&tmp.path()).err().expect("a directory is no journal");

        assert!(refused.0.starts_with(&format!("cannot read {}: ", tmp.path())), "{}", refused.0);
    }
}
