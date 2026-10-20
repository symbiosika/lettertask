//! Starts a private Dovecot IMAP server for integration tests.
//!
//! Plain IMAP on 127.0.0.1 with a random port, maildir storage in a temp dir,
//! any user name with password `secret`, special-use folders Sent/Drafts/
//! Trash/Junk/Archive. Returns `None` (test skips) when Dovecot is missing.

// Each test crate uses a different subset.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub const PASSWORD: &str = "secret";

pub struct Dovecot {
    pub port: u16,
    child: Option<Child>,
    pub dir: tempfile::TempDir,
}

fn find_dovecot() -> Option<String> {
    ["/usr/sbin/dovecot", "/usr/local/sbin/dovecot", "/opt/homebrew/sbin/dovecot"]
        .iter()
        .find(|p| Path::new(p).exists())
        .map(|p| p.to_string())
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn chmod_open(p: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o777));
    }
}

fn current_user() -> (bool, String) {
    let uid = Command::new("id").arg("-u").output().ok().and_then(|o| String::from_utf8(o.stdout).ok()).unwrap_or_default();
    let name = Command::new("id").arg("-un").output().ok().and_then(|o| String::from_utf8(o.stdout).ok()).unwrap_or_default();
    (uid.trim() == "0", name.trim().to_string())
}

/// Server layout variants.
#[derive(Clone, Copy)]
pub struct Options {
    /// Hierarchy separator.
    pub separator: char,
    /// Personal namespace below INBOX ("INBOX." like Courier/Cyrus) and no
    /// SPECIAL-USE folders: roles must be found by name.
    pub inbox_prefix: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            separator: '/',
            inbox_prefix: false,
        }
    }
}

impl Dovecot {
    pub fn start() -> Option<Dovecot> {
        Self::start_with(Options::default())
    }

    pub fn start_with(opts: Options) -> Option<Dovecot> {
        let bin = match find_dovecot() {
            Some(b) => b,
            None => {
                eprintln!("dovecot nicht installiert – IMAP-Test übersprungen");
                return None;
            }
        };
        // Unix socket paths must stay short, so use /tmp directly.
        let dir = tempfile::Builder::new().prefix("dv").tempdir_in("/tmp").unwrap();
        let base = dir.path();
        for sub in ["run", "state", "home"] {
            std::fs::create_dir_all(base.join(sub)).unwrap();
            chmod_open(&base.join(sub));
        }
        chmod_open(base);
        let port = free_port();
        let (root, me) = current_user();
        let (internal, login, mail_user) = if root {
            ("dovecot".to_string(), "dovenull".to_string(), "dovecot".to_string())
        } else {
            (me.clone(), me.clone(), me.clone())
        };
        let b = base.display();
        let sep = opts.separator;
        let namespace = if opts.inbox_prefix {
            format!("namespace inbox {{\n  inbox = yes\n  prefix = INBOX{sep}\n  separator = {sep}\n}}")
        } else {
            let mut ns = format!("namespace inbox {{\n  inbox = yes\n  separator = {sep}\n");
            for (name, flag) in [("Sent", "Sent"), ("Drafts", "Drafts"), ("Trash", "Trash"), ("Junk", "Junk"), ("Archive", "Archive")] {
                ns.push_str(&format!("  mailbox {name} {{\n    special_use = \\{flag}\n    auto = subscribe\n  }}\n"));
            }
            ns.push('}');
            ns
        };
        let conf = format!(
            r#"base_dir = {b}/run
state_dir = {b}/state
instance_name = dv{port}
listen = 127.0.0.1
protocols = imap
ssl = no
disable_plaintext_auth = no
auth_mechanisms = plain login
log_path = {b}/dovecot.log
default_internal_user = {internal}
default_internal_group = {internal}
default_login_user = {login}
first_valid_uid = 1
service imap-login {{
  chroot =
  inet_listener imap {{
    address = 127.0.0.1
    port = {port}
  }}
  inet_listener imaps {{
    port = 0
  }}
}}
service anvil {{
  chroot =
}}
passdb {{
  driver = static
  args = password={PASSWORD}
}}
userdb {{
  driver = static
  args = uid={mail_user} gid={mail_user} home={b}/home/%u
}}
mail_location = maildir:{b}/home/%u/Maildir
{namespace}
protocol imap {{
  imap_capability = +SPECIAL-USE
  mail_max_userip_connections = 50
}}
"#
        );
        let conf_path = base.join("dovecot.conf");
        std::fs::write(&conf_path, conf).unwrap();
        let child = Command::new(bin)
            .arg("-F")
            .arg("-c")
            .arg(&conf_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start dovecot");
        let d = Dovecot {
            port,
            child: Some(child),
            dir,
        };
        d.wait_ready();
        Some(d)
    }

    /// Waits until the server greets with "ready" (auth process is up).
    fn wait_ready(&self) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if let Ok(s) = TcpStream::connect(("127.0.0.1", self.port)) {
                s.set_read_timeout(Some(Duration::from_secs(3))).ok();
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut line = String::new();
                for _ in 0..2 {
                    line.clear();
                    if r.read_line(&mut line).is_err() {
                        break;
                    }
                    if line.contains("ready") {
                        let mut w = s;
                        let _ = w.write_all(b"a LOGOUT\r\n");
                        return;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        let log = std::fs::read_to_string(self.dir.path().join("dovecot.log")).unwrap_or_default();
        panic!("dovecot not ready:\n{log}");
    }

    /// Kills all established IMAP sessions (the server keeps running), like
    /// a NAT timeout or a server restart would.
    pub fn kill_sessions(&self) {
        let Some(c) = &self.child else { return };
        let out = Command::new("pgrep")
            .arg("-P")
            .arg(c.id().to_string())
            .arg("-x")
            .arg("imap")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).split_whitespace().map(str::to_string).collect::<Vec<_>>())
            .unwrap_or_default();
        for pid in out {
            let _ = Command::new("kill").arg("-KILL").arg(pid).stderr(Stdio::null()).status();
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    pub fn stop(&mut self) {
        if let Some(mut c) = self.child.take() {
            // Established imap sessions are children of the master and may
            // outlive it for a moment; stop them as well.
            let children = Command::new("pgrep")
                .arg("-P")
                .arg(c.id().to_string())
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).split_whitespace().map(str::to_string).collect::<Vec<_>>())
                .unwrap_or_default();
            let _ = Command::new("kill").arg("-TERM").arg(c.id().to_string()).status();
            let _ = c.wait();
            for pid in children {
                let _ = Command::new("kill").arg("-KILL").arg(pid).stderr(Stdio::null()).status();
            }
            // Give children a moment to exit before the temp dir goes.
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

impl Drop for Dovecot {
    fn drop(&mut self) {
        self.stop();
    }
}
