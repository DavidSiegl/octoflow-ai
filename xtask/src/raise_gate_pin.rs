//! `cargo xtask raise-gate-pin [<version>]`: make every change that moving `ci/gate.pin` to another octoflow-gate
//! release needs, in the git checkout the current directory is in, so that a raise is one command and a review of its
//! diff. With no version it takes the latest release, and only an immutable one, since nobody vouches for it; a
//! version given is taken as it is. Lowering the pin, to roll back, works the same way.
//!
//! The changes, made only once every check has passed (the release exists, is 0.7.0 or later, and its tarball
//! matches its `.sha256` asset; `engine/Cargo.toml`'s version is the one origin/main's tip has, since the raise bumps
//! main's version, and bumping one already bumped, by another change or an unmerged raise, can skip a release, while
//! a tree behind main can bump to a version main already has), in this order:
//! - `engine/tests/data/quality/`'s golden reports are the release's `golden/`, and its README names the release;
//! - `engine/Cargo.toml`'s version is bumped at the gate's level (a gate patch is a patch, anything else a minor, a
//!   rollback included), and `Cargo.lock` follows by `cargo update --workspace`;
//! - `docs/releases/v<new>.md` gets a section naming the release, made if missing;
//! - last, `ci/gate.pin`'s line names the release and its tarball's sha256, its comments kept. A run that fails
//!   before it leaves the old pin, so a second run does not take the half-made raise for a finished one; it says to
//!   restore the files first.
//!
//! Standard output gets the pull request's text, as a commit message is laid out: a title naming the release the pin
//! now names and octoflow's new version (`ci/gate.pin names octoflow-gate 0.12.0; version 0.90.0`), a blank line, and
//! the body, the notes of the releases the change brings (after the old pin up to the new one) or, for a rollback,
//! takes back (after the new pin up to the old one), newest first; nothing when the pin names the release already.
//! Progress goes to standard error. Exit status 0 when the pin was moved or already names the release with the sha256
//! its asset gives, 1 when a check or a step failed, 2 for a usage error, a version that does not parse included.
//! Needs gh (signed in, or GH_TOKEN), curl, git with origin/main fetched, cargo, and what ci/fetch-gate.sh needs.
//! raise-gate-pin.yml's `latest` step repeats `releases` and `chosen`'s choice of the latest release in jq.

use std::fs;
use std::path::{Path, PathBuf};

use crate::process::command;
use crate::Error;

/// The repository octoflow-gate releases are published from, as gh names it.
const GATE_REPO: &str = "DavidSiegl/octoflow-gate";
/// The first release that ships `gate`, which the Jenkinsfile's Gate stage and the engine's local gate run; the
/// engine's FIRST_WITH_GATE, which the tests hold it to.
const FIRST_WITH_GATE: Version = Version(0, 7, 0);

/// The files of a raise, relative to the checkout's top level.
const PIN: &str = "ci/gate.pin";
const GOLDEN_DIR: &str = "engine/tests/data/quality";
const ENGINE_MANIFEST: &str = "engine/Cargo.toml";
const RELEASE_NOTES_DIR: &str = "docs/releases";
/// The local remote-tracking ref of the branch a raise merges into, main, whose version the raise bumps; git must
/// have fetched it.
const TARGET_BRANCH: &str = "origin/main";

/// The command line `cargo xtask raise-gate-pin [<version>]`, as the module's comment says.
pub fn main(argv: &[String]) -> i32 {
    let wanted = match argv {
        [] => None,
        [v] if Version::parse(v).is_some() => Version::parse(v),
        _ => {
            eprintln!("usage: cargo xtask raise-gate-pin [<version>], the version as 0.15.2");
            return 2;
        }
    };
    match top_level().and_then(|top| raise(&top, wanted)) {
        Ok(text) => {
            print!("{text}");
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

/// A release's version, major.minor.patch, ordered as versions are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Version(u64, u64, u64);

impl Version {
    /// "1.2.3" or "v1.2.3"; None for anything else, a pre-release included.
    fn parse(text: &str) -> Option<Version> {
        let mut parts = text.strip_prefix('v').unwrap_or(text).split('.').map(|p| p.parse::<u64>().ok());
        match (parts.next(), parts.next(), parts.next(), parts.next()) {
            (Some(Some(a)), Some(Some(b)), Some(Some(c)), None) => Some(Version(a, b, c)),
            _ => None,
        }
    }

    /// This version bumped for a gate raise from old to new: a patch when the gate's is a patch, else a minor, which
    /// while the version is 0.x is also what a breaking change gets (CLAUDE.md, "Releases").
    fn bumped_for(self, old: Version, new: Version) -> Version {
        if (old.0, old.1) == (new.0, new.1) {
            Version(self.0, self.1, self.2 + 1)
        } else {
            Version(self.0, self.1 + 1, 0)
        }
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// A published octoflow-gate release, as GitHub lists it.
struct Release {
    version: Version,
    /// Whether GitHub refuses changes to its tag and assets.
    immutable: bool,
    /// Its notes, in Markdown.
    body: String,
}

/// The top level of the git checkout the current directory is in.
fn top_level() -> Result<PathBuf, Error> {
    let top = run(command("git").args(["rev-parse", "--show-toplevel"]), "finding the checkout's top level")?;
    Ok(PathBuf::from(top.trim_end()))
}

/// Move top's pin to wanted, or to the latest immutable release, and return the pull request's text, as the module's
/// comment lays it out; "" when the pin names that release already.
fn raise(top: &Path, wanted: Option<Version>) -> Result<String, Error> {
    // Every read and check first, so that a failed one leaves the checkout as it was.
    let pin_text = read(&top.join(PIN))?;
    let (old, old_sha) = pin_of(&pin_text).ok_or_else(|| Error(format!("{PIN} has no '<version> <sha256>' line")))?;
    let releases = releases()?;
    let new = chosen(&releases, wanted)?.version;
    if new < FIRST_WITH_GATE {
        return Err(Error(format!("octoflow-gate {new} ships no gate, which came in {FIRST_WITH_GATE}")));
    }
    let sha = release_sha256(new)?;
    if new == old {
        if sha != old_sha {
            return Err(Error(format!("{PIN} names octoflow-gate {old} with sha256 {old_sha}, its asset gives {sha}")));
        }
        eprintln!("{PIN} already names octoflow-gate {new}");
        return Ok(String::new());
    }
    let manifest = read(&top.join(ENGINE_MANIFEST))?;
    let (version_line, here) = package_version(&manifest, ENGINE_MANIFEST)?;
    let target = target_version(top)?;
    if here != target {
        return Err(Error(format!(
            "{ENGINE_MANIFEST}'s version is {here}, but {target} on {TARGET_BRANCH}: a raise bumps \
             {TARGET_BRANCH}'s version, so start it from a tree whose version is {TARGET_BRANCH}'s"
        )));
    }
    let unpacked = tempfile::Builder::new()
        .prefix("raise-gate-pin-")
        .tempdir()
        .map_err(|e| Error(format!("could not make a temporary directory: {e}")))?;
    let golden = fetch_golden(top, new, &sha, unpacked.path())?;
    let readme_path = top.join(GOLDEN_DIR).join("README.md");
    let readme = read(&readme_path)?;
    if !names_version(&readme, old) || !readme.contains(old_sha) {
        return Err(Error(format!("{} does not name octoflow-gate {old} and its sha256", readme_path.display())));
    }
    let octoflow = here.bumped_for(old, new);
    let manifest = with_line(&manifest, version_line, &format!("version = \"{octoflow}\""));
    let notes_path = top.join(RELEASE_NOTES_DIR).join(format!("v{octoflow}.md"));
    let notes = match fs::read_to_string(&notes_path) {
        Ok(text) => format!("{text}\n"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(Error(format!("could not read {}: {e}", notes_path.display()))),
    };
    let (low, high) = (old.min(new), old.max(new));
    let mut changed: Vec<&Release> = releases.iter().filter(|r| r.version > low && r.version <= high).collect();
    changed.sort_by_key(|r| std::cmp::Reverse(r.version));

    eprintln!("moving {PIN} from octoflow-gate {old} to {new}, octoflow to {octoflow}");
    let written = (|| {
        replace_golden(&top.join(GOLDEN_DIR), &golden)?;
        write(&readme_path, &replace_version(&readme.replace(old_sha, &sha), old, new))?;
        write(&top.join(ENGINE_MANIFEST), &manifest)?;
        run(command("cargo").args(["update", "--workspace"]).current_dir(top), "refreshing Cargo.lock")?;
        write(&notes_path, &format!("{notes}{}", release_notes_section(old, new, &changed)))?;
        // Through a file beside it, renamed over it, so that a write that fails leaves the old pin whole, as the
        // error below says.
        let staged = top.join(format!("{PIN}.raising"));
        write(&staged, &replace_pin_line(&pin_text, &format!("{new} {sha}")))?;
        fs::rename(&staged, top.join(PIN))
            .map_err(|e| Error(format!("could not rename {} over {PIN}: {e}", staged.display())))
    })();
    written.map_err(|Error(e)| {
        Error(format!(
            "{e}; the raise stopped partway, {PIN} still at {old}: restore the files it changed (git checkout, and git \
             clean for new ones) before running it again"
        ))
    })?;
    let heading =
        |r: &Release| if new > old { format!("{}", r.version) } else { format!("{}, rolled back", r.version) };
    let body: String =
        changed.iter().map(|r| format!("## octoflow-gate {}\n\n{}\n\n", heading(r), r.body.trim())).collect();
    Ok(format!("{PIN} names octoflow-gate {new}; version {octoflow}\n\n{body}"))
}

/// Whether text holds version on its own, not as part of a longer one, as 0.15.2 is part of 0.15.20 and 10.15.2.
fn names_version(text: &str, version: Version) -> bool {
    version_spans(text, version).next().is_some()
}

/// text with each version that names_version finds there replaced by by.
fn replace_version(text: &str, version: Version, by: Version) -> String {
    let mut replaced = String::new();
    let mut last = 0;
    for start in version_spans(text, version) {
        replaced.push_str(&text[last..start]);
        replaced.push_str(&by.to_string());
        last = start + version.to_string().len();
    }
    replaced.push_str(&text[last..]);
    replaced
}

/// Where version starts in text, at each place it is not part of a longer version.
fn version_spans(text: &str, version: Version) -> impl Iterator<Item = usize> + '_ {
    let needle = version.to_string();
    let len = needle.len();
    let bytes = text.as_bytes();
    let joined = |b: Option<&u8>| b.is_some_and(|b| b.is_ascii_digit() || *b == b'.');
    text.match_indices(&needle).map(|(i, _)| i).collect::<Vec<_>>().into_iter().filter(move |&i| {
        let before = i.checked_sub(1).and_then(|j| bytes.get(j));
        let after = bytes.get(i + len);
        let after_digit = after == Some(&b'.') && bytes.get(i + len + 1).is_some_and(u8::is_ascii_digit);
        !joined(before) && !after.is_some_and(u8::is_ascii_digit) && !after_digit
    })
}

/// The release among releases that wanted names, or with none the latest, which must be immutable since nobody named
/// it. Errors: no release is wanted's, there is none, or the latest is mutable.
fn chosen(releases: &[Release], wanted: Option<Version>) -> Result<&Release, Error> {
    match wanted {
        Some(v) => releases
            .iter()
            .find(|r| r.version == v)
            .ok_or_else(|| Error(format!("octoflow-gate has no published release {v}"))),
        None => {
            let latest = releases
                .iter()
                .max_by_key(|r| r.version)
                .ok_or_else(|| Error("octoflow-gate has no published release".into()))?;
            if !latest.immutable {
                return Err(Error(format!(
                    "octoflow-gate's latest release {} is not immutable, so nothing stops its tarball being replaced; \
                     name it to pin it anyway",
                    latest.version
                )));
            }
            Ok(latest)
        }
    }
}

/// octoflow-gate's published releases, drafts and pre-releases left out, as are tags that are no version.
fn releases() -> Result<Vec<Release>, Error> {
    let url = format!("repos/{GATE_REPO}/releases?per_page=100");
    let listed = run(command("gh").args(["api", "--paginate", "--slurp", &url]), "listing octoflow-gate's releases")?;
    let unread = |why: &str| Error(format!("gh api {url} printed no list of releases: {why}"));
    let pages: serde_json::Value = serde_json::from_str(&listed).map_err(|e| unread(&e.to_string()))?;
    let pages = pages.as_array().ok_or_else(|| unread("not an array of pages"))?;
    let mut releases = Vec::new();
    for release in pages.iter().filter_map(serde_json::Value::as_array).flatten() {
        let flag = |key: &str| release[key].as_bool().unwrap_or(false);
        if flag("draft") || flag("prerelease") {
            continue;
        }
        let Some(version) = release["tag_name"].as_str().and_then(Version::parse) else {
            continue;
        };
        let body = release["body"].as_str().unwrap_or("").to_owned();
        releases.push(Release { version, immutable: flag("immutable"), body });
    }
    Ok(releases)
}

/// The sha256 of version's tarball, as its release's `.sha256` asset gives it.
fn release_sha256(version: Version) -> Result<String, Error> {
    let url = format!("{}.sha256", release_url(version));
    let asset = run(command("curl").args(["-fsSL", &url]), &format!("downloading {url}"))?;
    let sha = asset.split_whitespace().next().unwrap_or("");
    if sha.len() != 64 || !sha.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return Err(Error(format!("{url} holds no sha256: {asset:?}")));
    }
    Ok(sha.to_owned())
}

/// Where version's release tarball is downloaded from: the address the engine's RELEASE_URL gives, which the tests
/// hold it to.
fn release_url(version: Version) -> String {
    format!("https://github.com/{GATE_REPO}/releases/download/v{version}/octoflow-gate-{version}.tar.gz")
}

/// The golden reports of version's release: its tarball is downloaded into dir and unpacked there by top's
/// ci/fetch-gate.sh, which refuses one that does not match sha, so the raise checks the release as the Gate stage
/// will.
fn fetch_golden(top: &Path, version: Version, sha: &str, dir: &Path) -> Result<Vec<(String, Vec<u8>)>, Error> {
    let pin = dir.join("gate.pin");
    write(&pin, &format!("{version} {sha}\n"))?;
    let out = dir.join("release");
    run(
        command("sh").arg(top.join("ci/fetch-gate.sh")).arg(&pin).arg(&out),
        &format!("fetching octoflow-gate {version}"),
    )?;
    golden_files(&out.join("golden"))
}

/// The files directly in dir, by name, with what they hold. Errors: dir cannot be read, or holds none.
fn golden_files(dir: &Path) -> Result<Vec<(String, Vec<u8>)>, Error> {
    let entries = fs::read_dir(dir).map_err(|e| Error(format!("could not read {}: {e}", dir.display())))?;
    let mut files = Vec::new();
    for entry in entries {
        let path = entry.map_err(|e| Error(format!("could not read {}: {e}", dir.display())))?.path();
        let bytes = fs::read(&path).map_err(|e| Error(format!("could not read {}: {e}", path.display())))?;
        files.push((path.file_name().unwrap_or_default().to_string_lossy().into_owned(), bytes));
    }
    if files.is_empty() {
        return Err(Error(format!("the release's {} holds no golden report", dir.display())));
    }
    Ok(files)
}

/// Make dir's golden reports, its .json and .txt files, those of golden; its other files, the README, stay.
fn replace_golden(dir: &Path, golden: &[(String, Vec<u8>)]) -> Result<(), Error> {
    let entries = fs::read_dir(dir).map_err(|e| Error(format!("could not read {}: {e}", dir.display())))?;
    for entry in entries {
        let path = entry.map_err(|e| Error(format!("could not read {}: {e}", dir.display())))?.path();
        if matches!(path.extension().and_then(|e| e.to_str()), Some("json" | "txt")) {
            fs::remove_file(&path).map_err(|e| Error(format!("could not remove {}: {e}", path.display())))?;
        }
    }
    for (name, bytes) in golden {
        let path = dir.join(name);
        fs::write(&path, bytes).map_err(|e| Error(format!("could not write {}: {e}", path.display())))?;
    }
    Ok(())
}

/// The version and sha256 of pin's first line that is not a comment, as ci/fetch-gate.sh and the engine's parse_pin
/// read it (the tests hold the two to the same reading).
fn pin_of(pin: &str) -> Option<(Version, &str)> {
    let (version, sha) = pin.lines().find(|l| !l.starts_with('#'))?.split_once(' ')?;
    Some((Version::parse(version)?, sha))
}

/// pin with its first line that is not a comment replaced by line.
fn replace_pin_line(pin: &str, line: &str) -> String {
    let mut replaced = false;
    pin.lines()
        .map(|l| {
            if replaced || l.starts_with('#') {
                l
            } else {
                replaced = true;
                line
            }
        })
        .map(|l| format!("{l}\n"))
        .collect()
}

/// The engine's version on TARGET_BRANCH's tip. Errors: git cannot read the manifest there (TARGET_BRANCH not
/// fetched, say), or its [package] has no version.
fn target_version(top: &Path) -> Result<Version, Error> {
    let object = format!("{TARGET_BRANCH}:{ENGINE_MANIFEST}");
    let manifest = run(command("git").args(["show", &object]).current_dir(top), &format!("reading {object}"))?;
    Ok(package_version(&manifest, &object)?.1)
}

/// The index among manifest's lines of its [package] table's `version = "x.y.z"` line, with the version it gives.
/// Errors: the table has none; the message names the manifest as source.
fn package_version(manifest: &str, source: &str) -> Result<(usize, Version), Error> {
    let mut in_package = false;
    manifest
        .lines()
        .enumerate()
        .find_map(|(i, line)| {
            if line.starts_with('[') {
                in_package = line.trim() == "[package]";
            }
            let version = line.strip_prefix("version = \"").and_then(|v| v.strip_suffix('"')).and_then(Version::parse);
            version.filter(|_| in_package).map(|v| (i, v))
        })
        .ok_or_else(|| Error(format!("{source}'s [package] has no version = \"x.y.z\"")))
}

/// text with its line at index `at` replaced by line, every line ending in a newline.
fn with_line(text: &str, at: usize, line: &str) -> String {
    text.lines().enumerate().map(|(i, l)| format!("{}\n", if i == at { line } else { l })).collect()
}

/// The release notes' section for moving the pin from old to new, wrapped at 120 columns as the docs are; changed
/// are the releases the move brings or, for a rollback, takes back, newest first.
fn release_notes_section(old: Version, new: Version, changed: &[&Release]) -> String {
    let link = |r: &Release| format!("[{}](https://github.com/{GATE_REPO}/releases/tag/v{})", r.version, r.version);
    let notes = match changed {
        [newest, .., oldest] => format!("the release notes from {} to {}", link(oldest), link(newest)),
        [only] => format!("the release notes of {}", link(only)),
        [] => "nothing".to_owned(),
    };
    let what = if new > old { "What the gate changed" } else { "What the rollback takes back" };
    let text = format!(
        "The octoflow-gate release this octoflow is built with is {new}, in place of {old}. It judges a project whose \
         `ci/gate.pin` is missing; a project that pins a release of its own, as `doctor --setup-gate --pin-gate` \
         does, keeps it. {what}: {notes}."
    );
    let mut lines = vec![String::new()];
    for word in text.split(' ') {
        let line = lines.last_mut().expect("never empty");
        if line.is_empty() {
            line.push_str(word);
        } else if line.len() + 1 + word.len() <= 120 {
            line.push(' ');
            line.push_str(word);
        } else {
            lines.push(word.to_owned());
        }
    }
    format!("## The quality gate is octoflow-gate {new}\n\n{}\n", lines.join("\n"))
}

/// Run command, saying what was being done when it cannot start or fails, and return its standard output.
fn run(command: &mut std::process::Command, doing: &str) -> Result<String, Error> {
    let output = command.output().map_err(|e| Error(format!("{doing}: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error(format!("{doing} failed ({}): {}", output.status, stderr.trim())));
    }
    String::from_utf8(output.stdout).map_err(|e| Error(format!("{doing}: its output is not UTF-8: {e}")))
}

fn read(path: &Path) -> Result<String, Error> {
    fs::read_to_string(path).map_err(|e| Error(format!("could not read {}: {e}", path.display())))
}

fn write(path: &Path, text: &str) -> Result<(), Error> {
    fs::write(path, text).map_err(|e| Error(format!("could not write {}: {e}", path.display())))
}

#[cfg(test)]
mod tests;
