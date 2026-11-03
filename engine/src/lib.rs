//! The orchestrator's engine: the Rust port of the Python engine (the rust-engine spec), which reads and writes its
//! files, prompts and run state in Rust's own terms since the octoflow-without-python spec (docs/architecture.md).

pub mod ci;
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
