//! `zohara-agent-runner`: the headless runner for a Linux VM. All the work is in `zo_core::runner::remote`; see docs/RUNNER.md.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(zo_core::runner::remote::main(&args));
}
