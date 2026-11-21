//! The tests of acp_adapter: installs made from a fake Node tarball, served by FakeDist in place of nodejs.org, whose
//! bin/node is FAKE_NODE, which stands in for npm too.

use std::collections::HashMap;
use std::process::Command;
use std::sync::Mutex;

use super::*;
use crate::http::Response;
use crate::test_support::TempDir;

/// The fake Node's version and platform.
const NODE: &str = "1.0.0";
const PLATFORM: &str = "test-x64";
const DIST: &str = "http://dist.test";
const REGISTRY_URL: &str = "http://registry.test/";

/// The bin/node of the fake tarball: run as npm-cli.js, it records its arguments in $FAKE_RECORD/argv, its
/// environment in $FAKE_RECORD/env and the sizes of its --userconfig and --globalconfig in $FAKE_RECORD/config-bytes,
/// then writes the script of the package that --prefix's package.json depends on under --prefix, as `npm ci` would,
/// and a file in --cache. As npm 10.9.9 does, it
/// fails when --userconfig and --globalconfig name the same file ("double-loading config"). With FAKE_NPM_FAILS set it
/// fails first, with FAKE_NPM_SLOW set it takes a second.
const FAKE_NODE: &str = r#"#!/bin/sh
echo "$@" > "$FAKE_RECORD/argv"
env > "$FAKE_RECORD/env"
[ -n "$FAKE_NPM_FAILS" ] && { echo "npm error boom" >&2; exit 1; }
[ -n "$FAKE_NPM_SLOW" ] && sleep 1
while [ $# -gt 0 ]; do
  case "$1" in
  --prefix) prefix=$2 ;;
  --cache) cache=$2 ;;
  --userconfig) user=$2 ;;
  --globalconfig) global=$2 ;;
  esac
  shift
done
[ "$user" = "$global" ] && { echo "double-loading config \"$global\" as \"global\"" >&2; exit 1; }
echo "$(wc -c < "$user" | tr -d ' ') $(wc -c < "$global" | tr -d ' ')" > "$FAKE_RECORD/config-bytes"
package=$(sed 's/.*"dependencies": *{ *"\([^"]*\)".*/\1/' "$prefix/package.json")
mkdir -p "$prefix/node_modules/$package/dist" "$cache"
echo script > "$prefix/node_modules/$package/dist/index.js"
echo cached > "$cache/entry"
"#;

/// Node's releases, from memory: each URL's body, and every URL asked for.
struct FakeDist {
    bodies: HashMap<String, Vec<u8>>,
    asked: Mutex<Vec<String>>,
}

impl Http for FakeDist {
    fn send(&self, request: &Request) -> Result<Response, HttpError> {
        self.asked.lock().unwrap().push(request.url.clone());
        self.bodies.get(&request.url).map(|b| Response::ok(b.clone())).ok_or(HttpError::Status(404))
    }
}

/// A test's directory: the data directory the installs go in, the fake npm's records, and the fake tarball.
struct Fixture {
    tmp: TempDir,
    tarball: Vec<u8>,
}

impl Fixture {
    fn new() -> Fixture {
        let tmp = TempDir::new();
        std::fs::create_dir(tmp.join("record")).unwrap();
        Fixture { tarball: tarball(&tmp, NODE), tmp }
    }

    /// Pins of Node node, its tarball's sha256 being sha256, and claude's adapter at version adapter.
    fn pins(&self, node: &str, sha256: &str, adapter: &str) -> Pins {
        pins_of(Adapter::Claude, node, sha256, adapter)
    }

    /// claude's installs for pins, as installs_of makes them.
    fn installs(&self, pins: Pins, tarballs: &[(&str, &[u8])], extra: &[(&str, &str)]) -> (Installs, Arc<FakeDist>) {
        self.installs_of(Adapter::Claude, pins, tarballs, extra)
    }

    /// adapter's installs for pins, with the fake dist serving each tarball of tarballs (Node's version, its bytes)
    /// and the environment holding extra as well as PATH, XDG_DATA_HOME and FAKE_RECORD.
    fn installs_of(
        &self,
        adapter: Adapter,
        pins: Pins,
        tarballs: &[(&str, &[u8])],
        extra: &[(&str, &str)],
    ) -> (Installs, Arc<FakeDist>) {
        let bodies = tarballs
            .iter()
            .map(|(node, body)| (format!("{DIST}/v{node}/node-v{node}-{PLATFORM}.tar.gz"), body.to_vec()));
        let dist = Arc::new(FakeDist { bodies: bodies.collect(), asked: Mutex::new(vec![]) });
        let mut env = vec![
            ("PATH".to_string(), std::env::var("PATH").unwrap()),
            ("XDG_DATA_HOME".to_string(), self.tmp.join("data")),
            ("FAKE_RECORD".to_string(), self.tmp.join("record")),
        ];
        env.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        let env = env.into_iter().map(|(k, v)| (OsString::from(k), OsString::from(v))).collect();
        let installs = Installs::in_environment(adapter, env).with_sources(pins, dist.clone(), DIST, REGISTRY_URL);
        (installs, dist)
    }

    /// claude's installs' directory.
    fn root(&self) -> String {
        self.root_of(Adapter::Claude)
    }

    /// adapter's installs' directory.
    fn root_of(&self, adapter: Adapter) -> String {
        format!("{}/octoflow-ai/{}-acp", self.tmp.join("data"), adapter.harness())
    }

    /// The names in claude's installs' directory, sorted.
    fn entries(&self) -> Vec<String> {
        self.entries_of(Adapter::Claude)
    }

    /// The names in adapter's installs' directory, sorted.
    fn entries_of(&self, adapter: Adapter) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.root_of(adapter))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// What the fake npm recorded as name.
    fn record(&self, name: &str) -> String {
        std::fs::read_to_string(self.tmp.join(&format!("record/{name}"))).unwrap()
    }
}

/// Pins of Node node, its tarball's sha256 being sha256, and adapter at version version.
fn pins_of(adapter: Adapter, node: &str, sha256: &str, version: &str) -> Pins {
    let package_json = format!(r#"{{"dependencies": {{"{}": "{version}"}}}}"#, adapter.package());
    let tarball = Some((PLATFORM.to_string(), sha256.to_string()));
    Pins::new(node, tarball, adapter.package(), &package_json, "{}").unwrap()
}

/// A Node tarball of version node for PLATFORM, made in tmp: FAKE_NODE as bin/node, and an empty npm-cli.js.
fn tarball(tmp: &TempDir, node: &str) -> Vec<u8> {
    let stem = format!("node-v{node}-{PLATFORM}");
    let staging = tmp.join(&format!("staging-{node}"));
    std::fs::create_dir_all(format!("{staging}/{stem}/bin")).unwrap();
    std::fs::create_dir_all(format!("{staging}/{stem}/lib/node_modules/npm/bin")).unwrap();
    let node_path = format!("{staging}/{stem}/bin/node");
    std::fs::write(&node_path, FAKE_NODE).unwrap();
    std::fs::set_permissions(&node_path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    std::fs::write(format!("{staging}/{stem}/lib/node_modules/npm/bin/npm-cli.js"), "").unwrap();
    let out = format!("{staging}.tar.gz");
    let tarred = Command::new("tar").args(["-czf", &out, "-C", &staging, &stem]).status().unwrap();
    assert!(tarred.success(), "tar made the fake tarball");
    std::fs::read(out).unwrap()
}

fn sha256_of(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn an_install_downloads_node_installs_the_adapter_and_is_found_in_place() {
    let f = Fixture::new();
    let pins = f.pins(NODE, &sha256_of(&f.tarball), "0.5.0");
    let (installs, dist) = f.installs(pins, &[(NODE, &f.tarball)], &[]);
    assert_eq!(installs.installed(), None);

    let installed = installs.install(&Interrupt::new()).unwrap();

    let dir = format!("{}/node-{NODE}-adapter-0.5.0", f.root());
    let expected = Installed {
        node: format!("{dir}/node/bin/node"),
        script: format!("{dir}/adapter/node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js"),
    };
    assert_eq!(installed, expected);
    assert_eq!(installs.installed(), Some(expected));
    assert_eq!(*dist.asked.lock().unwrap(), [format!("{DIST}/v{NODE}/node-v{NODE}-{PLATFORM}.tar.gz")]);
    assert_eq!(f.entries(), [".lock".to_string(), format!("node-{NODE}-adapter-0.5.0")]);
    let mut kept: Vec<String> =
        std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    kept.sort();
    assert_eq!(kept, ["adapter", "node"], "the tarball, the empty npmrcs and npm's cache are gone");
    let lock = std::fs::read_to_string(format!("{dir}/adapter/package.json")).unwrap();
    assert!(lock.contains("0.5.0"), "{lock}");
}

#[test]
fn npm_installs_from_the_lockfile_without_scripts_or_the_users_npm_settings() {
    let f = Fixture::new();
    let pins = f.pins(NODE, &sha256_of(&f.tarball), "0.5.0");
    let extra = [("npm_config_registry", "http://evil.test/"), ("NPM_CONFIG_IGNORE_SCRIPTS", "false")];
    let extra = [extra.as_slice(), &[("HTTPS_PROXY", "http://proxy.test:3128")]].concat();
    let (installs, _) = f.installs(pins, &[(NODE, &f.tarball)], &extra);

    installs.install(&Interrupt::new()).unwrap();

    let argv = f.record("argv");
    let dir = format!("{}/.tmp-node-{NODE}-adapter-0.5.0", f.root());
    let expected = format!(
        "{dir}/node/lib/node_modules/npm/bin/npm-cli.js ci --ignore-scripts --userconfig {dir}/empty-user-npmrc \
         --globalconfig {dir}/empty-global-npmrc --registry {REGISTRY_URL} --prefix {dir}/adapter \
         --cache {dir}/npm-cache --no-audit --no-fund --no-update-notifier\n"
    );
    assert_eq!(argv, expected);
    assert_eq!(f.record("config-bytes").trim(), "0 0");
    let env = f.record("env");
    assert!(!env.to_ascii_lowercase().contains("npm_config_"), "{env}");
    assert!(env.contains("HTTPS_PROXY=http://proxy.test:3128"), "{env}");
}

#[test]
fn a_tarball_whose_sha256_is_not_the_pins_is_refused_and_nothing_is_installed() {
    let f = Fixture::new();
    let pinned = sha256_of(b"another tarball");
    let (installs, _) = f.installs(f.pins(NODE, &pinned, "0.5.0"), &[(NODE, &f.tarball)], &[]);

    let err = installs.install(&Interrupt::new()).unwrap_err();

    let url = format!("{DIST}/v{NODE}/node-v{NODE}-{PLATFORM}.tar.gz");
    let got = sha256_of(&f.tarball);
    assert_eq!(err.0, format!("{url} has sha256 {got}, not the pinned {pinned}; refused"));
    assert_eq!(installs.installed(), None);
    assert_eq!(f.entries(), [".lock"]);
}

#[test]
fn a_failure_halfway_leaves_no_install_and_a_retry_starts_clean() {
    let f = Fixture::new();
    let pins = f.pins(NODE, &sha256_of(&f.tarball), "0.5.0");
    let (failing, _) = f.installs(pins.clone(), &[(NODE, &f.tarball)], &[("FAKE_NPM_FAILS", "1")]);

    let err = failing.install(&Interrupt::new()).unwrap_err();

    assert!(err.0.starts_with("cannot install claude's ACP adapter: "), "{}", err.0);
    assert!(err.0.ends_with("exited with status 1: npm error boom"), "{}", err.0);
    assert_eq!(failing.installed(), None);
    assert_eq!(f.entries(), [".lock"]);

    // As a killed install would leave it.
    std::fs::create_dir_all(format!("{}/.tmp-node-{NODE}-adapter-0.5.0/node", f.root())).unwrap();
    let (installs, _) = f.installs(pins, &[(NODE, &f.tarball)], &[]);
    installs.install(&Interrupt::new()).unwrap();
    assert_eq!(f.entries(), [".lock".to_string(), format!("node-{NODE}-adapter-0.5.0")]);
}

/// An install of the current pins with a part deleted, its Node or its script, is not taken for one, and the next
/// install replaces it.
#[test]
fn a_damaged_install_is_not_found_and_an_install_replaces_it() {
    let f = Fixture::new();
    let pins = f.pins(NODE, &sha256_of(&f.tarball), "0.5.0");
    let (installs, dist) = f.installs(pins, &[(NODE, &f.tarball)], &[]);
    let installed = installs.install(&Interrupt::new()).unwrap();

    for part in [&installed.node, &installed.script] {
        std::fs::remove_file(part).unwrap();
        assert_eq!(installs.installed(), None, "without {part}");
        assert_eq!(installs.install(&Interrupt::new()).unwrap(), installed);
        assert_eq!(installs.installed(), Some(installed.clone()));
    }
    assert_eq!(dist.asked.lock().unwrap().len(), 3);
    assert_eq!(f.entries(), [".lock".to_string(), format!("node-{NODE}-adapter-0.5.0")]);
}

#[test]
fn two_installs_at_once_make_one() {
    let f = Fixture::new();
    let pins = f.pins(NODE, &sha256_of(&f.tarball), "0.5.0");
    let (installs, dist) = f.installs(pins, &[(NODE, &f.tarball)], &[("FAKE_NPM_SLOW", "1")]);

    let (a, b) = std::thread::scope(|s| {
        let a = s.spawn(|| installs.install(&Interrupt::new()));
        let b = s.spawn(|| installs.install(&Interrupt::new()));
        (a.join().unwrap().unwrap(), b.join().unwrap().unwrap())
    });

    assert_eq!(a, b);
    assert_eq!(dist.asked.lock().unwrap().len(), 1, "the second waited for the first and found its install");
    assert_eq!(f.entries(), [".lock".to_string(), format!("node-{NODE}-adapter-0.5.0")]);
}

#[test]
fn a_raised_pin_installs_beside_the_old_install_and_then_removes_it() {
    let f = Fixture::new();
    let newer = tarball(&f.tmp, "1.0.1");
    let tarballs: [(&str, &[u8]); 2] = [(NODE, &f.tarball), ("1.0.1", &newer)];
    let old = f.pins(NODE, &sha256_of(&f.tarball), "0.5.0");
    let (old_installs, _) = f.installs(old, &tarballs, &[]);
    old_installs.install(&Interrupt::new()).unwrap();

    let (new_installs, _) = f.installs(f.pins("1.0.1", &sha256_of(&newer), "0.6.0"), &tarballs, &[]);
    assert_eq!(new_installs.installed(), None, "an install of other pins is not the current one");
    let installed = new_installs.install(&Interrupt::new()).unwrap();

    assert!(installed.script.contains("/node-1.0.1-adapter-0.6.0/"), "{}", installed.script);
    assert_eq!(f.entries(), [".lock", "node-1.0.1-adapter-0.6.0"]);
    assert_eq!(old_installs.installed(), None);
}

/// Anything else in the installs' directory, a file or a link as much as a directory, goes once an install is in place.
#[test]
fn an_install_removes_files_and_links_beside_it() {
    let f = Fixture::new();
    let pins = f.pins(NODE, &sha256_of(&f.tarball), "0.5.0");
    let (installs, _) = f.installs(pins, &[(NODE, &f.tarball)], &[]);
    std::fs::create_dir_all(f.root()).unwrap();
    std::fs::write(format!("{}/stray", f.root()), "").unwrap();
    std::os::unix::fs::symlink(f.root(), format!("{}/loop", f.root())).unwrap();

    installs.install(&Interrupt::new()).unwrap();

    assert_eq!(f.entries(), [".lock".to_string(), format!("node-{NODE}-adapter-0.5.0")]);
}

#[test]
fn a_platform_without_a_pinned_node_is_refused() {
    let f = Fixture::new();
    let mut pins = f.pins(NODE, &sha256_of(&f.tarball), "0.5.0");
    pins.tarball = None;
    let (installs, dist) = f.installs(pins, &[], &[]);

    let err = installs.install(&Interrupt::new()).unwrap_err();

    assert!(err.0.starts_with("the app installs claude's ACP adapter only on Linux and macOS"), "{}", err.0);
    assert!(dist.asked.lock().unwrap().is_empty());
}

#[test]
fn the_installs_are_in_the_data_directory_of_the_environment() {
    let env = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (OsString::from(k), OsString::from(v))).collect();
    let data: Vec<_> = env(&[("XDG_DATA_HOME", "/d/")]);
    assert_eq!(Installs::in_environment(Adapter::Claude, data.clone()).root, "/d/octoflow-ai/claude-acp");
    assert_eq!(Installs::in_environment(Adapter::Codex, data).root, "/d/octoflow-ai/codex-acp");
    let home = Installs::in_environment(Adapter::Claude, env(&[("XDG_DATA_HOME", ""), ("HOME", "/h")]));
    assert_eq!(home.root, "/h/.local/share/octoflow-ai/claude-acp");
}

/// Node's pins: a Node 22 with a sha256 for each platform the app is built for, this platform's taken.
#[test]
fn node_is_pinned_for_each_platform_the_app_is_built_for() {
    let pins = Pins::built_in(Adapter::Claude);
    assert!(pins.node.starts_with("22."), "{}", pins.node);
    let platforms: Vec<&str> = NODE_SHA256.iter().map(|(p, _)| *p).collect();
    assert_eq!(platforms, ["linux-x64", "linux-arm64", "darwin-x64", "darwin-arm64"]);
    assert!(NODE_SHA256.iter().all(|(_, s)| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())));
    let built = [("linux", "x86_64"), ("linux", "aarch64"), ("macos", "x86_64"), ("macos", "aarch64")];
    let named: Vec<Option<&str>> = built.iter().map(|(os, arch)| node_platform(os, arch)).collect();
    assert_eq!(named, platforms.iter().copied().map(Some).collect::<Vec<_>>(), "each built platform has its Node");
    assert_eq!(node_platform("windows", "x86_64"), None);
    let here = node_platform(std::env::consts::OS, std::env::consts::ARCH).expect("a platform the app is built for");
    let sha256 = NODE_SHA256.iter().find(|(p, _)| *p == here).unwrap().1;
    assert_eq!(pins.tarball, Some((here.to_string(), sha256.to_string())), "this platform's tarball is pinned");
}

/// Each adapter's committed pins: its version, and a lockfile made from its package.json that pins every package with
/// its integrity, its agent's binary for each platform the app is built for among them.
#[test]
fn the_built_in_pins_pin_every_package_of_each_adapter() {
    let binaries = |adapter| match adapter {
        Adapter::Claude => {
            ("0.101.0", "@anthropic-ai/claude-agent-sdk-", ["darwin-arm64", "darwin-x64", "linux-arm64", "linux-x64"])
        }
        Adapter::Codex => ("2.5.0", "@openai/codex-", ["darwin-arm64", "darwin-x64", "linux-arm64", "linux-x64"]),
    };
    for adapter in Adapter::ALL {
        let (version, binary, platforms) = binaries(adapter);
        let pins = Pins::built_in(adapter);
        assert_eq!(pins.adapter, version, "{adapter:?}");
        let lock: serde_json::Value = serde_json::from_str(&pins.lockfile).unwrap();
        let packages = lock["packages"].as_object().unwrap();
        let root = &packages[""]["dependencies"];
        assert_eq!(root[adapter.package()], version, "{adapter:?}'s lockfile was made from its package.json");
        for (path, package) in packages.iter().filter(|(path, _)| !path.is_empty()) {
            let integrity = package["integrity"].as_str().unwrap_or_default();
            assert!(integrity.starts_with("sha512-"), "{path} has no sha512 integrity");
            let resolved = package["resolved"].as_str().unwrap_or_default();
            assert!(resolved.starts_with(REGISTRY), "{path} comes from {resolved}");
            assert_ne!(package["hasInstallScript"], true, "{path} has an install script, which --ignore-scripts skips");
        }
        for platform in platforms {
            let path = format!("node_modules/{binary}{platform}");
            assert!(packages.keys().any(|k| k.ends_with(&path)), "{path} is in {adapter:?}'s lockfile");
        }
    }
}

#[test]
fn codex_installs_beside_claude_and_neither_removes_the_other() {
    let f = Fixture::new();
    let sha256 = sha256_of(&f.tarball);
    let (claude, _) = f.installs(f.pins(NODE, &sha256, "0.5.0"), &[(NODE, &f.tarball)], &[]);
    let claude_installed = claude.install(&Interrupt::new()).unwrap();
    let codex_pins = pins_of(Adapter::Codex, NODE, &sha256, "2.0.0");
    let (codex, _) = f.installs_of(Adapter::Codex, codex_pins, &[(NODE, &f.tarball)], &[]);

    let installed = codex.install(&Interrupt::new()).unwrap();

    let dir = format!("{}/node-{NODE}-adapter-2.0.0", f.root_of(Adapter::Codex));
    let script = format!("{dir}/adapter/node_modules/@agentclientprotocol/codex-acp/dist/index.js");
    assert_eq!(installed, Installed { node: format!("{dir}/node/bin/node"), script });
    assert_eq!(f.entries_of(Adapter::Codex), [".lock", "node-1.0.0-adapter-2.0.0"]);
    assert_eq!(claude.installed(), Some(claude_installed));
    assert_eq!(f.entries(), [".lock", "node-1.0.0-adapter-0.5.0"]);
}

/// codex's installs in an environment of vars, for the built-in pins.
fn codex_in(vars: &[(&str, &str)]) -> Installs {
    let env = vars.iter().map(|(k, v)| (OsString::from(k), OsString::from(v))).collect();
    Installs::in_environment(Adapter::Codex, env)
}

#[test]
fn codex_is_logged_in_by_its_auth_file_alone() {
    let tmp = TempDir::new();
    let home = tmp.join("home");
    std::fs::create_dir_all(format!("{home}/.codex")).unwrap();
    std::fs::create_dir_all(tmp.join("codex-home")).unwrap();
    let in_codex_home = [("HOME", home.as_str()), ("CODEX_HOME", &tmp.join("codex-home"))];
    assert!(codex_in(&in_codex_home).logged_in().is_err(), "~/.codex is not looked in while CODEX_HOME is set");
    std::fs::write(tmp.join("codex-home/auth.json"), "{}").unwrap();
    assert_eq!(codex_in(&in_codex_home).logged_in(), Ok(()));

    assert!(codex_in(&[("HOME", &home)]).logged_in().is_err());
    std::fs::write(format!("{home}/.codex/auth.json"), "{}").unwrap();
    assert_eq!(codex_in(&[("HOME", &home)]).logged_in(), Ok(()));

    // The adapter reads an API key only for an ACP `authenticate`, which the engine does not send.
    for key in ["OPENAI_API_KEY", "CODEX_API_KEY"] {
        assert!(codex_in(&[("HOME", "/nowhere"), (key, "sk-test")]).logged_in().is_err(), "{key}");
    }
    let claude = Installs::in_environment(Adapter::Claude, vec![(OsString::from("HOME"), OsString::from("/nowhere"))]);
    assert_eq!(claude.logged_in(), Ok(()), "claude's adapter reports no login before a turn");
}

#[test]
fn codex_not_logged_in_is_told_to_log_in_with_the_installed_codex() {
    let tmp = TempDir::new();
    let data = tmp.path();
    let vars = [("HOME", "/nowhere"), ("XDG_DATA_HOME", data.as_str())];
    let expected = NotLoggedIn {
        detail: "codex is not logged in: no /nowhere/.codex/auth.json".to_string(),
        fix: "log in with `codex login`".to_string(),
    };
    assert_eq!(codex_in(&vars).logged_in(), Err(expected));

    let installed = codex_in(&vars).install_fake("");
    let modules = installed.script.trim_end_matches("@agentclientprotocol/codex-acp/dist/index.js");
    let fix = format!("log in with `{} {modules}@openai/codex/bin/codex.js login`", installed.node);
    assert_eq!(codex_in(&vars).logged_in().unwrap_err().fix, fix);
}

#[test]
fn without_home_codex_s_home_is_the_user_s_in_passwd() {
    let detail = codex_in(&[]).logged_in().map(|()| String::new()).unwrap_or_else(|e| e.detail);

    assert!(!detail.contains('~'), "{detail}");
}
