//! The parts of a path the engine names files by, for paths it holds as text: the last component, and that split
//! before its extension.

use std::path::Path;

/// The last component of path: what follows its last `/`, all of path when it has none, and empty when it ends in
/// one.
pub fn file_name(path: &str) -> &str {
    &path[path.rfind('/').map_or(0, |i| i + 1)..]
}

/// path split before the extension of its last component (file_name): what comes before the extension's dot, and
/// the dot with the extension, which joined give path back. The extension is what Path::extension takes it to be:
/// after the last dot, unless the name's only dot leads it, as in `.bashrc`; a name without one splits as (path, "").
pub fn split_extension(path: &str) -> (&str, &str) {
    let dotted = Path::new(file_name(path)).extension().map_or(0, |ext| ext.len() + 1);
    path.split_at(path.len() - dotted)
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
