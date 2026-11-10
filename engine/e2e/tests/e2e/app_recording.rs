//! The desktop app's recording, `ui/src/test/recorded-stream.json`: what serve's event stream sent over the run of
//! scenarios::app_event_stream, in the form the app's tests replay it (`ui/src/test/fakes.ts`, RECORDED).
//!
//! The file is a list of batches, {"after", "events"}: the events serve sent after the step `after` names, each
//! {"kind", ...its payload}. Of the events of a batch, only the last record and the last files of each run are kept,
//! the record first, the runs in the order the batch first named them: how many polls a step took, and what each
//! caught of it, is the clock's.
//!
//! What else the clock, the process table or chance decides is written as the same stand-in every time, of the type
//! the app reads there, so that recording twice gives the same file:
//! - the first run is 20261024-090000-beef00 and the second 20261024-140000-c0ffee, their keys the last six digits;
//! - the project is /home/me/proj, the host `devbox`, and the pid of engine invocation n is 4000 + n;
//! - a turn starts ten minutes after the one before it in its run, the first at 14:00 UTC on 2026-10-24, and one
//!   that has ended took five minutes;
//! - a heartbeat is one second old, in heartbeat_age and in health_reason;
//! - a file's mtime is 14:00 UTC on 2026-10-24 plus a minute for each batch before the one that first held the
//!   file as it is, and a run's files are in the order of that time, then of their names.

use std::path::Path;

use octoflow_e2e::normalize::Normalizer;
use octoflow_e2e::{at, object, str_at};
use octoflow_engine::map::Map;
use serde_json::Value;

/// What serve's event stream sent after one step of a scenario.
pub struct Batch {
    /// The step, as the app's tests name the batch.
    pub after: &'static str,
    /// Each event's kind and payload, in the order serve sent them.
    pub events: Vec<(String, Value)>,
}

/// The stand-ins' ids of the runs, in the order the stream names them.
const RUN_IDS: [&str; 2] = ["20261024-090000-beef00", "20261024-140000-c0ffee"];
/// 14:00 UTC on 2026-10-24, in seconds since the epoch: the stand-ins' first time.
const START: i64 = 1_792_850_400;

/// The recording of batches, as the module comment has it. project is the directory serve was asked for, and
/// normalizer the world's, which learns the batches' runs here. Panics on an event that is neither a record nor a
/// files event, on more runs than RUN_IDS names, and on a placeholder of the normalizer's this module has no
/// stand-in for: something new that differs between two recordings, which needs one.
pub fn recording(batches: &[Batch], normalizer: &mut Normalizer, project: &Path) -> Value {
    let sent: Vec<Value> = batches.iter().flat_map(|b| b.events.iter().map(|(_, payload)| payload.clone())).collect();
    normalizer.learn(&Value::Array(sent));
    // By run and file name, the mtime serve last sent and the number of the batch that first held it.
    let mut versions: Map<(i64, usize)> = Map::new();
    let mut out = Vec::new();
    for (n, batch) in batches.iter().enumerate() {
        // By run, in the order the batch first names them, its last record and last files.
        let mut last: Map<(Option<Value>, Option<Value>)> = Map::new();
        for (kind, payload) in &batch.events {
            let run = str_at(payload, "run").to_string();
            if last.get(&run).is_none() {
                last.insert(run.clone(), (None, None));
            }
            let entry = last.get_mut(&run).expect("the run's entry");
            match kind.as_str() {
                "record" => entry.0 = Some(at(payload, "record").clone()),
                "files" => entry.1 = Some(at(payload, "files").clone()),
                _ => panic!("serve sent a {kind} event after: {}: {payload}", batch.after),
            }
        }
        let mut events = Vec::new();
        for (run, (record, files)) in last.iter() {
            let run_id = stand_in_text(&normalizer.text(run, false), project);
            if let Some(record) = record {
                let record = stand_in(&normalizer.json(record, false, None), None, project);
                events.push(object([("kind", "record".into()), ("run", run_id.as_str().into()), ("record", record)]));
            }
            if let Some(files) = files {
                let mut listed: Vec<(usize, String)> = Vec::new();
                for file in files.as_array().expect("a files event's files") {
                    let (name, mtime) = (str_at(file, "name"), at(file, "mtime").as_i64().expect("an mtime"));
                    let key = format!("{run}/{name}");
                    if versions.get(&key).is_none_or(|(sent, _)| *sent != mtime) {
                        versions.insert(key.clone(), (mtime, n));
                    }
                    listed.push((versions.get(&key).expect("the file's version").1, name.to_string()));
                }
                listed.sort();
                let files = listed.iter().map(|(batch, name)| {
                    object([("name", name.as_str().into()), ("mtime", (START + 60 * *batch as i64).into())])
                });
                events.push(object([("kind", "files".into()), ("run", run_id.as_str().into()), ("files", files.collect())]));
            }
        }
        out.push(object([("after", batch.after.into()), ("events", Value::Array(events))]));
    }
    Value::Array(out)
}

/// node, a record as the normalizer wrote it or a part of one under key, with each placeholder replaced by its
/// stand-in.
fn stand_in(node: &Value, key: Option<&str>, project: &Path) -> Value {
    match node {
        Value::Object(map) => Value::Object(map.iter().map(|(k, v)| (k.clone(), stand_in(v, Some(k), project))).collect()),
        Value::Array(items) if key == Some("turns") => Value::Array(items.iter().enumerate().map(|(k, turn)| stand_in_turn(turn, k, project)).collect()),
        Value::Array(items) => Value::Array(items.iter().map(|v| stand_in(v, key, project)).collect()),
        Value::String(s) if s == "<seconds>" && key == Some("heartbeat_age") => Value::from(1),
        Value::String(s) => Value::String(stand_in_text(s, project)),
        other => other.clone(),
    }
}

/// turn, the k-th of its run from 0, with the stand-ins of its times, and of the rest of it as stand_in has them.
fn stand_in_turn(turn: &Value, k: usize, project: &Path) -> Value {
    let started = START + 600 * k as i64;
    let Value::Object(fields) = turn else { panic!("a turn that is no object: {turn}") };
    let timed = fields.iter().map(|(key, v)| {
        let v = match (key.as_str(), v.as_str()) {
            ("started_at", Some("<time>")) => Value::from(octoflow_engine::summary::iso_utc(started)),
            ("ended_at", Some("<time>")) => Value::from(octoflow_engine::summary::iso_utc(started + 300)),
            ("seconds", Some("<seconds>")) => Value::from(300),
            _ => stand_in(v, Some(key), project),
        };
        (key.clone(), v)
    });
    Value::Object(timed.collect())
}

/// text as the normalizer wrote it, with each placeholder replaced by its stand-in. Panics on one it has none for.
fn stand_in_text(text: &str, project: &Path) -> String {
    let mut out = text.replace(&project.to_string_lossy().replace(&project.parent().expect("the world").to_string_lossy().into_owned(), "<world>"), "/home/me/proj");
    for (n, id) in RUN_IDS.iter().enumerate() {
        out = out.replace(&format!("<run:{}>", n + 1), id).replace(&format!("<key:{}>", n + 1), &id[id.len() - 6..]);
    }
    out = out.replace("<host>", "devbox").replace("<duration>", "1s");
    while let Some((before, rest)) = out.split_once("<pid:") {
        let Some((n, after)) = rest.split_once('>') else { break };
        let n: i64 = n.parse().unwrap_or_else(|_| panic!("a pid of no engine invocation in {text:?}"));
        out = format!("{before}{}{after}", 4000 + n);
    }
    let placeholder = out.find('<').filter(|at| out[*at..].contains('>'));
    assert!(placeholder.is_none(), "no stand-in for the placeholder in {text:?}; app_recording.rs needs one");
    out
}
