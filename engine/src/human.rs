//! The human watching a run, as the engine reaches them: [`Human`] is what a run tells them, in notifications and log
//! lines; [`Notify`] is where a notification is shown, which the human's implementation (crate::workflow::LocalHuman)
//! is given.

use std::fmt;

use crate::error::OrchestratorError;

/// The human watching the run: told what needs them in a notification, and everything else in log lines.
pub trait Human {
    /// A notification, with its title and body.
    fn notify(&self, title: &str, body: &str);
    /// One log line, without the indent the terminal shows it with.
    fn log(&self, line: &str);
}

/// Where the human is shown a notification.
pub trait Notify: fmt::Debug {
    /// Show a notification with title and body.
    fn notify(&self, title: &str, body: &str) -> Result<(), OrchestratorError>;
}
