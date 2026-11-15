//! serve's endpoints for editing workflows, GET /workflow and POST /workflow/check, /workflow/save and
//! /workflow/delete, as RunServer's doc comment gives them: workflow_file's read_workflow, check_workflow,
//! save_workflow and delete_workflow, in HTTP. They reach no project: workflows are this machine's, where serve starts
//! its runs.

use std::collections::HashMap;

use serde_json::Value;

use super::{error, json_object, object, Answer};
use crate::env::ConfigDir;
use crate::json::Object;
use crate::workflow_file::{check_workflow, delete_workflow, read_workflow, save_workflow, WorkflowEditError};

/// GET /workflow?name=NAME's answer, from config's workflows.
pub(super) fn read(config: &ConfigDir, query: &HashMap<String, String>) -> Answer {
    let Some(name) = query.get("name") else {
        return (400, error("missing parameter name"));
    };
    match read_workflow(config, name) {
        Ok(workflow) => {
            let file = workflow.file.as_ref();
            let answer = object([
                ("name", Value::from(name.as_str())),
                ("built_in", Value::from(file.is_none())),
                ("path", file.map_or(Value::Null, |f| Value::from(f.path.as_str()))),
                ("text", file.map_or(Value::Null, |f| Value::from(f.text.as_str()))),
                ("hand_written", Value::from(file.is_some_and(|f| f.hand_written))),
                ("definition", workflow.definition),
            ]);
            (200, answer)
        }
        Err(e) => failure(e),
    }
}

/// POST /workflow/check's answer to body.
pub(super) fn check(body: &[u8]) -> Answer {
    let fields = match fields(body, r#"{"name": "...", "definition": {...}}"#, &[NAME, DEFINITION]) {
        Ok(fields) => fields,
        Err(answer) => return answer,
    };
    let (name, definition) = (text(&fields, "name"), &fields["definition"]);
    let checked = check_workflow(name, definition);
    (200, object([("problem", Value::from(checked.problem)), ("placeholders", Value::from(checked.placeholders))]))
}

/// POST /workflow/save's answer to body, saving into config's workflows directory.
pub(super) fn save(config: &ConfigDir, body: &[u8]) -> Answer {
    let shape = r#"{"name": "...", "definition": {...}, "base": "..." or null}"#;
    let base_or_new: Key = ("base", |v| v.is_string() || v.is_null());
    let fields = match fields(body, shape, &[NAME, DEFINITION, base_or_new]) {
        Ok(fields) => fields,
        Err(answer) => return answer,
    };
    match save_workflow(config, text(&fields, "name"), &fields["definition"], fields["base"].as_str()) {
        Ok(path) => (201, object([("path", Value::from(path))])),
        Err(e) => failure(e),
    }
}

/// POST /workflow/delete's answer to body, deleting from config's workflows directory.
pub(super) fn delete(config: &ConfigDir, body: &[u8]) -> Answer {
    let fields = match fields(body, r#"{"name": "...", "base": "..."}"#, &[NAME, ("base", Value::is_string)]) {
        Ok(fields) => fields,
        Err(answer) => return answer,
    };
    match delete_workflow(config, text(&fields, "name"), text(&fields, "base")) {
        Ok(path) => (200, object([("path", Value::from(path))])),
        Err(e) => failure(e),
    }
}

/// A key a body must have, and whether a value fits it.
type Key = (&'static str, fn(&Value) -> bool);
const NAME: Key = ("name", Value::is_string);
const DEFINITION: Key = ("definition", Value::is_object);

/// body's JSON object, once it has exactly keys, each value fitting its key. Errors: 400 naming shape, the body's
/// expected form, for any other body.
fn fields(body: &[u8], shape: &str, keys: &[Key]) -> Result<Object, Answer> {
    let refused = || (400, error(&format!("the body must be {shape}")));
    let fields = json_object(body).ok_or_else(refused)?;
    let fits = |(key, fits): &Key| fields.get(*key).is_some_and(fits);
    if fields.len() != keys.len() || !keys.iter().all(fits) {
        return Err(refused());
    }
    Ok(fields)
}

/// The string at key of fields, which fields has checked is one.
fn text<'f>(fields: &'f Object, key: &str) -> &'f str {
    fields[key].as_str().unwrap_or_default()
}

/// The answer to an edit that failed with e.
fn failure(e: WorkflowEditError) -> Answer {
    match e {
        WorkflowEditError::Unknown(why) => (404, error(&why)),
        WorkflowEditError::Invalid { error: why, path: Some(path), text } => {
            (422, object([("error", Value::from(why)), ("path", Value::from(path)), ("text", Value::from(text))]))
        }
        WorkflowEditError::Invalid { error: why, path: None, .. } => (422, error(&why)),
        WorkflowEditError::Changed(why) => (409, error(&why)),
        WorkflowEditError::BuiltIn(why) => (400, error(&why)),
        WorkflowEditError::Failed(e) => (500, error(&e.0)),
    }
}
