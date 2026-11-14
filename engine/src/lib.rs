//! The orchestrator's engine: the workflows, run state, agents and checks the `orchestrator` program and the desktop
//! app's `serve` drive (docs/architecture.md).

pub mod cli;
pub mod checks;
pub mod constants;
pub mod crates_io;
pub mod env;
pub mod error;
pub mod handoff;
pub mod harness;
pub mod herdr;
pub mod host;
pub mod http;
pub mod interrupt;
pub mod json;
pub mod local_gate;
pub mod map;
pub mod package_index;
pub mod patch;
pub mod paths;
pub mod pipeline;
pub mod proofs;
pub mod prompts;
pub mod pull_request;
pub mod pypi;
pub mod quality;
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
