//! The desktop shell: a window over the orchestrator's `serve`, and the orchestrator itself, whose engine it links
//! (`octoflow_engine`). Started with arguments, the app is the orchestrator's CLI: see `cli`. Started with none, it
//! opens its window and starts itself, as `serve`, as its sidecar, which it kills when it exits. The webview talks to
//! the sidecar's server over HTTP; the shell only hands it the server's address and token, through the `server`
//! command.

use std::ffi::{OsStr, OsString};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{Manager, RunEvent};

/// The top of the checkout this app was built in, two directories above its crate's.
fn checkout() -> PathBuf {
    canonical(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
}

/// The environment and working directory the shell that started the app had, as far as they can be recovered: what
/// the app's process takes as its own when it is the CLI (`UserShell::enter`), the sidecar included, so that the
/// engine, and every process it starts, sees the user's libraries, Python and project rather than the bundle's. The
/// process that shows the window keeps its environment, which GTK and WebKit need.
///
/// Only an AppImage changes them. Its start-up (linuxdeploy's `AppRun`, its GTK hook and AppImageKit's
/// `AppRun.wrapped`, as measured from v0.25.0's) points path lists and Python and GTK settings into the bundle,
/// sets a few variables of its own, and starts the app in `$APPDIR/usr`. It names the bundle both as `$APPDIR` and
/// as what that resolves to, which `AppRun.wrapped` writes, so the two differ when `TMPDIR` is a link. The same
/// start-up runs when a user starts an AppImage they extracted (`squashfs-root/AppRun`), with no `APPIMAGE` set.
/// The user's directory survives in `OWD`,
/// which the runtime sets when it mounts the AppImage but not when it extracts it (`APPIMAGE_EXTRACT_AND_RUN`, as
/// on a machine without FUSE), and in `PWD`, which `AppRun`, a bash script, checks against its directory at start,
/// before `AppRun.wrapped` changes it. Elsewhere, macOS and the .deb included, the environment and directory are
/// the app's own.
#[derive(Debug, PartialEq)]
struct UserShell {
    /// The whole environment, in the order it came.
    vars: Vec<(OsString, OsString)>,
    /// The working directory to start in; None to keep the app's own.
    dir: Option<PathBuf>,
}

/// Path lists the AppImage's start-up prefixes with entries in the bundle, followed by the user's value, or by an
/// empty entry when the user had none. The GTK hook first sets GSETTINGS_SCHEMA_DIR, GI_TYPELIB_PATH,
/// GIO_MODULE_DIR and GTK_PATH to the bundle's alone, so the user's value of those four is already lost and they
/// end up unset.
const APPIMAGE_PATH_LISTS: &[&str] = &[
    "LD_LIBRARY_PATH",
    "PATH",
    "PYTHONPATH",
    "PERLLIB",
    "QT_PLUGIN_PATH",
    "XDG_DATA_DIRS",
    "GSETTINGS_SCHEMA_DIR",
    "GI_TYPELIB_PATH",
    "GIO_MODULE_DIR",
    "GTK_PATH",
    "GST_PLUGIN_SYSTEM_PATH",
    "GST_PLUGIN_SYSTEM_PATH_1_0",
];

/// Entries outside the bundle that the start-up puts in a path list's prefix, after the bundle's own: the GTK hook's
/// `/usr/share` in XDG_DATA_DIRS. Only the first such entry is the start-up's; another is the user's.
const APPIMAGE_PREFIX_ENTRIES: &[(&str, &str)] = &[("XDG_DATA_DIRS", "/usr/share")];

/// Variables the AppImage's start-up sets to a path in the bundle, overwriting the user's; a value outside the
/// bundle is the user's own.
const APPIMAGE_PATHS: &[&str] =
    &["PYTHONHOME", "GTK_DATA_PREFIX", "GTK_EXE_PREFIX", "GTK_IM_MODULE_FILE", "GDK_PIXBUF_MODULE_FILE"];

/// Variables the AppImage's start-up sets whatever the user had, to values that do not name the bundle, so the
/// user's own value, if any, is already lost: unset whatever they hold. GTK_THEME is the GTK hook's
/// `Adwaita:light` or `Adwaita:dark`, set over any GTK_THEME of the user's (who sets APPIMAGE_GTK_THEME to choose
/// it); PYTHONDONTWRITEBYTECODE is AppImageKit's `1`.
const APPIMAGE_ONLY: &[&str] = &["GTK_THEME", "PYTHONDONTWRITEBYTECODE", "APPDIR", "APPIMAGE", "ARGV0", "OWD"];

impl UserShell {
    /// The shell that started this process.
    fn current() -> UserShell {
        UserShell::from_vars(std::env::vars_os(), std::env::current_exe().ok().as_deref())
    }

    /// The user's shell, recovered from the app's environment vars, given the app's executable exe, None when it is
    /// unknown. Under an AppImage (`APPDIR` an absolute path below `/`, with `APPIMAGE` set, or with exe in the
    /// bundle and the bundle's `AppRun` there, as when the user runs an extracted one): each path list in
    /// APPIMAGE_PATH_LISTS keeps only the user's value (`users_path_list`) and is unset when that is empty; each of
    /// APPIMAGE_PATHS is unset when it is in the bundle; each of APPIMAGE_ONLY is unset; and the directory is `OWD`,
    /// else `PWD`, else the app's own. The bundle is `$APPDIR` and what it resolves to, and everything under
    /// either. Otherwise vars and the app's directory as they are.
    fn from_vars(vars: impl IntoIterator<Item = (OsString, OsString)>, exe: Option<&Path>) -> UserShell {
        let vars: Vec<(OsString, OsString)> = vars.into_iter().collect();
        let get = |name: &str| vars.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
        // An empty APPDIR, or `/`, would put every path in the bundle.
        let Some(appdir) = get("APPDIR").map(PathBuf::from).filter(|dir| dir.is_absolute() && dir.parent().is_some())
        else {
            return UserShell { vars, dir: None };
        };
        let bundle = [canonical(appdir.clone()), appdir];
        let in_bundle = |path: &Path| bundle.iter().any(|root| path.starts_with(root));
        let extracted = exe.is_some_and(in_bundle) && appdir_marker(&bundle[0]);
        if get("APPIMAGE").is_none() && !extracted {
            return UserShell { vars, dir: None };
        }
        let dir = get("OWD").or_else(|| get("PWD")).map(PathBuf::from);
        let vars = vars
            .into_iter()
            .filter_map(|(name, value)| {
                let Some(name_str) = name.to_str() else { return Some((name, value)) };
                if APPIMAGE_ONLY.contains(&name_str) {
                    return None;
                }
                if APPIMAGE_PATHS.contains(&name_str) {
                    return (!in_bundle(Path::new(&value))).then_some((name, value));
                }
                if APPIMAGE_PATH_LISTS.contains(&name_str) {
                    let users = users_path_list(name_str, &value, in_bundle);
                    return (!users.is_empty()).then_some((name, users));
                }
                Some((name, value))
            })
            .collect();
        UserShell { vars, dir }
    }

    /// Make this process's environment and working directory the shell's: each variable the shell lacks removed,
    /// each it holds set where the process's differs, and the directory entered when the shell names one. Call it
    /// before the process starts another thread, which might read the environment meanwhile. Errors, saying which
    /// directory and why: the shell's directory is not one, with nothing changed; or it cannot be entered, with the
    /// environment already the shell's.
    fn enter(&self) -> Result<(), String> {
        if let Some(gone) = self.dir.as_ref().filter(|dir| !dir.is_dir()) {
            return Err(format!("cannot start in {}: it is not a directory", gone.display()));
        }
        let own: Vec<(OsString, OsString)> = std::env::vars_os().collect();
        let value_in = |vars: &[(OsString, OsString)], name: &OsStr| {
            vars.iter().find(|(k, _)| k == name).map(|(_, value)| value.clone())
        };
        for (name, _) in own.iter().filter(|(name, _)| value_in(&self.vars, name).is_none()) {
            std::env::remove_var(name);
        }
        for (name, value) in self.vars.iter().filter(|(name, value)| value_in(&own, name).as_ref() != Some(value)) {
            std::env::set_var(name, value);
        }
        match &self.dir {
            Some(dir) => {
                std::env::set_current_dir(dir).map_err(|e| format!("cannot start in {}: {e}", dir.display()))
            }
            None => Ok(()),
        }
    }
}

/// Whether appdir holds the `AppRun` every AppImage's bundle starts with.
fn appdir_marker(appdir: &Path) -> bool {
    appdir.join("AppRun").is_file()
}

/// The user's own value of the path list name, from value as the AppImage's start-up left it: without the prefix
/// the start-up added (its leading entries in_bundle, and the entry APPIMAGE_PREFIX_ENTRIES names for name after
/// them) and without any other entry in_bundle. Empty when nothing of the user's is left: when they had none, which
/// the start-up marks with a lone empty entry after its prefix, or when the start-up overwrote it
/// (APPIMAGE_PATH_LISTS). The user's own empty entries are kept.
fn users_path_list(name: &str, value: &OsStr, in_bundle: impl Fn(&Path) -> bool) -> OsString {
    let mut start_up_entry =
        APPIMAGE_PREFIX_ENTRIES.iter().find(|(list, _)| *list == name).map(|(_, entry)| Path::new(entry));
    let mut entries = std::env::split_paths(value).peekable();
    let mut prefixed = false;
    while let Some(entry) = entries.peek() {
        if in_bundle(entry) {
            prefixed = true;
        } else if prefixed && start_up_entry == Some(entry.as_path()) {
            start_up_entry = None;
        } else {
            break;
        }
        entries.next();
    }
    let kept: Vec<PathBuf> = entries.filter(|entry| !in_bundle(entry)).collect();
    // Rejoining entries split at the separator cannot meet one that holds it.
    std::env::join_paths(kept).expect("entries without a separator")
}

/// The orchestrator's CLI, ready to take over the app's process.
pub struct Cli {
    /// The whole command line, the app's own name first.
    argv: Vec<OsString>,
}

/// What argv, the app's command line with its own name first, asks for: with any argument, the CLI, given exactly
/// that command line; with none, or on macOS only the one a Finder launch may pass (`is_finder_launch`), None, for
/// the window.
pub fn cli(argv: impl IntoIterator<Item = OsString>) -> Option<Cli> {
    let argv: Vec<OsString> = argv.into_iter().collect();
    let args = argv.get(1..).unwrap_or_default();
    if args.is_empty() || cfg!(target_os = "macos") && is_finder_launch(args) {
        return None;
    }
    Some(Cli { argv })
}

/// Whether args are only the process serial number, `-psn_<n>`, that macOS may pass an app opened from the Finder,
/// as on the first, quarantined launch.
fn is_finder_launch(args: &[OsString]) -> bool {
    matches!(args, [only] if only.to_str().is_some_and(|arg| arg.starts_with("-psn_")))
}

impl Cli {
    /// Run the engine's CLI (`octoflow_engine::cli::main`) as this process, in the user's shell's environment and
    /// directory (`UserShell`), and end the process as the orchestrator's own program ends: with the CLI's exit
    /// status, or by the SIGTERM or SIGHUP that interrupted it. Under an AppImage run without FUSE the process the
    /// caller started is the AppImage's runtime, which stays this one's parent and passes on no signal sent to it:
    /// the CLI gets SIGTERM when the runtime dies, whatever signal stopped it (`stop_with_appimage_runtime`).
    /// Returns never: when the user's directory is gone, prints why on standard error and exits 1. Call it on the
    /// main thread before anything else starts a thread or opens a window: it changes the process's environment and
    /// starts its signal handling.
    #[cfg(unix)]
    pub fn run(self) -> ! {
        // A CLI that outlives the runtime is still better than none: say so and run it. Before the user's shell is
        // entered, which has no APPIMAGE.
        #[cfg(target_os = "linux")]
        if let Err(e) = stop_with_appimage_runtime(std::env::var_os("APPIMAGE").map(PathBuf::from).as_deref()) {
            eprintln!("octoflow: the CLI will not stop when the AppImage that started it does: {e}");
        }
        if let Err(why) = UserShell::current().enter() {
            eprintln!("octoflow: {why}");
            std::process::exit(1)
        }
        octoflow_engine::interrupt::exit(octoflow_engine::cli::main(self.argv))
    }
}

/// When this process's parent is the AppImage's runtime that started it, appimage being the file `$APPIMAGE` names
/// (None when it is unset), ask the kernel to send this process SIGTERM when that parent dies; when the parent died
/// before the request, end this process now by SIGTERM, as the request would have. Elsewhere, nothing. The runtime
/// stays the parent under `APPIMAGE_EXTRACT_AND_RUN` (a machine without FUSE): it forks, runs the app in the child,
/// waits, and passes on no signal sent to it. A mounted AppImage's runtime execs the app instead, so the app's parent
/// is the caller's own, whose death must not stop the CLI. Errors when the kernel refuses the request or the signal.
#[cfg(target_os = "linux")]
fn stop_with_appimage_runtime(appimage: Option<&Path>) -> nix::Result<()> {
    use nix::sys::signal::{raise, Signal};
    use nix::unistd::getppid;
    let parent = getppid();
    // A parent whose executable cannot be read is taken for no runtime: the CLI then runs as it always has.
    let parent_exe = std::fs::read_link(format!("/proc/{parent}/exe")).ok();
    if !is_appimage(appimage, parent_exe.as_deref()) {
        return Ok(());
    }
    nix::sys::prctl::set_pdeathsig(Signal::SIGTERM)?;
    // prctl(2): a parent that died before the request sends nothing, and this process has another parent since.
    if getppid() != parent {
        raise(Signal::SIGTERM)?;
    }
    Ok(())
}

/// Whether exe, a process's executable as `/proc/<pid>/exe` names it, is the file appimage, `$APPIMAGE`, names: the
/// AppImage, which is its own runtime. False when either is unknown.
#[cfg(any(test, target_os = "linux"))]
fn is_appimage(appimage: Option<&Path>, exe: Option<&Path>) -> bool {
    match (appimage, exe) {
        (Some(appimage), Some(exe)) => canonical(appimage.to_path_buf()) == canonical(exe.to_path_buf()),
        _ => false,
    }
}

/// path made absolute with its links resolved, or path itself when it does not exist.
fn canonical(path: PathBuf) -> PathBuf {
    path.canonicalize().unwrap_or(path)
}

/// The line `serve` prints first: where its server listens, and the token every request needs.
#[derive(Deserialize)]
struct Served {
    url: String,
    token: String,
}

/// What the webview needs to reach the sidecar, as the `server` command gives it.
#[derive(Clone, Serialize)]
struct Connection {
    url: String,
    token: String,
    /// The project the app opens first: the checkout a development build was built in (`Sidecar::of_app`); None for
    /// any other build, which belongs to no checkout.
    project: Option<String>,
}

/// The sidecar: its process while it runs, and how to reach it, or why it could not be started.
struct Sidecar {
    /// None once it has been stopped, or when it never started.
    child: Mutex<Option<Child>>,
    connection: Result<Connection, String>,
}

impl Sidecar {
    /// The app itself as its sidecar: `Sidecar::start` with this process's executable, which is the CLI when given
    /// arguments (`cli`), and with the checkout the app was built in as the project of a development build
    /// (`tauri dev`), none otherwise. When the executable cannot be told, that is the connection's reason.
    fn of_app(origin: &str) -> Sidecar {
        let project = tauri::is_dev().then(|| checkout().display().to_string());
        match std::env::current_exe() {
            Ok(app) => Sidecar::start(&app, origin, project),
            Err(e) => {
                let reason = format!("cannot tell the app's own program: {e}");
                Sidecar { child: Mutex::new(None), connection: Err(reason) }
            }
        }
    }

    /// Start program's `serve`, answering requests from origin, and read the server's first line; project is the
    /// one the window opens first. The sidecar inherits the app's environment and directory, and takes the user's
    /// shell itself, as the CLI does (`Cli::run`). A sidecar that cannot be started, or prints no such line, keeps
    /// the reason as its connection.
    fn start(program: &Path, origin: &str, project: Option<String>) -> Sidecar {
        match spawn(program, origin) {
            Ok((child, served)) => {
                let connection = Connection { url: served.url, token: served.token, project };
                Sidecar { child: Mutex::new(Some(child)), connection: Ok(connection) }
            }
            Err(reason) => Sidecar { child: Mutex::new(None), connection: Err(reason) },
        }
    }

    /// Kill the sidecar and wait for it; nothing when it is not running.
    fn stop(&self) {
        let mut child = self.child.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(mut child) = child.take() {
            // It may have exited already, which is what killing it is for.
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// The started sidecar and its first line; Err with why, the sidecar killed, when it cannot be started or that line
/// is not `serve`'s: what the sidecar wrote to its standard error, as `serve` and the CLI say why they refuse
/// (`Cli::run`), else what it printed instead. A started sidecar's standard error goes on to the app's.
fn spawn(program: &Path, origin: &str) -> Result<(Child, Served), String> {
    let mut child = Command::new(program)
        .args(["serve", "--origin", origin])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run {}: {e}", program.display()))?;
    let mut line = String::new();
    let read = match child.stdout.take() {
        Some(stdout) => BufReader::new(stdout).read_line(&mut line).map_err(|e| e.to_string()),
        None => Err("its standard output is not a pipe".to_string()),
    };
    let served = read.and_then(|_| {
        serde_json::from_str::<Served>(&line)
            .map_err(|e| format!("{} serve printed {:?}, not its address: {e}", program.display(), line.trim()))
    });
    let stderr = child.stderr.take();
    match served {
        Ok(served) => {
            // The forwarder ends when the sidecar does; what it fails to copy is lost to no one but the log.
            if let Some(mut stderr) = stderr {
                std::thread::spawn(move || std::io::copy(&mut stderr, &mut std::io::stderr()));
            }
            Ok((child, served))
        }
        Err(printed) => {
            let _ = child.kill();
            let _ = child.wait();
            let mut said = String::new();
            if let Some(mut stderr) = stderr {
                // Read after the kill, so that it ends: a sidecar that started no process of its own holds the pipe
                // alone. A failed read leaves what was read.
                let _ = std::io::Read::read_to_string(&mut stderr, &mut said);
            }
            let said = said.trim();
            Err(if said.is_empty() { printed } else { said.to_string() })
        }
    }
}

/// The origin of the webview's pages, which the sidecar must answer: Vite's dev server under `tauri dev`, else the
/// custom protocol the bundled frontend is served by.
fn webview_origin(app: &tauri::App) -> String {
    if tauri::is_dev() {
        if let Some(url) = &app.config().build.dev_url {
            return url.origin().ascii_serialization();
        }
    }
    if cfg!(windows) { "http://tauri.localhost" } else { "tauri://localhost" }.to_string()
}

/// The sidecar's address and token, or why it could not be started.
#[tauri::command]
fn server(sidecar: tauri::State<'_, Sidecar>) -> Result<Connection, String> {
    sidecar.connection.clone()
}

/// Run the app until its window closes, then stop the sidecar.
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            app.manage(Sidecar::of_app(&webview_origin(app)));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![server])
        .build(tauri::generate_context!())
        .expect("error while building the app")
        .run(|app, event| {
            if let RunEvent::Exit = event {
                app.state::<Sidecar>().stop();
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether a process with this pid still exists, zombies included, by its /proc entry.
    fn exists(pid: u32) -> bool {
        Path::new(&format!("/proc/{pid}")).exists()
    }

    /// A new directory of its own under the system's temporary directory, removed with everything in it when
    /// dropped, so it must outlive every use of its path.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static COUNT: AtomicU64 = AtomicU64::new(0);
            let n = COUNT.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("octoflow-ui-test-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("a scratch directory");
            TempDir(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        /// The path of name in the directory.
        fn join(&self, name: impl AsRef<Path>) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            if let Err(e) = std::fs::remove_dir_all(&self.0) {
                eprintln!("could not remove {}: {e}", self.0.display());
            }
        }
    }

    /// The orchestrator binary of this checkout, built as `cargo build -p octoflow-engine` builds it, with the cargo
    /// that runs these tests; its path is the one cargo reports, wherever CARGO_TARGET_DIR puts it.
    fn built_orchestrator() -> PathBuf {
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let built = Command::new(cargo)
            .args(["build", "--locked", "--quiet", "-p", "octoflow-engine", "--bin", "orchestrator"])
            .args(["--message-format", "json"])
            .current_dir(checkout())
            .output()
            .expect("cargo runs");
        assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
        let report = String::from_utf8(built.stdout).expect("UTF-8");
        let executable = report
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .find_map(|message| message["executable"].as_str().map(PathBuf::from));
        executable.expect("cargo reports the orchestrator it built")
    }

    /// Write an executable script with the given text at path, by a child process; for why a child process writes it:
    /// `tool` in engine/src/host/tests.rs.
    #[cfg(unix)]
    fn put_executable(path: &Path, text: &str) {
        let written = Command::new("sh")
            .args(["-c", "printf %s \"$2\" > \"$1\" && chmod 755 \"$1\"", "sh"])
            .arg(path)
            .arg(text)
            .status()
            .unwrap_or_else(|e| panic!("writing the script {}: {e}", path.display()));
        assert!(written.success(), "writing the script {}: {written}", path.display());
    }

    #[test]
    #[cfg(unix)]
    fn serves_a_program_that_prints_serve_s_first_line() {
        let dir = TempDir::new();
        let program = dir.join("octoflow");
        // serve's first line, then a process for stop to kill.
        let script = "#!/bin/sh\necho '{\"url\": \"http://127.0.0.1:9\", \"token\": \"t\"}'\nexec sleep 60\n";
        put_executable(&program, script);
        let sidecar = Sidecar::start(&program, "tauri://localhost", None);
        let connection = sidecar.connection.clone().expect("the sidecar started");
        assert_eq!(connection.url, "http://127.0.0.1:9");
        assert_eq!(connection.project, None);
        sidecar.stop();
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn serves_the_engine_s_serve_and_stops() {
        let sidecar = Sidecar::start(&built_orchestrator(), "tauri://localhost", Some("/home/u/proj".into()));
        let connection = sidecar.connection.clone().expect("the sidecar started");
        assert!(connection.url.starts_with("http://127.0.0.1:"), "{}", connection.url);
        assert!(!connection.token.is_empty());
        assert_eq!(connection.project.as_deref(), Some("/home/u/proj"));
        let pid = sidecar.child.lock().unwrap().as_ref().expect("a child").id();
        assert!(exists(pid));
        sidecar.stop();
        assert!(!exists(pid), "serve {pid} is still there");
        sidecar.stop();
    }

    #[test]
    #[cfg(unix)]
    fn a_sidecar_that_refuses_gives_its_reason() {
        let dir = TempDir::new();
        let program = dir.join("octoflow");
        let script = "#!/bin/sh\necho 'octoflow: cannot start in /gone: it is not a directory' >&2\nexit 1\n";
        put_executable(&program, script);
        let reason = Sidecar::start(&program, "tauri://localhost", None).connection.err().expect("an error");
        assert_eq!(reason, "octoflow: cannot start in /gone: it is not a directory");
    }

    #[test]
    #[cfg(unix)]
    fn a_sidecar_that_prints_something_else_is_named_with_what_it_printed() {
        let dir = TempDir::new();
        let program = dir.join("octoflow");
        put_executable(&program, "#!/bin/sh\necho hello\n");
        let reason = Sidecar::start(&program, "tauri://localhost", None).connection.err().expect("an error");
        assert!(reason.contains("serve printed \"hello\", not its address"), "{reason}");
    }

    #[test]
    fn a_program_that_is_not_there_is_an_error() {
        let sidecar = Sidecar::start(Path::new("/nonexistent/octoflow"), "tauri://localhost", None);
        let reason = sidecar.connection.err().expect("an error");
        assert!(reason.contains("/nonexistent/octoflow"), "{reason}");
        assert!(sidecar.child.lock().unwrap().is_none());
    }

    /// The user's shell from env, the app's executable unknown.
    fn shell_of(env: Vec<(OsString, OsString)>) -> UserShell {
        UserShell::from_vars(env, None)
    }

    /// An environment of name=value pairs, in order.
    fn vars(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs.iter().map(|(k, v)| (OsString::from(k), OsString::from(v))).collect()
    }

    /// An AppImage's start-up: APPDIR /tmp/.mount_x, the AppImage, and the user's directory /home/u/proj, before
    /// rest.
    fn appimage(rest: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        let mut env =
            vars(&[("APPDIR", "/tmp/.mount_x"), ("APPIMAGE", "/home/u/app.AppImage"), ("OWD", "/home/u/proj")]);
        env.extend(vars(rest));
        env
    }

    #[test]
    fn the_arguments_ask_for_the_cli_and_none_for_the_window() {
        assert!(cli(Vec::<OsString>::new()).is_none());
        assert!(cli(["octoflow".into()]).is_none());
        let Cli { argv } = cli(["octoflow".into(), "run".into(), "add a limiter".into()]).expect("the CLI");
        assert_eq!(argv, ["octoflow", "run", "add a limiter"]);
    }

    #[test]
    #[cfg(unix)]
    fn the_cli_gets_the_arguments_as_they_came() {
        use std::os::unix::ffi::OsStringExt;
        let odd = OsString::from_vec(b"not \xff utf-8".to_vec());
        let Cli { argv } = cli(["octoflow".into(), odd.clone(), "--help".into()]).expect("the CLI");
        assert_eq!(argv, [OsString::from("octoflow"), odd, OsString::from("--help")]);
    }

    #[test]
    fn outside_an_appimage_nothing_changes() {
        // APPDIR alone, as a user might export it, is not an AppImage.
        let env =
            vars(&[("APPDIR", "/opt/x"), ("LD_LIBRARY_PATH", "/opt/x/lib"), ("PYTHONHOME", "/opt/x"), ("OWD", "/")]);
        assert_eq!(shell_of(env.clone()), UserShell { vars: env.clone(), dir: None });
        let elsewhere = UserShell::from_vars(env.clone(), Some(Path::new("/usr/bin/octoflow-ui")));
        assert_eq!(elsewhere, UserShell { vars: env, dir: None });
    }

    #[test]
    fn an_extracted_appimage_started_by_its_apprun_is_one_too() {
        // squashfs-root/AppRun sets APPDIR and the bundle's variables, but no APPIMAGE.
        let dir = TempDir::new();
        let root = dir.path().canonicalize().expect("a scratch directory");
        std::fs::write(root.join("AppRun"), "").expect("a stand-in AppRun");
        let python_home = format!("{}/usr/", root.display());
        let env = vars(&[("APPDIR", root.to_str().unwrap()), ("PYTHONHOME", &python_home), ("PWD", "/u/p")]);
        let exe = root.join("usr/bin/octoflow-ui");
        let shell = UserShell::from_vars(env.clone(), Some(&exe));
        assert_eq!(shell, UserShell { vars: vars(&[("PWD", "/u/p")]), dir: Some("/u/p".into()) });
        // Without AppRun, an APPDIR that holds the executable is a user's, not a bundle.
        std::fs::remove_file(root.join("AppRun")).expect("AppRun removed");
        assert_eq!(UserShell::from_vars(env.clone(), Some(&exe)), UserShell { vars: env, dir: None });
    }

    #[test]
    fn an_empty_relative_or_root_appdir_is_no_appimage() {
        for appdir in ["", "/", "squashfs-root"] {
            let env = vars(&[("APPDIR", appdir), ("APPIMAGE", "/a.AppImage"), ("PATH", "/usr/bin:/bin")]);
            let shell = UserShell::from_vars(env.clone(), Some(Path::new("/usr/bin/octoflow-ui")));
            assert_eq!(shell, UserShell { vars: env, dir: None }, "APPDIR={appdir:?}");
        }
    }

    #[test]
    #[cfg(unix)]
    fn the_bundle_is_appdir_and_what_it_resolves_to() {
        let dir = TempDir::new();
        std::fs::create_dir_all(dir.join("real/app/usr/lib")).expect("an AppDir");
        std::fs::write(dir.join("real/app/AppRun"), "").expect("a stand-in AppRun");
        std::os::unix::fs::symlink(dir.join("real"), dir.join("link")).expect("a link");
        let (real, link) = (dir.join("real/app"), dir.join("link/app"));
        let real = real.canonicalize().expect("the AppDir resolves");
        // As AppRun.wrapped writes them: from /proc/self/exe, resolved; the GTK hook's from APPDIR, as it is.
        let path_list = format!("{}/usr/lib/:{}/usr/lib:", real.display(), link.display());
        let python_home = format!("{}/usr/", real.display());
        let env = vars(&[
            ("APPDIR", link.to_str().unwrap()),
            ("APPIMAGE", "/a.AppImage"),
            ("LD_LIBRARY_PATH", &path_list),
            ("PYTHONHOME", &python_home),
            ("HOME", "/home/u"),
        ]);
        assert_eq!(shell_of(env).vars, vars(&[("HOME", "/home/u")]));
        let started = UserShell::from_vars(vars(&[("APPDIR", link.to_str().unwrap())]), Some(&real.join("usr/bin/x")));
        assert_eq!(started, UserShell { vars: vec![], dir: None });
    }

    #[test]
    fn under_an_appimage_the_directory_is_the_users() {
        let shell = shell_of(appimage(&[("PWD", "/home/u")]));
        assert_eq!(shell, UserShell { vars: vars(&[("PWD", "/home/u")]), dir: Some("/home/u/proj".into()) });
    }

    #[test]
    fn under_an_extracted_appimage_the_directory_is_pwd() {
        let env =
            vars(&[("APPDIR", "/tmp/appimage_extracted_x"), ("APPIMAGE", "/a.AppImage"), ("PWD", "/home/u/proj")]);
        let shell = shell_of(env);
        assert_eq!(shell, UserShell { vars: vars(&[("PWD", "/home/u/proj")]), dir: Some("/home/u/proj".into()) });
    }

    #[test]
    fn under_an_appimage_without_owd_or_pwd_the_directory_stays() {
        let env = vars(&[("APPDIR", "/tmp/.mount_x"), ("APPIMAGE", "/a.AppImage"), ("HOME", "/home/u")]);
        assert_eq!(shell_of(env), UserShell { vars: vars(&[("HOME", "/home/u")]), dir: None });
    }

    #[test]
    fn each_path_list_loses_the_bundles_entries_and_keeps_the_users_in_order() {
        for name in APPIMAGE_PATH_LISTS {
            let mixed = "/tmp/.mount_x/usr/lib:/usr/local/lib:/tmp/.mount_x:/usr/lib::/tmp/.mount_xy/lib";
            let shell = shell_of(appimage(&[(name, mixed), ("HOME", "/home/u")]));
            // The empty entry is the user's: the current directory, in a PATH.
            let expected = vars(&[(name, "/usr/local/lib:/usr/lib::/tmp/.mount_xy/lib"), ("HOME", "/home/u")]);
            assert_eq!(shell.vars, expected, "{name}");
        }
    }

    #[test]
    fn a_path_list_of_only_the_bundles_entries_is_unset() {
        for name in APPIMAGE_PATH_LISTS {
            // AppRun's prefixing of an unset variable leaves an empty entry at the end.
            let shell = shell_of(appimage(&[(name, "/tmp/.mount_x/usr/lib:/tmp/.mount_x/usr/lib64:")]));
            assert_eq!(shell.vars, vec![], "{name}");
        }
    }

    #[test]
    fn a_path_list_keeps_the_users_value_as_it_was() {
        for name in APPIMAGE_PATH_LISTS {
            let shell = shell_of(appimage(&[(name, "/tmp/.mount_x/usr/lib/:/usr/lib::/lib:")]));
            assert_eq!(shell.vars, vars(&[(name, "/usr/lib::/lib:")]), "{name}");
        }
    }

    #[test]
    fn the_gtk_hooks_system_data_dir_is_the_start_ups() {
        let hooked = "/tmp/.mount_x/usr/share/:/tmp/.mount_x/usr/share:/usr/share:";
        assert_eq!(shell_of(appimage(&[("XDG_DATA_DIRS", hooked)])).vars, vec![]);
        let users = "/tmp/.mount_x/usr/share/:/tmp/.mount_x/usr/share:/usr/share:/usr/share:/opt/share";
        assert_eq!(
            shell_of(appimage(&[("XDG_DATA_DIRS", users)])).vars,
            vars(&[("XDG_DATA_DIRS", "/usr/share:/opt/share")])
        );
        // Only XDG_DATA_DIRS gets the hook's entry.
        let other = shell_of(appimage(&[("LD_LIBRARY_PATH", "/tmp/.mount_x/usr/lib:/usr/share")]));
        assert_eq!(other.vars, vars(&[("LD_LIBRARY_PATH", "/usr/share")]));
    }

    #[test]
    fn a_lone_process_serial_number_is_a_finder_launch() {
        assert!(is_finder_launch(&["-psn_0_12345".into()]));
        assert!(!is_finder_launch(&["-psn_0_12345".into(), "list".into()]));
        assert!(!is_finder_launch(&["list".into()]));
        assert!(!is_finder_launch(&[]));
    }

    #[test]
    fn a_setting_in_the_bundle_is_unset_and_the_users_own_kept() {
        for name in APPIMAGE_PATHS {
            // The GTK hook writes `$APPDIR//usr/...`.
            for bundled in ["/tmp/.mount_x/usr", "/tmp/.mount_x//usr/lib", "/tmp/.mount_x"] {
                let shell = shell_of(appimage(&[(name, bundled)]));
                assert_eq!(shell.vars, vec![], "{name}={bundled}");
            }
            let shell = shell_of(appimage(&[(name, "/opt/python")]));
            assert_eq!(shell.vars, vars(&[(name, "/opt/python")]), "{name}");
        }
    }

    #[test]
    fn what_only_the_appimage_sets_is_unset() {
        for name in APPIMAGE_ONLY {
            let shell = shell_of(appimage(&[(name, "Adwaita:dark"), ("HOME", "/home/u")]));
            assert_eq!(shell.vars, vars(&[("HOME", "/home/u")]), "{name}");
        }
    }

    /// Set for the process `the_process_takes_the_users_shell` starts: that run of the test stands in for the app,
    /// enters a shell and execs into sh, which prints what it got.
    #[cfg(unix)]
    const ENTERS_SHELL: &str = "OCTOFLOW_UI_TEST_ENTERS_SHELL";

    #[test]
    #[cfg(unix)]
    fn the_process_takes_the_users_shell() {
        use std::os::unix::process::CommandExt;
        if std::env::var_os(ENTERS_SHELL).is_some() {
            let mut users = vars(&[("KEPT", "kept"), ("CHANGED", "the user's"), ("ADDED", "added")]);
            // By which the exec below finds sh.
            users.extend(std::env::var_os("PATH").map(|path| (OsString::from("PATH"), path)));
            UserShell { vars: users, dir: Some("/".into()) }.enter().expect("the shell entered");
            let print = "pwd; echo \"$KEPT,$CHANGED,$ADDED,${DROPPED-unset},${HOME-unset}\"";
            let e = Command::new("sh").args(["-c", print]).exec();
            panic!("cannot exec sh: {e}");
        }
        let out = Command::new(std::env::current_exe().expect("the test binary"))
            .args(["tests::the_process_takes_the_users_shell", "--exact"])
            .env(ENTERS_SHELL, "1")
            .env("KEPT", "kept")
            .env("CHANGED", "the bundle's")
            .env("DROPPED", "dropped")
            .env("HOME", "/home/u")
            .env_remove("ADDED")
            .output()
            .expect("the test binary runs");
        let printed = String::from_utf8_lossy(&out.stdout);
        assert!(printed.ends_with("/\nkept,the user's,added,unset,unset\n"), "{printed}");
    }

    #[test]
    fn a_directory_since_removed_is_refused_with_nothing_changed() {
        let gone = UserShell { vars: Vec::new(), dir: Some("/nonexistent/proj".into()) };
        assert_eq!(gone.enter(), Err("cannot start in /nonexistent/proj: it is not a directory".to_string()));
        assert!(std::env::vars_os().next().is_some(), "the environment was emptied");
    }

    #[test]
    fn only_the_appimage_file_itself_is_the_runtime() {
        let dir = TempDir::new();
        let appimage = dir.join("x.AppImage");
        std::fs::write(&appimage, "").expect("a stand-in AppImage");
        assert!(is_appimage(Some(&appimage), Some(&appimage)));
        assert!(!is_appimage(Some(&appimage), Some(Path::new("/bin/sh"))), "a parent that is a shell");
        assert!(!is_appimage(None, Some(&appimage)), "no APPIMAGE");
        assert!(!is_appimage(Some(&appimage), None), "a parent whose executable cannot be read");
    }

    #[test]
    #[cfg(unix)]
    fn the_appimage_named_through_a_link_is_the_runtime_too() {
        let dir = TempDir::new();
        let appimage = dir.join("x.AppImage");
        std::fs::write(&appimage, "").expect("a stand-in AppImage");
        let link = dir.join("link.AppImage");
        std::os::unix::fs::symlink(&appimage, &link).expect("a link to it");
        assert!(is_appimage(Some(&link), Some(&appimage)));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn the_death_signal_is_asked_for_only_under_the_appimage() {
        use nix::sys::prctl::{get_pdeathsig, set_pdeathsig};
        use nix::sys::signal::Signal;
        // This process's parent, the test runner, stands in for the runtime. The request armed below only sends this
        // process SIGTERM should the runner die first, and is withdrawn before the assertions.
        let runner = std::fs::read_link(format!("/proc/{}/exe", nix::unistd::getppid())).expect("the runner's exe");
        let dir = TempDir::new();
        let other = dir.join("x.AppImage");
        std::fs::write(&other, "").expect("a stand-in AppImage");
        stop_with_appimage_runtime(None).expect("no APPIMAGE");
        let without = get_pdeathsig();
        stop_with_appimage_runtime(Some(&other)).expect("another AppImage");
        let elsewhere = get_pdeathsig();
        stop_with_appimage_runtime(Some(&runner)).expect("the death signal set");
        let under = get_pdeathsig();
        set_pdeathsig(None).expect("the death signal withdrawn");
        assert_eq!(without.expect("the death signal read"), None, "no APPIMAGE");
        assert_eq!(elsewhere.expect("the death signal read"), None, "a parent that is not the AppImage");
        assert_eq!(under.expect("the death signal read"), Some(Signal::SIGTERM));
    }

    /// Set, to the path of a file to create, for the process `a_cli_gets_sigterm_when_its_appimage_runtime_dies`
    /// starts: that process stands in for the app, and creates the file once it has asked for its signal.
    #[cfg(target_os = "linux")]
    const STAND_IN_READY: &str = "OCTOFLOW_UI_TEST_STAND_IN_READY";

    /// Whether the process with this pid is running: it exists and is no zombie.
    #[cfg(target_os = "linux")]
    fn running(pid: u32) -> bool {
        // /proc/<pid>/stat: pid (comm) state …, comm holding no ')' here.
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        stat.rsplit_once(") ").is_some_and(|(_, rest)| !rest.starts_with('Z'))
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn a_cli_gets_sigterm_when_its_appimage_runtime_dies() {
        use std::io::Read;
        use std::os::unix::process::CommandExt;
        // The stand-in for the app: a run of this test with STAND_IN_READY set, whose parent is a shell named as the
        // AppImage. It asks for its signal, says so, and execs into sleep, which stands in for the CLI.
        if let Some(ready) = std::env::var_os(STAND_IN_READY) {
            let appimage = std::env::var_os("APPIMAGE").map(PathBuf::from);
            stop_with_appimage_runtime(appimage.as_deref()).expect("the death signal set");
            std::fs::write(&ready, "").expect("the ready file written");
            let e = Command::new("sleep").arg("30").exec();
            panic!("cannot exec sleep: {e}");
        }
        let dir = TempDir::new();
        let ready = dir.join("ready");
        let test = std::env::current_exe().expect("the test binary");
        // The runtime: a shell that starts the stand-in, prints its pid, and waits for its standard input to close.
        let mut runtime = Command::new("sh")
            .args(["-c", "\"$0\" \"$1\" --exact >/dev/null 2>&1 & echo $!; read _"])
            .arg(&test)
            .arg("tests::a_cli_gets_sigterm_when_its_appimage_runtime_dies")
            .env(STAND_IN_READY, &ready)
            .env("APPIMAGE", "/bin/sh")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("sh runs");
        let mut out = String::new();
        let mut stdout = runtime.stdout.take().expect("the runtime's output");
        let mut byte = [0u8];
        while stdout.read(&mut byte).expect("the runtime's output read") == 1 && byte[0] != b'\n' {
            out.push(byte[0] as char);
        }
        let cli: u32 = out.trim().parse().unwrap_or_else(|e| panic!("the stand-in's pid, not {out:?}: {e}"));
        let started = std::time::Instant::now();
        while !ready.exists() {
            assert!(started.elapsed().as_secs() < 30, "the stand-in never asked for its signal");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        drop(runtime.stdin.take());
        runtime.wait().expect("the runtime exits");
        let died = std::time::Instant::now();
        while running(cli) && died.elapsed().as_secs() < 2 {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let left = running(cli);
        if left {
            Command::new("kill").arg(cli.to_string()).status().expect("kill runs");
        }
        assert!(!left, "the CLI still runs 2 s after its runtime died");
    }
}
