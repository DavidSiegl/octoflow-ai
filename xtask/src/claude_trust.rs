//! Claude Code's record of the folders the human trusts, which the eval runner writes so that a trial's fresh clone
//! does not stop at Claude Code's first-run question, "Do you trust the files in this folder?", before its first
//! turn ([`crate::eval::run_trial`]).
//!
//! Claude Code keeps that answer in its global config file ([`config_file`]) as
//! `projects.<absolute path>.hasTrustDialogAccepted: true`, checked against Claude Code 2.1.323. That key is not
//! documented: TODO.md's "No `doctor` check that a harness can start in the project without asking" decided against
//! reading the file for doctor for that reason. The runner writes it all the same, because a trial cannot answer the
//! question, and it fails loudly, naming the file, when the file cannot be read, parsed or written, rather than
//! running a trial that would wait on the question. Should a Claude Code release move the record, trials wait again,
//! as the first eval run's Builder did (docs/design.md, Evals).
//!
//! Claude Code rewrites the file while it runs, so each change here is a read and an atomic replace (a temporary file
//! renamed over it) that keeps every other key; a write Claude Code makes between the two is lost. Nothing of Claude
//! Code's runs in a trial's folder while the runner marks or forgets it.

use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::Error;

/// Claude Code's global config file, as env gives each variable by name: `.claude.json` in CLAUDE_CONFIG_DIR when that
/// is set and not empty, else in HOME; None when neither is set.
pub fn config_file(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let set = |name: &str| env(name).filter(|value| !value.is_empty());
    set("CLAUDE_CONFIG_DIR").or_else(|| set("HOME")).map(|dir| Path::new(&dir).join(".claude.json"))
}

/// Records in config, Claude Code's config file, that the human trusts folder, an absolute path as Claude Code's
/// working directory names it (symbolic links resolved). A config that does not exist is left so: Claude Code has
/// not run as this user, and would ask its own first-run questions before this one. Errors: config cannot be read,
/// is not a JSON object, or cannot be replaced; each names the file.
pub fn trust(config: &Path, folder: &Path) -> Result<(), Error> {
    edit(config, |top| {
        let projects = top.entry("projects").or_insert_with(|| json!({}));
        let projects = projects.as_object_mut().ok_or("its `projects` is not an object")?;
        let project = projects.entry(folder.to_string_lossy().into_owned()).or_insert_with(|| json!({}));
        let project = project.as_object_mut().ok_or("its entry for the folder is not an object")?;
        project.insert("hasTrustDialogAccepted".to_string(), Value::Bool(true));
        Ok(())
    })
}

/// Removes folder's entry from config's `projects`, with the trust [`trust`] recorded and whatever Claude Code kept
/// there about its sessions in folder: for a folder that is gone, or about to be. Nothing to remove is no error.
/// Errors: as trust's.
pub fn forget(config: &Path, folder: &Path) -> Result<(), Error> {
    edit(config, |top| {
        if let Some(projects) = top.get_mut("projects").and_then(Value::as_object_mut) {
            // shift_remove, not remove, which would move the last project into its place.
            projects.shift_remove(folder.to_string_lossy().as_ref());
        }
        Ok(())
    })
}

/// Replaces config, when it exists, with its JSON object as change leaves it, written as Claude Code writes it
/// (two-space indents) with the file's permissions kept. Errors: as trust's, or change's, which says what in the file
/// is shaped otherwise.
fn edit(config: &Path, change: impl FnOnce(&mut Map<String, Value>) -> Result<(), &'static str>) -> Result<(), Error> {
    let failed = |what: &str, why: &dyn std::fmt::Display| {
        Error(format!("could not {what} Claude Code's config {}: {why}", config.display()))
    };
    let text = match fs::read_to_string(config) {
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
        read => read.map_err(|e| failed("read", &e))?,
    };
    let mut top: Value = serde_json::from_str(&text).map_err(|e| failed("parse", &e))?;
    let object = top.as_object_mut().ok_or_else(|| failed("change", &"it is not a JSON object"))?;
    change(object).map_err(|why| failed("change", &why))?;
    let mut text = serde_json::to_string_pretty(&top).map_err(|e| failed("write", &e))?;
    text.push('\n');
    let permissions = fs::metadata(config).map_err(|e| failed("read", &e))?.permissions();
    let mut name = config.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".xtask-{}", std::process::id()));
    let temporary = config.with_file_name(name);
    // Owner-only until the file's own permissions are set, so that the user's tokens in it are never readable by
    // others.
    let written = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .and_then(|mut file| {
            file.write_all(text.as_bytes())?;
            file.set_permissions(permissions)?;
            file.sync_all()
        })
        .and_then(|()| fs::rename(&temporary, config));
    if let Err(e) = written {
        // The temporary file may be partly written, or whole and not renamed; either way it is only ours. A removal
        // that fails too leaves it beside the config, where the error returned points the human.
        let _ = fs::remove_file(&temporary);
        return Err(failed("write", &e));
    }
    Ok(())
}
