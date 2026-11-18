//! `cargo xtask <command> [ARGS...]`: the repository's chores in Rust (the xtask library). The commands are `eval`,
//! the eval set's runner, `bundles`, which builds its tasks' bundles, and `raise-gate-pin`, which moves ci/gate.pin to
//! another octoflow-gate release; each says its arguments and exit status.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let status = match args.split_first() {
        Some((name, rest)) if name == "eval" => xtask::eval::main(rest),
        Some((name, rest)) if name == "bundles" => xtask::bundles::main(rest),
        Some((name, rest)) if name == "raise-gate-pin" => xtask::raise_gate_pin::main(rest),
        _ => {
            eprintln!("usage: cargo xtask <command> [ARGS...], the command eval, bundles or raise-gate-pin");
            2
        }
    };
    std::process::exit(status);
}
