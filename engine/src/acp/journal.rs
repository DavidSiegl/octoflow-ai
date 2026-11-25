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
//! A crash or a failed write can cut the last line anywhere, inside a character as well, so every reader decodes the
//! file lossily (text here, crate::serve's ProjectRuns::read for the app) and passes over a line that does not read;
//! the next line appended starts on a line of its own (Lines::cut).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use crate::error::OrchestratorError;
use crate::timestamp::{epoch_seconds, iso_utc};

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

/// The text of the journal at path, each byte that is not UTF-8 replaced; empty when there is no such file. Errors:
/// the file cannot be read.
pub(super) fn text(path: &str) -> Result<String, OrchestratorError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(OrchestratorError(format!("cannot read {path}: {e}"))),
    }
}

/// The waits of crate::acp::waited_seconds (which says how each is measured) from `from` to `to`, in seconds since the
/// epoch, in the journal whose text is text. A line that does not read, or whose `at` is not a time, is passed over;
/// an empty text has no waits.
pub(super) fn waited_seconds_in(text: &str, from: i64, to: i64) -> i64 {
    let mut asked: Vec<(u64, i64)> = vec![];
    let mut waits: Vec<(i64, i64)> = vec![];
    // The `at` of the last line read.
    let mut last = from;
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
            Some("exited") => waits.extend(asked.drain(..).map(|(_, start)| (start, at))),
            Some("session") => waits.extend(asked.drain(..).map(|(_, start)| (start, last))),
            _ => {}
        }
        last = at;
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
    /// True when the file may end inside a line, as one a crash or a failed write cut short: the next line is then
    /// written after a line end of its own.
    cut: bool,
}

impl Journal {
    /// The journal at path, begun with its first line where it does not exist or holds no whole line, as when a crash
    /// cut its first; otherwise continued: its next event takes the `seq` after the file's last, and its next
    /// permission the `request` after the file's last, as its lines that read have them, and starts on a line of its
    /// own after a last line a crash cut short. Errors: the file cannot be read, created or begun.
    pub(super) fn open(path: &str) -> Result<Journal, OrchestratorError> {
        let cannot = |what: &str, e: std::io::Error| OrchestratorError(format!("cannot {what} {path}: {e}"));
        let mut text = text(path)?;
        let mut file = OpenOptions::new().create(true).append(true).open(path).map_err(|e| cannot("open", e))?;
        // The first line is written before any other, so a file with no line end holds at most part of it.
        if !text.contains('\n') {
            if !text.is_empty() {
                text.clear();
                file.set_len(0).map_err(|e| cannot("write", e))?;
            }
            file.write_all(format!("{}\n", json!({"journal": PROTOCOL})).as_bytes()).map_err(|e| cannot("write", e))?;
        }
        let (mut seq, mut request) = (0, 0);
        for line in text.lines() {
            let Ok(event) = serde_json::from_str::<Value>(line) else { continue };
            seq = event.get("seq").and_then(Value::as_u64).unwrap_or(seq).max(seq);
            if event.get("kind").and_then(Value::as_str) == Some("permission") {
                request = event.get("request").and_then(Value::as_u64).unwrap_or(request).max(request);
            }
        }
        let cut = !text.is_empty() && !text.ends_with('\n');
        Ok(Journal { lines: Mutex::new(Lines { file, seq, request, cut }) })
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
        // reader never sees half of one, unless the write fails partway; the next line then starts on its own.
        let start = if self.cut { "\n" } else { "" };
        self.cut = self.file.write_all(format!("{start}{}\n", Value::Object(event)).as_bytes()).is_err();
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

        assert_eq!(waited_seconds_in(&text, 100, 200), 43);
    }

    #[test]
    fn waited_seconds_counts_overlapping_waits_once() {
        let text = journal(&[
            (110, "permission", json!({"request": 1})),
            (120, "permission", json!({"request": 2})),
            (130, "answered", json!({"request": 1, "option": "allow", "by": "human"})),
            (140, "answered", json!({"request": 2, "option": "allow", "by": "human"})),
        ]);

        assert_eq!(waited_seconds_in(&text, 100, 200), 30);
    }

    #[test]
    fn waited_seconds_counts_only_the_part_of_a_wait_between_from_and_to() {
        let text = journal(&[
            (90, "permission", json!({"request": 1})),
            (110, "answered", json!({"request": 1, "option": "allow", "by": "human"})),
            (190, "permission", json!({"request": 2})),
            (220, "answered", json!({"request": 2, "option": "allow", "by": "human"})),
        ]);

        assert_eq!(waited_seconds_in(&text, 100, 200), 20);
    }

    #[test]
    fn an_unanswered_permission_waits_until_its_session_ends_or_until_to() {
        let text = journal(&[
            (110, "permission", json!({"request": 1})),
            (120, "exited", json!({})),
            (130, "session", json!({"session": "t", "harness": "claude", "resumed": true})),
            (140, "permission", json!({"request": 2})),
        ]);

        assert_eq!(waited_seconds_in(&text, 100, 200), 10 + 60);
    }

    #[test]
    fn a_wait_cut_off_by_a_lost_orchestrator_ends_at_its_session_s_last_line() {
        // The orchestrator dies twice while the role's agent waits, and each resume starts it a new session; nothing
        // journals the old agent's end, as end_leftovers kills it without a line.
        let text = journal(&[
            (110, "permission", json!({"request": 1})),
            (115, "update", json!({"update": {}})),
            (3700, "session", json!({"session": "t", "harness": "claude", "resumed": true})),
            (3710, "permission", json!({"request": 2})),
            (3800, "session", json!({"session": "u", "harness": "claude", "resumed": true})),
        ]);

        assert_eq!(waited_seconds_in(&text, 100, 4000), 5);
    }

    #[test]
    fn waited_seconds_passes_over_a_line_that_does_not_read() {
        let mut text = journal(&[(110, "permission", json!({"request": 1}))]);
        text.push_str(
            "{\"seq\": 2, \"at\": \"not a time\", \"kind\": \"answered\", \"request\": 1}\n{\"seq\": 3, \"at\n",
        );
        text.push_str(&format!("{}\n", json!({"seq": 4, "at": iso_utc(115), "kind": "answered", "request": 1})));

        assert_eq!(waited_seconds_in(&text, 100, 200), 5);
        assert_eq!(waited_seconds_in("", 100, 200), 0);
    }

    #[test]
    fn a_journal_whose_last_line_a_crash_cut_mid_character_is_continued_on_a_line_of_its_own() {
        let tmp = TempDir::new();
        let path = format!("{}/build.events.jsonl", tmp.path());
        let mut cut =
            b"{\"journal\":1}\n{\"seq\":1,\"at\":\"2026-11-25T12:00:00Z\",\"kind\":\"permission\",\"request\":4}\n"
                .to_vec();
        cut.extend(b"{\"seq\":2,\"kind\":\"update\",\"text\":\"\xe2\x80");
        std::fs::write(&path, &cut).unwrap();

        let journal = Journal::open(&path).expect("a cut journal is continued");
        journal.write("turn_end", json!({"stopReason": "end_turn"}));
        assert_eq!(journal.permission(&json!({})), 5);

        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.starts_with(&cut), "the lines already there stay as they were");
        let added: Vec<Value> = String::from_utf8(bytes[cut.len()..].to_vec())
            .expect("what open and write add is UTF-8")
            .lines()
            .skip(1)
            .map(|line| serde_json::from_str(line).expect("each added line reads alone"))
            .collect();
        assert_eq!(bytes[cut.len()], b'\n', "the cut line is ended before anything is added");
        let kinds: Vec<_> = added.iter().map(|e| (e["seq"].as_u64(), e["kind"].as_str())).collect();
        assert_eq!(kinds, [(Some(2), Some("turn_end")), (Some(3), Some("permission"))]);
    }

    #[test]
    fn a_journal_whose_first_line_a_crash_cut_is_begun_again() {
        let tmp = TempDir::new();
        let path = format!("{}/build.events.jsonl", tmp.path());
        std::fs::write(&path, b"{\"jour").unwrap();

        let journal = Journal::open(&path).expect("a journal cut in its first line is begun again");
        journal.write("turn_end", json!({"stopReason": "end_turn"}));

        let replay = replay(&std::fs::read_to_string(&path).unwrap(), 0).expect("the journal names its version");
        let kinds: Vec<_> = replay.events.iter().map(|e| (e["seq"].as_u64(), e["kind"].as_str())).collect();
        assert_eq!(kinds, [(Some(1), Some("turn_end"))]);
    }

    #[test]
    fn the_line_after_a_write_that_failed_starts_on_a_line_of_its_own() {
        let tmp = TempDir::new();
        let path = format!("{}/build.events.jsonl", tmp.path());
        // /dev/full refuses every write, as a full disk refuses the rest of a line it took part of.
        let full = OpenOptions::new().append(true).open("/dev/full").unwrap();
        let mut lines = Lines { file: full, seq: 0, request: 0, cut: false };
        lines.append("turn_end", json!({}));
        std::fs::write(&path, "{\"journal\":1}\n{\"seq\":1,\"at").unwrap();
        lines.file = OpenOptions::new().append(true).open(&path).unwrap();

        lines.append("exited", json!({}));

        let text = std::fs::read_to_string(&path).unwrap();
        let kinds: Vec<_> = replay(&text, 0).unwrap().events.iter().map(|e| e["seq"].as_u64()).collect();
        assert_eq!(kinds, [Some(2)], "{text}");
    }

    #[test]
    fn a_journal_that_cannot_be_read_is_an_error_not_a_new_file() {
        let tmp = TempDir::new();

        let refused = Journal::open(&tmp.path()).err().expect("a directory is no journal");

        assert!(refused.0.starts_with(&format!("cannot read {}: ", tmp.path())), "{}", refused.0);
    }
}
