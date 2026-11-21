#!/bin/sh
# Write the shell registration scripts the .deb installs (packaging/build-desktop.sh maps them to the distribution's
# vendor completion paths) as the octoflow built from this checkout prints them: octoflow for bash, _octoflow for zsh
# and octoflow.fish for fish, in this directory. Run it after a change to clap_complete's pin or to how
# cli::complete::answer_shell registers; engine/tests/program.rs fails while a committed script differs. Exits 0
# having written all three; exits 1 when cargo cannot build or run the program.
#
#   packaging/completions/generate.sh
#
# The scripts call `octoflow` as PATH finds it. The .deb ships them with its /usr/bin/octoflow, from the same
# exact-pinned clap_complete, so the script-to-binary protocol, which clap_complete keeps unstable, matches for that
# binary; another octoflow earlier on PATH, of another version, answers in its place and may offer nothing.
set -eu

cd "$(dirname "$0")/../.."
dir=packaging/completions

# The registration script of shell, as `OCTOFLOW_COMPLETE=<shell> octoflow` prints it.
registration() {
    OCTOFLOW_COMPLETE="$1" cargo run --locked -q -p octoflow-engine --bin octoflow \
        || { echo "generate.sh: octoflow did not print the $1 script" >&2; exit 1; }
}

bash=$(registration bash)
zsh=$(registration zsh)
fish=$(registration fish)
# Command substitution drops the trailing newlines, which the printf below puts back as one.
printf '%s\n' "$bash" > "$dir/octoflow"
# zsh autoloads _octoflow from fpath as the body of the function _octoflow, which compinit calls on a shell's first
# Tab at octoflow. clap_complete's script only defines _clap_dynamic_completer_octoflow and hands octoflow to it with
# compdef, so that first Tab would offer nothing; the appended call answers it, and compdef's binding every Tab after.
printf '%s\n%s\n' "$zsh" '_clap_dynamic_completer_octoflow "$@"' > "$dir/_octoflow"
printf '%s\n' "$fish" > "$dir/octoflow.fish"
