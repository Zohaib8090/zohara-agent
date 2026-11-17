//! The headless runner for Linux VMs (docs/RUNNER.md, docs/VM-MIGRATION-PLAN.md sections 2 and 4).
//!
//! * `remote`: what runs ON the VM (the `zohara-agent-runner` program): its own task list, a model and key, copied connected apps,
//!   `run-due` for the systemd timer, and the small JSON commands the owner's computer drives over SSH.
//! * `host`: what runs on the owner's computer (or on the cloud server, for the admin): install and remove the runner over SSH, check
//!   its status, push the task list and the model, pull what the runner did.
//! * `apps`: copying connected apps (MCP servers) and, only after a named confirmation, their credentials to a VM.
//!
//! The VM never gets anything it was not sent over the SSH channel, and nothing here goes through the cloud server.

pub mod apps;
pub mod host;
pub mod remote;

use sha2::{Digest, Sha256};

/// The version of the runner (the same as this crate: the host and the runner are built together).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Where the program is put on the VM, relative to the home folder there.
pub const BIN_REL: &str = ".local/bin/zohara-agent-runner";

/// The lowercase hex SHA-256 of `data`.
pub fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// The most JSON the runner reads from standard input in one command.
pub const MAX_STDIN: usize = 8 * 1024 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_the_known_answer() {
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
