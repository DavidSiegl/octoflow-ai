//! RunCommands: how a hint words a command for the human to type about one of a project's runs. Its own module, since
//! cli and the Workflow both word such hints, and the Workflow cannot ask cli.

use crate::constants::PROGRAM;
use crate::host::shlex_join;
use crate::state::run_key;

/// How a hint words a command for the human to type about one of a project's runs: as the human invokes the program,
/// in the project the human named. The default is PROGRAM in the current directory, with no flags.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunCommands {
    /// The program's name as the human invokes it (cli::program_name).
    program: String,
    /// The --machine and --cwd flags that name the project, as the human gave them (cli::Target::flags); empty for
    /// this machine's current directory.
    target: Vec<String>,
}

impl RunCommands {
    /// The commands of program, in the project the target flags name (cli::Target::flags).
    pub fn new(program: &str, target: &[String]) -> RunCommands {
        RunCommands { program: program.to_string(), target: target.to_vec() }
    }

    /// The command that runs subcommand verb ("resume", "close", ...) on the run of run_id, named by its key
    /// (run_key), as one line of sh: `<program> <verb> <key> <target flags>`, each word quoted as sh needs.
    pub fn on_run(&self, verb: &str, run_id: &str) -> String {
        let argv: Vec<&str> = [self.program.as_str(), verb, run_key(run_id)]
            .into_iter()
            .chain(self.target.iter().map(String::as_str))
            .collect();
        shlex_join(&argv)
    }
}

impl Default for RunCommands {
    fn default() -> RunCommands {
        RunCommands::new(PROGRAM, &[])
    }
}
