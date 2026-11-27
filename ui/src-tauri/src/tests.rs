use super::*;

/// Whether a process with this pid still exists, zombies included, by its /proc entry.
fn exists(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

/// A new directory of its own under the system's temporary directory, removed with everything in it when
/// dropped, so it must outlive every use of its path.
fn scratch_dir() -> tempfile::TempDir {
    tempfile::Builder::new().prefix("octoflow-ui-test-").tempdir().expect("a scratch directory")
}

/// The orchestrator binary of this checkout, built as `cargo build -p octoflow-engine` builds it, with the cargo
/// that runs these tests; its path is the one cargo reports, wherever CARGO_TARGET_DIR puts it.
fn built_orchestrator() -> PathBuf {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let built = Command::new(cargo)
        .args(["build", "--locked", "--quiet", "-p", "octoflow-engine", "--bin", "octoflow"])
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
    let dir = scratch_dir();
    let program = dir.path().join("octoflow");
    // serve's first line, then a process for stop to kill.
    let script = "#!/bin/sh\necho '{\"url\": \"http://127.0.0.1:9\", \"token\": \"t\"}'\nexec sleep 60\n";
    put_executable(&program, script);
    let sidecar = Sidecar::start(&program, "tauri://localhost", None);
    let connection = sidecar.connection.clone().expect("the sidecar started");
    assert_eq!(connection.url, "http://127.0.0.1:9");
    assert_eq!(connection.project, None);
    sidecar.stop();
}

/// argv as the CLI is given it, the app's own name first.
fn argv(args: &[&str]) -> Vec<OsString> {
    std::iter::once("octoflow-desktop").chain(args.iter().copied()).map(OsString::from).collect()
}

/// The bundle `APPDIR=dir` names.
fn bundle(dir: &str) -> Option<Bundle> {
    Bundle::of(Some(OsStr::new(dir)))
}

#[test]
fn serve_from_a_mounted_appimage_starts_its_runs_by_the_appimage_file() {
    // `<AppImage> serve`, from the window or a terminal, the app's program on the AppImage's mount.
    let exe = Path::new("/tmp/.mount_Octofl/usr/bin/octoflow-desktop");
    let runs_by =
        run_program(Some("/home/u/Octoflow.AppImage".into()), bundle("/tmp/.mount_Octofl"), exe, |_: &Path| true);
    let given = with_run_program(argv(&["serve", "--origin", "tauri://localhost"]), runs_by);
    let expected = argv(&["serve", "--run-program", "/home/u/Octoflow.AppImage", "--origin", "tauri://localhost"]);
    assert_eq!(given, expected);
}

#[test]
fn only_serve_is_given_a_program_for_its_runs() {
    let appimage = || Some(PathBuf::from("/home/u/Octoflow.AppImage"));
    assert_eq!(with_run_program(argv(&["serve"]), None), argv(&["serve"]));
    assert_eq!(with_run_program(argv(&["list"]), appimage()), argv(&["list"]));
    assert_eq!(with_run_program(argv(&["--help"]), appimage()), argv(&["--help"]));
    // Another command's argument that reads `serve`, as a run's task can.
    assert_eq!(with_run_program(argv(&["run", "serve"]), appimage()), argv(&["run", "serve"]));
}

#[test]
fn a_run_program_the_user_gives_serve_wins_over_the_app_s() {
    let given =
        with_run_program(argv(&["serve", "--run-prog=/opt/octoflow"]), Some("/home/u/Octoflow.AppImage".into()));
    let parsed = octoflow_engine::cli::args::parse_args(
        given.into_iter().skip(1),
        &octoflow_engine::env::ConfigDir::at("/nowhere"),
    );
    match parsed.expect("serve's arguments parse") {
        octoflow_engine::cli::args::Command::Serve { run_program, .. } => {
            assert_eq!(run_program.as_deref(), Some("/opt/octoflow"));
        }
        _ => panic!("not serve"),
    }
}

#[test]
fn runs_start_by_the_appimage_file_only_while_the_app_runs_from_its_mount() {
    let exe = Path::new("/tmp/.mount_Octofl/usr/bin/octoflow-desktop");
    let appimage = || Some(OsString::from("/home/u/Octoflow.AppImage"));
    let mounted = |path: &Path| path.starts_with("/tmp/.mount_Octofl");
    let its_mount = || bundle("/tmp/.mount_Octofl");
    assert_eq!(run_program(appimage(), its_mount(), exe, mounted), Some(PathBuf::from("/home/u/Octoflow.AppImage")));
    // Extracted by the runtime (APPIMAGE_EXTRACT_AND_RUN), which still sets APPIMAGE.
    let extracted = Path::new("/tmp/appimage_extracted_0123/usr/bin/octoflow-desktop");
    assert_eq!(run_program(appimage(), bundle("/tmp/appimage_extracted_0123"), extracted, mounted), None);
    // The .deb and macOS: no AppImage.
    assert_eq!(run_program(None, None, Path::new("/usr/bin/octoflow-desktop"), |_: &Path| false), None);
    assert_eq!(run_program(Some(OsString::new()), its_mount(), exe, mounted), None);
}

#[test]
fn an_appimage_leaked_to_a_program_on_another_fuse_mount_is_not_trusted() {
    // An Octoflow on a FUSE home (sshfs, gocryptfs), started by another AppImage that passed on its variables.
    let exe = Path::new("/home/u/apps/octoflow/usr/bin/octoflow-desktop");
    let other = || Some(OsString::from("/home/u/Other.AppImage"));
    let fuse = |_: &Path| true;
    assert_eq!(run_program(other(), bundle("/tmp/.mount_Other1"), exe, fuse), None);
    assert_eq!(run_program(other(), None, exe, fuse), None);
    // An APPDIR of `/` or a relative one is no bundle, rather than one holding every path.
    assert_eq!(run_program(other(), bundle("/"), exe, fuse), None);
    assert_eq!(run_program(other(), bundle("home/u/apps/octoflow"), exe, fuse), None);
}

#[test]
#[cfg(unix)]
fn an_appimage_path_that_is_not_utf_8_is_not_passed_on() {
    use std::os::unix::ffi::OsStringExt;
    let exe = Path::new("/tmp/.mount_Octofl/usr/bin/octoflow-desktop");
    let latin1 = OsString::from_vec(b"/home/u/T\xe9l\xe9chargements/Octoflow.AppImage".to_vec());
    assert_eq!(run_program(Some(latin1), bundle("/tmp/.mount_Octofl"), exe, |_: &Path| true), None);
}

#[test]
#[cfg(target_os = "linux")]
fn a_directory_on_disk_is_no_fuse_mount() {
    assert!(!on_fuse(scratch_dir().path()));
    assert!(!on_fuse(Path::new("/nonexistent/octoflow")));
}

#[test]
#[cfg(unix)]
fn the_connection_s_version_is_the_release_s() {
    let printed = Command::new(checkout().join("ci/release-version.sh")).output().expect("release-version.sh runs");
    assert!(printed.status.success(), "{}", String::from_utf8_lossy(&printed.stderr));
    let release = String::from_utf8(printed.stdout).expect("UTF-8");
    let dir = scratch_dir();
    let program = dir.path().join("octoflow");
    put_executable(&program, "#!/bin/sh\necho '{\"url\": \"http://127.0.0.1:9\", \"token\": \"t\"}'\n");
    let sidecar = Sidecar::start(&program, "tauri://localhost", None);
    let connection = sidecar.connection.clone().expect("the sidecar started");
    assert_eq!(connection.version, release.trim());
    sidecar.stop();
}

#[test]
fn only_an_https_page_opens_and_a_license_file_only_when_bundled() {
    let resources = scratch_dir();
    let dir = || Ok(resources.path().to_path_buf());
    // A page needs no resource directory: one that cannot be told fails license files only.
    let page = |url: &str| Outside::Page(url.to_string()).target(|| Err("no resources".to_string()));
    assert_eq!(page("https://github.com/o/r/pull/1"), Ok("https://github.com/o/r/pull/1".into()));
    for refused in ["http://github.com/o/r", "file:///etc/passwd", "https://", "-https://x", "javascript:alert(1)"] {
        assert_eq!(page(refused), Err(format!("only an https page opens outside the app, not {refused}")));
    }
    let missing = Outside::License(LicenseFile::NpmLicenses).target(dir);
    assert_eq!(missing, Err("THIRD-PARTY-LICENSES-NPM.html is not bundled with this build".to_string()));
    let bundled = resources.path().join("THIRD-PARTY-LICENSES.html");
    std::fs::write(&bundled, "<html></html>").expect("the license file is written");
    let found = Outside::License(LicenseFile::CrateLicenses).target(dir);
    assert_eq!(found, Ok(bundled.into_os_string()));
    let untold = Outside::License(LicenseFile::CrateLicenses).target(|| Err("no resources".to_string()));
    assert_eq!(untold, Err("no resources".to_string()));
}

#[test]
fn the_window_names_a_page_by_its_url_and_a_license_file_by_its_name() {
    let parse = |json: &str| serde_json::from_str::<Outside>(json);
    assert_eq!(
        parse(r#"{"page": "https://example.com"}"#).expect("a page"),
        Outside::Page("https://example.com".into())
    );
    let license = parse(r#"{"license": "crate-licenses"}"#).expect("a license file");
    assert_eq!(license, Outside::License(LicenseFile::CrateLicenses));
    assert!(parse(r#"{"license": "/etc/passwd"}"#).is_err());
}

#[test]
#[cfg(unix)]
fn the_opener_runs_in_the_user_s_shell_only() {
    let dir = scratch_dir();
    let (opener, out) = (dir.path().join("opener"), dir.path().join("out"));
    let fields = "\"$1\" \"$MARK\" \"${HOME-unset}\" \"$(pwd)\"";
    let record = format!("printf '%s|%s|%s|%s' {fields} > '{}'", out.display());
    let script = format!("#!/bin/sh\n{record}\n");
    put_executable(&opener, &script);
    let users = scratch_dir();
    let shell = UserShell {
        vars: vars(&[("PATH", "/usr/bin:/bin"), ("MARK", "user")]),
        dir: Some(canonical(users.path().to_path_buf())),
    };
    assert_eq!(run_opener(&opener, OsStr::new("https://example.com"), &shell), Ok(()));
    // HOME is the test process's own, which the window's environment stands for.
    let ran = std::fs::read_to_string(&out).expect("the opener ran");
    assert_eq!(ran, format!("https://example.com|user|unset|{}", canonical(users.path().to_path_buf()).display()));
}

#[test]
#[cfg(unix)]
fn an_opener_that_fails_or_is_not_there_is_an_error() {
    let dir = scratch_dir();
    let opener = dir.path().join("opener");
    put_executable(&opener, "#!/bin/sh\nexit 3\n");
    let shell = shell_of(vars(&[("PATH", "/usr/bin:/bin")]));
    let failed = run_opener(&opener, OsStr::new("/x.html"), &shell).expect_err("exit 3 fails");
    assert!(failed.ends_with("could not open /x.html: it exited with exit status: 3"), "{failed}");
    let missing = run_opener(&dir.path().join("gone"), OsStr::new("/x.html"), &shell).expect_err("no opener");
    assert!(missing.starts_with("cannot run ") && missing.contains("to open /x.html"), "{missing}");
}

#[test]
#[cfg(unix)]
fn an_opener_still_running_is_left_to_run() {
    let dir = scratch_dir();
    let opener = dir.path().join("opener");
    put_executable(&opener, "#!/bin/sh\nexec sleep 5\n");
    let shell = shell_of(vars(&[("PATH", "/usr/bin:/bin")]));
    let started = Instant::now();
    assert_eq!(run_opener(&opener, OsStr::new("https://example.com"), &shell), Ok(()));
    assert!(started.elapsed() < OPENER_CHECK + Duration::from_secs(1));
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
    let dir = scratch_dir();
    let program = dir.path().join("octoflow");
    let script = "#!/bin/sh\necho 'octoflow: cannot start in /gone: it is not a directory' >&2\nexit 1\n";
    put_executable(&program, script);
    let reason = Sidecar::start(&program, "tauri://localhost", None).connection.err().expect("an error");
    assert_eq!(reason, "octoflow: cannot start in /gone: it is not a directory");
}

#[test]
#[cfg(unix)]
fn a_sidecar_that_prints_something_else_is_named_with_what_it_printed() {
    let dir = scratch_dir();
    let program = dir.path().join("octoflow");
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
    let mut env = vars(&[("APPDIR", "/tmp/.mount_x"), ("APPIMAGE", "/home/u/app.AppImage"), ("OWD", "/home/u/proj")]);
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
    let env = vars(&[("APPDIR", "/opt/x"), ("LD_LIBRARY_PATH", "/opt/x/lib"), ("PYTHONHOME", "/opt/x"), ("OWD", "/")]);
    assert_eq!(shell_of(env.clone()), UserShell { vars: env.clone(), dir: None });
    let elsewhere = UserShell::from_vars(env.clone(), Some(Path::new("/usr/bin/octoflow-ui")));
    assert_eq!(elsewhere, UserShell { vars: env, dir: None });
}

#[test]
fn an_extracted_appimage_started_by_its_apprun_is_one_too() {
    // squashfs-root/AppRun sets APPDIR and the bundle's variables, but no APPIMAGE.
    let dir = scratch_dir();
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
    let dir = scratch_dir();
    std::fs::create_dir_all(dir.path().join("real/app/usr/lib")).expect("an AppDir");
    std::fs::write(dir.path().join("real/app/AppRun"), "").expect("a stand-in AppRun");
    std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("link")).expect("a link");
    let (real, link) = (dir.path().join("real/app"), dir.path().join("link/app"));
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
    let env = vars(&[("APPDIR", "/tmp/appimage_extracted_x"), ("APPIMAGE", "/a.AppImage"), ("PWD", "/home/u/proj")]);
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
    let dir = scratch_dir();
    let appimage = dir.path().join("x.AppImage");
    std::fs::write(&appimage, "").expect("a stand-in AppImage");
    assert!(is_appimage(Some(&appimage), Some(&appimage)));
    assert!(!is_appimage(Some(&appimage), Some(Path::new("/bin/sh"))), "a parent that is a shell");
    assert!(!is_appimage(None, Some(&appimage)), "no APPIMAGE");
    assert!(!is_appimage(Some(&appimage), None), "a parent whose executable cannot be read");
}

#[test]
#[cfg(unix)]
fn the_appimage_named_through_a_link_is_the_runtime_too() {
    let dir = scratch_dir();
    let appimage = dir.path().join("x.AppImage");
    std::fs::write(&appimage, "").expect("a stand-in AppImage");
    let link = dir.path().join("link.AppImage");
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
    let dir = scratch_dir();
    let other = dir.path().join("x.AppImage");
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

/// Set, to any value, for the process `a_cli_gets_sigterm_when_its_appimage_runtime_dies` starts: that process
/// stands in for the app.
#[cfg(target_os = "linux")]
const STAND_IN: &str = "OCTOFLOW_UI_TEST_STAND_IN";

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
    // The stand-in for the app: a run of this test with STAND_IN set, whose parent is a shell named as the AppImage.
    // It asks for its signal and execs into sleep, which stands in for the CLI.
    if std::env::var_os(STAND_IN).is_some() {
        let appimage = std::env::var_os("APPIMAGE").map(PathBuf::from);
        stop_with_appimage_runtime(appimage.as_deref()).expect("the death signal set");
        let e = Command::new("sleep").arg("30").exec();
        panic!("cannot exec sleep: {e}");
    }
    let test = std::env::current_exe().expect("the test binary");
    // The runtime: a shell that starts the stand-in, prints its pid, and waits for its standard input to close. The
    // stand-in's output, where its test harness reports a failure, goes to this test's standard error.
    let mut runtime = Command::new("sh")
        .args(["-c", "\"$0\" \"$1\" --exact >&2 & echo $!; read _"])
        .arg(&test)
        .arg("tests::a_cli_gets_sigterm_when_its_appimage_runtime_dies")
        .env(STAND_IN, "1")
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
    // The kernel names the process sleep only once its exec is over, the CLI running. Until then the pid can read as
    // a zombie and then running again, an exec from a thread other than the main one (GitHub issue #650), which the
    // wait below would take for the CLI ending.
    let started = std::time::Instant::now();
    while std::fs::read_to_string(format!("/proc/{cli}/comm")).unwrap_or_default() != "sleep\n" {
        assert!(started.elapsed().as_secs() < 30, "the stand-in never became the CLI");
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
