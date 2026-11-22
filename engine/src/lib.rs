//! The orchestrator's engine: the workflows, run state, agents and checks the `octoflow` program and the desktop
//! app's `serve` drive (docs/architecture.md).

/// The engine's version, `[package] version` of engine/Cargo.toml, which is the release's (ci/release-version.sh):
/// what `octoflow --version` prints and the desktop app's About shows.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod acp;
pub mod acp_adapter;
pub mod agents;
pub mod checks;
pub mod cli;
pub mod constants;
pub mod crates_io;
pub mod env;
pub mod error;
pub mod handoff;
pub mod harness;
pub mod herdr;
pub mod host;
pub mod http;
pub mod human;
pub mod interrupt;
pub mod json;
pub mod local_gate;
pub mod old_project_dir;
pub mod package_index;
pub mod patch;
pub mod paths;
pub mod pipeline;
pub mod prompts;
pub mod proofs;
pub mod pull_request;
pub mod pypi;
pub mod quality;
pub mod run_commands;
pub mod runs;
pub mod serve;
pub mod spec_file;
pub mod state;
pub mod summary;
pub mod workflow;
pub mod workflow_file;

#[cfg(test)]
mod prompt_parity;
#[cfg(test)]
mod test_support;
