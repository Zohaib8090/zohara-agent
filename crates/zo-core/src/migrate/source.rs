//! Where the files come from: this computer's home folder, a machine added in Settings (read over SSH, read-only), or text the person
//! pasted or uploaded. A source only reads; nothing here writes anywhere.

use std::collections::HashMap;
use std::io::Read;

/// Biggest file read (Claude Code's own `~/.claude.json` can be a few megabytes).
pub const MAX_FILE: usize = 8 * 1024 * 1024;
/// Most entries listed from one folder.
pub const MAX_DIR: usize = 200;

#[derive(Default, Debug)]
pub struct Fetched {
    pub files: HashMap<String, String>,
    pub dirs: HashMap<String, Vec<String>>,
    /// Files that exist but are bigger than [`MAX_FILE`] (not read).
    pub too_big: Vec<String>,
}

/// Reads the named files and lists the named folders. A path is relative to the home folder, or absolute. A missing or unreadable
/// one is simply absent from the answer.
pub trait Reader {
    /// Where this is, for messages: "this computer", the machine's name, "what you pasted".
    fn label(&self) -> String;
    fn fetch(&self, files: &[String], dirs: &[String]) -> Result<Fetched, String>;
    /// `~/x` for a path as asked (what the person recognises).
    fn shown(&self, path: &str) -> String {
        if path.starts_with('/') { path.to_string() } else { format!("~/{path}") }
    }
}

/// This computer.
pub struct Local {
    home: std::path::PathBuf,
}

impl Local {
    pub fn new() -> Result<Local, String> {
        let h = std::env::var_os("HOME").filter(|h| !h.is_empty()).ok_or("This computer has no home folder.")?;
        let home = std::fs::canonicalize(h).map_err(|_| "The home folder is not available.".to_string())?;
        Ok(Local { home })
    }

    #[cfg(test)]
    pub fn at(home: &std::path::Path) -> Local {
        Local { home: std::fs::canonicalize(home).unwrap() }
    }

    /// A path inside the home folder, symbolic links followed (a link that leads out of home is not followed out).
    fn inside(&self, path: &str) -> Option<std::path::PathBuf> {
        if path.is_empty() || path.contains('\0') || path.len() > 600 {
            return None;
        }
        let joined = if path.starts_with('/') { std::path::PathBuf::from(path) } else { self.home.join(path) };
        let real = std::fs::canonicalize(joined).ok()?;
        real.starts_with(&self.home).then_some(real)
    }

    /// None: absent or unreadable; Some(Err): bigger than [`MAX_FILE`].
    fn read(&self, path: &str) -> Option<Result<String, ()>> {
        use std::os::unix::fs::OpenOptionsExt;
        let real = self.inside(path)?;
        // O_NONBLOCK so that a pipe with no writer cannot hang the open; the type is checked on the opened file
        let f = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY).open(real).ok()?;
        if !f.metadata().ok()?.is_file() {
            return None;
        }
        let mut buf = Vec::new();
        f.take(MAX_FILE as u64 + 1).read_to_end(&mut buf).ok()?;
        if buf.len() > MAX_FILE {
            return Some(Err(()));
        }
        if buf.contains(&0) {
            return None;
        }
        String::from_utf8(buf).ok().map(Ok)
    }
}

impl Reader for Local {
    fn label(&self) -> String {
        "this computer".into()
    }
    fn fetch(&self, files: &[String], dirs: &[String]) -> Result<Fetched, String> {
        let mut out = Fetched::default();
        for f in files {
            match self.read(f) {
                Some(Ok(t)) => {
                    out.files.insert(f.clone(), t);
                }
                Some(Err(())) => out.too_big.push(f.clone()),
                None => {}
            }
        }
        for d in dirs {
            let Some(real) = self.inside(d) else { continue };
            if !real.is_dir() {
                continue;
            }
            let mut names: Vec<String> = std::fs::read_dir(real).map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
            names.sort();
            names.truncate(MAX_DIR);
            out.dirs.insert(d.clone(), names);
        }
        Ok(out)
    }
}

/// Text that was handed over (pasted or uploaded): files by name. Folders do not exist here.
pub struct Given {
    pub files: Vec<(String, String)>,
}

impl Reader for Given {
    fn label(&self) -> String {
        "what you pasted".into()
    }
    fn fetch(&self, files: &[String], _dirs: &[String]) -> Result<Fetched, String> {
        let mut out = Fetched::default();
        for f in files {
            if let Some((_, t)) = self.files.iter().find(|(n, _)| n == f) {
                out.files.insert(f.clone(), t.clone());
            }
        }
        Ok(out)
    }
    fn shown(&self, path: &str) -> String {
        path.to_string()
    }
}

/// A machine added in Settings > Machines, read over SSH with ONE command per round: only `cat`-like reads, nothing is written there.
pub struct Ssh {
    pub machine: String,
}

impl Ssh {
    /// The shell text that prints the files and folders between markers. The marker is random per run, so a file cannot make
    /// itself look like several files or like the end of the output.
    pub(crate) fn script(files: &[String], dirs: &[String], mark: &str) -> Result<String, String> {
        let p = |s: &str| crate::tools::ssh::remote_path(&if s.starts_with('/') { s.to_string() } else { format!("~/{s}") });
        let mut sh = String::new();
        sh.push_str(&format!("M='{mark}'; f() {{ if [ -f \"$1\" ] && [ -r \"$1\" ]; then printf '\\n%s FILE %s\\n' \"$M\" \"$2\"; head -c {MAX_FILE} -- \"$1\"; printf '\\n%s END\\n' \"$M\"; fi; }}; "));
        sh.push_str(&format!("d() {{ if [ -d \"$1\" ]; then printf '\\n%s DIR %s\\n' \"$M\" \"$2\"; ls -1 -- \"$1\" 2>/dev/null | head -n {MAX_DIR}; printf '\\n%s END\\n' \"$M\"; fi; }}; "));
        for f in files {
            sh.push_str(&format!("f {} {}; ", p(f)?, crate::tools::ssh::quote(f)));
        }
        for d in dirs {
            sh.push_str(&format!("d {} {}; ", p(d)?, crate::tools::ssh::quote(d)));
        }
        sh.push_str("true");
        Ok(sh)
    }

    /// The answer of [`Ssh::script`] taken apart.
    pub fn parse(out: &str, mark: &str) -> Fetched {
        let mut r = Fetched::default();
        let (file_tag, dir_tag, end) = (format!("{mark} FILE "), format!("{mark} DIR "), format!("{mark} END"));
        let mut cur: Option<(bool, String, Vec<&str>)> = None;
        for line in out.split('\n') {
            if let Some((is_file, name, body)) = cur.as_mut() {
                if line == end {
                    let (is_file, name, body) = (*is_file, name.clone(), std::mem::take(body));
                    if is_file {
                        // the printf before the END added one newline
                        r.files.insert(name, body.join("\n").trim_end_matches('\n').to_string());
                    } else {
                        r.dirs.insert(name, body.iter().filter(|l| !l.is_empty()).map(|l| l.to_string()).collect());
                    }
                    cur = None;
                } else {
                    body.push(line);
                }
            } else if let Some(n) = line.strip_prefix(&file_tag) {
                cur = Some((true, n.to_string(), Vec::new()));
            } else if let Some(n) = line.strip_prefix(&dir_tag) {
                cur = Some((false, n.to_string(), Vec::new()));
            }
        }
        r
    }
}

impl Reader for Ssh {
    fn label(&self) -> String {
        self.machine.clone()
    }
    fn fetch(&self, files: &[String], dirs: &[String]) -> Result<Fetched, String> {
        let (files, dirs) = &plain_paths(files, dirs)?;
        let mark = format!("zo{}", random_hex());
        let sh = Self::script(files, dirs, &mark)?;
        let d = crate::tools::ssh::run_on(&self.machine, &sh, None, 90, 3 * MAX_FILE)?;
        if d.code == 255 {
            let why = d.stderr.trim();
            return Err(format!("Could not use {} over SSH: {}", self.machine, if why.is_empty() { "ssh failed".to_string() } else { why.chars().take(300).collect::<String>() }));
        }
        let mut got = Self::parse(&d.stdout, &mark);
        // `head -c` cut these at the limit: they are not whole files
        let cut: Vec<String> = got.files.iter().filter(|(_, t)| t.len() >= MAX_FILE).map(|(n, _)| n.clone()).collect();
        for n in cut {
            got.files.remove(&n);
            got.too_big.push(n);
        }
        Ok(got)
    }
}

/// A path from a remote file (a project folder) is data: only plain paths are asked for. A path with control characters or `..`
/// refuses the whole call; one that is merely too long is left out (the rest of the scan still works).
pub(crate) fn plain_paths(files: &[String], dirs: &[String]) -> Result<(Vec<String>, Vec<String>), String> {
    for f in files.iter().chain(dirs) {
        if f.chars().any(char::is_control) || f.contains("..") {
            return Err("A path to read is not valid.".into());
        }
    }
    let ok = |l: &[String]| l.iter().filter(|f| f.len() <= 400).cloned().collect::<Vec<_>>();
    Ok((ok(files), ok(dirs)))
}

fn random_hex() -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(std::process::id().to_le_bytes());
    h.update(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0).to_le_bytes());
    let mut buf = [0u8; 16];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut buf);
    }
    h.update(buf);
    h.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect()
}
