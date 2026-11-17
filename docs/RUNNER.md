# The runner for Linux VMs (`zohara-agent-runner`)

Scheduled tasks normally run in the desktop app, so they stop when the computer is off. The runner is a small headless program that is
copied to a Linux machine you added (see docs/SSH.md). It keeps its own copy of the task list on that machine and runs the tasks there on
time, whether or not your computer is on. It is specified in docs/VM-MIGRATION-PLAN.md, sections 2, 4, 5 and 6.

## What you see, in every app

The same screens exist in the desktop app, on the web page and in the Android app (owner or administrator only; a restricted account gets
none of this, the service refuses every `machine_*` operation for it):

- A **Machines** pill next to the model pill on the chat screen opens a popup with every machine: a status dot (green: reachable and the
  runner is installed; amber: reachable, no runner; red: not reachable; grey: not checked yet), **Test**, **Terminal**, **Manage...** and
  **Add machine**. Settings > Machines is the full page with the same list.
- **Terminal**: on the desktop a terminal tab in the window that is an ssh session to the machine; on the web the terminal panel of the
  cloud server, started as an ssh session (administrator only: a shell that starts connections from the server's address is not for
  everyone); on Android the terminal page (the existing one-command box of the cloud session, administrator only).
- **Add machine** is a wizard: name, address, user, port, then Zohara Agent makes a NEW ed25519 key for the machine (the private half goes
  to the key store, never shown), shows the public line and the one command that puts it in `authorized_keys`, and has a **Test connection**
  button. If the machine accepts a password, "Use it once" puts the key there with a one-time password that is used for that one `ssh` run and
  never stored or logged. "Use my own key" keeps the old way (paste a private key without a passphrase).
- **Manage...** per machine: Install runner / Update / Remove runner, two switches ("Let tasks run commands there", "Let tasks write files
  there"), **Copy model key**, **Copy connected apps**, Check, Sync, Revoke on machine, **Run a task here**, Remove machine.

## Install

"Install runner" (service op `machine_runner_install`) does this over SSH, and tells you each step:

1. Checks the machine: Linux, the same kind of processor as this computer (an x86_64 laptop cannot install on an arm64 machine; it says so),
   systemd for the user.
2. Copies the program to `~/.local/bin/zohara-agent-runner` through `ssh` (the bytes go on standard input), then runs `sha256sum` there and
   compares it with the checksum made here. A different checksum removes the copy and stops.
3. Runs it once (`version`) to see that it starts on that system.
4. `install-units`: writes `~/.config/systemd/user/zohara-agent-runner.service` and `.timer` (`OnCalendar=minutely`, `Persistent=true`) and enables
   the timer. Each wake-up runs `zohara-agent-runner run-due`. (`loginctl enable-linger <user>` is needed on the machine for the timer to
   run when nobody is logged in; the install says so when it is not set.)
5. Sends the tasks whose "Run on" is this machine.

The program is found next to the desktop app (`zohara-agent-runner` in the same folder), in `/usr/lib/zohara-agent/`, `/usr/bin/` or
`/usr/local/bin/`, or at `ZOHARA_RUNNER_BINARY`. It is built from the same source as the app, so the two always speak the same protocol.

**Remove runner** fetches what the runner did, stops and disables the timer, deletes its folder (its tasks, key file and copied apps) and the
program, and the machine's tasks go back to running from this computer.

## What the runner keeps, and where

Everything is in `~/.local/share/zohara-agent-runner` (or `$ZOHARA_RUNNER_HOME`), mode 0700, separate from any Zohara Agent that may also
be installed there: `schedules.json` (its task list), `allow.json`, `keys.json` (0600), `cfg/zohara-agent/mcp-servers.json` (0600, the copied connected apps),
`results.jsonl` (the last 200 results until your computer has fetched them).

## How a task runs there

`run-due` runs every task whose time has come, with the same agent loop as the desktop, and no window. Rules:

- Approvals are never skipped. Nobody is there to answer, so a tool that would ask is **refused**, except the ones you named for that machine
  with the two switches: "run commands" allows `zohara_shell`, "write files" allows `zohara_fs_write`, `zohara_fs_mkdir` and adding memory.
  Removing files, stopping processes and everything else is never on that list.
- A missed run (the machine was off) runs once at the next start, marked "missed".
- It needs a model there: **Copy model key** sends the chosen model and its API key (a local model needs no key; Claude Code cannot be copied,
  it needs its own login on the machine). Without one the task is marked "skipped" with the reason.
- Results are written to the local log, and your computer fetches them (`machine_runner_sync`, every few minutes and when you open the
  popup): each becomes a chat titled "Scheduled on <machine>: <task>" and a notification, like a desktop run. When the server is reachable
  from the machine the task list is not needed there again until it changes.
- "Run a task here" makes a one-time task due now and wakes the runner at once (`setsid ... run-due`). Without a runner on the machine the
  same task runs from this computer through the SSH tools and asks as always (phase 1 of the plan).

## Connected apps (MCP) on a machine

**Copy connected apps** (`machine_apps_plan`, `machine_apps_copy`, `machine_apps_check`, `machine_apps_sync`, `machine_apps_revoke`):

- Without credentials only the definition goes: name, address or command, and the **names** of the variables and headers it needs, with
  empty values.
- With credentials (API keys, header values, a saved sign-in, a client secret): per app, and only after you tick a switch that shows the
  exact sentence, `This copies the <apps> credentials to <machine>.` The page sends that sentence back unchanged and the service refuses
  anything else, so a script or a click-through cannot copy a secret without the named sentence. The files on the machine are mode 0600.
- An app signed in with a login (OAuth) is a second live copy once copied. The dialog warns, **Revoke on machine** deletes it there, and
  revoking at the provider ends both. Apps that start a program by a path on this computer are flagged.
- **Check** asks the machine whether the programs the apps start exist (`npx`, `uvx`, `python`...) and says what to install; nothing is
  installed for you.
- **Sync** brings the machine's copy in step: changed apps are sent again (credentials need the sentence again, because they travel again),
  removed apps are removed there. It is manual.
- Nothing goes through the cloud server: the copy goes from the computer that has the credentials straight to the machine. On the web page the
  copy is made by the cloud server (administrator only, and only credentials that the administrator stored there) and still goes straight
  to the machine.

## "AI, add a machine for me"

The tool `zohara_ssh_add_machine` (asks first unless the mode is Bypass, never offered to a restricted account) takes a name, host, user and port (the
AI asks for them with the question card), makes a new key, and answers with the public line and the one command for the machine; then
`zohara_ssh_test` checks it. It takes no password: a password typed in a chat would be in the model's context. A one-time password is only
offered in the wizard of the app. If a task needs a machine that is not there, the AI says so and offers this instead of failing.

## Service operations

`machine_status`, `machine_new`, `machine_pubkey`, `machine_authorize`, `machine_runner_install`, `machine_runner_uninstall`,
`machine_runner_allow`, `machine_runner_sync`, `machine_model_plan`, `machine_model_copy`, `machine_model_clear`, `machine_apps_plan`,
`machine_apps_copy`, `machine_apps_revoke`, `machine_apps_check`, `machine_apps_sync`, `machine_task_run`, `machine_terminal` (desktop),
`machine_terminal_spec` (the cloud server's terminal). Plus the older `machines`, `machine_add`, `machine_remove`, `machine_test`.

## Security

- Private keys and credentials go from the computer that holds them to the machine, over SSH, and never to the cloud server unless the
  person turned on key sync. The audit log records that a copy happened, to which machine and for which apps, never a value.
- Every credential copy needs the named sentence; model keys too (`This copies your <provider> API key to <machine>.`).
- Files on the machine: 0600 (the folder 0700). Revoke buttons delete them there.
- SSH from the cloud server (terminal and every machine operation there) is for the administrator only. A restricted account cannot reach
  any of it: `restricted_refusal` refuses every operation whose name starts with `machine`, and the handler checks again.
- The runner reads secrets only from standard input, never from an argument, and prints none of them.

## Tests

`crates/zo-runner/tests/vm.rs` starts a REAL `sshd` on a random port as the current user, with its own `HOME` for the "machine" (`SetEnv`),
no systemd (the unit files are written to a folder and checked) and a tiny fake model server, and drives install, checksum, status, tasks,
model copy, the confirmation sentence, app copy and revoke, and uninstall against it. They are skipped (and say so) without `sshd`.
`crates/zo-server/src/tests.rs` has the cloud terminal as an ssh session and the restricted-account refusals. Run:
`cargo test -p zo-runner`, `cargo test -p zo-core runner`, `cargo test -p zo-server machine`. No real remote machine is used.
