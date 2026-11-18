//! The desktop shell: a window over the orchestrator's `serve`, and the orchestrator itself, whose engine it links
//! (`octoflow_engine`). Started with arguments, the app is the orchestrator's CLI: see `cli`. Started with none, it
//! opens its window and starts itself, as `serve`, as its sidecar, which it kills when it exits. The webview talks to
//! the sidecar's server over HTTP; the shell hands it the server's address and token, through the `server` command,
//! and opens links outside the app, through `open_outside`.

use std::ffi::{OsStr, OsString};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

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
            Some(dir) => std::env::set_current_dir(dir).map_err(|e| format!("cannot start in {}: {e}", dir.display())),
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

/// What the webview is given by the `server` command: how to reach the sidecar, the project to open first, and the
/// release version.
#[derive(Clone, Serialize)]
struct Connection {
    url: String,
    token: String,
    /// The project the app opens first: the checkout a development build was built in (`Sidecar::of_app`); None for
    /// any other build, which belongs to no checkout.
    project: Option<String>,
    /// The release version, the linked engine's (`octoflow_engine::VERSION`), as the About dialog shows it.
    version: &'static str,
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
                let version = octoflow_engine::VERSION;
                let connection = Connection { url: served.url, token: served.token, project, version };
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

/// The sidecar's Connection, or why it could not be started.
#[tauri::command]
fn server(sidecar: tauri::State<'_, Sidecar>) -> Result<Connection, String> {
    sidecar.connection.clone()
}

/// What the window can have opened outside the app, as it asks `open_outside` for it: `{"page": "<url>"}`, a web page
/// in the system's browser, or `{"license": "crate-licenses"}` (or `npm-licenses`), a third-party license file a
/// packaged app bundles among its resources (packaging/build-desktop.sh), in its HTML viewer. A page must be an
/// `https` URL, and a file is named, never given by its path, so that nothing the window sends opens a program, a
/// local file or another scheme's handler.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
enum Outside {
    Page(String),
    License(LicenseFile),
}

/// A third-party license file a packaged app bundles, by the name the window gives it.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
enum LicenseFile {
    CrateLicenses,
    NpmLicenses,
}

impl Outside {
    /// What the opener is given for it: a page's URL as `tauri::Url` writes it, or a license file's path in the app's
    /// resource directory, which resources gives, called for a license file only. Errors, saying so: a page that is
    /// not an `https` URL with a host; resources' error; a license file the directory does not hold, as in any build
    /// but a packaged one.
    fn target(&self, resources: impl FnOnce() -> Result<PathBuf, String>) -> Result<OsString, String> {
        let file = match self {
            Outside::Page(page) => {
                return match tauri::Url::parse(page) {
                    Ok(url) if url.scheme() == "https" && url.host().is_some() => Ok(url.as_str().into()),
                    _ => Err(format!("only an https page opens outside the app, not {page}")),
                }
            }
            Outside::License(LicenseFile::CrateLicenses) => "THIRD-PARTY-LICENSES.html",
            Outside::License(LicenseFile::NpmLicenses) => "THIRD-PARTY-LICENSES-NPM.html",
        };
        let path = resources()?.join(file);
        if !path.is_file() {
            return Err(format!("{file} is not bundled with this build"));
        }
        Ok(path.into_os_string())
    }
}

/// Open target outside the app, with the system's default handler, as the user's own shell would: in the user's
/// environment (`UserShell`), not the window's, whose AppImage variables would start the browser against the bundle's
/// libraries. Errors, as a line for the window to show: Outside::target's, among them that a license file's directory,
/// the app's resources, cannot be told; or run_opener's.
#[tauri::command(async)]
fn open_outside(app: tauri::AppHandle, target: Outside) -> Result<(), String> {
    let resources = || app.path().resource_dir().map_err(|e| format!("cannot tell where the app's resources are: {e}"));
    run_opener(Path::new(OPENER), &target.target(resources)?, &UserShell::current())
}

/// The program that opens a URL or a file with the system's default handler.
const OPENER: &str = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };

/// How long run_opener waits for the opener to fail. `xdg-open` and `open` exit once they have handed the target
/// over, but `xdg-open` on a desktop it does not know runs `$BROWSER` itself and waits for it.
const OPENER_CHECK: Duration = Duration::from_secs(2);

/// Run opener on target in shell's environment and directory (the app's own when shell names none, or one that is no
/// longer a directory), its standard input closed and its output the app's, and return once
/// it has exited successfully or OPENER_CHECK has passed; one still running then is waited for in the background, so
/// it leaves no zombie. Errors, naming the target: opener cannot be started, or exits failing within OPENER_CHECK.
fn run_opener(opener: &Path, target: &OsStr, shell: &UserShell) -> Result<(), String> {
    let shown = target.to_string_lossy();
    let mut command = Command::new(opener);
    command.arg(target).env_clear().envs(shell.vars.iter().map(|(name, value)| (name, value))).stdin(Stdio::null());
    // Under an AppImage the app's own directory is in the bundle's mount, which goes when the app quits, while a
    // browser the opener starts lives on.
    if let Some(dir) = shell.dir.as_ref().filter(|dir| dir.is_dir()) {
        command.current_dir(dir);
    }
    let mut child = command.spawn().map_err(|e| format!("cannot run {} to open {shown}: {e}", opener.display()))?;
    let started = Instant::now();
    while started.elapsed() < OPENER_CHECK {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                return Err(format!("{} could not open {shown}: it exited with {status}", opener.display()))
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(format!("cannot tell whether {} opened {shown}: {e}", opener.display())),
        }
    }
    // What the reaper fails to wait for is lost to no one: the opener runs on.
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// Run the app until its window closes, then stop the sidecar.
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            app.manage(Sidecar::of_app(&webview_origin(app)));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![server, open_outside])
        .build(tauri::generate_context!())
        .expect("error while building the app")
        .run(|app, event| {
            if let RunEvent::Exit = event {
                app.state::<Sidecar>().stop();
            }
        });
}

#[cfg(test)]
mod tests;
