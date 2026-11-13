//! The Python Package Index's JSON API (https://docs.pypi.org/api/json/), as orchestrator.py's PyPI asks it: what
//! the new-dependencies check (crate::checks) knows of a package from PyPI.

use std::sync::Arc;

use crate::constants::PYPI_TIMEOUT;
use crate::error::shortened;
use crate::http::{quote, Http, HttpError, Request};
use crate::package_index::{IndexError, IndexedPackage, PackageIndex, Upload};
use serde_json::Value;
use crate::json::ValueExt;

#[cfg(test)]
pub(crate) mod fake;

/// The index, reached through an [`Http`].
pub struct PyPI {
    http: Arc<dyn Http>,
}

impl PyPI {
    pub fn new(http: Arc<dyn Http>) -> PyPI {
        PyPI { http }
    }

    /// What PyPI knows of the project name; None when it has no such project (HTTP 404). Errors: IndexError, naming
    /// the URL, when it gives no usable answer within PYPI_TIMEOUT seconds a step.
    pub fn project(&self, name: &str) -> Result<Option<IndexedPackage>, IndexError> {
        let url = format!("https://pypi.org/pypi/{}/json", quote(name));
        let request = Request {
            url: url.clone(),
            headers: vec![("Accept".to_string(), "application/json".to_string())],
            timeout: PYPI_TIMEOUT,
        };
        let body = match self.http.send(&request) {
            Ok(answer) => answer.body,
            Err(HttpError::Status(404)) => return Ok(None),
            Err(HttpError::Status(status)) => return Err(IndexError(format!("GET {url}: HTTP {status}"))),
            Err(HttpError::NoAnswer(why)) => return Err(IndexError(format!("GET {url}: {why}"))),
        };
        project_of(&body).map(Some).map_err(|why| IndexError(format!("GET {url}: the answer is not PyPI's JSON: {why}")))
    }
}

impl PackageIndex for PyPI {
    fn name(&self) -> &'static str {
        "PyPI"
    }

    fn unknown_rule(&self) -> &'static str {
        "not-on-pypi"
    }

    /// PyPI::project's: PyPI takes a project's normalized name for the project.
    fn package(&self, key: &str) -> Result<Option<IndexedPackage>, IndexError> {
        self.project(key)
    }
}

/// The project of PyPI's JSON answer; why not, as Python's error of the step that fails says it, for an answer of
/// another shape.
fn project_of(body: &[u8]) -> Result<IndexedPackage, String> {
    let text = std::str::from_utf8(body).map_err(|e| e.to_string())?;
    let answer = serde_json::from_str::<Value>(text).map_err(|e| e.to_string())?;
    let info = match &answer {
        Value::Object(map) => map.get("info").ok_or("\"info\"")?,
        _ => return Err("it has no info.version".to_string()),
    };
    let version = match info.get("version") {
        Some(Value::String(version)) => version.clone(),
        _ => return Err("it has no info.version".to_string()),
    };
    let mut files = Vec::new();
    match answer.get("releases").filter(|r| r.is_set()) {
        None => {}
        Some(Value::Object(releases)) => {
            for release in releases.values() {
                files.extend(iterated(release)?);
            }
        }
        Some(other) => return Err(format!("releases is {}, not an object", other.kind())),
    }
    let first = first_upload(&files)?;
    let latest = first_upload(&answer.get("urls").map_or(Ok(Vec::new()), iterated)?)?;
    Ok(IndexedPackage { first_release: first, version, latest_release: latest, license: license(info)? })
}

/// The items of `value or []`; an error for a true value that is not a list.
fn iterated(value: &Value) -> Result<Vec<&Value>, String> {
    match value {
        Value::Array(items) => Ok(items.iter().collect()),
        other if !other.is_set() => Ok(Vec::new()),
        other => Err(format!("{} where an array belongs", other.kind())),
    }
}

/// The earliest upload_time_iso_8601 of PyPI's files, None for none.
fn first_upload(files: &[&Value]) -> Result<Option<Upload>, String> {
    let mut first: Option<Upload> = None;
    for file in files {
        let time = match file {
            Value::Object(file) => file.get("upload_time_iso_8601").ok_or("\"upload_time_iso_8601\"")?,
            other => return Err(format!("a file is {}, not an object", other.kind())),
        };
        let Value::String(time) = time else { return Err("upload_time_iso_8601 is not a string".to_string()) };
        let upload = Upload::from_iso(time)?;
        if first.as_ref().is_none_or(|f| upload.at < f.at) {
            first = Some(upload);
        }
    }
    Ok(first)
}


/// The project's license, as IndexedPackage::license holds it: its license expression, else the first line of its
/// license field, cut to 80 characters, else its license classifier's last part; None when it states none.
fn license(info: &Value) -> Result<Option<String>, String> {
    let text = |key: &str| -> Result<String, String> {
        match info.get(key) {
            Some(Value::String(s)) => Ok(s.trim().to_string()),
            Some(value) if value.is_set() => Err(format!("{key} is {}, not a string", value.kind())),
            _ => Ok(String::new()),
        }
    };
    let expression = text("license_expression")?;
    if !expression.is_empty() {
        return Ok(Some(expression));
    }
    let field = text("license")?;
    if !field.is_empty() {
        return Ok(Some(shortened(field.lines().next().unwrap_or_default(), 80)));
    }
    // Iterated as `info.get("classifiers") or []` iterates it: a string by its characters, an object by its keys.
    let classifiers: Vec<Value> = match info.get("classifiers") {
        Some(Value::String(text)) => text.chars().map(|c| Value::String(c.to_string())).collect(),
        Some(Value::Object(map)) => map.keys().map(|k| Value::from(k.as_str())).collect(),
        Some(value) => iterated(value)?.into_iter().cloned().collect(),
        None => Vec::new(),
    };
    let mut licenses = Vec::new();
    for classifier in &classifiers {
        let Value::String(classifier) = classifier else {
            return Err(format!("a classifier is {}, not a string", classifier.kind()));
        };
        if classifier.starts_with("License :: ") {
            licenses.push(classifier);
        }
    }
    Ok(licenses.first().and_then(|c| c.rsplit_once(" :: ")).map(|(_, last)| last.to_string()))
}

#[cfg(test)]
mod tests;
