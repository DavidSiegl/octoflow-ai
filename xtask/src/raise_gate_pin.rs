//! `cargo xtask raise-gate-pin [<version>]`: make every change that moving `ci/gate.pin` to another octoflow-gate
//! release needs, in the git checkout the current directory is in, so that a raise is one command and a review of its
//! diff. With no version it takes the latest release, and only an immutable one, since nobody vouches for it; a
//! version given is taken as it is. Lowering the pin, to roll back, works the same way.
//!
//! The changes, made only once every check has passed (the release exists, is 0.7.0 or later, and its tarball
//! matches its `.sha256` asset):
//! - `ci/gate.pin`'s line names the release and its tarball's sha256, its comments kept;
//! - `engine/tests/data/quality/`'s golden reports are the release's `golden/`, and its README names the release;
//! - `engine/Cargo.toml`'s version is bumped at the gate's level (a gate patch is a patch, anything else a minor), and
//!   `Cargo.lock` follows by `cargo update --workspace`;
//! - `docs/releases/v<new>.md` gets a section naming the release, made if missing.
//!
//! Standard output gets the notes of every release after the old pin up to the new one, newest first, for the pull
//! request's body; progress goes to standard error. Exit status 0 when the pin was moved or already names the
//! release, 1 when a check or a step failed, 2 for a usage error. Needs gh (signed in, or GH_TOKEN), curl, git, cargo,
//! and what ci/fetch-gate.sh needs.

use std::fs;
use std::path::{Path, PathBuf};

use crate::process::command;
use crate::Error;

/// The repository octoflow-gate releases are published from, as gh names it.
const GATE_REPO: &str = "DavidSiegl/octoflow-gate";
/// The first release that ships `gate`, which the Jenkinsfile's Gate stage and the engine's local gate run; as
/// engine/src/local_gate.rs's FIRST_WITH_GATE.
const FIRST_WITH_GATE: Version = Version(0, 7, 0);

/// The files of a raise, relative to the checkout's top level.
const PIN: &str = "ci/gate.pin";
const GOLDEN_DIR: &str = "engine/tests/data/quality";
const ENGINE_MANIFEST: &str = "engine/Cargo.toml";
const RELEASE_NOTES_DIR: &str = "docs/releases";

/// The command line `cargo xtask raise-gate-pin [<version>]`, as the module's comment says.
pub fn main(argv: &[String]) -> i32 {
    let wanted = match argv {
        [] => None,
        [v] if !v.starts_with('-') => match Version::parse(v) {
            Some(v) => Some(v),
            None => {
                eprintln!("error: {v} is not a version like 0.15.2");
                return 1;
            }
        },
        _ => {
            eprintln!("usage: cargo xtask raise-gate-pin [<version>]");
            return 2;
        }
    };
    match top_level().and_then(|top| raise(&top, wanted)) {
        Ok(notes) => {
            print!("{notes}");
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

/// Move top's pin to wanted, or to the latest immutable release, and return the notes for the pull request; "" when
/// the pin names that release already.
fn raise(top: &Path, wanted: Option<Version>) -> Result<String, Error> {
    // Every read and check first, so that a failed one leaves the checkout as it was.
    let pin_text = read(&top.join(PIN))?;
    let (old, old_sha) = pin_of(&pin_text).ok_or_else(|| Error(format!("{PIN} has no '<version> <sha256>' line")))?;
    let releases = releases()?;
    let new = chosen(&releases, wanted)?.version;
    if new == old {
        eprintln!("{PIN} already names octoflow-gate {new}");
        return Ok(String::new());
    }
    if new < FIRST_WITH_GATE {
        return Err(Error(format!("octoflow-gate {new} ships no gate, which came in {FIRST_WITH_GATE}")));
    }
    let sha = release_sha256(new)?;
    let unpacked = tempfile::Builder::new()
        .prefix("raise-gate-pin-")
        .tempdir()
        .map_err(|e| Error(format!("could not make a temporary directory: {e}")))?;
    let golden = fetch_golden(top, new, &sha, unpacked.path())?;
    let readme_path = top.join(GOLDEN_DIR).join("README.md");
    let readme = read(&readme_path)?;
    if !readme.contains(&old.to_string()) || !readme.contains(old_sha) {
        return Err(Error(format!("{} does not name octoflow-gate {old} and its sha256", readme_path.display())));
    }
    let manifest = read(&top.join(ENGINE_MANIFEST))?;
    let (octoflow, manifest) = bump_package_version(&manifest, old, new)?;
    let notes_path = top.join(RELEASE_NOTES_DIR).join(format!("v{octoflow}.md"));
    let notes = match fs::read_to_string(&notes_path) {
        Ok(text) => format!("{text}\n"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(Error(format!("could not read {}: {e}", notes_path.display()))),
    };
    let mut between: Vec<&Release> = releases
        .iter()
        .filter(|r| if new > old { r.version > old && r.version <= new } else { r.version == new })
        .collect();
    between.sort_by_key(|r| std::cmp::Reverse(r.version));

    // The writes.
    eprintln!("raising {PIN} from octoflow-gate {old} to {new}, octoflow to {octoflow}");
    write(&top.join(PIN), &replace_pin_line(&pin_text, &format!("{new} {sha}")))?;
    replace_golden(&top.join(GOLDEN_DIR), &golden)?;
    write(&readme_path, &readme.replace(old_sha, &sha).replace(&old.to_string(), &new.to_string()))?;
    write(&top.join(ENGINE_MANIFEST), &manifest)?;
    run(command("cargo").args(["update", "--workspace"]).current_dir(top), "refreshing Cargo.lock")?;
    write(&notes_path, &format!("{notes}{}", release_notes_section(old, new, &between)))?;
    Ok(between.iter().map(|r| format!("## octoflow-gate {}\n\n{}\n\n", r.version, r.body.trim())).collect())
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
    let url =
        format!("https://github.com/{GATE_REPO}/releases/download/v{version}/octoflow-gate-{version}.tar.gz.sha256");
    let asset = run(command("curl").args(["-fsSL", &url]), &format!("downloading {url}"))?;
    let sha = asset.split_whitespace().next().unwrap_or("");
    if sha.len() != 64 || !sha.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return Err(Error(format!("{url} holds no sha256: {asset:?}")));
    }
    Ok(sha.to_owned())
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

/// The version and sha256 of pin's first line that is not a comment, as ci/fetch-gate.sh reads it.
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

/// manifest's [package] version bumped for a gate raise from old to new (Version::bumped_for), with manifest so
/// changed. Errors: the [package] table has no `version = "x.y.z"` line.
fn bump_package_version(manifest: &str, old: Version, new: Version) -> Result<(Version, String), Error> {
    let mut in_package = false;
    let mut bumped = None;
    let mut lines = Vec::new();
    for line in manifest.lines() {
        if line.starts_with('[') {
            in_package = line.trim() == "[package]";
        }
        let current = line.strip_prefix("version = \"").and_then(|v| v.strip_suffix('"')).and_then(Version::parse);
        match current {
            Some(v) if in_package && bumped.is_none() => {
                let next = v.bumped_for(old, new);
                bumped = Some(next);
                lines.push(format!("version = \"{next}\""));
            }
            _ => lines.push(line.to_owned()),
        }
    }
    let bumped = bumped.ok_or_else(|| Error(format!("{ENGINE_MANIFEST}'s [package] has no version = \"x.y.z\"")))?;
    Ok((bumped, lines.iter().map(|l| format!("{l}\n")).collect()))
}

/// The release notes' section for a raise from old to new, wrapped at 120 columns as the docs are; between are the
/// releases it brings, newest first.
fn release_notes_section(old: Version, new: Version, between: &[&Release]) -> String {
    let link = |r: &Release| format!("[{}](https://github.com/{GATE_REPO}/releases/tag/v{})", r.version, r.version);
    let changes = match between {
        [newest, .., oldest] => format!("its release notes from {} to {}", link(oldest), link(newest)),
        [only] => format!("its release notes, {}", link(only)),
        [] => String::new(),
    };
    let text = format!(
        "The octoflow-gate release this octoflow is built with is {new}, in place of {old}. It judges a project whose \
         `ci/gate.pin` is missing, and `doctor --setup-gate` pins it in a project it sets up; a project that pins a \
         release of its own keeps it. What the gate changed: {changes}."
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
