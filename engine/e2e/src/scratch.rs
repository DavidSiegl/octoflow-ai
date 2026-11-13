//! Directories a test makes for itself under the system's temporary directory.

use std::fs;
use std::path::{Path, PathBuf};

/// A directory of a test's own, `octoflow-e2e-<name>-` and a suffix no other directory there has, under the system's
/// temporary directory (TMPDIR's, else /tmp), removed with everything in it when dropped; one that keeps on failure is
/// kept instead when it is dropped while its thread panics, and its path printed, for a look at what the failing test
/// left. No other Scratch, in this process or any other over the same temporary directory, is ever given its path.
pub struct Scratch {
    /// The directory, its symbolic links resolved.
    path: PathBuf,
    /// true means a drop during a panic keeps the directory.
    kept_on_failure: bool,
}

impl Scratch {
    /// A new, empty directory for name, removed when dropped. Panics when the directory cannot be made or resolved.
    pub fn new(name: &str) -> Scratch {
        // tempfile makes the directory under a name it retries until no one has it; this type does the removal, so
        // that it can keep the directory on failure and report a removal that fails.
        let dir = tempfile::Builder::new()
            .prefix(&format!("octoflow-e2e-{name}-"))
            .tempdir()
            .unwrap_or_else(|e| panic!("making a directory for {name}: {e}"))
            .keep();
        let path = dir.canonicalize().unwrap_or_else(|e| panic!("resolving {}: {e}", dir.display()));
        Scratch { path, kept_on_failure: false }
    }

    /// As new, but kept when dropped during a panic.
    pub fn kept_on_failure(name: &str) -> Scratch {
        let mut scratch = Scratch::new(name);
        scratch.kept_on_failure = true;
        scratch
    }

    /// The directory's path, its symbolic links resolved.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Scratch {
    /// Reports on standard error, rather than panicking, where the directory cannot be removed.
    fn drop(&mut self) {
        if self.kept_on_failure && std::thread::panicking() {
            eprintln!("kept for a look at the failure: {}", self.path.display());
        } else if let Err(e) = fs::remove_dir_all(&self.path) {
            eprintln!("removing {}: {e}", self.path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scratch_is_made_empty_and_removed_when_dropped() {
        let scratch = Scratch::new("scratch-removed");
        assert_eq!(fs::read_dir(scratch.path()).map(Iterator::count).ok(), Some(0));
        fs::write(scratch.path().join("left"), "").expect("a file in the scratch");
        let path = scratch.path().to_path_buf();
        drop(scratch);
        assert!(!path.exists());
    }

    #[test]
    fn two_scratches_of_one_name_are_two_directories() {
        let first = Scratch::new("scratch-twice");
        fs::write(first.path().join("left"), "").expect("a file in the scratch");
        let second = Scratch::new("scratch-twice");
        assert_ne!(first.path(), second.path());
        assert!(first.path().join("left").exists(), "the second scratch removed the first's file");
    }

    #[test]
    fn a_scratch_kept_on_failure_is_kept_by_a_panic_only() {
        let path = Scratch::kept_on_failure("scratch-kept").path().to_path_buf();
        assert!(!path.exists(), "kept though nothing failed");
        let kept = std::panic::catch_unwind(|| {
            let scratch = Scratch::kept_on_failure("scratch-kept");
            std::panic::panic_any(scratch.path().to_path_buf());
        });
        let path = *kept.expect_err("the test failed").downcast::<PathBuf>().expect("the scratch's path");
        assert!(path.exists(), "removed though the test failed");
        fs::remove_dir_all(&path).expect("the kept scratch removed");
        let removed = std::panic::catch_unwind(|| {
            let scratch = Scratch::new("scratch-kept");
            std::panic::panic_any(scratch.path().to_path_buf());
        });
        let path = *removed.expect_err("the test failed").downcast::<PathBuf>().expect("the scratch's path");
        assert!(!path.exists(), "a plain scratch kept by a panic");
    }
}
