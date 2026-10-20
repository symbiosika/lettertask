//! Minimal SMTP sink for tests: EHLO/HELO, AUTH PLAIN/LOGIN, MAIL, RCPT,
//! DATA, RSET, NOOP, QUIT. No TLS. Accepted messages are kept in memory.

use std::sync::{Arc, Mutex};

use base64::Engine;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

#[derive(Debug, Clone)]
pub struct Received {
    pub from: String,
    pub rcpts: Vec<String>,
    pub raw: String,
}

pub struct SmtpSink {
    pub port: u16,
    messages: Arc<Mutex<Vec<Received>>>,
    task: tokio::task::JoinHandle<()>,
}

impl SmtpSink {
    pub fn messages(&self) -> Vec<Received> {
        self.messages.lock().unwrap().clone()
    }
}

impl Drop for SmtpSink {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Starts the sink on 127.0.0.1; `password` is the only accepted password
/// (any user name).
pub async fn spawn_sink(password: &str) -> SmtpSink {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind smtp sink");
    let port = listener.local_addr().unwrap().port();
    let messages = Arc::new(Mutex::new(Vec::new()));
    let store = messages.clone();
    let password = password.to_string();
    let task = tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else { return };
            let store = store.clone();
            let password = password.clone();
            tokio::spawn(async move {
                let _ = session(sock, store, password).await;
            });
        }
    });
    SmtpSink { port, messages, task }
}

fn b64(s: &str) -> String {
    let bytes = base64::engine::general_purpose::STANDARD.decode(s.trim()).unwrap_or_default();
    String::from_utf8_lossy(&bytes).into_owned()
}

async fn session(sock: tokio::net::TcpStream, store: Arc<Mutex<Vec<Received>>>, password: String) -> std::io::Result<()> {
    let (r, mut w) = sock.into_split();
    let mut lines = BufReader::new(r);
    w.write_all(b"220 sink.local ESMTP test sink\r\n").await?;
    let (mut from, mut rcpts, mut authed) = (String::new(), Vec::new(), false);
    let mut line = String::new();
    loop {
        line.clear();
        if lines.read_line(&mut line).await? == 0 {
            return Ok(());
        }
        let cmd = line.trim_end().to_string();
        let upper = cmd.to_uppercase();
        let reply: String = if upper.starts_with("EHLO") {
            "250-sink.local\r\n250-AUTH PLAIN LOGIN\r\n250-8BITMIME\r\n250 SIZE 52428800\r\n".into()
        } else if upper.starts_with("HELO") {
            "250 sink.local\r\n".into()
        } else if upper.starts_with("AUTH PLAIN") {
            let arg = cmd.split_whitespace().nth(2).map(str::to_string);
            let arg = match arg {
                Some(a) => a,
                None => {
                    w.write_all(b"334 \r\n").await?;
                    line.clear();
                    lines.read_line(&mut line).await?;
                    line.trim().to_string()
                }
            };
            let decoded = b64(&arg);
            let pw = decoded.split('\0').nth(2).unwrap_or("");
            authed = pw == password;
            if authed { "235 2.7.0 Authentication successful\r\n" } else { "535 5.7.8 Authentication failed\r\n" }.into()
        } else if upper.starts_with("AUTH LOGIN") {
            w.write_all(b"334 VXNlcm5hbWU6\r\n").await?;
            line.clear();
            lines.read_line(&mut line).await?;
            w.write_all(b"334 UGFzc3dvcmQ6\r\n").await?;
            line.clear();
            lines.read_line(&mut line).await?;
            authed = b64(&line) == password;
            if authed { "235 2.7.0 Authentication successful\r\n" } else { "535 5.7.8 Authentication failed\r\n" }.into()
        } else if upper.starts_with("MAIL FROM:") {
            if !authed {
                "530 5.7.0 Authentication required\r\n".into()
            } else {
                from = cmd[10..].trim().trim_matches(['<', '>']).split_whitespace().next().unwrap_or("").trim_matches(['<', '>']).to_string();
                rcpts.clear();
                "250 2.1.0 Ok\r\n".into()
            }
        } else if upper.starts_with("RCPT TO:") {
            rcpts.push(cmd[8..].trim().trim_matches(['<', '>']).to_string());
            "250 2.1.5 Ok\r\n".into()
        } else if upper == "DATA" {
            w.write_all(b"354 End data with <CR><LF>.<CR><LF>\r\n").await?;
            let mut raw = String::new();
            loop {
                line.clear();
                if lines.read_line(&mut line).await? == 0 {
                    return Ok(());
                }
                if line == ".\r\n" || line == ".\n" {
                    break;
                }
                let l = line.strip_prefix('.').unwrap_or(&line);
                raw.push_str(l);
            }
            store.lock().unwrap().push(Received {
                from: from.clone(),
                rcpts: std::mem::take(&mut rcpts),
                raw,
            });
            "250 2.0.0 Ok: queued\r\n".into()
        } else if upper == "RSET" {
            rcpts.clear();
            "250 2.0.0 Ok\r\n".into()
        } else if upper == "NOOP" {
            "250 2.0.0 Ok\r\n".into()
        } else if upper == "QUIT" {
            w.write_all(b"221 2.0.0 Bye\r\n").await?;
            return Ok(());
        } else {
            "502 5.5.2 Command not recognized\r\n".into()
        };
        w.write_all(reply.as_bytes()).await?;
    }
}
