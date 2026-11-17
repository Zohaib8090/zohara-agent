//! The runner and the copy flows against a REAL sshd on this computer (docs/RUNNER.md): a private sshd on a random port, as the current
//! user, with its own `HOME` for the "machine" (set with `SetEnv`), no systemd (the unit files are written to a folder and checked),
//! and a tiny fake model server on 127.0.0.1. Nothing outside the temporary folder is touched. The tests are skipped (and say so) when
//! `sshd` or `ssh-keygen` is not installed.

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use zo_core::runner::{apps, host};
use zo_core::store::{machines, schedules};

static LOCK: Mutex<()> = Mutex::new(());

fn have(tool: &str) -> bool {
    Command::new(tool).arg("-V").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok() || Path::new("/usr/bin").join(tool).exists()
}

struct Rig {
    dir: PathBuf,
    sshd: Child,
    vmhome: PathBuf,
    units: PathBuf,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = self.sshd.kill();
        let _ = self.sshd.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn me() -> String {
    String::from_utf8(Command::new("id").arg("-un").output().unwrap().stdout).unwrap().trim().to_string()
}

fn sh(dir: &Path, args: &[&str]) {
    let st = Command::new(args[0]).args(&args[1..]).current_dir(dir).stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap();
    assert!(st.success(), "{args:?}");
}

/// Starts the "VM" and points this process at fresh settings. None when the tools are missing.
fn rig(name: &str) -> Option<Rig> {
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if !(Path::new("/usr/bin/sshd").exists() || Path::new("/usr/sbin/sshd").exists()) || !have("ssh-keygen") || !have("ssh") {
        eprintln!("SKIPPED {name}: sshd, ssh or ssh-keygen is not installed");
        return None;
    }
    let sshd_bin = if Path::new("/usr/bin/sshd").exists() { "/usr/bin/sshd" } else { "/usr/sbin/sshd" };
    let dir = std::env::temp_dir().join(format!("zo-vm-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (vmhome, units, srv) = (dir.join("vmhome"), dir.join("units"), dir.join("sshd"));
    for d in [&vmhome, &srv, &dir.join("host/cfg"), &dir.join("host/data"), &dir.join("run")] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::set_permissions(dir.join("run"), std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    sh(&srv, &["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", "hostkey"]);
    sh(&srv, &["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", "client"]);
    std::fs::copy(srv.join("client.pub"), srv.join("authorized_keys")).unwrap();
    std::fs::set_permissions(srv.join("authorized_keys"), std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    let port = free_port();
    std::fs::write(
        srv.join("sshd_config"),
        format!(
            "Port {port}\nListenAddress 127.0.0.1\nHostKey {s}/hostkey\nPidFile {s}/sshd.pid\nAuthorizedKeysFile {s}/authorized_keys\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nUsePAM no\nStrictModes no\nSetEnv HOME={h} ZOHARA_RUNNER_NO_SYSTEMD=1 ZOHARA_RUNNER_UNIT_DIR={u}\nLogLevel ERROR\n",
            s = srv.display(),
            h = vmhome.display(),
            u = units.display()
        ),
    )
    .unwrap();
    let sshd = Command::new(sshd_bin).arg("-D").arg("-f").arg(srv.join("sshd_config")).arg("-E").arg(srv.join("sshd.log")).spawn().unwrap();
    let t0 = Instant::now();
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(t0.elapsed() < Duration::from_secs(10), "sshd did not start");
        std::thread::sleep(Duration::from_millis(50));
    }
    // this process is "the owner's computer"
    std::env::set_var("XDG_CONFIG_HOME", dir.join("host/cfg"));
    std::env::set_var("XDG_DATA_HOME", dir.join("host/data"));
    std::env::set_var("XDG_RUNTIME_DIR", dir.join("run"));
    std::env::set_var("ZOHARA_AGENT_KEYS_FILE", dir.join("host/keys.json"));
    std::env::set_var("ZOHARA_RUNNER_BINARY", env!("CARGO_BIN_EXE_zohara-agent-runner"));
    let key = std::fs::read_to_string(srv.join("client")).unwrap();
    machines::add(machines::Machine { name: "vm1".into(), host: "127.0.0.1".into(), user: me(), port, note: String::new() }, Some(&key)).unwrap();
    Some(Rig { dir, sshd, vmhome, units, _guard: guard })
}

/// A model server that answers a request with a tool call (the command in `cmd`) when the last message is not a tool result, and with
/// a final text when it is. `auth` collects the Authorization headers it saw.
struct Model {
    url: String,
    cmd: Arc<Mutex<String>>,
    auth: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
}

impl Drop for Model {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn model() -> Model {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.set_nonblocking(true).unwrap();
    let url = format!("http://{}/v1", l.local_addr().unwrap());
    let (cmd, auth, stop) = (Arc::new(Mutex::new(String::new())), Arc::new(Mutex::new(Vec::new())), Arc::new(AtomicBool::new(false)));
    let (c2, a2, s2) = (cmd.clone(), auth.clone(), stop.clone());
    std::thread::spawn(move || {
        while !s2.load(Ordering::SeqCst) {
            let Ok((mut s, _)) = l.accept() else {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            };
            s.set_nonblocking(false).unwrap();
            let mut buf = Vec::new();
            let mut tmp = [0u8; 8192];
            let (head_end, len) = loop {
                let n = s.read(&mut tmp).unwrap_or(0);
                if n == 0 {
                    break (buf.len(), 0);
                }
                buf.extend_from_slice(&tmp[..n]);
                if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buf[..i]).to_lowercase();
                    break (i + 4, head.lines().find_map(|l| l.strip_prefix("content-length:")).and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(0));
                }
            };
            while buf.len() < head_end + len {
                let n = s.read(&mut tmp).unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..n]);
            }
            let head = String::from_utf8_lossy(&buf[..head_end.min(buf.len())]).to_string();
            if let Some(a) = head.lines().find(|l| l.to_lowercase().starts_with("authorization:")) {
                a2.lock().unwrap().push(a.split_once(':').unwrap().1.trim().to_string());
            }
            let body: Value = serde_json::from_slice(&buf[head_end.min(buf.len())..]).unwrap_or(Value::Null);
            let last_role = body["messages"].as_array().and_then(|m| m.last()).and_then(|m| m["role"].as_str()).unwrap_or("").to_string();
            let answer = if last_role == "tool" {
                json!({"choices":[{"message":{"content":"All done on the machine."}}]})
            } else {
                let cmd = c2.lock().unwrap().clone();
                json!({"choices":[{"message":{"content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"zohara_shell","arguments":json!({"cmd": cmd}).to_string()}}]}}]})
            };
            let b = answer.to_string();
            let _ = write!(s, "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{b}", b.len());
        }
    });
    Model { url, cmd, auth, stop }
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let t0 = Instant::now();
    while !f() {
        assert!(t0.elapsed() < Duration::from_secs(40), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn mode_of(p: &Path) -> u32 {
    std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(p).unwrap().permissions()) & 0o777
}

#[test]
fn the_runner_installs_runs_tasks_asks_nothing_and_goes_away() {
    let Some(rig) = rig("life") else { return };
    // before: the machine answers and has no runner
    let st = host::status("vm1");
    assert_eq!((st["reachable"].clone(), st["installed"].clone()), (json!(true), json!(false)), "{st}");

    // install: the program is copied with a checksum, the units are written
    let steps = host::install("vm1").unwrap();
    assert!(steps.iter().any(|s| s.contains("checksum")), "{steps:?}");
    let bin = rig.vmhome.join(".local/bin/zohara-agent-runner");
    assert_eq!(mode_of(&bin), 0o755);
    assert_eq!(std::fs::read(&bin).unwrap(), std::fs::read(env!("CARGO_BIN_EXE_zohara-agent-runner")).unwrap(), "the same bytes arrived");
    let service = std::fs::read_to_string(rig.units.join("zohara-agent-runner.service")).unwrap();
    assert!(service.contains("run-due") && service.contains(&bin.display().to_string()), "{service}");
    assert!(std::fs::read_to_string(rig.units.join("zohara-agent-runner.timer")).unwrap().contains("OnCalendar=minutely"));
    assert!(host::has_runner("vm1"));
    let st = host::status("vm1");
    assert_eq!(st["installed"], json!(true), "{st}");
    assert_eq!(st["runner"]["version"], json!(zo_core::runner::VERSION));
    assert_eq!(st["runner"]["has_model"], json!(false));
    // a second install is an update
    host::install("vm1").unwrap();

    // a task on the machine is now the runner's: this computer no longer runs it
    let task = schedules::add_new(schedules::New { title: "Disk check".into(), prompt: "Check the disk".into(), next_run: schedules::now_millis(), repeat: "once".into(), run_on: "machine:vm1".into(), ..Default::default() }).unwrap();
    assert!(!zo_core::scheduler::host_runs(&task));
    assert!(zo_core::scheduler::place_text(&task).contains("runner"));
    assert_eq!(host::sync("vm1").unwrap(), 1);
    assert_eq!(host::status("vm1")["runner"]["tasks"], json!(1));

    // no model there yet: the run is skipped with a clear reason, and comes back as a result
    host::trigger("vm1").unwrap();
    wait_for("the first result", || host::pull("vm1").unwrap_or(0) > 0 || schedules::get(&task.id).and_then(|t| t.last_status).is_some());
    let t = schedules::get(&task.id).unwrap();
    assert_eq!(t.last_status.as_deref(), Some("skipped"), "{t:?}");
    assert!(t.last_output.unwrap().contains("No model is chosen"));

    // the model: a wrong confirmation is refused, the right one copies the key (mode 0600, only on the machine)
    let m = model();
    let mut sel = zo_core::ai::Selection { provider: "openai".into(), model: "gpt-x".into(), ..Default::default() };
    sel.base_urls.insert("openai".into(), m.url.clone());
    sel.save();
    zo_core::ai::keys::set("openai", "sk-test-secret-123").unwrap();
    let err = host::push_model("vm1", "yes").unwrap_err();
    assert!(err.contains("This copies your OpenAI API key to vm1."), "{err}");
    assert!(!rig.vmhome.join(".local/share/zohara-agent-runner/keys.json").exists(), "nothing was sent before the confirmation");
    host::push_model("vm1", &host::model_confirmation("vm1", "OpenAI")).unwrap();
    let keys = rig.vmhome.join(".local/share/zohara-agent-runner/keys.json");
    assert_eq!(mode_of(&keys), 0o600);
    assert!(std::fs::read_to_string(&keys).unwrap().contains("sk-test-secret-123"));
    let audit = std::fs::read_to_string(rig.dir.join("host/data/zohara-agent/audit.jsonl")).unwrap_or_default();
    assert!(audit.contains("runner_copy_model") && !audit.contains("sk-test-secret-123"), "the audit log has the fact, never the key");

    // approvals: a command the task wants to run is refused (nobody can answer) ...
    let proof1 = rig.vmhome.join("proof1");
    *m.cmd.lock().unwrap() = format!("echo hi > {}", proof1.display());
    let (t1, by_runner) = host::run_here("vm1", "Make proof1", "Proof one").unwrap();
    assert!(by_runner);
    wait_for("run 1", || {
        let _ = host::pull("vm1");
        schedules::get(&t1.id).and_then(|t| t.last_status).is_some()
    });
    let r1 = schedules::get(&t1.id).unwrap();
    assert_eq!(r1.last_status.as_deref(), Some("ok"), "{r1:?}");
    assert!(!proof1.exists(), "the runner asks nobody, so a command that needs approval is refused");
    assert!(m.auth.lock().unwrap().iter().any(|a| a == "Bearer sk-test-secret-123"), "the copied key was used");
    // ... and the result is a chat on this computer with the task's history filled in
    let chat = zo_core::store::chats::load().into_iter().find(|c| Some(c.id.clone()) == r1.last_chat).expect("a chat for the run");
    assert!(chat.title.starts_with("Scheduled on vm1:"), "{}", chat.title);
    assert!(chat.entries.iter().any(|e| e.text.contains("All done on the machine.")));

    // ... unless the owner allowed commands for this machine
    host::set_allow("vm1", true, false).unwrap();
    let proof2 = rig.vmhome.join("proof2");
    *m.cmd.lock().unwrap() = format!("echo hi > {}", proof2.display());
    let (t2, _) = host::run_here("vm1", "Make proof2", "Proof two").unwrap();
    wait_for("run 2", || {
        let _ = host::pull("vm1");
        schedules::get(&t2.id).and_then(|t| t.last_status).is_some()
    });
    assert!(proof2.exists(), "an allowed tool runs");
    // "Run now" of a task the runner owns goes to the runner too
    let again = schedules::add_new(schedules::New { title: "Again".into(), prompt: "again".into(), next_run: schedules::now_millis() + 86_400_000, repeat: "daily".into(), run_on: "machine:vm1".into(), ..Default::default() }).unwrap();
    let proof3 = rig.vmhome.join("proof3");
    *m.cmd.lock().unwrap() = format!("echo hi > {}", proof3.display());
    host::run_task_now(&again.id).unwrap();
    wait_for("run now", || {
        let _ = host::pull("vm1");
        schedules::get(&again.id).and_then(|t| t.last_status).is_some()
    });
    assert!(proof3.exists());
    assert!(schedules::get(&again.id).unwrap().next_run > schedules::now_millis(), "a Run now does not use up the planned time");

    // remove: the timer, the folder, the program; the task is this computer's again
    host::uninstall("vm1").unwrap();
    assert!(!bin.exists() && !rig.units.join("zohara-agent-runner.timer").exists() && !rig.vmhome.join(".local/share/zohara-agent-runner").exists());
    assert!(!host::has_runner("vm1"));
    let again = schedules::get(&again.id).unwrap();
    assert!(zo_core::scheduler::host_runs(&again));
    assert_eq!(host::status("vm1")["installed"], json!(false));
}

#[test]
fn install_refuses_what_cannot_work_and_changes_nothing() {
    let Some(rig) = rig("refuse") else { return };
    // a binary that is not where it should be
    std::env::set_var("ZOHARA_RUNNER_BINARY", rig.dir.join("missing"));
    let cur = std::env::current_exe().unwrap();
    let _ = cur; // the fallbacks beside the test program do not exist either
    let e = host::install("vm1").unwrap_err();
    assert!(e.contains("was not found"), "{e}");
    assert!(!rig.vmhome.join(".local/bin").exists(), "nothing was copied");
    // an unknown machine
    assert!(host::install("nope").is_err());
    // a machine that is not reachable is reported, not hung on
    machines::add(machines::Machine { name: "gone".into(), host: "127.0.0.1".into(), user: me(), port: free_port(), note: String::new() }, Some(&std::fs::read_to_string(rig.dir.join("sshd/client")).unwrap())).unwrap();
    let st = host::status("gone");
    assert_eq!(st["reachable"], json!(false), "{st}");
    assert!(st["error"].as_str().unwrap_or("").len() > 3);
}

#[test]
fn connected_apps_copy_only_after_a_named_confirmation_and_are_revoked() {
    let Some(rig) = rig("apps") else { return };
    std::env::set_var("ZOHARA_RUNNER_BINARY", env!("CARGO_BIN_EXE_zohara-agent-runner"));
    // two apps here: Notion (a key in an environment variable and a stored sign-in) and a plain one
    let mut notion = zo_core::mcp_client::ServerConfig { name: "Notion".into(), command: "npx -y notion-mcp".into(), enabled: true, ..Default::default() };
    notion.env.insert("NOTION_TOKEN".into(), "ntn_secret_value".into());
    let github = zo_core::mcp_client::ServerConfig { name: "GitHub".into(), command: "https://api.example.com/mcp".into(), enabled: true, secret_headers: vec!["Authorization".into()], ..Default::default() };
    zo_core::ai::keys::set(&zo_core::mcp_client::header_id("GitHub", "Authorization"), "Bearer gh_secret_value").unwrap();
    let plain = zo_core::mcp_client::ServerConfig { name: "Plain".into(), command: "definitely-not-installed --serve".into(), enabled: true, ..Default::default() };
    zo_core::mcp_client::save_servers(&[notion, github, plain]);

    // the runner must be there first
    let e = apps::copy("vm1", &[("Plain".into(), false)], "").unwrap_err();
    assert!(e.contains("not installed"), "{e}");
    host::install("vm1").unwrap();

    // the plan shows names, never values
    let plan = apps::plan("vm1").unwrap().to_string();
    assert!(plan.contains("variable NOTION_TOKEN") && plan.contains("header Authorization"));
    assert!(!plan.contains("ntn_secret_value") && !plan.contains("gh_secret_value"), "{plan}");

    // without credentials: only the names go, the values stay here
    apps::copy("vm1", &[("Notion".into(), false), ("Plain".into(), false)], "").unwrap();
    let cfg = rig.vmhome.join(".local/share/zohara-agent-runner/cfg/zohara-agent/mcp-servers.json");
    let text = std::fs::read_to_string(&cfg).unwrap();
    assert!(text.contains("NOTION_TOKEN") && !text.contains("ntn_secret_value"), "{text}");
    assert_eq!(mode_of(&cfg), 0o600);

    // with credentials: refused without the sentence that names the apps and the machine, and with a wrong one
    let both = [("Notion".to_string(), true), ("GitHub".to_string(), true)];
    assert!(apps::copy("vm1", &both, "").unwrap_err().contains("Confirm exactly: This copies the GitHub and Notion credentials to vm1."));
    assert!(apps::copy("vm1", &both, "This copies the GitHub and Notion credentials to vm2.").is_err(), "another machine's name does not count");
    assert!(apps::copy("vm1", &both, "This copies the Notion credentials to vm1.").is_err(), "a sentence for fewer apps does not count");
    assert!(!std::fs::read_to_string(&cfg).unwrap().contains("ntn_secret_value"), "nothing moved before the confirmation");
    let ok = apps::copy("vm1", &both, &apps::confirmation("vm1", &["Notion".into(), "GitHub".into()])).unwrap();
    assert_eq!(ok["credentials"], json!(2));
    let text = std::fs::read_to_string(&cfg).unwrap();
    assert!(text.contains("ntn_secret_value"));
    let keys = rig.vmhome.join(".local/share/zohara-agent-runner/keys.json");
    assert_eq!(mode_of(&keys), 0o600);
    assert!(std::fs::read_to_string(&keys).unwrap().contains("gh_secret_value"));
    // the audit log names the apps and the machine, never a value
    let audit = std::fs::read_to_string(rig.dir.join("host/data/zohara-agent/audit.jsonl")).unwrap();
    assert!(audit.contains("runner_copy_apps") && audit.contains("Notion") && !audit.contains("ntn_secret_value") && !audit.contains("gh_secret_value"), "{audit}");

    // the runtime check: npx and the made-up program are missing on this machine unless installed (npx may exist here)
    let chk = apps::check("vm1").unwrap();
    let plain_row = chk["servers"].as_array().unwrap().iter().find(|s| s["name"] == "Plain").unwrap();
    assert_eq!(plain_row["present"], json!(false));
    assert!(plain_row["hint"].as_str().unwrap().contains("definitely-not-installed"));
    let gh_row = chk["servers"].as_array().unwrap().iter().find(|s| s["name"] == "GitHub").unwrap();
    assert_eq!(gh_row["present"], json!(true), "an address needs nothing installed");

    // sync: a changed app is sent again (with a new confirmation), a removed one is removed there
    let mut all = zo_core::mcp_client::load_servers();
    all.retain(|s| s.name != "Plain");
    all.iter_mut().find(|s| s.name == "Notion").unwrap().env.insert("NOTION_TOKEN".into(), "ntn_rotated".into());
    zo_core::mcp_client::save_servers(&all);
    assert!(apps::sync("vm1", "").is_err());
    let r = apps::sync("vm1", &apps::confirmation("vm1", &["Notion".into(), "GitHub".into()])).unwrap();
    assert_eq!(r["removed"], json!(["Plain"]));
    let text = std::fs::read_to_string(&cfg).unwrap();
    assert!(text.contains("ntn_rotated") && !text.contains("Plain") && !text.contains("ntn_secret_value"), "{text}");

    // revoke: everything copied is gone from the machine, and the registry forgets it
    apps::revoke("vm1").unwrap();
    assert!(!cfg.exists());
    assert!(!std::fs::read_to_string(&keys).unwrap().contains("gh_secret_value"));
    assert!(apps::copied("vm1").is_null());
    host::uninstall("vm1").unwrap();
}

#[test]
fn a_key_made_for_zohara_agent_lets_it_in_and_the_terminal_command_is_exact() {
    let Some(rig) = rig("keys") else { return };
    // a new machine through the tool: a new key is made; the person puts its line on the machine; then the test passes
    let srv = rig.dir.join("sshd");
    let port = machines::get("vm1").unwrap().port;
    let out = zo_core::call("zohara_ssh_add_machine", &json!({"name": "vm2", "host": "127.0.0.1", "user": me(), "port": port})).unwrap();
    assert!(out.contains("authorized_keys") && out.contains("ssh-ed25519") && !out.contains("PRIVATE KEY"), "{out}");
    assert!(zo_core::call("zohara_ssh_test", &json!({"machine": "vm2"})).is_err(), "not let in yet");
    assert!(zo_core::call("zohara_ssh_add_machine", &json!({"name": "vm2", "host": "127.0.0.1", "user": me()})).is_err(), "no second machine with the same name");
    assert!(zo_core::call("zohara_ssh_add_machine", &json!({"name": "-oProxyCommand=x", "host": "h", "user": "u"})).is_err());
    let public = zo_core::tools::ssh::public_of("vm2").unwrap();
    assert!(out.contains(&public));
    let mut ak = std::fs::read_to_string(srv.join("authorized_keys")).unwrap();
    ak.push_str(&format!("{public}\n"));
    std::fs::write(srv.join("authorized_keys"), ak).unwrap();
    assert!(zo_core::call("zohara_ssh_test", &json!({"machine": "vm2"})).unwrap().contains("Connected"));
    // the private key is in the key store and is not in the machines file
    let file = std::fs::read_to_string(rig.dir.join("host/cfg/zohara-agent/machines.json")).unwrap();
    assert!(!file.contains("PRIVATE KEY"));
    // the terminal command: a login shell, a pseudo-terminal, no remote command, no -T
    let (args, key) = zo_core::tools::ssh::terminal_command("vm2").unwrap();
    assert!(args.iter().any(|a| a == "-tt") && !args.iter().any(|a| a == "-T"));
    assert_eq!(args.last().unwrap(), &format!("{}@127.0.0.1", me()));
    assert!(key.path().exists());
    let p = key.path().to_path_buf();
    drop(key);
    assert!(!p.exists(), "the key file goes away");
}
