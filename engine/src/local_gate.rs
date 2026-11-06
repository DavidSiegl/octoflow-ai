//! `--quality-gate local`: the engine runs octoflow-gate itself, in the run's worktree on the project's host, in place
//! of a Jenkins job (docs/quality-gate.md, "The local gate").
//!
//! The release is the one the run's base pins in ci/gate.pin, else BUILT_IN_PIN. It is fetched once per version into
//! the host's cache, `${XDG_CACHE_HOME:-~/.cache}/octoflow/gate/<version>/`, and checked against the pin's sha256.
//! The gate judges by the base's gate.toml and pyproject.toml, never the change's, so a change cannot loosen how it
//! is itself judged, as a Jenkins quality build judges by main's. Every step runs as a command on the host
//! (curl, sha256sum, tar, python3), so a project on another machine is judged there, as its git steps are.

use crate::constants::NETWORK_TIMEOUT;
use crate::error::OrchestratorError;
use crate::host::{Host, DEFAULT_TIMEOUT};
use crate::quality::last_lines;

/// The release a base without ci/gate.pin is judged by, in ci/gate.pin's form: the octoflow-gate release this engine
/// was tested against, whose quality.json schema crate::quality::read_report reads. Raise it with the repository's own
/// ci/gate.pin.
pub const BUILT_IN_PIN: &str = "0.5.0 4ebf96264d5ae20cc2dd0516122cd50ebf11dac9f1944c14d899aece882773f8";

/// Where a pin's release is downloaded from: the GitHub release `v<version>` of octoflow-gate, with {v} the version.
const RELEASE_URL: &str = "https://github.com/DavidSiegl/octoflow-gate/releases/download/v{v}/octoflow-gate-{v}.tar.gz";

/// The host's gate cache, as sh expands it: one directory per release version.
const CACHE_DIR: &str = "${XDG_CACHE_HOME:-$HOME/.cache}/octoflow/gate";

/// The pin file, relative to the top level of the judged repository.
const PIN_PATH: &str = "ci/gate.pin";

/// What a run of the local gate came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalVerdict {
    /// The gate judged the change.
    Judged {
        /// The version of the release that judged it.
        release: String,
        /// Its quality.md.
        quality: String,
        /// Its quality.json.
        json: String,
    },
    /// The gate could not judge (exit status 2) and wrote nothing, as for a gate.toml it refuses: the end of what it
    /// said on standard error.
    CouldNotJudge(String),
}

/// Fails, before a run's interview, if host could not run the local gate: python3 does not start from PATH, or the
/// gate cache cannot be made or written to. Fetching needs curl, sha256sum and tar too, which a fetch names when one
/// is missing.
pub fn preflight(host: &dyn Host) -> Result<(), OrchestratorError> {
    let python = host.run(&["python3", "--version"], None, DEFAULT_TIMEOUT);
    if let Some(problem) = match &python {
        Err(e) => Some(e.to_string()),
        Ok(proc) if proc.status != 0 => Some(format!("`python3 --version` exits {}", proc.status)),
        Ok(_) => None,
    } {
        return Err(OrchestratorError(format!(
            "the local quality gate runs octoflow-gate with python3 from PATH, and {problem}"
        )));
    }
    let script = format!("d={CACHE_DIR}; mkdir -p -- \"$d\" && [ -w \"$d\" ] || {{ echo \"cannot write $d\" >&2; exit 1; }}");
    let made = host.run(&["sh", "-c", &script], None, DEFAULT_TIMEOUT)?;
    if made.status != 0 {
        return Err(OrchestratorError(format!(
            "the local quality gate keeps its releases in a cache directory: {}",
            made.stderr.trim()
        )));
    }
    Ok(())
}

/// Have the gate judge the change in work_dir since commit base, within timeout seconds, writing its reports and the
/// judging settings into out_dir, made if missing (quality.md, quality.json, judge/), what an earlier judgement left
/// there removed first. The release, the gate.toml and the pyproject.toml are base's (the module's comment). The
/// gate runs the project's tests itself, with gate.toml's [[test]] commands, as the user running the engine, with no
/// sandbox; it is killed with its process group at the timeout or on the Ctrl-C.
///
/// Errors: base's ci/gate.pin has no `<version> <sha256>` line; the release cannot be fetched or does not match the
/// sha256, or the cache holds it checked against another; the gate cannot be started, runs out of time, is
/// interrupted, exits other than 0, 1 or 2, or exits 0 or 1 without both reports; git or a file fails.
pub fn judge(
    host: &dyn Host,
    work_dir: &str,
    base: &str,
    out_dir: &str,
    timeout: u64,
) -> Result<LocalVerdict, OrchestratorError> {
    let text = |path: &str| -> Result<Option<String>, OrchestratorError> {
        Ok(host.file_at(work_dir, base, path)?.map(|bytes| String::from_utf8_lossy(&bytes).into_owned()))
    };
    let (version, sha256) = match text(PIN_PATH)? {
        Some(pin) => parse_pin(&pin).map_err(|why| OrchestratorError(format!("the run's base's {PIN_PATH}: {why}")))?,
        None => parse_pin(BUILT_IN_PIN)?,
    };
    // The judging settings: base's gate.toml (empty, so the defaults, without one), and base's pyproject.toml beside
    // it, whose linter rules --config then makes the gate take.
    let judge = format!("{out_dir}/judge");
    host.write(&format!("{judge}/gate.toml"), &text("gate.toml")?.unwrap_or_default(), false)?;
    let config = match text("pyproject.toml")? {
        Some(pyproject) => {
            host.write(&format!("{judge}/pyproject.toml"), &pyproject, false)?;
            format!("{judge}/pyproject.toml")
        }
        None => format!("{judge}/gate.toml"),
    };
    let release = fetch(host, &version, &sha256)?;
    let (quality, json) = (format!("{out_dir}/quality.md"), format!("{out_dir}/quality.json"));
    host.check(&["rm", "-f", "--", &quality, &json], None, DEFAULT_TIMEOUT)?;
    let (gate, helper) = (format!("{release}/gate.py"), format!("{release}/gate-rust-linux-x86_64"));
    let argv = [
        "sh", "-c", "cd -- \"$1\" || exit 125; shift; exec \"$@\"", "_", work_dir,
        "python3", &gate, "--base", base, "--config", &config, "--rust-syntax", &helper, "--out", &quality,
    ];
    let ran = host.run_verbatim(&argv, None, timeout)?;
    let said = last_lines(&ran.stderr);
    match ran.status {
        0 | 1 => {
            let read = |path: &str| {
                host.read(path)?.ok_or_else(|| {
                    OrchestratorError(format!("octoflow-gate {version} exited {} but wrote no {path}", ran.status))
                })
            };
            Ok(LocalVerdict::Judged { quality: read(&quality)?, json: read(&json)?, release: version })
        }
        2 => Ok(LocalVerdict::CouldNotJudge(said)),
        status => Err(OrchestratorError(format!("octoflow-gate {version} in {work_dir} exited {status}: {said}"))),
    }
}

/// The directory in the host's cache that holds release version, fetched and unpacked there first unless it already
/// is. The tarball is checked against sha256 before it is unpacked, and unpacked into a temporary directory renamed
/// into place, so a partial download never sits under the final name; the directory keeps the sha256 it was checked
/// against in .sha256. Errors: curl, sha256sum, tar or the cache fail, the tarball does not match, or the cache holds
/// the version checked against another sha256 (a pin whose hash changed for the same version: delete the directory
/// to fetch it again).
fn fetch(host: &dyn Host, version: &str, sha256: &str) -> Result<String, OrchestratorError> {
    let script = format!(
        r#"set -u
v=$1 sum=$2 url=$3 cache={CACHE_DIR}
dir=$cache/$v
if [ ! -d "$dir" ]; then
    mkdir -p -- "$cache" && tmp=$(mktemp -d "$cache/.fetch-$v.XXXXXX") || exit 1
    trap 'rm -rf -- "$tmp"' EXIT
    curl -fsSL -o "$tmp/gate.tar.gz" -- "$url" || {{ echo "cannot download $url" >&2; exit 1; }}
    got=$(sha256sum < "$tmp/gate.tar.gz") || exit 1
    got=${{got%% *}}
    [ "$got" = "$sum" ] || {{ echo "$url has sha256 $got, not the pinned $sum" >&2; exit 1; }}
    mkdir -- "$tmp/release" && tar -xzf "$tmp/gate.tar.gz" -C "$tmp/release" &&
        printf '%s\n' "$sum" > "$tmp/release/.sha256" || exit 1
    # Another run may have put the version in place meanwhile, which is checked below as any cached one.
    mv -T -- "$tmp/release" "$dir" 2>/dev/null || [ -d "$dir" ] || {{ echo "cannot move the release to $dir" >&2; exit 1; }}
fi
[ "$(cat -- "$dir/.sha256" 2>/dev/null)" = "$sum" ] ||
    {{ echo "$dir was checked against another sha256 than the pinned $sum; delete it to fetch it again" >&2; exit 1; }}
printf '%s\n' "$dir""#
    );
    let url = RELEASE_URL.replace("{v}", version);
    let ran = host.run(&["sh", "-c", &script, "_", version, sha256, &url], None, NETWORK_TIMEOUT)?;
    if ran.status != 0 {
        return Err(OrchestratorError(format!("fetching octoflow-gate {version}: {}", ran.stderr.trim())));
    }
    Ok(ran.stdout.trim_end_matches('\n').to_string())
}

/// The release version and sha256 a pin names, read as ci/fetch-gate.sh reads one: the first line not starting with
/// #, `<version> <sha256>`, the version only letters, digits and `.+-`, as it goes into a URL, and the sha256 64
/// lowercase hex digits. Errors: no such line.
pub fn parse_pin(pin: &str) -> Result<(String, String), OrchestratorError> {
    let line = pin.lines().find(|l| !l.starts_with('#')).unwrap_or("");
    let (version, sha256) = line.split_once(' ').unwrap_or((line, ""));
    let version_ok = !version.is_empty() && version.bytes().all(|b| b.is_ascii_alphanumeric() || b".+-".contains(&b));
    let sha_ok = sha256.len() == 64 && sha256.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if !version_ok || !sha_ok {
        return Err(OrchestratorError(format!("its first line '{line}' is not '<version> <sha256>'")));
    }
    Ok((version.to_string(), sha256.to_string()))
}

#[cfg(test)]
mod tests;
