//! ci/release-version.sh without a revision: the version it reads from the tree it is in, which needs no git, as in a
//! source archive. Its reading of a revision is covered through ci/version-raised.sh (version_raised.rs).

use std::fs;
use std::process::Output;


/// The script's run from a copy of it in a tree without git, whose engine/Cargo.toml holds manifest.
fn release_version(manifest: &str) -> Output {
    let dir = tempfile::Builder::new().prefix("xtask-release-version-test-").tempdir().expect("a temporary directory");
    for sub in ["ci", "engine"] {
        fs::create_dir(dir.path().join(sub)).expect("made");
    }
    fs::copy(xtask::checkout().join("ci/release-version.sh"), dir.path().join("ci/release-version.sh"))
        .expect("copied");
    fs::write(dir.path().join("engine/Cargo.toml"), manifest).expect("written");
    std::process::Command::new("sh")
        .arg("ci/release-version.sh")
        .current_dir(dir.path())
        // So that no repository around the temporary directory is found either.
        .env("GIT_CEILING_DIRECTORIES", dir.path().parent().expect("a parent"))
        .output()
        .expect("sh runs")
}

#[test]
fn the_package_version_is_read_however_cargo_s_toml_spells_it() {
    for manifest in [
        "[workspace]\nversion = \"9.9.9\"\n\n[package]\nname = \"p\"\nversion = \"0.9.1\"\n\n\
         [dependencies.x]\nversion = \"8\"\n",
        "[package]\r\nname = \"p\"\r\nversion = \"0.9.1\"\r\n",
        "[package] # the engine\nname = \"p\"\nversion=\"0.9.1\"\n",
        "[package]\nname = \"p\"\n  version = \"0.9.1\"  # raised for the fix\n",
    ] {
        let output = release_version(manifest);
        assert_eq!(
            (output.status.code(), String::from_utf8_lossy(&output.stdout).into_owned()),
            (Some(0), "0.9.1\n".to_owned()),
            "{manifest:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn a_version_outside_the_package_table_is_none() {
    let output = release_version(
        "[workspace]\nversion = \"9.9.9\"\n\n[package]\nname = \"p\"\n\n[dependencies.x]\nversion = \"8\"\n",
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("no [package] version in"));
}
