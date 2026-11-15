//! The parts of a path the engine names files by, for paths it holds as text: the last component, and that split
//! before its extension; whether a name stays inside the directory it is joined to; and replacing a file's contents
//! so that a failure leaves the old ones.

use std::path::Path;

use crate::error::{os_error, path_text, strerror, OrchestratorError};

/// The last component of path: what follows its last `/`, all of path when it has none, and empty when it ends in
/// one.
pub fn file_name(path: &str) -> &str {
    &path[path.rfind('/').map_or(0, |i| i + 1)..]
}

/// name, if it names an entry of a directory itself, so that joined to the directory's path it names nothing outside
/// it: not empty, `.` or `..`, and without a slash or NUL. Errors: otherwise, naming it quoted as Rust's Debug quotes
/// a str, "… is not a name in the run directory", as the engine checks only run ids and the names of run files.
pub fn plain_name(name: &str) -> Result<&str, OrchestratorError> {
    if matches!(name, "" | "." | "..") || name.contains(['/', '\0']) {
        return Err(OrchestratorError(format!("{name:?} is not a name in the run directory")));
    }
    Ok(name)
}

/// path split before the extension of its last component (file_name): what comes before the extension's dot, and
/// the dot with the extension, which joined give path back. The extension is what Path::extension takes it to be:
/// after the last dot, unless the name's only dot leads it, as in `.bashrc`; a name without one splits as (path, "").
pub fn split_extension(path: &str) -> (&str, &str) {
    let dotted = Path::new(file_name(path)).extension().map_or(0, |ext| ext.len() + 1);
    path.split_at(path.len() - dotted)
}

/// Make text, line endings as they are, the contents of the file at path on this machine.
///
/// Written aside and renamed into place, so a failed write leaves the old file, not half a new one. A symlink at path
/// is followed, so the link stays and its target changes, and an existing file keeps its permissions. Errors: it
/// cannot be written, naming path.
pub fn replace_local_file(path: &str, text: &str) -> Result<(), OrchestratorError> {
    let real = std::fs::canonicalize(path).map_or_else(|_| path.to_string(), |p| p.to_string_lossy().into_owned());
    let tmp = format!("{real}.tmp.{}", std::process::id());
    let written = (|| {
        std::fs::write(&tmp, text).map_err(|e| os_error(&e, Some(&tmp)))?;
        if let Ok(meta) = std::fs::metadata(&real) {
            std::fs::set_permissions(&tmp, meta.permissions()).map_err(|e| os_error(&e, Some(&tmp)))?;
        }
        std::fs::rename(&tmp, &real).map_err(|e| format!("{} -> {}: {}", path_text(&tmp), path_text(&real), strerror(&e)))
    })();
    written.map_err(|why| {
        let _ = std::fs::remove_file(&tmp);
        OrchestratorError(format!("cannot write {path}: {why}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_splits_at_its_last_slash_and_its_extension_dot() {
        assert_eq!(file_name("a/b/c.toml"), "c.toml");
        assert_eq!(file_name("c"), "c");
        assert_eq!(file_name("a/"), "");
        assert_eq!(split_extension("a/b.c/spec.md"), ("a/b.c/spec", ".md"));
        assert_eq!(split_extension("a/b.c/spec"), ("a/b.c/spec", ""));
        assert_eq!(split_extension("x.tar.gz"), ("x.tar", ".gz"));
        assert_eq!(split_extension("d/.bashrc"), ("d/.bashrc", ""));
        assert_eq!(split_extension("a.b/"), ("a.b/", ""));
        assert_eq!(split_extension("x."), ("x", "."));
    }
}
