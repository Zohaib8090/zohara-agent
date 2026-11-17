//! Work on other computers over SSH. The machines are added in Settings > Machines (see `store::machines`). Nothing here goes
//! through a local shell: `ssh` is started with an argument list, and the only string a remote shell reads is the remote command.

use super::{flag, text};
use crate::store::machines::{self, Machine};
use serde_json::Value;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Most output kept from a command, and from a file that is read.
const RUN_LIMIT: usize = 20_000;
const READ_LIMIT: usize = 100_000;
/// Most that can be written to a remote file in one call.
const WRITE_LIMIT: usize = 1_000_000;

/// `s` as one word for a POSIX shell: inside single quotes, a quote itself becomes `'\''`.
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// A remote path as shell text. `~` and `~/…` work (the home folder of the remote user); everything else is quoted.
pub fn remote_path(p: &str) -> Result<String, String> {
    if p.is_empty() || p.contains('\0') {
        return Err("The path is empty or not valid.".into());
    }
    Ok(match p {
        "~" => "\"$HOME\"".to_string(),
        _ if p.starts_with("~/") => format!("\"$HOME\"/{}", quote(&p[2..])),
        _ => quote(p),
    })
}

fn known_hosts() -> PathBuf {
    crate::audit::config_dir().join("known_hosts")
}

/// The ssh arguments for `m`, ending with `--`, the destination and the remote command. No key text is ever in here, only the
/// path of the temporary key file.
pub fn argv(m: &Machine, key_file: &Path, known: &Path, remote_cmd: &str) -> Result<Vec<String>, String> {
    m.validate()?;
    let known = known.to_string_lossy();
    if known.contains(['"', '\n', '\0']) {
        return Err("The settings folder has a character ssh cannot take in its path.".into());
    }
    // ssh reads this value like a config line: a path with a space needs quotes
    let known = if known.contains(char::is_whitespace) { format!("\"{known}\"") } else { known.into_owned() };
    let mut v: Vec<String> = Vec::new();
    for s in [
        "-F",
        "/dev/null", // not the person's ssh config: no ProxyCommand, Include or Host surprises
        "-T",
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=15",
        "-o",
        "StrictHostKeyChecking=accept-new",
        "-o",
        "IdentitiesOnly=yes",
        "-o",
        "IdentityAgent=none",
        "-o",
        "LogLevel=ERROR",
        "-o",
    ] {
        v.push(s.into());
    }
    v.push(format!("UserKnownHostsFile={known}"));
    v.push("-i".into());
    v.push(key_file.to_string_lossy().into_owned());
    v.push("-p".into());
    v.push(m.port.to_string());
    v.push("--".into());
    v.push(format!("{}@{}", m.user, m.host));
    v.push(remote_cmd.to_string());
    Ok(v)
}

/// A private key in a file only its owner can read; deleted when this goes out of scope (also when a run times out or fails).
pub struct TempKey(PathBuf);

impl TempKey {
    pub(crate) fn new(key: &str) -> Result<TempKey, String> {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("zohara-agent");
        std::fs::create_dir_all(&dir).map_err(|e| format!("no place for the key file: {e}"))?;
        let _ = std::fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o700));
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = dir.join(format!("ssh-key-{}-{nanos}-{n}", std::process::id()));
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path).map_err(|e| format!("cannot write the key file: {e}"))?;
        let guard = TempKey(path); // from here on the file is removed even if the write fails
        let mut k = key.trim().to_string();
        k.push('\n');
        f.write_all(k.as_bytes()).map_err(|e| e.to_string())?;
        Ok(guard)
    }
}

impl TempKey {
    pub fn path(&self) -> &Path {
        &self.0
    }
    /// Keeps the file for `secs` more seconds on a thread of its own, then removes it (an interactive ssh reads its key right at the start).
    pub fn remove_later(self, secs: u64) {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(secs));
            drop(self);
        });
    }
}

impl Drop for TempKey {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

pub struct Done {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
    pub cut: bool,
}

/// Reads a pipe to its end but keeps only `limit` bytes; says whether more came.
fn drain(mut r: impl Read + Send + 'static, limit: usize) -> std::thread::JoinHandle<(Vec<u8>, bool)> {
    std::thread::spawn(move || {
        let (mut buf, mut cut, mut chunk) = (Vec::new(), false, [0u8; 8192]);
        while let Ok(n) = r.read(&mut chunk) {
            if n == 0 {
                break;
            }
            let room = limit.saturating_sub(buf.len());
            cut |= n > room;
            buf.extend_from_slice(&chunk[..n.min(room)]);
        }
        (buf, cut)
    })
}

/// Runs `remote_cmd` on the machine, with `input` on its standard input. At most `timeout` seconds; after that the whole process
/// group is killed.
pub fn run_on(name: &str, remote_cmd: &str, input: Option<&[u8]>, timeout: u64, limit: usize) -> Result<Done, String> {
    if remote_cmd.trim().is_empty() || remote_cmd.contains('\0') {
        return Err("No command provided".into());
    }
    let m = machines::get(name).ok_or_else(|| format!("There is no machine called \"{name}\". Use zohara_ssh_machines to see the list, or add it with zohara_ssh_add_machine (that asks the person first)."))?;
    let key = crate::ai::keys::get(&machines::key_id(&m.name)).ok_or("This machine has no private key saved.")?;
    let key_file = TempKey::new(&key)?;
    drop(key);
    let _ = std::fs::create_dir_all(crate::audit::config_dir());
    let args = argv(&m, &key_file.0, &known_hosts(), remote_cmd)?;
    let mut ssh = Command::new("ssh");
    crate::exec::scrub(&mut ssh);
    let mut child = ssh
        .args(&args)
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { "The ssh program is not installed on this computer.".to_string() } else { format!("could not start ssh: {e}") })?;
    let pid = child.id() as i32;
    let out = drain(child.stdout.take().ok_or("no stdout")?, limit);
    let err = drain(child.stderr.take().ok_or("no stderr")?, limit);
    let writer = input.and_then(|data| {
        let mut stdin = child.stdin.take()?;
        let data = data.to_vec();
        Some(std::thread::spawn(move || {
            let _ = stdin.write_all(&data); // a broken pipe just means the other side stopped listening
        }))
    });

    let start = Instant::now();
    let limit_t = Duration::from_secs(timeout.clamp(1, 600));
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let _activity = crate::activity::start(crate::activity::Kind::Command, remote_cmd, &format!("on {name}"), "desktop", None, Some(cancel.clone()));
    let status = loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(s) => break s,
            None if cancel.load(std::sync::atomic::Ordering::Relaxed) => {
                unsafe { libc::kill(-pid, libc::SIGKILL) };
                let _ = child.kill();
                let _ = child.wait();
                drop(key_file);
                return Err(format!("The command on {name} was stopped."));
            }
            None if start.elapsed() >= limit_t => {
                unsafe { libc::kill(-pid, libc::SIGKILL) };
                let _ = child.kill();
                let _ = child.wait();
                drop(key_file);
                return Err(format!("timed out after {} s on {name}; the command was stopped", limit_t.as_secs()));
            }
            None => std::thread::sleep(Duration::from_millis(15)),
        }
    };
    drop(key_file);
    if let Some(w) = writer {
        let _ = w.join();
    }
    let (o, c1) = out.join().unwrap_or_default();
    let (e, c2) = err.join().unwrap_or_default();
    Ok(Done {
        stdout: String::from_utf8_lossy(&o).into_owned(),
        stderr: String::from_utf8_lossy(&e).into_owned(),
        code: status.code().unwrap_or(-1),
        cut: c1 || c2,
    })
}

/// 255 is ssh's own failure (cannot connect, key refused, host key changed): a clean error instead of a command result.
pub(crate) fn failure(d: &Done, name: &str) -> Option<String> {
    (d.code == 255).then(|| {
        let why = d.stderr.trim();
        format!("Could not use {name} over SSH: {}", if why.is_empty() { "ssh failed" } else { why })
    })
}

pub(crate) fn report(d: &Done) -> String {
    let mut s = d.stdout.trim_end().to_string();
    if !d.stderr.trim().is_empty() {
        if !s.is_empty() {
            s.push_str("\n[stderr]\n");
        }
        s.push_str(d.stderr.trim_end());
    }
    if s.is_empty() {
        s.push_str("Command completed (no output)");
    }
    if d.cut {
        s.push_str("\n[output cut, it was longer than what is kept]");
    }
    if d.code != 0 {
        s.push_str(&format!("\n[exit code {}]", d.code));
    }
    s
}

pub fn list() -> Result<String, String> {
    let all = machines::load();
    if all.is_empty() {
        return Ok("No machines are added. The person can add one in Settings > Machines.".into());
    }
    Ok(all
        .iter()
        .map(|m| {
            let note = if m.note.is_empty() { String::new() } else { format!(" ({})", m.note) };
            format!("{}: {}@{}:{}{}{}", m.name, m.user, m.host, m.port, note, if m.has_key() { "" } else { " [no key saved]" })
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

pub fn run(a: &Value) -> Result<String, String> {
    let timeout = a.get("timeout_s").and_then(Value::as_u64).unwrap_or(120);
    let name = text(a, "machine")?;
    let d = run_on(name, text(a, "command")?, None, timeout, RUN_LIMIT)?;
    failure(&d, name).map_or_else(|| Ok(report(&d)), Err)
}

pub fn read_file(a: &Value) -> Result<String, String> {
    let name = text(a, "machine")?;
    let d = run_on(name, &format!("cat -- {}", remote_path(text(a, "path")?)?), None, 60, READ_LIMIT)?;
    if let Some(e) = failure(&d, name) {
        return Err(e);
    }
    if d.code != 0 {
        return Err(d.stderr.trim().to_string());
    }
    Ok(report(&d))
}

pub fn write_file(a: &Value) -> Result<String, String> {
    let name = text(a, "machine")?;
    let content = text(a, "content")?;
    if content.len() > WRITE_LIMIT {
        return Err("That is too much to write in one go (1 MB at most).".into());
    }
    let path = text(a, "path")?;
    let op = if flag(a, "append") { ">>" } else { ">" };
    let d = run_on(name, &format!("cat {op} {}", remote_path(path)?), Some(content.as_bytes()), 60, 4_000)?;
    if let Some(e) = failure(&d, name) {
        return Err(e);
    }
    if d.code != 0 {
        return Err(d.stderr.trim().to_string());
    }
    Ok(format!("Wrote {} bytes to {path} on {name}", content.len()))
}

/// The settings page's Test button: runs `echo ok` and says what came back.
pub fn test(name: &str) -> Result<String, String> {
    let d = run_on(name, "echo ok", None, 30, 2_000)?;
    if let Some(e) = failure(&d, name) {
        return Err(e);
    }
    if d.code == 0 && d.stdout.trim() == "ok" {
        Ok("Connected: the machine answered.".into())
    } else {
        Err(report(&d))
    }
}


// ── Keys made for Zohara Agent, and an interactive terminal ──

/// A throw-away folder (`0700`) that is removed when this goes away.
struct TempDir(PathBuf);

impl TempDir {
    fn new(prefix: &str) -> Result<TempDir, String> {
        use std::os::unix::fs::DirBuilderExt;
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let base = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("zohara-agent");
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        let dir = base.join(format!("{prefix}-{}-{nanos}-{n}", std::process::id()));
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir).map_err(|e| format!("no place for a private file: {e}"))?;
        Ok(TempDir(dir))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A new ed25519 key pair without a passphrase, made with the system's `ssh-keygen`: `(private key text, public key line)`. The files
/// exist only for a moment, in a `0700` folder, and are removed at once.
pub fn generate_key(comment: &str) -> Result<(String, String), String> {
    if comment.is_empty() || !comment.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '@')) {
        return Err("The key comment has a character that is not allowed.".into());
    }
    let dir = TempDir::new("keygen")?;
    let file = dir.0.join("key");
    let mut cmd = Command::new("ssh-keygen");
    crate::exec::scrub(&mut cmd);
    let out = cmd
        .args(["-q", "-t", "ed25519", "-N", "", "-C", comment, "-f"])
        .arg(&file)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { "ssh-keygen is not installed on this computer.".to_string() } else { e.to_string() })?;
    if !out.status.success() {
        return Err(format!("ssh-keygen failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let private = std::fs::read_to_string(&file).map_err(|e| e.to_string())?;
    let public = std::fs::read_to_string(dir.0.join("key.pub")).map_err(|e| e.to_string())?.trim().to_string();
    Ok((private, public))
}

/// The public key line that belongs to the private key saved for machine `name` (made with `ssh-keygen -y`).
pub fn public_of(name: &str) -> Result<String, String> {
    let key = crate::ai::keys::get(&machines::key_id(name)).ok_or("This machine has no private key saved.")?;
    let key_file = TempKey::new(&key)?;
    drop(key);
    let mut cmd = Command::new("ssh-keygen");
    crate::exec::scrub(&mut cmd);
    let out = cmd.args(["-y", "-f"]).arg(key_file.path()).stdin(Stdio::null()).output().map_err(|e| e.to_string())?;
    drop(key_file);
    if !out.status.success() {
        return Err("The saved key could not be read.".into());
    }
    clean_public_key(String::from_utf8_lossy(&out.stdout).trim())
}

/// A public key line is `ssh-ed25519 AAAA… comment`; anything with other characters is never put in a command.
pub fn clean_public_key(line: &str) -> Result<String, String> {
    let t = line.trim();
    let ok = t.len() < 1000
        && (t.starts_with("ssh-ed25519 ") || t.starts_with("ssh-rsa ") || t.starts_with("ecdsa-sha2-"))
        && t.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '+' | '/' | '=' | '@' | '.' | '_' | '-'));
    if ok { Ok(t.to_string()) } else { Err("That is not a public key line.".into()) }
}

/// The one line the person pastes on the machine to let Zohara Agent in.
pub fn authorize_command(public: &str) -> Result<String, String> {
    let p = clean_public_key(public)?;
    Ok(format!("mkdir -p ~/.ssh && chmod 700 ~/.ssh && echo '{p}' >> ~/.ssh/authorized_keys && chmod 600 ~/.ssh/authorized_keys"))
}

/// Puts a public key into `authorized_keys` of machine `name`, logging in ONCE with a password the person typed. The password goes to
/// ssh through a throw-away `SSH_ASKPASS` helper in a `0700` folder (never an argument, never an environment variable, never stored,
/// never in a log) and the folder is removed when ssh ends.
pub fn authorize_with_password(name: &str, password: &str, public: &str) -> Result<String, String> {
    let m = machines::get(name).ok_or_else(|| format!("There is no machine called \"{name}\"."))?;
    m.validate()?;
    if password.is_empty() || password.len() > 512 || password.contains('\0') || password.contains('\n') {
        return Err("The password is empty or not valid.".into());
    }
    let remote = authorize_command(public)?;
    let dir = TempDir::new("askpass")?;
    let (pw, script) = (dir.0.join("pw"), dir.0.join("askpass.sh"));
    for (path, body, mode) in [(&pw, password.to_string(), 0o600), (&script, format!("#!/bin/sh\nexec cat '{}'\n", pw.to_string_lossy().replace('\'', "")), 0o700)] {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(mode).open(path).map_err(|e| e.to_string())?;
        f.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
    }
    let _ = std::fs::create_dir_all(crate::audit::config_dir());
    let known = known_hosts();
    let known = known.to_string_lossy();
    if known.contains(['"', '\n', '\0']) || known.contains(char::is_whitespace) {
        return Err("The settings folder has a character ssh cannot take in its path.".into());
    }
    let mut c = Command::new("ssh");
    crate::exec::scrub(&mut c);
    c.args(["-F", "/dev/null", "-T", "-o", "BatchMode=no", "-o", "ConnectTimeout=15", "-o", "StrictHostKeyChecking=accept-new", "-o", "PubkeyAuthentication=no"])
        .args(["-o", "PreferredAuthentications=password,keyboard-interactive", "-o", "NumberOfPasswordPrompts=1", "-o", "LogLevel=ERROR"])
        .arg("-o")
        .arg(format!("UserKnownHostsFile={known}"))
        .args(["-p", &m.port.to_string(), "--"])
        .arg(format!("{}@{}", m.user, m.host))
        .arg(remote)
        .env("SSH_ASKPASS", &script)
        .env("SSH_ASKPASS_REQUIRE", "force")
        .env("DISPLAY", "zohara-agent:0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let child = c.spawn().map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { "The ssh program is not installed on this computer.".to_string() } else { e.to_string() })?;
    let pid = child.id() as i32;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    let out = match rx.recv_timeout(Duration::from_secs(45)) {
        Ok(r) => r.map_err(|e| e.to_string())?,
        Err(_) => {
            unsafe { libc::kill(-pid, libc::SIGKILL) };
            return Err("The machine did not answer in time.".into());
        }
    };
    if out.status.success() {
        Ok("The key is in place: Zohara Agent can log in without the password now. The password was not kept.".into())
    } else {
        let why = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(format!("Could not log in with the password: {}", if why.is_empty() { "ssh failed" } else { why.as_str() }))
    }
}

/// The arguments that open an interactive ssh session to `name`, and the key file it reads (ssh reads it at the start; the caller keeps
/// it a short while). Used for a terminal tab and for the web terminal.
pub fn terminal_command(name: &str) -> Result<(Vec<String>, TempKey), String> {
    let m = machines::get(name).ok_or_else(|| format!("There is no machine called \"{name}\"."))?;
    let key = crate::ai::keys::get(&machines::key_id(&m.name)).ok_or("This machine has no private key saved.")?;
    let key_file = TempKey::new(&key)?;
    drop(key);
    let _ = std::fs::create_dir_all(crate::audit::config_dir());
    let mut v = argv(&m, key_file.path(), &known_hosts(), "")?;
    v.pop(); // no remote command: a login shell
    if let Some(t) = v.iter_mut().find(|a| *a == "-T") {
        *t = "-tt".into();
    }
    Ok((v, key_file))
}

/// `terminal_command` as one line for a local shell (every word quoted), for a terminal tab that starts programs through `$SHELL -c`.
pub fn terminal_shell_line(name: &str) -> Result<String, String> {
    let (args, key) = terminal_command(name)?;
    key.remove_later(60);
    Ok(format!("exec ssh {}", args.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ")))
}

/// The tool `zohara_ssh_add_machine`: adds a machine with a NEW key made for Zohara Agent (the private key goes to the key store and is
/// never shown), and says what the person has to put on the machine. It takes no password: a password typed into a chat would be in
/// the model's context, so a password login is only offered in the Add machine dialog of the app.
pub fn add_machine(a: &Value) -> Result<String, String> {
    let name = text(a, "name")?.trim();
    let port = match a.get("port") {
        Some(Value::Number(n)) => n.as_u64().filter(|p| (1..=65535).contains(p)).ok_or("The port must be a number from 1 to 65535.")? as u16,
        Some(Value::String(t)) => machines::parse_port(t)?,
        _ => 22,
    };
    let m = Machine { name: name.into(), host: text(a, "host")?.trim().into(), user: text(a, "user")?.trim().into(), port, note: a.get("note").and_then(Value::as_str).unwrap_or("").trim().into() };
    m.validate()?;
    if machines::get(&m.name).is_some() {
        return Err(format!("A machine called \"{}\" is added already. Use zohara_ssh_test to check it.", m.name));
    }
    let (private, public) = generate_key(&format!("zohara-agent-{}", m.name))?;
    machines::add(m.clone(), Some(&private))?;
    drop(private);
    let cmd = authorize_command(&public)?;
    let tested = match test(&m.name) {
        Ok(msg) => format!("Test: {msg} The machine already lets Zohara Agent in."),
        Err(_) => "Test: it does not let Zohara Agent in yet (expected: the key is new).".to_string(),
    };
    Ok(format!(
        "Added the machine \"{}\" ({}@{}:{}). Zohara Agent made a new key for it; the private key stays in Zohara Agent's key store.\n\
         To let Zohara Agent in, the person runs this one line ON the machine (for example in the provider's web console):\n{cmd}\n\
         The public key is: {public}\n{tested}\n\
         When they have done it, call zohara_ssh_test with machine \"{}\".",
        m.name, m.user, m.host, m.port, m.name
    ))
}

/// The tool `zohara_ssh_test`.
pub fn test_tool(a: &Value) -> Result<String, String> {
    test(text(a, "machine")?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{decide, Decision, Mode, Risk};

    fn machine() -> Machine {
        Machine { name: "vm".into(), host: "10.0.0.5".into(), user: "ubuntu".into(), port: 2200, note: String::new() }
    }

    #[test]
    fn quote_handles_hard_text() {
        assert_eq!(quote("a b"), "'a b'");
        assert_eq!(quote("it's"), "'it'\\''s'");
        assert_eq!(quote(""), "''");
        assert_eq!(quote("$(rm -rf /); `x` \"y\" \n"), "'$(rm -rf /); `x` \"y\" \n'");
        assert_eq!(remote_path("~/my notes/a'b").unwrap(), "\"$HOME\"/'my notes/a'\\''b'");
        assert_eq!(remote_path("~").unwrap(), "\"$HOME\"");
        assert_eq!(remote_path("/etc/hosts").unwrap(), "'/etc/hosts'");
        assert!(remote_path("").is_err() && remote_path("a\0b").is_err());
    }

    #[test]
    fn quoted_text_reaches_a_real_shell_unchanged() {
        for s in ["a b", "it's", "$(echo pwned)", "`echo pwned`", "x; echo pwned", "\"q\"", "a\nb", "-n", "*", "\\"] {
            let out = Command::new("sh").arg("-c").arg(format!("printf %s {}", quote(s))).output().unwrap();
            assert_eq!(String::from_utf8_lossy(&out.stdout), s);
        }
    }

    #[test]
    fn argv_is_exact_and_safe() {
        let v = argv(&machine(), Path::new("/run/zohara-agent/key1"), Path::new("/home/z/.config/zohara-agent/known_hosts"), "uptime").unwrap();
        let s = v.join(" ");
        assert_eq!(
            s,
            "-F /dev/null -T -o BatchMode=yes -o ConnectTimeout=15 -o StrictHostKeyChecking=accept-new -o IdentitiesOnly=yes -o IdentityAgent=none \
             -o LogLevel=ERROR -o UserKnownHostsFile=/home/z/.config/zohara-agent/known_hosts -i /run/zohara-agent/key1 -p 2200 -- ubuntu@10.0.0.5 uptime"
        );
        // the destination comes right after `--`, then the remote command and nothing else
        let dd = v.iter().position(|x| x == "--").unwrap();
        assert_eq!(v.len() - dd, 3);
        // a command that looks like an option stays a command
        let v = argv(&machine(), Path::new("/k"), Path::new("/kh"), "-oProxyCommand=x").unwrap();
        assert_eq!(v.last().unwrap(), "-oProxyCommand=x");
        assert_eq!(v[v.iter().position(|x| x == "--").unwrap() + 1], "ubuntu@10.0.0.5");
    }

    #[test]
    fn argv_refuses_a_machine_that_could_inject_an_option() {
        for (host, user) in [("-oProxyCommand=x", "u"), ("h", "-oProxyCommand=x"), ("a b", "u"), ("h", "a;b")] {
            let m = Machine { host: host.into(), user: user.into(), ..machine() };
            assert!(argv(&m, Path::new("/k"), Path::new("/kh"), "id").is_err(), "{host} {user}");
        }
        let m = Machine { name: "-x".into(), ..machine() };
        assert!(argv(&m, Path::new("/k"), Path::new("/kh"), "id").is_err());
    }

    #[test]
    fn argv_quotes_a_known_hosts_path_with_a_space() {
        let v = argv(&machine(), Path::new("/k"), Path::new("/home/a b/known_hosts"), "id").unwrap();
        assert!(v.contains(&"UserKnownHostsFile=\"/home/a b/known_hosts\"".to_string()));
        assert!(argv(&machine(), Path::new("/k"), Path::new("/home/a\"b/known_hosts"), "id").is_err());
    }

    #[test]
    fn temp_key_is_private_holds_the_key_and_goes_away() {
        let _g = crate::store::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("zo-ssh-key-test-{}", std::process::id()));
        std::env::set_var("XDG_RUNTIME_DIR", &dir);
        let path;
        {
            let k = TempKey::new("-----BEGIN X PRIVATE KEY-----\nabc\n-----END X PRIVATE KEY-----").unwrap();
            path = k.0.clone();
            let mode = std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&path).unwrap().permissions());
            assert_eq!(mode & 0o777, 0o600);
            assert!(std::fs::read_to_string(&path).unwrap().ends_with("END X PRIVATE KEY-----\n"));
            assert!(TempKey::new("k").unwrap().0 != path, "every key file has its own name");
        }
        assert!(!path.exists(), "removed when done");
        std::env::remove_var("XDG_RUNTIME_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_machine_and_missing_key_are_clean_errors() {
        let _g = crate::store::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("zo-ssh-none-test-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let e = run(&serde_json::json!({"machine": "nope", "command": "id"})).unwrap_err();
        assert!(e.contains("no machine called"));
        assert!(run(&serde_json::json!({"machine": "nope"})).is_err());
        assert!(list().unwrap().contains("No machines"));
        std::env::remove_var("XDG_CONFIG_HOME");
    }

    #[test]
    fn permissions_follow_the_shell_and_file_tools() {
        let defs = super::super::definitions();
        let risk = |n: &str| defs.iter().find(|d| d.name == n).unwrap().risk;
        assert_eq!(risk("zohara_ssh_machines"), Risk::Read);
        assert_eq!(risk("zohara_ssh_read_file"), Risk::Read);
        assert_eq!(risk("zohara_ssh_write_file"), Risk::Change);
        assert_eq!(risk("zohara_ssh_run"), Risk::Shell);
        // zohara_ssh_run has no "cmd" argument, so the policy sees an empty command: it asks, except in Bypass
        assert_eq!(decide(Mode::Auto, Risk::Shell, Some("")), Decision::Ask);
        assert_eq!(decide(Mode::Manual, Risk::Shell, Some("")), Decision::Ask);
        assert_eq!(decide(Mode::Bypass, Risk::Shell, Some("")), Decision::Allow);
        assert_eq!(decide(Mode::Auto, Risk::Change, None), Decision::Ask);
    }

    #[test]
    fn reports() {
        let d = |o: &str, e: &str, code: i32| Done { stdout: o.into(), stderr: e.into(), code, cut: false };
        assert_eq!(report(&d("hi\n", "", 0)), "hi");
        assert_eq!(report(&d("", "", 0)), "Command completed (no output)");
        assert_eq!(report(&d("a", "b", 3)), "a\n[stderr]\nb\n[exit code 3]");
        assert!(failure(&d("", "Permission denied (publickey).", 255), "vm").unwrap().contains("Permission denied"));
        assert!(failure(&d("", "", 1), "vm").is_none());
    }
}
