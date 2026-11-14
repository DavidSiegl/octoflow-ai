//! The environment the engine gives the processes it starts, and where its configuration lives (ConfigDir).

use std::ffi::OsString;
use std::fs::File;
use std::io::Read;

use crate::constants::GIT_PATHSPEC_ENV;

/// The environment for every process the orchestrator starts: its own, without GIT_PATHSPEC_ENV.
///
/// So the git the orchestrator runs reads each pathspec in its default syntax, whatever switch the user exported. An
/// agent does not inherit the switches either, but its pane's shell reads the user's startup files and takes any they
/// set. Over SSH this is the local ssh process's environment; the remote git reads the remote shell's.
pub fn child_env() -> Vec<(OsString, OsString)> {
    without_withheld(std::env::vars_os())
}

/// vars without the variables child_env withholds, in their order.
pub(crate) fn without_withheld(vars: impl Iterator<Item = (OsString, OsString)>) -> Vec<(OsString, OsString)> {
    vars.filter(|(k, _)| !GIT_PATHSPEC_ENV.iter().any(|w| k == w)).collect()
}

/// The orchestrator's own configuration directory, on the machine it runs on, and where in it each of its files is:
/// the prompt files, the workflow files and the desktop app's setup marker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigDir {
    /// The directory's path, without a trailing "/".
    path: String,
}

impl ConfigDir {
    /// The configuration directory of the user running the engine: ai-agents-orchestrator in $XDG_CONFIG_HOME when
    /// that is an absolute path, else in ~/.config, ~ being $HOME, or without it the user's home in /etc/passwd.
    pub fn from_env() -> ConfigDir {
        ConfigDir::for_env(&|name| std::env::var(name).ok(), &passwd_home)
    }

    /// from_env for the environment env gives, by variable name, and home, the user's home without HOME.
    fn for_env(env: &dyn Fn(&str) -> Option<String>, home: &dyn Fn() -> Option<String>) -> ConfigDir {
        let config = env("XDG_CONFIG_HOME").filter(|c| c.starts_with('/'));
        let config = config.unwrap_or_else(|| match env("HOME").or_else(home) {
            Some(home) => format!("{}/.config", home.trim_end_matches('/')),
            None => "~/.config".to_string(),
        });
        ConfigDir::at(&format!("{}/ai-agents-orchestrator", config.trim_end_matches('/')))
    }

    /// The configuration directory at path.
    pub fn at(path: &str) -> ConfigDir {
        let trimmed = path.trim_end_matches('/');
        ConfigDir { path: if trimmed.is_empty() { "/".to_string() } else { trimmed.to_string() } }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    /// Where a new run finds its prompt files, NAME.md for the Prompt named NAME (crate::prompts).
    pub fn prompts_dir(&self) -> String {
        self.join("prompts")
    }

    /// Where `run --workflow NAME` finds NAME.toml (crate::workflow_file).
    pub fn workflows_dir(&self) -> String {
        self.join("workflows")
    }

    /// The file whose presence means the desktop app's setup dialog once closed with every Required check passed, so
    /// that it no longer opens by itself (crate::serve::setup); empty.
    pub fn setup_checked_path(&self) -> String {
        self.join("setup-checked")
    }

    fn join(&self, name: &str) -> String {
        format!("{}/{name}", self.path.trim_end_matches('/'))
    }
}

/// The home directory /etc/passwd gives the user the engine runs as, which os.path.expanduser falls back on without
/// HOME; None when either file cannot be read or names none. Unlike Python's pwd module, it reads the file alone, not
/// whatever else the system's name service has.
fn passwd_home() -> Option<String> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let uid = status.lines().find_map(|line| line.strip_prefix("Uid:"))?.split_whitespace().next()?.to_string();
    home_of(&std::fs::read_to_string("/etc/passwd").ok()?, &uid)
}

/// The home directory of the user of uid in passwd, a passwd(5) file's text.
fn home_of(passwd: &str, uid: &str) -> Option<String> {
    passwd.lines().find_map(|line| {
        let fields: Vec<&str> = line.split(':').collect();
        (fields.len() >= 7 && fields[2] == uid).then(|| fields[5].to_string())
    })
}

/// N bytes from the system's random source, /dev/urandom, for run ids and serve's tokens. Errors: it cannot be read.
pub fn random_bytes<const N: usize>() -> std::io::Result<[u8; N]> {
    let mut bytes = [0u8; N];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests;
