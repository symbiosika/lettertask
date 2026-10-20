//! Starts a private Dovecot IMAP server for integration tests.
//!
//! Plain IMAP on 127.0.0.1 with a random port, maildir storage in a temp dir,
//! any user name with password `secret`, special-use folders Sent/Drafts/
//! Trash/Junk/Archive. Returns `None` (test skips) when Dovecot is missing.

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

impl Dovecot {
    pub fn start() -> Option<Dovecot> {
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
namespace inbox {{
  inbox = yes
  separator = /
  mailbox Sent {{
    special_use = \Sent
    auto = subscribe
  }}
  mailbox Drafts {{
    special_use = \Drafts
    auto = subscribe
  }}
  mailbox Trash {{
    special_use = \Trash
    auto = subscribe
  }}
  mailbox Junk {{
    special_use = \Junk
    auto = subscribe
  }}
  mailbox Archive {{
    special_use = \Archive
    auto = subscribe
  }}
}}
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
                let _ = Command::new("kill").arg("-KILL").arg(pid).status();
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
