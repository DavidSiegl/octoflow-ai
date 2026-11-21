//! The ACP adapters the desktop app installs on first use (herdr-gui-decoupling spec R6, R8): claude and codex speak
//! the Agent Client Protocol only through an adapter ([`Adapter`]), a Node program that brings its agent's own binary,
//! so the app installs a pinned Node and the pinned adapter, and runs the adapter's script with that Node
//! ([`Installed`]).
//!
//! Both are pinned in the engine ([`Pins::built_in`]): Node by its version and the sha256 its release's
//! `SHASUMS256.txt` gives each platform's tarball, which a download must match; each adapter by its directory under
//! `engine/acp-adapters/`, a package.json naming its version and the package-lock.json that pins every package it
//! pulls in, with its `integrity`, which npm checks. npm installs from that lockfile alone (`npm ci`), running no
//! package's scripts, and reads none of the user's npm settings ([`Installs::install`] says how). Raising a pin is a
//! change to this module or that directory, released like any other.
//!
//! An install is a directory named by its pins, `node-<node>-adapter-<adapter>`, among one adapter's installs in
//! `<adapter>-acp` in the engine's data directory (crate::env::data_dir), each with a Node of its own. It is made in
//! a temporary directory beside it and renamed into place whole, under a lock on a file there, so that a directory of
//! that name is always a whole install, and two installs at once, from two of the app's windows, make one. A raised
//! pin installs beside the old install, which goes once the new one is in place.

use std::ffi::OsString;
use std::fs::{self, File};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::error::{os_error, OrchestratorError};
use crate::host::{run_process, ProcessError};
use crate::http::{Http, HttpClient, HttpError, Request};
use crate::interrupt::Interrupt;

/// Node's version: a 22 LTS, since the adapter declares `"engines": {"node": ">=22"}`. 22.23.4 is from 2026-10-20,
/// the newest 22 at least two weeks old on 2026-11-21.
const NODE_VERSION: &str = "22.23.4";

/// The sha256 of Node NODE_VERSION's `node-v<version>-<platform>.tar.gz` for each platform the desktop app is built
/// for (packaging/build-desktop.sh: Linux and macOS, x86_64 and arm64), as
/// <https://nodejs.org/dist/v22.23.4/SHASUMS256.txt> lists it.
const NODE_SHA256: [(&str, &str); 4] = [
    ("linux-x64", "b8492284c76a54baf58d61668dd0e41ee0fc98adabf9503d30f05d08f55b4406"),
    ("linux-arm64", "f7a1e486dbc8b286847815b477d4fb5e9fb963ed4489c3e1588853d685f589f9"),
    ("darwin-x64", "efcc2b0db6f94bf53eddb038fe2b370277a042bd29e4f19b8f8e23598795b819"),
    ("darwin-arm64", "c9e1dbc748b33ed2d14f42445191ecaf14c56259c7a9a26d0042c49ec2b66428"),
];

/// An agent the app runs over ACP through an adapter it installs. Each adapter's pins are its directory in
/// `engine/acp-adapters/`: package.json, which names the adapter's version, the newest release at least two weeks old
/// when it was pinned, and the lockfile npm installs it from, every package with its integrity hash, made from
/// package.json by the pinned Node's npm (10.9.9) with `npm install --package-lock-only --ignore-scripts`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Adapter {
    /// `@agentclientprotocol/claude-agent-acp` 0.101.0 (2026-11-05), which brings its own Claude Code.
    Claude,
    /// `@agentclientprotocol/codex-acp` 2.5.0 (2026-10-29), which brings its own codex (`@openai/codex` 0.165.0) and
    /// runs it, not a codex on PATH, while CODEX_PATH is unset.
    Codex,
}

impl Adapter {
    /// Every adapter, in HARNESSES' order of their harnesses.
    pub const ALL: [Adapter; 2] = [Adapter::Claude, Adapter::Codex];

    /// The harness whose agent this adapter is, as crate::harness::HARNESSES names it; also what /setup and
    /// POST /setup/adapter call the adapter.
    pub fn harness(self) -> &'static str {
        match self {
            Adapter::Claude => "claude",
            Adapter::Codex => "codex",
        }
    }

    /// The adapter named name, as harness gives it; None for any other name.
    pub fn named(name: &str) -> Option<Adapter> {
        Adapter::ALL.into_iter().find(|a| a.harness() == name)
    }

    /// The disk an install of the built-in pins takes, in megabytes, as the setup dialog's button says before it
    /// downloads, measured on linux-x64 on 2026-11-21 with Node's 203 MB. Measure it again when a pin is raised.
    pub fn megabytes(self) -> u64 {
        match self {
            // npm installs both the glibc and the musl build of the Agent SDK's Claude Code binary, about 310 MB each,
            // since the lockfile records no package's libc.
            Adapter::Claude => 913,
            // @openai/codex's linux-x64 build is 426 MB of it.
            Adapter::Codex => 649,
        }
    }

    /// The adapter's npm package, as its package.json depends on it.
    fn package(self) -> &'static str {
        match self {
            Adapter::Claude => "@agentclientprotocol/claude-agent-acp",
            Adapter::Codex => "@agentclientprotocol/codex-acp",
        }
    }

    /// The text of its package.json and of its lockfile.
    fn pinned(self) -> (&'static str, &'static str) {
        match self {
            Adapter::Claude => (
                include_str!("../acp-adapters/claude/package.json"),
                include_str!("../acp-adapters/claude/package-lock.json"),
            ),
            Adapter::Codex => (
                include_str!("../acp-adapters/codex/package.json"),
                include_str!("../acp-adapters/codex/package-lock.json"),
            ),
        }
    }
}

/// Where Node's releases are downloaded from.
const NODE_DIST: &str = "https://nodejs.org/dist";

/// The npm registry the adapter's packages come from, as package-lock.json's `resolved` URLs name it.
const REGISTRY: &str = "https://registry.npmjs.org/";

/// How long connecting to Node's server, and then each read of its answer, may take, in seconds.
const DOWNLOAD_TIMEOUT: u64 = 60;

/// How long unpacking Node may take.
const UNPACK_TIMEOUT: Duration = Duration::from_secs(600);
/// How long npm's install of the adapter may take: it downloads most of an install's megabytes.
const NPM_TIMEOUT: Duration = Duration::from_secs(3600);

/// The start of the name of an install still being made, in the installs' directory, which no lookup takes for one.
const TEMPORARY: &str = ".tmp-";

/// What an install holds: Node's version and its tarball for this platform, and an adapter's package.json and
/// lockfile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pins {
    /// Node's version, as its release names it, without the `v`.
    node: String,
    /// Node's platform, as its tarballs name it (`linux-x64`), with the sha256 of its tarball, in lowercase hex; None
    /// on a platform the app is not built for, which install refuses.
    tarball: Option<(String, String)>,
    /// The adapter's npm package, whose script an install runs.
    package: &'static str,
    /// The adapter's version, as package.json depends on it.
    adapter: String,
    /// The text of the adapter's package.json, written into an install for npm to read.
    package_json: String,
    /// The text of the lockfile npm installs package_json's dependencies from, written beside it.
    lockfile: String,
}

impl Pins {
    /// The pins this engine was built with for adapter, for the platform it was built for.
    pub fn built_in(adapter: Adapter) -> Pins {
        let platform = node_platform(std::env::consts::OS, std::env::consts::ARCH);
        let tarball = platform.and_then(|p| NODE_SHA256.iter().find(|(q, _)| *q == p));
        let tarball = tarball.map(|(p, sha256)| (p.to_string(), sha256.to_string()));
        let (package_json, lockfile) = adapter.pinned();
        Pins::new(NODE_VERSION, tarball, adapter.package(), package_json, lockfile)
            .expect("each package.json in engine/acp-adapters/ names its adapter")
    }

    /// Pins of Node node, its tarball (platform, sha256) as Pins::tarball says, and the version of package, an npm
    /// package, that package_json depends on, installed from lockfile. Errors: package_json is not JSON, or names no
    /// version of package.
    pub fn new(
        node: &str,
        tarball: Option<(String, String)>,
        package: &'static str,
        package_json: &str,
        lockfile: &str,
    ) -> Result<Pins, OrchestratorError> {
        let manifest: serde_json::Value = serde_json::from_str(package_json)
            .map_err(|e| OrchestratorError(format!("{package}'s package.json is not JSON: {e}")))?;
        let adapter = manifest.pointer(&format!("/dependencies/{}", package.replace('/', "~1")));
        let Some(adapter) = adapter.and_then(serde_json::Value::as_str) else {
            return Err(OrchestratorError(format!("the adapter's package.json names no version of {package}")));
        };
        Ok(Pins {
            node: node.to_string(),
            tarball,
            package,
            adapter: adapter.to_string(),
            package_json: package_json.to_string(),
            lockfile: lockfile.to_string(),
        })
    }

    /// The name of these pins' install in the installs' directory.
    fn install_name(&self) -> String {
        format!("node-{}-adapter-{}", self.node, self.adapter)
    }
}

/// How an install's agent starts: `<node> <script>`, then the launch's arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    /// The install's Node, an absolute path.
    pub node: String,
    /// The adapter's script, which Node runs, an absolute path.
    pub script: String,
}

/// An adapter's agent that cannot log in on its own (Installs::logged_in).
#[derive(Debug, PartialEq, Eq)]
pub struct NotLoggedIn {
    /// What is missing: the files and variables looked for.
    pub detail: String,
    /// One sentence on how to log in: with the codex of the current pins' install, or the codex CLI before there is
    /// one.
    pub fix: String,
}

/// The installs of one ACP adapter in one environment's data directory, and how a new one is made: for one set of
/// pins, the current ones, with Node and the adapter downloaded from where it says.
pub struct Installs {
    /// Whose installs these are.
    adapter: Adapter,
    /// The directory the installs are in, `<adapter>-acp` in the data directory; made by the first install.
    root: String,
    /// The pins of the one install this looks for and makes; installs of any others are removed by the next install.
    pins: Pins,
    /// Where Node's releases are, without a trailing "/": NODE_DIST, or a test's server.
    node_dist: String,
    /// The npm registry, as npm's --registry takes it: REGISTRY, or a test's.
    registry: String,
    /// The environment npm and tar run in, and that the download's proxies and certificates come from: the one the
    /// installs were found in, without npm's own variables (without_npm_config).
    env: Vec<(OsString, OsString)>,
    /// How Node is downloaded; None for the HttpClient of env, made when an install downloads.
    http: Option<Arc<dyn Http>>,
}

impl Installs {
    /// adapter's installs in the data directory of env, the environment of the processes the engine starts (an ACP
    /// run's agents', which find the install there), for the built-in pins, downloaded from Node's and npm's own
    /// servers.
    pub fn in_environment(adapter: Adapter, env: Vec<(OsString, OsString)>) -> Installs {
        let var = |name: &str| {
            let found = env.iter().find(|(k, _)| k == name).map(|(_, v)| v.to_string_lossy().into_owned());
            found.filter(|v| !v.is_empty())
        };
        let root = format!("{}/{}-acp", crate::env::data_dir(&var), adapter.harness());
        Installs {
            adapter,
            root,
            pins: Pins::built_in(adapter),
            node_dist: NODE_DIST.to_string(),
            registry: REGISTRY.to_string(),
            env: without_npm_config(env),
            http: None,
        }
    }

    /// These installs, but of pins, downloaded by http from node_dist (Node's releases, as NODE_DIST) and registry (as
    /// REGISTRY): what the tests make in place of the network.
    #[cfg(test)]
    pub(crate) fn with_sources(self, pins: Pins, http: Arc<dyn Http>, node_dist: &str, registry: &str) -> Installs {
        let node_dist = node_dist.trim_end_matches('/').to_string();
        Installs { pins, http: Some(http), node_dist, registry: registry.to_string(), ..self }
    }

    /// Put in place an install of the current pins whose Node is a script of text and whose adapter script is empty,
    /// as the tests of what runs an install want one.
    #[cfg(test)]
    pub(crate) fn install_fake(&self, node: &str) -> Installed {
        use std::os::unix::fs::PermissionsExt;
        let installed = self.at(&format!("{}/{}", self.root, self.pins.install_name()));
        for path in [&installed.node, &installed.script] {
            fs::create_dir_all(Path::new(path).parent().unwrap()).unwrap();
        }
        fs::write(&installed.node, node).unwrap();
        fs::set_permissions(&installed.node, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(&installed.script, "").unwrap();
        installed
    }

    /// Whose installs these are.
    pub fn adapter(&self) -> Adapter {
        self.adapter
    }

    /// The install of the current pins, when its Node and its script are there; None when either is not, as before the
    /// first install or after a pin was raised. It checks no other file, nor that the install still runs.
    pub fn installed(&self) -> Option<Installed> {
        let installed = self.at(&format!("{}/{}", self.root, self.pins.install_name()));
        let whole = Path::new(&installed.node).is_file() && Path::new(&installed.script).is_file();
        whole.then_some(installed)
    }

    /// Whether the adapter's agent can log in on its own, as far as the files and variables of the installs'
    /// environment tell: codex when `auth.json` is in its home (CODEX_HOME, else ~/.codex) or OPENAI_API_KEY or
    /// CODEX_API_KEY is set, which is what codex-acp 2.5.0 reads; claude always, its adapter reporting no login it
    /// lacks until a turn. Errors: codex is not logged in.
    pub fn logged_in(&self) -> Result<(), NotLoggedIn> {
        if self.adapter != Adapter::Codex {
            return Ok(());
        }
        let var =
            |name: &str| self.env.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone()).filter(|v| !v.is_empty());
        if var("OPENAI_API_KEY").is_some() || var("CODEX_API_KEY").is_some() {
            return Ok(());
        }
        let home = match (var("CODEX_HOME"), var("HOME")) {
            (Some(codex_home), _) => codex_home.to_string_lossy().into_owned(),
            (None, Some(home)) => format!("{}/.codex", home.to_string_lossy()),
            (None, None) => "~/.codex".to_string(),
        };
        if Path::new(&format!("{home}/auth.json")).is_file() {
            return Ok(());
        }
        let codex = match self.installed() {
            Some(installed) => {
                let package = installed.script.trim_end_matches(&format!("{}/dist/index.js", self.pins.package));
                format!("`{} {package}@openai/codex/bin/codex.js login`", installed.node)
            }
            None => "`codex login`".to_string(),
        };
        let detail = format!("codex is not logged in: no {home}/auth.json, no OPENAI_API_KEY");
        Err(NotLoggedIn { detail, fix: format!("log in with {codex}") })
    }

    /// Install the current pins, unless they are installed already, and then remove every other install: download
    /// Node's tarball for this platform and check its sha256, unpack it with `tar`, and install the adapter with its
    /// npm: `npm ci --ignore-scripts`, with `--userconfig` and `--globalconfig` each an empty file, an explicit
    /// `--registry`, a cache of its own, and none of npm's `npm_config_*` variables, so that nothing of the user's npm
    /// setup applies; proxies and certificates come from the environment, as http.rs and npm read them. Waits for
    /// another install in progress, by this or another process, to end first. A failure leaves no install behind, and
    /// whatever a failed or killed one left is removed when the next starts.
    ///
    /// Returns the install. Errors: this platform has no pinned Node; the installs' directory or a file in it cannot
    /// be made, locked, written or removed; the download fails, or its sha256 is not the pin's; tar or npm fails, or
    /// stopping is raised while either runs; npm's install has no adapter script.
    pub fn install(&self, stopping: &Interrupt) -> Result<Installed, OrchestratorError> {
        fs::create_dir_all(&self.root).map_err(|e| self.failed("create", &self.root, &e))?;
        let lock_path = format!("{}/.lock", self.root);
        let lock = File::create(&lock_path).map_err(|e| self.failed("create", &lock_path, &e))?;
        lock.lock().map_err(|e| self.failed("lock", &lock_path, &e))?;
        // Unlocked when lock is dropped, at the return, or by the system when the process dies.
        let installed = match self.installed() {
            Some(installed) => installed,
            None => self.install_locked(stopping)?,
        };
        self.remove_others()?;
        Ok(installed)
    }

    /// install's work once the lock is held and the current pins are not installed: clear what earlier attempts
    /// left, and what is left of a damaged install of these pins, make the install in a temporary directory, and
    /// rename it into place, removing the temporary directory when any step fails.
    fn install_locked(&self, stopping: &Interrupt) -> Result<Installed, OrchestratorError> {
        let Some((platform, sha256)) = &self.pins.tarball else {
            return Err(OrchestratorError(format!(
                "the app installs {}'s ACP adapter only on Linux and macOS, on x86_64 or arm64, not on {} {}",
                self.adapter.harness(),
                std::env::consts::OS,
                std::env::consts::ARCH
            )));
        };
        let name = self.pins.install_name();
        for left in self.entries()?.iter().filter(|n| n.starts_with(TEMPORARY) || **n == name) {
            self.remove(left)?;
        }
        let temporary = format!("{TEMPORARY}{name}");
        let dir = format!("{}/{temporary}", self.root);
        fs::create_dir(&dir).map_err(|e| self.failed("create", &dir, &e))?;
        let made = self.make(&dir, platform, sha256, stopping).and_then(|()| {
            let path = format!("{}/{name}", self.root);
            fs::rename(&dir, &path).map_err(|e| self.failed("rename into place", &dir, &e))?;
            Ok(self.at(&path))
        });
        // What a removal that fails leaves, the next install removes; the caller is told of both failures.
        made.map_err(|e| match fs::remove_dir_all(&dir) {
            Ok(()) => e,
            Err(removing) => OrchestratorError(format!("{}; and {}", e.0, self.failed("remove", &dir, &removing).0)),
        })
    }

    /// Make an install of the pins in dir, an empty directory: Node for platform, whose tarball has sha256, in
    /// dir/node, and the adapter in dir/adapter.
    fn make(&self, dir: &str, platform: &str, sha256: &str, stopping: &Interrupt) -> Result<(), OrchestratorError> {
        let node = &self.pins.node;
        let stem = format!("node-v{node}-{platform}");
        let url = format!("{}/v{node}/{stem}.tar.gz", self.node_dist);
        let tarball = self.download(&url, sha256)?;
        let tarball_path = format!("{dir}/node.tar.gz");
        write(&tarball_path, &tarball)?;
        self.run(&["tar", "-xzf", &tarball_path, "-C", dir], UNPACK_TIMEOUT, stopping, "unpack Node")?;
        let unpacked = format!("{dir}/{stem}");
        fs::rename(&unpacked, format!("{dir}/node")).map_err(|e| self.failed("rename", &unpacked, &e))?;
        fs::remove_file(&tarball_path).map_err(|e| self.failed("remove", &tarball_path, &e))?;

        let adapter = format!("{dir}/adapter");
        fs::create_dir(&adapter).map_err(|e| self.failed("create", &adapter, &e))?;
        write(&format!("{adapter}/package.json"), self.pins.package_json.as_bytes())?;
        write(&format!("{adapter}/package-lock.json"), self.pins.lockfile.as_bytes())?;
        // Two empty files: npm refuses to load one file as both, in @npmcli/config 9.0.0's `#loadObject`
        // (lib/index.js, "double-loading config ... as global"), the version npm 10.9.9 bundles.
        let (user_npmrc, global_npmrc) = (format!("{dir}/empty-user-npmrc"), format!("{dir}/empty-global-npmrc"));
        let cache = format!("{dir}/npm-cache");
        write(&user_npmrc, b"")?;
        write(&global_npmrc, b"")?;
        let npm = format!("{dir}/node/lib/node_modules/npm/bin/npm-cli.js");
        let node_bin = format!("{dir}/node/bin/node");
        let isolated = ["--userconfig", &user_npmrc, "--globalconfig", &global_npmrc, "--registry", &self.registry];
        let quiet = ["--no-audit", "--no-fund", "--no-update-notifier"];
        let into = ["--prefix", &adapter, "--cache", &cache];
        let argv = [&[node_bin.as_str(), &npm, "ci", "--ignore-scripts"][..], &isolated, &into, &quiet].concat();
        let doing = format!("install {}'s ACP adapter", self.adapter.harness());
        self.run(&argv, NPM_TIMEOUT, stopping, &doing)?;
        if !Path::new(&self.at(dir).script).is_file() {
            return Err(OrchestratorError(format!("npm installed no {} script in {adapter}", self.pins.package)));
        }
        for path in [&user_npmrc, &global_npmrc, &cache] {
            if Path::new(path).exists() {
                let removed = if path == &cache { fs::remove_dir_all(path) } else { fs::remove_file(path) };
                removed.map_err(|e| self.failed("remove", path, &e))?;
            }
        }
        Ok(())
    }

    /// The body of url, which must have the sha256 pinned. Errors: no 2xx answer comes; its sha256 differs.
    fn download(&self, url: &str, pinned: &str) -> Result<Vec<u8>, OrchestratorError> {
        let request = Request { url: url.to_string(), headers: vec![], timeout: DOWNLOAD_TIMEOUT };
        let answer = match &self.http {
            Some(http) => http.send(&request),
            None => {
                let env = |name: &str| {
                    let found = self.env.iter().find(|(k, _)| k == name);
                    found.map(|(_, v)| v.to_string_lossy().into_owned())
                };
                HttpClient::with_env(&env).send(&request)
            }
        };
        let body = answer
            .map_err(|e| match e {
                HttpError::Status(code) => OrchestratorError(format!("cannot download {url}: HTTP {code}")),
                HttpError::NoAnswer(why) => OrchestratorError(format!("cannot download {url}: {why}")),
            })?
            .body;
        let got: String = Sha256::digest(&body).iter().map(|b| format!("{b:02x}")).collect();
        if got != pinned {
            return Err(OrchestratorError(format!("{url} has sha256 {got}, not the pinned {pinned}; refused")));
        }
        Ok(body)
    }

    /// Run argv, argv[0] found on env's PATH, in env, for at most timeout. Errors: it does not start, does not exit 0,
    /// runs out of time, or stopping is raised; the message starts "cannot <doing>" and ends with its standard error.
    fn run(
        &self,
        argv: &[&str],
        timeout: Duration,
        stopping: &Interrupt,
        doing: &str,
    ) -> Result<(), OrchestratorError> {
        let argv: Vec<String> = argv.iter().map(|a| a.to_string()).collect();
        let why = match run_process(&argv, None, Some(timeout), &self.env, stopping) {
            Ok((0, _, _)) => return Ok(()),
            Ok((status, _, stderr)) => {
                let stderr = String::from_utf8_lossy(&stderr);
                format!("{} exited with status {status}: {}", argv[0], stderr.trim())
            }
            Err(ProcessError::Start(e)) => os_error(&e, Some(&argv[0])),
            Err(ProcessError::Io(e)) => os_error(&e, None),
            Err(ProcessError::Timeout) => format!("it took longer than {} seconds", timeout.as_secs()),
            Err(ProcessError::Interrupted) => "interrupted".to_string(),
        };
        Err(OrchestratorError(format!("cannot {doing}: {why}")))
    }

    /// Remove every install but the current pins', and anything else in the installs' directory but its lock.
    fn remove_others(&self) -> Result<(), OrchestratorError> {
        let current = self.pins.install_name();
        for name in self.entries()?.iter().filter(|n| **n != current && *n != ".lock") {
            self.remove(name)?;
        }
        Ok(())
    }

    /// The names in the installs' directory.
    fn entries(&self) -> Result<Vec<String>, OrchestratorError> {
        let entries = fs::read_dir(&self.root).map_err(|e| self.failed("list", &self.root, &e))?;
        let mut names = vec![];
        for entry in entries {
            let entry = entry.map_err(|e| self.failed("list", &self.root, &e))?;
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
        Ok(names)
    }

    /// Remove name, a file or a directory with everything in it, from the installs' directory.
    fn remove(&self, name: &str) -> Result<(), OrchestratorError> {
        let path = format!("{}/{name}", self.root);
        let removed = match fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_dir() => fs::remove_dir_all(&path),
            Ok(_) => fs::remove_file(&path),
            Err(e) => Err(e),
        };
        removed.map_err(|e| self.failed("remove", &path, &e))
    }

    /// The Installed of an install in dir, whether or not it is there.
    fn at(&self, dir: &str) -> Installed {
        Installed {
            node: format!("{dir}/node/bin/node"),
            // Both adapters' package.json name dist/index.js as their bin.
            script: format!("{dir}/adapter/node_modules/{}/dist/index.js", self.pins.package),
        }
    }

    /// The error of doing to path that failed with e.
    fn failed(&self, doing: &str, path: &str, e: &std::io::Error) -> OrchestratorError {
        OrchestratorError(format!("cannot {doing} {}", os_error(e, Some(path))))
    }
}

/// Write bytes to path, a new file. Errors: it cannot be written.
fn write(path: &str, bytes: &[u8]) -> Result<(), OrchestratorError> {
    fs::write(path, bytes).map_err(|e| OrchestratorError(format!("cannot write {}", os_error(&e, Some(path)))))
}

/// Node's name for the platform of os and arch, as std::env::consts name them, in its tarballs' names (`linux-x64`);
/// None for a platform the app is not built for (packaging/build-desktop.sh).
fn node_platform(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("linux", "x86_64") => Some("linux-x64"),
        ("linux", "aarch64") => Some("linux-arm64"),
        ("macos", "x86_64") => Some("darwin-x64"),
        ("macos", "aarch64") => Some("darwin-arm64"),
        _ => None,
    }
}

/// env without the variables npm reads its configuration from, `npm_config_*` in any case.
fn without_npm_config(env: Vec<(OsString, OsString)>) -> Vec<(OsString, OsString)> {
    let is_npm = |k: &OsString| k.to_string_lossy().to_ascii_lowercase().starts_with("npm_config_");
    env.into_iter().filter(|(k, _)| !is_npm(k)).collect()
}

#[cfg(test)]
mod tests;
