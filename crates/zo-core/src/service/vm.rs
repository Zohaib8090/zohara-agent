//! The machine operations of the service that go beyond the machines list: status, the add-machine wizard, the runner, the model and
//! the connected apps on a machine, "run a task here" and the terminal (docs/SSH.md, docs/RUNNER.md, docs/VM-MIGRATION-PLAN.md).
//!
//! All of them start with `machine`, so `restricted_refusal` refuses every one of them for a restricted account.

use super::*;
use crate::runner::{apps, host};
use crate::store::machines::{self, Machine};
use crate::tools::ssh;

pub fn handles(op: &str) -> bool {
    matches!(
        op,
        "machine_status"
            | "machine_new"
            | "machine_pubkey"
            | "machine_authorize"
            | "machine_runner_install"
            | "machine_runner_uninstall"
            | "machine_runner_allow"
            | "machine_runner_sync"
            | "machine_model_plan"
            | "machine_model_copy"
            | "machine_model_clear"
            | "machine_apps_plan"
            | "machine_apps_copy"
            | "machine_apps_revoke"
            | "machine_apps_check"
            | "machine_apps_sync"
            | "machine_task_run"
            | "machine_terminal"
            | "machine_terminal_spec"
    )
}

fn s<'a>(req: &'a Value, k: &str) -> &'a str {
    req.get(k).and_then(Value::as_str).unwrap_or("").trim()
}

fn name_of(req: &Value) -> Result<&str, String> {
    let n = s(req, "name");
    if n.is_empty() {
        return Err("Say which machine.".into());
    }
    machines::get(n).ok_or_else(|| format!("There is no machine called \"{n}\"."))?;
    Ok(n)
}

pub fn handle(op: &str, req: &Value) -> Result<Value, String> {
    if tenant::is_restricted() {
        return Err("Machines (SSH) are not available to this account.".into());
    }
    match op {
        "machine_status" => {
            // one machine, or all of them at the same time; results that the runners have are fetched on the way
            let names: Vec<String> = match s(req, "name") {
                "" => machines::load().into_iter().map(|m| m.name).collect(),
                n => vec![name_of(req).map(|_| n.to_string())?],
            };
            let handles: Vec<_> = names
                .into_iter()
                .map(|n| {
                    tenant::spawn(move || {
                        let mut v = host::status(&n);
                        if v["reachable"].as_bool() == Some(true) && v["installed"].as_bool() == Some(true) {
                            let _ = host::sync(&n);
                        }
                        v["has_key"] = json!(machines::get(&n).is_some_and(|m| m.has_key()));
                        v
                    })
                })
                .collect();
            let list: Vec<Value> = handles.into_iter().filter_map(|h| h.join().ok()).collect();
            Ok(json!({"machines": list, "terminal": crate::term::enabled()}))
        }
        "machine_new" => {
            let m = Machine {
                name: s(req, "name").into(),
                host: s(req, "host").into(),
                user: s(req, "user").into(),
                port: match req.get("port") {
                    Some(Value::Number(n)) => n.as_u64().filter(|p| (1..=65535).contains(p)).ok_or("The port must be a number from 1 to 65535.")? as u16,
                    Some(Value::String(t)) => machines::parse_port(t)?,
                    _ => 22,
                },
                note: s(req, "note").into(),
            };
            m.validate()?;
            if machines::get(&m.name).is_some() {
                return Err(format!("A machine called \"{}\" is added already.", m.name));
            }
            let (private, public) = ssh::generate_key(&format!("zohara-agent-{}", m.name))?;
            machines::add(m.clone(), Some(&private))?;
            drop(private);
            Ok(json!({"name": m.name, "public_key": public, "authorize_command": ssh::authorize_command(&public)?}))
        }
        "machine_pubkey" => {
            let n = name_of(req)?;
            let p = ssh::public_of(n)?;
            Ok(json!({"public_key": p, "authorize_command": ssh::authorize_command(&p)?}))
        }
        "machine_authorize" => {
            let n = name_of(req)?;
            let public = ssh::public_of(n)?;
            // the password is used once and never kept; it is not logged (the audit log gets only that this happened)
            let r = ssh::authorize_with_password(n, req.get("password").and_then(Value::as_str).unwrap_or(""), &public);
            crate::audit::log("machine_authorize", &json!({"machine": n}), if r.is_ok() { "ok" } else { "failed" });
            r.map(|m| json!({"message": m}))
        }
        "machine_runner_install" => {
            let n = name_of(req)?;
            host::install(n).map(|steps| json!({"steps": steps}))
        }
        "machine_runner_uninstall" => {
            host::uninstall(name_of(req)?)?;
            Ok(json!({}))
        }
        "machine_runner_allow" => {
            let n = name_of(req)?;
            let flag = |k: &str| req.get(k).and_then(Value::as_bool).unwrap_or(false);
            host::set_allow(n, flag("commands"), flag("files"))?;
            Ok(json!({}))
        }
        "machine_runner_sync" => {
            let n = name_of(req)?;
            host::sync(n).map(|tasks| json!({"tasks": tasks}))
        }
        "machine_model_plan" => {
            let n = name_of(req)?;
            let sel = Selection::load();
            let p = providers::find(&sel.provider);
            let (can, why) = match p {
                None => (false, "No model is chosen here yet.".to_string()),
                Some(_) if sel.provider == "claude-code" => (false, "Claude Code needs its own login on the machine, so it cannot be copied.".to_string()),
                Some(p) if !p.local && crate::ai::profiles::get_key(&sel, p.id).is_none() => (false, format!("There is no API key for {} here to copy.", p.name)),
                Some(_) => (true, String::new()),
            };
            Ok(json!({
                "provider": p.map(|p| p.name).unwrap_or(""), "model": sel.model, "can": can && host::has_runner(n), "why": if !host::has_runner(n) { "Install the runner first.".to_string() } else { why },
                "needs_key": p.is_some_and(|p| !p.local),
                "confirm": p.map(|p| host::model_confirmation(n, p.name)).unwrap_or_default(),
            }))
        }
        "machine_model_copy" => host::push_model(name_of(req)?, s(req, "confirm")).map(|m| json!({"message": m})),
        "machine_model_clear" => host::clear_model(name_of(req)?).map(|_| json!({})),
        "machine_apps_plan" => apps::plan(name_of(req)?),
        "machine_apps_copy" => {
            let n = name_of(req)?;
            let picks: Vec<(String, bool)> = req
                .get("servers")
                .and_then(Value::as_array)
                .ok_or("Pick the apps to copy.")?
                .iter()
                .map(|p| (p["name"].as_str().unwrap_or("").to_string(), p["credentials"].as_bool().unwrap_or(false)))
                .collect();
            let creds: Vec<String> = picks.iter().filter(|(_, c)| *c).map(|(n, _)| n.clone()).collect();
            let mut out = apps::copy(n, &picks, s(req, "confirm"))?;
            out["confirm_was"] = json!(apps::confirmation(n, &creds));
            Ok(out)
        }
        "machine_apps_revoke" => apps::revoke(name_of(req)?).map(|_| json!({})),
        "machine_apps_check" => apps::check(name_of(req)?),
        "machine_apps_sync" => apps::sync(name_of(req)?, s(req, "confirm")),
        "machine_task_run" => {
            let n = name_of(req)?;
            let prompt = s(req, "prompt");
            if prompt.is_empty() {
                return Err("Say what to do there.".into());
            }
            let title = if s(req, "title").is_empty() { format!("On {n}: {}", prompt.chars().take(40).collect::<String>()) } else { s(req, "title").to_string() };
            let (task, by_runner) = host::run_here(n, prompt, &title)?;
            if !by_runner {
                // no runner: this computer does the work, over SSH, and asks the person as it always does
                let approver: Arc<dyn Approver + Send + Sync> = Arc::new(crate::approval::AppApprover);
                crate::scheduler::run_now_in_background(&task.id, approver, "machines", Arc::new(|t, o| crate::scheduler::notify("", t, &o.notice)))?;
            }
            Ok(json!({"task": task.id, "by_runner": by_runner}))
        }
        "machine_terminal" => {
            let n = name_of(req)?;
            if !crate::term::enabled() {
                return Err("Terminals open in the Zohara Agent window; this is not the app.".into());
            }
            let line = ssh::terminal_shell_line(n)?;
            crate::audit::log("machine_terminal", &json!({"machine": n}), "ok");
            let t = crate::term::open(&format!("ssh {n}"), None, Some(&line), false)?;
            Ok(json!({"term": t.id}))
        }
        "machine_terminal_spec" => {
            // for the cloud server's terminal (admin only, checked there): the ssh arguments and the path of a key file that lives for a minute
            let n = name_of(req)?;
            let (args, key) = ssh::terminal_command(n)?;
            let path = key.path().to_string_lossy().into_owned();
            key.remove_later(60);
            crate::audit::log("machine_terminal", &json!({"machine": n}), "ok");
            Ok(json!({"args": args, "key_file": path}))
        }
        _ => Err("Unknown machine operation.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPS: [&str; 19] = [
        "machine_status", "machine_new", "machine_pubkey", "machine_authorize", "machine_runner_install", "machine_runner_uninstall", "machine_runner_allow",
        "machine_runner_sync", "machine_model_plan", "machine_model_copy", "machine_model_clear", "machine_apps_plan", "machine_apps_copy", "machine_apps_revoke",
        "machine_apps_check", "machine_apps_sync", "machine_task_run", "machine_terminal", "machine_terminal_spec",
    ];

    #[test]
    fn every_machine_operation_is_refused_to_a_restricted_account() {
        for op in OPS {
            assert!(handles(op), "{op}");
            assert!(restricted_refusal(op, &json!({"op": op})).is_some(), "{op} must be refused for a restricted account");
        }
        assert!(!handles("machines") && !handles("send"));
    }

    #[test]
    fn the_handler_refuses_a_restricted_account_even_if_the_check_before_it_is_skipped() {
        let _g = crate::store::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("zo-vm-restricted-{}", std::process::id()));
        std::env::set_var("ZO_TENANT_ROOT", &dir);
        let t = tenant::Tenant::open(&"ab".repeat(16), true).unwrap();
        let r = tenant::scope(&t, || handle("machine_status", &json!({})));
        assert!(r.unwrap_err().contains("not available"));
        std::env::remove_var("ZO_TENANT_ROOT");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_add_machine_tool_asks_first_and_is_not_for_restricted_accounts() {
        let d = crate::tools::find("zohara_ssh_add_machine").expect("the tool exists");
        assert_eq!(d.risk, crate::policy::Risk::Change);
        for m in [crate::policy::Mode::Auto, crate::policy::Mode::Manual] {
            assert_eq!(crate::policy::decide(m, d.risk, None), crate::policy::Decision::Ask);
        }
        assert!(!crate::tools::allowed_for_restricted("zohara_ssh_add_machine") && !crate::tools::allowed_for_restricted("zohara_ssh_test"));
        // it takes no password: nothing a person types into a chat becomes a credential in the model's context
        assert!(!d.params.iter().any(|p| p.0.contains("pass")));
    }

    #[test]
    fn a_public_key_line_is_checked_before_it_is_put_in_a_command() {
        assert!(crate::tools::ssh::authorize_command("ssh-ed25519 AAAAC3Nza zohara-agent-vm1").is_ok());
        for bad in ["ssh-ed25519 AAAA'; rm -rf ~; echo '", "ssh-ed25519 AAAA\nx", "rm -rf /", "ssh-ed25519 AAAA$(id)"] {
            assert!(crate::tools::ssh::authorize_command(bad).is_err(), "{bad}");
        }
    }
}
