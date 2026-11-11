// No console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Before the window initialises GTK or WebKit and their threads, which the CLI must start ahead of. Windows builds
    // no CLI: its release has no console to print to.
    #[cfg(unix)]
    if let Some(cli) = octoflow_ui_lib::cli(std::env::args_os()) {
        cli.run()
    }
    octoflow_ui_lib::run()
}
