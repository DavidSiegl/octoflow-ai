//! Workflow files: a workflow's definition as TOML in the configuration directory, or as saved in a run's
//! state.json, read into a Pipeline and written back.
//!
//! A workflow file is TOML holding a workflow's definition, named after the file:
//!
//! ```text
//! description = "..."                      # optional, shown by `workflows`
//! max_files = 15                           # optional: the change's budget (Pipeline), 15 files and 600 lines
//! max_lines = 600                          # when left out
//! proof_command = "python -m pytest {proofs}"   # optional: runs the Reviewer's proofs (Pipeline)
//! [roles]                                  # optional
//! build = "Builder"                        # a role's pane label,
//! tests = { label = "Test Writer", model = "sonnet", agent = "codex" }   # or its label, default model
//!                                                                       # and harness (HARNESS_NAMES)
//! [[steps]]                                # one table per step, in order
//! use = "build"                            # optional: start from that step of the default workflow
//! id = "build"                             # and Step's fields; role defaults to id
//! ```
//!
//! A role the [roles] table leaves out, or gives no label, is labelled after the default workflow's role of that
//! key, or else after the key itself. The panes are laid out in the order the steps first use the roles.
//! workflow_definition writes the canonical form, which such a file may also be: every role with its label, and every
//! step with its fields.

use crate::constants::{BUDGET_KEYS, DEFAULT_MAX_FILES, DEFAULT_MAX_LINES};
use crate::env::ConfigDir;
use crate::error::{closest_name, strerror, OrchestratorError};
use crate::harness::replaced_by;
use crate::paths::{file_name, replace_local_file, split_extension};
use crate::pipeline::{default_workflow, prompt_placeholders, Pipeline, PipelineSettings, Step, DEFAULT_WORKFLOW};
use crate::prompts::template::{is_name, placeholders};
use crate::prompts::{files_with_extension, Prompts};
use serde_json::Value;
use crate::map::Map;
use crate::json::Object;

/// The keys a workflow definition's top table may have.
pub const WORKFLOW_KEYS: [&str; 6] = ["description", "roles", "steps", "proof_command", "max_files", "max_lines"];
/// The keys a role's table may have.
pub const ROLE_KEYS: [&str; 3] = ["label", "model", "agent"];
/// A step's keys: Step's fields, in their order.
pub const STEP_KEYS: [&str; 12] = [
    "id",
    "role",
    "file",
    "prompt",
    "again",
    "fresh_note",
    "fresh_repeats_again",
    "human_paced",
    "edits",
    "loop_to",
    "quality_gated",
    "locks",
];
/// The steps' flags, the fields of STEP_KEYS that are false by default.
const STEP_FLAGS: [&str; 5] = ["fresh_repeats_again", "human_paced", "edits", "quality_gated", "locks"];

/// The built-in workflows other than the default, in name order: each one's name and the text of its workflow file,
/// which the binary carries. Each `use`s steps of the default workflow, so prompt files word those as they would in a
/// workflow file of the user's. A new built-in needs only its line here.
const BUILT_IN_FILES: [(&str, &str); 3] = [
    ("design", include_str!("workflows/design.toml")),
    ("product-team", include_str!("workflows/product-team.toml")),
    ("quick", include_str!("workflows/quick.toml")),
];

/// The names of the workflows `run --workflow` offers built in, in name order: `default`, then BUILT_IN_FILES's. A
/// workflow file cannot take one.
pub const WORKFLOWS: [&str; BUILT_IN_FILES.len() + 1] = built_in_names();

/// WORKFLOWS, built from BUILT_IN_FILES at compile time.
const fn built_in_names() -> [&'static str; BUILT_IN_FILES.len() + 1] {
    let mut names = ["default"; BUILT_IN_FILES.len() + 1];
    let mut i = 0;
    while i < BUILT_IN_FILES.len() {
        names[i + 1] = BUILT_IN_FILES[i].0;
        i += 1;
    }
    names
}

/// The built-in workflow of that name, worded by prompts; an error naming the built-in ones when there is none.
/// Panics if a built-in file does not parse under prompts, which cannot happen for prompts load_prompts accepts:
/// the tests parse every built-in, and parse_workflow checks a prompt only for its placeholders, which load_prompts
/// has already checked.
pub fn find_workflow(name: &str, prompts: &Prompts) -> Result<Pipeline, OrchestratorError> {
    if name == "default" {
        return Ok(default_workflow(prompts));
    }
    if let Some((_, text)) = BUILT_IN_FILES.iter().find(|(n, _)| *n == name) {
        let definition = parse_toml(text).unwrap_or_else(|e| panic!("built-in workflow {name} is not TOML: {e}"));
        return Ok(parse_workflow(name, &definition, prompts)
            .unwrap_or_else(|e| panic!("built-in workflow {name} is invalid: {e}")));
    }
    Err(OrchestratorError(format!("unknown workflow {name}; this orchestrator has {}", WORKFLOWS.join(", "))))
}

/// Whether p is the built-in workflow of its name as prompts word it, which a run need not save the definition of:
/// its name and prompts give it back (run_pipeline).
pub fn is_built_in(p: &Pipeline, prompts: &Prompts) -> bool {
    WORKFLOWS.contains(&p.name()) && find_workflow(p.name(), prompts).is_ok_and(|built_in| &built_in == p)
}

/// Name -> path of each workflow file in config's workflows_dir, unparsed, in name order; none without the
/// directory. Errors: the directory cannot be listed.
pub fn workflow_files(config: &ConfigDir) -> Result<Map<String>, OrchestratorError> {
    files_with_extension(&config.workflows_dir(), ".toml")
}

/// The names of the built-in workflows and the workflow files, in order.
pub fn workflow_names(config: &ConfigDir) -> Result<Vec<String>, OrchestratorError> {
    let mut names: Vec<String> = WORKFLOWS.iter().map(|n| n.to_string()).collect();
    for name in workflow_files(config)?.keys() {
        if !names.iter().any(|n| n == name) {
            names.push(name.to_string());
        }
    }
    names.sort();
    Ok(names)
}

/// The built-in workflow of that name, or else the one in config's workflows_dir, worded by prompts as
/// load_workflow_file words one. Errors: no workflow has that name, or its file cannot be read or defines none.
pub fn named_workflow(config: &ConfigDir, name: &str, prompts: &Prompts) -> Result<Pipeline, OrchestratorError> {
    if WORKFLOWS.contains(&name) {
        return find_workflow(name, prompts);
    }
    if let Some(path) = workflow_files(config)?.get(name) {
        return load_workflow_file(path, prompts);
    }
    Err(OrchestratorError(format!("unknown workflow {name}; there are {}", workflow_names(config)?.join(", "))))
}

/// The workflow the file at path defines, named after the file, a step that uses one of the default workflow's
/// taking its texts from prompts. Errors, naming the file: it cannot be read, is not TOML, takes a built-in
/// workflow's name or one that is not a name, or defines no valid workflow (parse_workflow).
pub fn load_workflow_file(path: &str, prompts: &Prompts) -> Result<Pipeline, OrchestratorError> {
    workflow_from_text(path, &workflow_file_text(path)?, prompts)
}

/// The text of the workflow file at path. Errors, naming the file: it cannot be read, or is not UTF-8.
fn workflow_file_text(path: &str) -> Result<String, OrchestratorError> {
    let bytes = std::fs::read(path).map_err(|e| unreadable(path, &e))?;
    String::from_utf8(bytes).map_err(|e| OrchestratorError(format!("{path}: {e}")))
}

/// The error of a workflow file at path that cannot be read, for e.
fn unreadable(path: &str, e: &std::io::Error) -> OrchestratorError {
    OrchestratorError(format!("cannot read workflow file {path}: {}", strerror(e)))
}

/// The workflow text, the contents of the workflow file at path, defines, as load_workflow_file gives it.
fn workflow_from_text(path: &str, text: &str, prompts: &Prompts) -> Result<Pipeline, OrchestratorError> {
    let name = split_extension(file_name(path)).0;
    if let Some(problem) = name_problem(name) {
        // A file of a built-in's name may predate the name's becoming built in, as a quick.toml or product-team.toml
        // copied from the example workflows of releases before 0.85.0, or a design.toml of one's own from before
        // 0.86.0, does: say how to get either workflow.
        let fix = if WORKFLOWS.contains(&name) {
            "; rename the file to keep it as a workflow of its own, or delete it and run the built-in"
        } else {
            ""
        };
        return Err(OrchestratorError(format!("{path}: {problem}{fix}")));
    }
    let definition = parse_toml(text).map_err(|e| OrchestratorError(format!("{path}: {e}")))?;
    parse_workflow(name, &definition, prompts).map_err(|e| OrchestratorError(format!("{path}: {e}")))
}

/// What is wrong with name as the name of a workflow file's workflow: it is built in, or not a name (is_name); None
/// when nothing is.
fn name_problem(name: &str) -> Option<String> {
    if WORKFLOWS.contains(&name) {
        return Some(format!("workflow {name} is built in, so no file may take its name"));
    }
    if !is_name(name) {
        return Some("a workflow's name, its file's, may use only letters, digits, - and _".into());
    }
    None
}

/// The table TOML text holds, as Value; the parser's error, with its line and column, for text that is not TOML.
pub fn parse_toml(text: &str) -> Result<Value, String> {
    toml::from_str::<Value>(text).map_err(|e| crate::error::toml_syntax_error(text, &e))
}

/// The workflow a definition describes, as a workflow file or a run's state.json holds it, a step with a use taking
/// the fields of that step of the default workflow as prompts words it; what is wrong with it otherwise.
pub fn parse_workflow(name: &str, definition: &Value, prompts: &Prompts) -> Result<Pipeline, String> {
    let empty = Object::new();
    let definition = definition.as_object().unwrap_or(&empty);
    check_keys(definition, &WORKFLOW_KEYS, "the file")?;
    let raw_steps = match definition.get("steps").and_then(Value::as_array) {
        Some(steps) if !steps.is_empty() => steps,
        _ => return Err("it has no [[steps]]".into()),
    };
    let default = default_workflow(prompts);
    let steps = raw_steps.iter().enumerate().map(|(i, raw)| parse_step(i + 1, raw, &default)).collect::<Result<Vec<_>, _>>()?;
    let declared = match definition.get("roles") {
        None => Map::new(),
        Some(Value::Object(raw_roles)) => {
            raw_roles.iter().map(|(role, raw)| parse_role(role, raw).map(|table| (role, table))).collect::<Result<_, _>>()?
        }
        Some(_) => return Err("roles must be a table".into()),
    };
    // The panes follow the steps; a declared role no step uses goes last, for Pipeline to reject.
    let mut labels: Map<String> = Map::new();
    for role in steps.iter().map(|st| st.role.as_str()).chain(declared.keys()) {
        if !labels.contains_key(role) {
            let label = declared.get(role).and_then(|t: &Map<String>| t.get("label")).filter(|l| !l.is_empty());
            labels.insert(role, label.cloned().unwrap_or_else(|| default_label(role)));
        }
    }
    let role_setting = |key: &str| -> Map<String> {
        declared
            .iter()
            .filter_map(|(role, table)| table.get(key).filter(|v| !v.is_empty()).map(|v| (role, v.clone())))
            .collect()
    };
    let description = match definition.get("description") {
        None => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(_) => return Err("description must be a string".into()),
    };
    let proof_command = match definition.get("proof_command") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err("proof_command must be a string".into()),
    };
    let mut budget = [DEFAULT_MAX_FILES, DEFAULT_MAX_LINES];
    for (key, slot) in BUDGET_KEYS.iter().zip(budget.iter_mut()) {
        if let Some(value) = definition.get(*key) {
            // A bool is no number of lines: true is not taken for 1.
            let number = value.as_i64();
            *slot = number.ok_or_else(|| format!("{key} must be a whole number"))?;
        }
    }
    let settings = PipelineSettings {
        models: role_setting("model"),
        description,
        agents: role_setting("agent"),
        max_files: budget[0],
        max_lines: budget[1],
        proof_command,
    };
    Pipeline::with(name, labels, steps, settings)
}

/// The ROLE_KEYS a role's entry in [roles] sets: a string is its label.
fn parse_role(role: &str, raw: &Value) -> Result<Map<String>, String> {
    let place = format!("role {role}");
    if !is_name(role) {
        return Err(format!("{place}: a role key may use only letters, digits, - and _"));
    }
    let table = match raw {
        Value::String(label) => return Ok([("label", label.clone())].into_iter().collect()),
        Value::Object(table) => table,
        _ => return Err(format!("{place}: give a label, or a table with label, model and agent")),
    };
    check_keys(table, &ROLE_KEYS, &place)?;
    table
        .iter()
        .map(|(key, value)| match value {
            Value::String(s) => Ok((key, s.clone())),
            _ => Err(format!("{place}: {key} must be a string")),
        })
        .collect()
}

/// Step i of a definition, its use resolved against default, the default workflow.
fn parse_step(i: usize, raw: &Value, default: &Pipeline) -> Result<Step, String> {
    let Value::Object(raw) = raw else {
        return Err(format!("step {i} must be a table"));
    };
    let place = match raw.get("id") {
        Some(Value::String(id)) => format!("step {i} ({id})"),
        _ => format!("step {i}"),
    };
    let mut keys: Vec<&str> = STEP_KEYS.to_vec();
    keys.push("use");
    check_keys(raw, &keys, &place)?;
    let mut fields = used_step(raw, &place, default)?;
    for (key, value) in raw.iter().filter(|(key, _)| *key != "use") {
        fields.insert(key.clone(), value.clone());
    }
    if !fields.contains_key("role") {
        fields.insert("role".into(), fields.get("id").cloned().unwrap_or(Value::Null));
    }
    for key in ["id", "file", "prompt"] {
        if !fields.contains_key(key) {
            return Err(format!("{place}: {key} is missing"));
        }
    }
    for (key, value) in fields.iter() {
        check_step_value(key, value, &place)?;
    }
    let text = |key: &str| fields.get(key).and_then(Value::as_str).map(String::from);
    let flag = |key: &str| fields.get(key).and_then(Value::as_bool).unwrap_or(false);
    let mut st = Step::new(
        &text("id").unwrap_or_default(),
        &text("role").unwrap_or_default(),
        &text("file").unwrap_or_default(),
        &text("prompt").unwrap_or_default(),
    );
    st.again = text("again");
    st.fresh_note = text("fresh_note");
    st.fresh_repeats_again = flag("fresh_repeats_again");
    st.human_paced = flag("human_paced");
    st.edits = flag("edits");
    st.loop_to = text("loop_to");
    st.quality_gated = flag("quality_gated");
    st.locks = flag("locks");
    Ok(st)
}

/// The fields of the step of default, the default workflow, that raw's use names; none without a use.
fn used_step(raw: &Object, place: &str, default: &Pipeline) -> Result<Object, String> {
    let Some(used) = raw.get("use") else { return Ok(Object::new()) };
    match used.as_str().and_then(|id| default.step(id)) {
        Some(st) => Ok(step_fields(st)),
        None => {
            let ids: Vec<&str> = default.steps().iter().map(|st| st.id.as_str()).collect();
            Err(format!("{place}: use names no step of the default workflow; it has {}", ids.join(", ")))
        }
    }
}

fn check_step_value(key: &str, value: &Value, place: &str) -> Result<(), String> {
    if STEP_FLAGS.contains(&key) {
        return match value {
            Value::Bool(_) => Ok(()),
            _ => Err(format!("{place}: {key} must be true or false")),
        };
    }
    let Value::String(text) = value else {
        return Err(format!("{place}: {key} must be a string"));
    };
    if (key == "id" || key == "role") && !is_name(text) {
        return Err(format!("{place}: {key} may use only letters, digits, - and _"));
    }
    placeholders(text).map(|_| ()).map_err(|e| format!("{place}: {key}: {e}"))
}

/// An error naming the first key of table that is not one of known, with the known key closest to it, or else all
/// of them.
fn check_keys(table: &Object, known: &[&str], place: &str) -> Result<(), String> {
    for key in table.keys() {
        if known.contains(&key.as_str()) {
            continue;
        }
        let hint = match closest_name(key, known.iter().copied()) {
            Some(close) => format!("did you mean {close}?"),
            None => {
                let mut sorted = known.to_vec();
                sorted.sort();
                format!("it takes {}", sorted.join(", "))
            }
        };
        return Err(format!("{place}: unknown key {key}; {hint}"));
    }
    Ok(())
}

/// The label of a role no table labels: the default workflow's role of that key's, else the key with each run of -
/// and _ as a space, title-cased.
fn default_label(role: &str) -> String {
    if let Some(label) = DEFAULT_WORKFLOW.roles().get(role) {
        return label.clone();
    }
    let mut spaced = String::new();
    let mut separating = false;
    for c in role.chars() {
        let separator = c == '-' || c == '_';
        if !separator {
            spaced.push(c);
        } else if !separating {
            spaced.push(' ');
        }
        separating = separator;
    }
    title_ascii(&spaced)
}

/// s with the first letter of each run of ASCII letters in upper case and the others in lower case: "test writer" as
/// "Test Writer", "a1b2 XY" as "A1B2 Xy". Any other character is left as it is and ends the run.
fn title_ascii(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_word = false;
    for c in s.chars() {
        if c.is_ascii_alphabetic() {
            out.push(if in_word { c.to_ascii_lowercase() } else { c.to_ascii_uppercase() });
            in_word = true;
        } else {
            out.push(c);
            in_word = false;
        }
    }
    out
}

/// The step's fields that differ from their defaults, in declaration order.
pub fn step_fields(st: &Step) -> Object {
    let mut fields = Object::new();
    fields.insert("id".into(), Value::from(st.id.as_str()));
    fields.insert("role".into(), Value::from(st.role.as_str()));
    fields.insert("file".into(), Value::from(st.file.as_str()));
    fields.insert("prompt".into(), Value::from(st.prompt.as_str()));
    let optional = [("again", &st.again), ("fresh_note", &st.fresh_note)];
    for (key, value) in optional {
        if let Some(text) = value {
            fields.insert(key.into(), Value::from(text.as_str()));
        }
    }
    let flags = [("fresh_repeats_again", st.fresh_repeats_again), ("human_paced", st.human_paced), ("edits", st.edits)];
    for (key, on) in flags {
        if on {
            fields.insert(key.into(), Value::Bool(true));
        }
    }
    if let Some(target) = &st.loop_to {
        fields.insert("loop_to".into(), Value::from(target.as_str()));
    }
    for (key, on) in [("quality_gated", st.quality_gated), ("locks", st.locks)] {
        if on {
            fields.insert(key.into(), Value::Bool(true));
        }
    }
    fields
}

/// The canonical definition of a workflow, which parse_workflow turns back into it: description, max_files,
/// max_lines and proof_command where they are not the default, every role with its label, model and agent, and
/// every step's step_fields.
pub fn workflow_definition(p: &Pipeline) -> Value {
    let mut d = Object::new();
    if !p.description().is_empty() {
        d.insert("description".into(), Value::from(p.description()));
    }
    if p.max_files() != DEFAULT_MAX_FILES {
        d.insert("max_files".into(), Value::from(p.max_files()));
    }
    if p.max_lines() != DEFAULT_MAX_LINES {
        d.insert("max_lines".into(), Value::from(p.max_lines()));
    }
    if let Some(command) = p.proof_command().filter(|c| !c.is_empty()) {
        d.insert("proof_command".into(), Value::from(command));
    }
    let roles = p
        .roles()
        .iter()
        .map(|(role, label)| {
            let mut table = Object::new();
            table.insert("label".into(), Value::from(label.as_str()));
            for (key, setting) in [("model", p.models()), ("agent", p.agents())] {
                if let Some(value) = setting.get(role) {
                    table.insert(key.into(), Value::from(value.as_str()));
                }
            }
            (role.to_string(), Value::Object(table))
        })
        .collect();
    d.insert("roles".into(), Value::Object(roles));
    d.insert("steps".into(), Value::Array(p.steps().iter().map(|st| Value::Object(step_fields(st))).collect()));
    Value::Object(d)
}

/// The workflow as a workflow file: workflow_definition as TOML, a role and a step a table each.
pub fn workflow_toml(p: &Pipeline) -> String {
    let d = workflow_definition(p);
    let text = |key: &str| d.get(key).and_then(Value::as_str);
    let mut lines = Vec::new();
    if let Some(description) = text("description") {
        lines.push(format!("description = {}", toml_string(description)));
    }
    for key in BUDGET_KEYS {
        if let Some(value) = d.get(key) {
            lines.push(format!("{key} = {value}"));
        }
    }
    if let Some(command) = text("proof_command") {
        lines.push(format!("proof_command = {}", toml_string(command)));
    }
    let tables = |key: &str| d.get(key).cloned();
    if let Some(Value::Object(roles)) = tables("roles") {
        for (role, table) in roles.iter() {
            lines.extend(toml_table(&format!("[roles.{role}]"), table));
        }
    }
    if let Some(Value::Array(steps)) = tables("steps") {
        for st in &steps {
            lines.extend(toml_table("[[steps]]", st));
        }
    }
    lines.join("\n") + "\n"
}

/// The lines of a TOML table under header, such as "[[steps]]", led by a blank line: a `key = value` line for each
/// field of table in its order, a boolean written as one and anything else as toml_string writes a string, "" for a
/// value that is not one. A table that is not a JSON object has no fields.
fn toml_table(header: &str, table: &Value) -> Vec<String> {
    let mut lines = vec![String::new(), header.to_string()];
    for (k, v) in table.as_object().into_iter().flat_map(Object::iter) {
        let value = match v {
            Value::Bool(b) => b.to_string(),
            _ => toml_string(v.as_str().unwrap_or_default()),
        };
        lines.push(format!("{k} = {value}"));
    }
    lines
}

/// s as a TOML string. A multi-line literal string keeps a prompt readable, but cannot hold ''' or end in a quote,
/// and holds no control characters besides tab and newline; anything else is written with JSON's escapes, which are
/// TOML's too, except that TOML also forbids a raw DEL.
fn toml_string(s: &str) -> String {
    let literal_ok = s.chars().all(|c| c == '\t' || c == '\n' || (c >= ' ' && c != '\u{7f}'));
    if s.contains('\n') && !s.contains("'''") && !s.ends_with('\'') && literal_ok {
        return format!("'''\n{s}'''");
    }
    // serde_json escapes the control characters, as \u and four hex digits, but not DEL.
    serde_json::to_string(s).expect("a string serializes").replace('\u{7f}', "\\u007f")
}

/// A workflow as an editor opens it (read_workflow).
#[derive(Debug, PartialEq)]
pub struct EditableWorkflow {
    /// Its canonical definition (workflow_definition), each `use` resolved with the built-in prompts.
    pub definition: Value,
    /// The file that defines it; None for a built-in workflow, which has none.
    pub file: Option<DefinitionFile>,
}

/// A workflow file as read_workflow found it.
#[derive(Debug, PartialEq)]
pub struct DefinitionFile {
    /// Where the file is: `{name}.toml` in the workflows directory, as workflow_files lists it.
    pub path: String,
    /// The file's text as read: what save_workflow and delete_workflow take as their base.
    pub text: String,
    /// Whether text is not workflow_toml of the workflow it defines, as when it has a comment or a `use`, so that
    /// saving it again loses something of its text.
    pub hand_written: bool,
}

/// What a workflow definition an editor holds would give: the first thing wrong with it, and the placeholders its
/// prompts may use (check_workflow).
#[derive(Debug, PartialEq)]
pub struct WorkflowCheck {
    /// The first problem, as `run` would give it, of the definition or its name; None when there is none.
    pub problem: Option<String>,
    /// prompt_placeholders of its steps' ids, as far as they can be read.
    pub placeholders: Vec<String>,
}

/// Why reading, saving or deleting a workflow for an editor failed. Each message says which workflow or file.
#[derive(Debug, PartialEq)]
pub enum WorkflowEditError {
    /// No workflow has the name, or for delete_workflow no workflow file.
    Unknown(String),
    /// The definition, or its name, has a problem. For read_workflow, path is the file whose workflow does not load,
    /// and text its text when it can be read at all, each sequence that is not UTF-8 replaced by U+FFFD: the base
    /// delete_workflow takes for it. Both are None for a definition given.
    Invalid { error: String, path: Option<String>, text: Option<String> },
    /// The file is not the text the caller started from: it exists where the caller named none, or is gone, or
    /// changed since.
    Changed(String),
    /// delete_workflow was given a built-in workflow's name, which has no file.
    BuiltIn(String),
    /// The workflows directory or a file in it could not be listed, read, written or removed.
    Failed(OrchestratorError),
}

/// The workflow of that name, as named_workflow finds it, for an editor: its definition with each `use` resolved by
/// the built-in prompts, as start::offered_workflows words workflows, and its file. Errors: Unknown for no such
/// workflow; Invalid, with the file's path and text, for a file that does not load; Failed when the workflows
/// directory cannot be listed.
pub fn read_workflow(config: &ConfigDir, name: &str) -> Result<EditableWorkflow, WorkflowEditError> {
    let built_in = Prompts::default();
    if WORKFLOWS.contains(&name) {
        let pipeline = find_workflow(name, &built_in).map_err(WorkflowEditError::Failed)?;
        return Ok(EditableWorkflow { definition: workflow_definition(&pipeline), file: None });
    }
    let files = workflow_files(config).map_err(WorkflowEditError::Failed)?;
    let Some(path) = files.get(name) else {
        return Err(WorkflowEditError::Unknown(format!("no workflow {name}")));
    };
    let invalid = |e: OrchestratorError, text: Option<&str>| WorkflowEditError::Invalid {
        error: e.0,
        path: Some(path.clone()),
        text: text.map(str::to_string),
    };
    let bytes = std::fs::read(path).map_err(|e| invalid(unreadable(path, &e), None))?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|e| invalid(OrchestratorError(format!("{path}: {e}")), Some(&String::from_utf8_lossy(&bytes))))?;
    let pipeline = workflow_from_text(path, text, &built_in).map_err(|e| invalid(e, Some(text)))?;
    let hand_written = text != workflow_toml(&pipeline);
    let file = DefinitionFile { path: path.clone(), text: text.to_string(), hand_written };
    Ok(EditableWorkflow { definition: workflow_definition(&pipeline), file: Some(file) })
}

/// What saving definition as workflow name would refuse, and the placeholders its prompts may use. The steps' ids
/// are read from the definition as parse_workflow reads them, so the placeholders are there even when it has a
/// problem.
pub fn check_workflow(name: &str, definition: &Value) -> WorkflowCheck {
    WorkflowCheck {
        problem: edited_workflow(name, definition).err(),
        placeholders: prompt_placeholders(step_ids(definition)),
    }
}

/// Save definition as workflow name's file in config's workflows_dir, in canonical form (workflow_toml), making the
/// directory if need be, and return the file's path. base is the file's text the caller started from, as
/// read_workflow gave it, or None for a workflow that must have no file yet. The file is written aside and renamed
/// into place (replace_local_file). Errors, with nothing written: Invalid for what check_workflow finds; Changed
/// when the file is not base; Failed when it cannot be read or written, the old file left as it was.
pub fn save_workflow(
    config: &ConfigDir,
    name: &str,
    definition: &Value,
    base: Option<&str>,
) -> Result<String, WorkflowEditError> {
    let pipeline =
        edited_workflow(name, definition).map_err(|error| WorkflowEditError::Invalid { error, path: None, text: None })?;
    let dir = config.workflows_dir();
    let path = format!("{dir}/{name}.toml");
    let _editing = lock_edits();
    expect_base(&path, base)?;
    std::fs::create_dir_all(&dir).map_err(|e| {
        WorkflowEditError::Failed(OrchestratorError(format!("cannot make the workflows directory {dir}: {}", strerror(&e))))
    })?;
    replace_local_file(&path, &workflow_toml(&pipeline)).map_err(WorkflowEditError::Failed)?;
    Ok(path)
}

/// Remove workflow name's file from config's workflows_dir and return its path; base is its text the caller
/// started from, as read_workflow gave it. A run that saved its workflow's definition (run_pipeline) still has it.
/// Errors, with nothing removed: BuiltIn for a built-in workflow; Unknown for no such file; Changed when the file is
/// not base; Failed when it cannot be listed, read or removed.
pub fn delete_workflow(config: &ConfigDir, name: &str, base: &str) -> Result<String, WorkflowEditError> {
    if WORKFLOWS.contains(&name) {
        return Err(WorkflowEditError::BuiltIn(format!("workflow {name} is built in; it has no file to delete")));
    }
    let files = workflow_files(config).map_err(WorkflowEditError::Failed)?;
    let Some(path) = files.get(name) else {
        return Err(WorkflowEditError::Unknown(format!("no workflow file {name}")));
    };
    let _editing = lock_edits();
    expect_base(path, Some(base))?;
    std::fs::remove_file(path).map_err(|e| {
        WorkflowEditError::Failed(OrchestratorError(format!("cannot remove workflow file {path}: {}", strerror(&e))))
    })?;
    Ok(path.clone())
}

/// The workflow definition describes under name, `use` resolved with the built-in prompts; what is wrong with the
/// name, as load_workflow_file has it, or with the definition, as parse_workflow has it.
fn edited_workflow(name: &str, definition: &Value) -> Result<Pipeline, String> {
    if let Some(problem) = name_problem(name) {
        return Err(problem);
    }
    parse_workflow(name, definition, &Prompts::default())
}

/// The ids definition's steps have, as parse_step reads them: a step's id, else that of the default workflow's step
/// its use names, which is the use. A step with neither as a string, or that is not a table, has none.
fn step_ids(definition: &Value) -> Vec<&str> {
    let steps = definition.get("steps").and_then(Value::as_array).map_or(&[][..], Vec::as_slice);
    steps.iter().filter_map(|st| ["id", "use"].into_iter().find_map(|key| st.get(key)?.as_str())).collect()
}

/// This process's lock on its edits of workflow files, held from save_workflow's or delete_workflow's check of the
/// base to the write or removal, so that two of serve's requests cannot both pass the check of one file, nor write
/// the temporary file replace_local_file names after the process at once.
fn lock_edits() -> std::sync::MutexGuard<'static, ()> {
    static EDITS: std::sync::Mutex<()> = std::sync::Mutex::new(());
    // A panic while it was held left no edit half done that the lock guards: the file is replaced by a rename.
    EDITS.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Refuse, as Changed, when the file at path is not base: None for no file. A file that is not UTF-8 is compared
/// with each sequence that is not UTF-8 replaced by U+FFFD, as read_workflow's Invalid gives its text. Errors: Failed when
/// the file cannot be read.
fn expect_base(path: &str, base: Option<&str>) -> Result<(), WorkflowEditError> {
    let current = match std::fs::read(path) {
        Ok(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            let why = format!("cannot read workflow file {path}: {}", strerror(&e));
            return Err(WorkflowEditError::Failed(OrchestratorError(why)));
        }
    };
    let why = match (current.as_deref(), base) {
        (current, base) if current == base => return Ok(()),
        (Some(_), None) => format!("{path} already exists"),
        (None, _) => format!("{path} was removed since it was opened"),
        (Some(_), Some(_)) => format!("{path} changed since it was opened"),
    };
    Err(WorkflowEditError::Changed(why))
}

/// A run's workflow: the definition saved with it, or the built-in workflow of its name worded by the run's prompts.
/// Errors: no built-in workflow has the name, or the saved definition is invalid.
pub fn run_pipeline(name: &str, definition: Option<&Value>, prompts: &Prompts) -> Result<Pipeline, OrchestratorError> {
    match definition {
        None => find_workflow(name, prompts),
        // A saved definition is canonical, with no use for prompts to word.
        Some(definition) => parse_workflow(name, &with_replaced_agents(definition), prompts).map_err(|e| {
            OrchestratorError(format!("the definition of workflow {name} saved with the run is invalid: {e}"))
        }),
    }
}

/// A saved workflow definition with each role's agent of a replaced kind (harness::replaced_by) as the kind that
/// replaced it, so that a run saved before the replacement still has its workflow. A resumed role runs in the
/// harness the run's next_harnesses name, never the definition's, and resume refuses a replaced one there
/// (crate::cli::start's resumable_state).
fn with_replaced_agents(definition: &Value) -> std::borrow::Cow<'_, Value> {
    let named_replaced = |table: &Value| table.get("agent").and_then(Value::as_str).and_then(replaced_by).is_some();
    let roles = definition.get("roles").and_then(Value::as_object);
    if !roles.is_some_and(|roles| roles.values().any(named_replaced)) {
        return std::borrow::Cow::Borrowed(definition);
    }
    let mut definition = definition.clone();
    let tables = definition.get_mut("roles").and_then(Value::as_object_mut).into_iter().flat_map(|r| r.values_mut());
    for table in tables.filter_map(Value::as_object_mut) {
        if let Some(new) = table.get("agent").and_then(Value::as_str).and_then(replaced_by) {
            table.insert("agent".into(), Value::from(new));
        }
    }
    std::borrow::Cow::Owned(definition)
}

#[cfg(test)]
mod tests;
