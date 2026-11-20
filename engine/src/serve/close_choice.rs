//! What the desktop app does with its running runs when its window closes, once the user chose not to be asked again:
//! kept in the engine's configuration directory (ConfigDir::close_choice_path), not in the webview's storage, so that
//! it outlives a reset of the webview.

use std::io::ErrorKind;

use crate::env::ConfigDir;
use crate::error::os_error;

/// What the app does with its running runs when its window closes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseChoice {
    /// Ask, each time: the choice until the user makes one with "Don't ask again".
    Ask,
    /// Close the window and leave the runs running: they do not depend on the app.
    Keep,
    /// Stop each of them first, as "Stop all" does (crate::cli::close::stop_all).
    Stop,
}

impl CloseChoice {
    /// Its name, as serve's /close-choice gives and takes it: "ask", "keep" or "stop".
    pub fn name(self) -> &'static str {
        match self {
            CloseChoice::Ask => "ask",
            CloseChoice::Keep => "keep",
            CloseChoice::Stop => "stop",
        }
    }

    /// The choice of that name; None for any other.
    pub fn named(name: &str) -> Option<CloseChoice> {
        [CloseChoice::Ask, CloseChoice::Keep, CloseChoice::Stop].into_iter().find(|c| c.name() == name)
    }

    /// The choice saved in config; Ask when none is. Errors: the file cannot be read, or holds no choice's name.
    pub fn read(config: &ConfigDir) -> Result<CloseChoice, String> {
        let path = config.close_choice_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => CloseChoice::named(text.trim_end())
                .ok_or_else(|| format!("{path} holds {:?}, not ask, keep or stop", text.trim_end())),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(CloseChoice::Ask),
            Err(e) => Err(format!("cannot read {path}: {}", os_error(&e, None))),
        }
    }

    /// Save it in config, so that read gives it, creating the directory when missing; Ask removes the file. Errors:
    /// the directory or the file cannot be made, or the file cannot be removed.
    pub fn save(self, config: &ConfigDir) -> Result<(), String> {
        let path = config.close_choice_path();
        let failed = |doing: &str, e: std::io::Error| format!("cannot {doing} {path}: {}", os_error(&e, None));
        if self == CloseChoice::Ask {
            return match std::fs::remove_file(&path) {
                Err(e) if e.kind() != ErrorKind::NotFound => Err(failed("remove", e)),
                _ => Ok(()),
            };
        }
        std::fs::create_dir_all(config.path()).map_err(|e| failed("create the directory of", e))?;
        std::fs::write(&path, format!("{}\n", self.name())).map_err(|e| failed("write", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    #[test]
    fn a_saved_choice_is_read_back_and_ask_removes_it() {
        let dir = TempDir::new();
        let config = ConfigDir::at(&format!("{}/missing/octoflow-ai", dir.path()));
        assert_eq!(CloseChoice::read(&config), Ok(CloseChoice::Ask));
        CloseChoice::Stop.save(&config).unwrap();
        assert_eq!(std::fs::read_to_string(config.close_choice_path()).unwrap(), "stop\n");
        assert_eq!(CloseChoice::read(&config), Ok(CloseChoice::Stop));
        CloseChoice::Keep.save(&config).unwrap();
        assert_eq!(CloseChoice::read(&config), Ok(CloseChoice::Keep));
        CloseChoice::Ask.save(&config).unwrap();
        assert!(!std::path::Path::new(&config.close_choice_path()).exists());
        assert_eq!(CloseChoice::read(&config), Ok(CloseChoice::Ask));
        CloseChoice::Ask.save(&config).unwrap();
    }

    #[test]
    fn a_file_without_a_choice_is_an_error_naming_it() {
        let dir = TempDir::new();
        let config = ConfigDir::at(&dir.path());
        std::fs::write(config.close_choice_path(), "maybe\n").unwrap();
        let path = config.close_choice_path();
        assert_eq!(CloseChoice::read(&config), Err(format!("{path} holds \"maybe\", not ask, keep or stop")));
    }
}
