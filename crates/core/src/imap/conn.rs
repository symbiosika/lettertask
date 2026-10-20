//! IMAP connections (implicit TLS, STARTTLS or plain) and a per-account
//! session cache. Sessions are reused and re-created after any error.

use std::collections::HashMap;
use std::fmt;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tokio_rustls::TlsConnector;

use super::{Credential, ServerSettings};
use crate::error::{Error, Result};

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
pub const OP_TIMEOUT: Duration = Duration::from_secs(120);

/// TCP or TLS stream behind one type, as async-imap needs a concrete stream.
pub enum ImapStream {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::client::TlsStream<TcpStream>>),
}

impl fmt::Debug for ImapStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ImapStream::Plain(_) => "ImapStream::Plain",
            ImapStream::Tls(_) => "ImapStream::Tls",
        })
    }
}

impl AsyncRead for ImapStream {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            ImapStream::Plain(s) => Pin::new(s).poll_read(cx, buf),
            ImapStream::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for ImapStream {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            ImapStream::Plain(s) => Pin::new(s).poll_write(cx, buf),
            ImapStream::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            ImapStream::Plain(s) => Pin::new(s).poll_flush(cx),
            ImapStream::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            ImapStream::Plain(s) => Pin::new(s).poll_shutdown(cx),
            ImapStream::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

pub type Session = async_imap::Session<ImapStream>;

/// A logged-in session plus what we know about it.
pub struct Conn {
    pub session: Session,
    pub selected: Option<String>,
    pub can_move: bool,
    pub uidplus: bool,
    /// Non-synchronizing literals (LITERAL+ / LITERAL-) are allowed.
    pub nonsync_literals: bool,
}

impl Conn {
    /// The session, after discarding queued unsolicited responses (async-imap
    /// buffers them in a bounded channel that would otherwise fill up).
    pub fn s(&mut self) -> &mut Session {
        while self.session.unsolicited_responses.try_recv().is_ok() {}
        &mut self.session
    }

    /// (messages, unseen) of `folder` via STATUS. async-imap matches the
    /// STATUS response against the unescaped name, so for names with `"` or
    /// `\` the response lands in the unsolicited queue; pick it up there.
    pub async fn status_counts(&mut self, folder: &str) -> Result<(u32, u32)> {
        let st = timed(self.s().status(folder, "(MESSAGES UNSEEN)")).await?;
        if !folder.contains(['"', '\\']) {
            return Ok((st.exists, st.unseen.unwrap_or(0)));
        }
        let wanted = escape_name(folder);
        let (mut exists, mut unseen) = (st.exists, st.unseen.unwrap_or(0));
        while let Ok(r) = self.session.unsolicited_responses.try_recv() {
            if let async_imap::types::UnsolicitedResponse::Status { mailbox, attributes } = r {
                if mailbox == wanted || mailbox == folder {
                    for a in attributes {
                        match a {
                            async_imap::imap_proto::StatusAttribute::Messages(n) => exists = n,
                            async_imap::imap_proto::StatusAttribute::Unseen(n) => unseen = n,
                            _ => {}
                        }
                    }
                }
            }
        }
        Ok((exists, unseen))
    }

    /// NOOP round trip; false when the session is unusable (error, timeout,
    /// or the server announced BYE, e.g. because another client deleted the
    /// selected folder).
    pub async fn alive(&mut self) -> bool {
        let ok = tokio::time::timeout(Duration::from_secs(10), self.s().noop())
            .await
            .is_ok_and(|r| r.is_ok());
        let mut bye = false;
        while let Ok(r) = self.session.unsolicited_responses.try_recv() {
            if let async_imap::types::UnsolicitedResponse::Other(d) = r {
                if matches!(
                    d.parsed(),
                    async_imap::imap_proto::Response::Data {
                        status: async_imap::imap_proto::Status::Bye,
                        ..
                    }
                ) {
                    bye = true;
                }
            }
        }
        ok && !bye
    }

    /// SELECTs `folder` and makes sure its UIDVALIDITY still is `uv`: after
    /// a change the cached UIDs may point at different messages.
    pub async fn select_checked(&mut self, folder: &str, uv: u32) -> Result<async_imap::types::Mailbox> {
        let mb = self.select(folder).await?;
        if mb.uid_validity.unwrap_or(0) != uv {
            return Err(Error::NotFound(
                "Nachricht (der Ordner wurde auf dem Server neu aufgebaut, bitte aktualisieren)".into(),
            ));
        }
        Ok(mb)
    }

    /// SELECTs `folder`; returns its status.
    pub async fn select(&mut self, folder: &str) -> Result<async_imap::types::Mailbox> {
        let mb = timed(self.s().select(folder)).await.inspect_err(|_| {
            self.selected = None;
        })?;
        self.selected = Some(folder.to_string());
        Ok(mb)
    }
}

/// Mailbox names (and delimiters) from LIST keep the quoted-string escapes
/// (imap-proto returns the raw slice): `Kunde \"M\"` → `Kunde "M"`.
/// Commands quote names again themselves.
pub fn unescape_name(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            if let Some(n) = it.next() {
                out.push(n);
                continue;
            }
        }
        out.push(c);
    }
    out
}

/// Inverse of [`unescape_name`] (the form servers send in responses).
pub fn escape_name(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn tls_connector() -> TlsConnector {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = ClientConfig::builder_with_provider(Arc::new(tokio_rustls::rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("TLS-Versionen")
        .with_root_certificates(roots)
        .with_no_client_auth();
    TlsConnector::from(Arc::new(config))
}

async fn tls_wrap(tcp: TcpStream, host: &str) -> Result<ImapStream> {
    let name = ServerName::try_from(host.to_string()).map_err(|_| Error::invalid(format!("Ungültiger Servername: {host}")))?;
    let tls = tls_connector()
        .connect(name, tcp)
        .await
        .map_err(|e| Error::Network(format!("TLS-Verbindung zu {host} fehlgeschlagen: {e}")))?;
    Ok(ImapStream::Tls(Box::new(tls)))
}

pub fn io_err(e: impl fmt::Display) -> Error {
    Error::Network(format!("IMAP: {e}"))
}

const UNREADABLE: &str = "Unlesbare Antwort des IMAP-Servers";

/// The server sent a line imap-proto cannot parse (e.g. a NO whose text
/// contains a UTF-8 folder name, as Dovecot sends). The line stays in the
/// read buffer, so the session is unusable and must be replaced; the
/// command itself was usually just rejected.
pub fn is_unreadable(e: &Error) -> bool {
    matches!(e, Error::Network(m) if m.starts_with(UNREADABLE))
}

/// After the session was dropped: an unreadable tagged NO/BAD is reported as
/// the rejection it is, not as a connection problem.
pub fn unreadable_to_rejection(e: Error) -> Error {
    let Error::Network(m) = &e else { return e };
    let Some(line) = m.strip_prefix(UNREADABLE).map(|l| l.trim_start_matches(':').trim()) else {
        return e;
    };
    let mut words = line.splitn(3, ' ');
    let (_tag, status, text) = (words.next(), words.next(), words.next().unwrap_or(""));
    match status {
        Some(s) if s.eq_ignore_ascii_case("NO") => Error::Other(format!("IMAP-Server lehnt ab: {text}")),
        Some(s) if s.eq_ignore_ascii_case("BAD") => Error::Other(format!("IMAP-Befehl ungültig: {text}")),
        _ => e,
    }
}

/// The rejected command referred to a folder that does not exist.
pub fn is_nonexistent(e: &Error) -> bool {
    let m = e.to_string().to_ascii_lowercase();
    m.contains("nonexistent") || m.contains("doesn't exist") || m.contains("does not exist") || m.contains("unknown folder")
}

pub fn imap_err(e: async_imap::error::Error) -> Error {
    use async_imap::error::Error as E;
    match e {
        E::Io(e) if e.to_string().contains("during parsing of") => {
            let s = e.to_string();
            let line = s
                .split_once("during parsing of \"")
                .map(|(_, l)| l.split("\\r\\n").next().unwrap_or(l))
                .unwrap_or("")
                .chars()
                .take(300)
                .collect::<String>();
            Error::Network(format!("{UNREADABLE}: {line}"))
        }
        E::Io(e) => Error::Network(format!("IMAP-Verbindung: {e}")),
        E::ConnectionLost => Error::Network("IMAP-Verbindung unterbrochen".into()),
        E::No(m) => Error::Other(format!("IMAP-Server lehnt ab: {m}")),
        E::Bad(m) => Error::Other(format!("IMAP-Befehl ungültig: {m}")),
        other => Error::Other(format!("IMAP: {other}")),
    }
}

/// Runs an IMAP future with the operation timeout.
pub async fn timed<T>(f: impl std::future::Future<Output = async_imap::error::Result<T>>) -> Result<T> {
    tokio::time::timeout(OP_TIMEOUT, f)
        .await
        .map_err(|_| Error::Network("Zeitüberschreitung beim IMAP-Server".into()))?
        .map_err(imap_err)
}

/// Opens a connection and logs in with a password.
pub async fn connect(s: &ServerSettings, password: &str) -> Result<Conn> {
    connect_with(s, &Credential::Password(password.to_string())).await
}

/// SASL XOAUTH2: the initial response, then an empty one to the error
/// challenge a server sends when it rejects the token.
struct XOAuth2 {
    response: Option<String>,
}

impl async_imap::Authenticator for XOAuth2 {
    type Response = String;
    fn process(&mut self, _challenge: &[u8]) -> String {
        self.response.take().unwrap_or_default()
    }
}

/// Opens a connection and logs in.
pub async fn connect_with(s: &ServerSettings, credential: &Credential) -> Result<Conn> {
    let host = s.host.trim();
    if host.is_empty() {
        return Err(Error::invalid("IMAP-Server fehlt."));
    }
    // Never fall back to plain text for an unknown setting.
    if !matches!(s.security.as_str(), "ssl" | "starttls" | "none") {
        return Err(Error::invalid(format!("Unbekannte Verschlüsselung: {}", s.security)));
    }
    let tcp = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((host, s.port)))
        .await
        .map_err(|_| Error::Network(format!("Zeitüberschreitung beim Verbinden mit {host}:{}", s.port)))?
        .map_err(|e| Error::Network(format!("Keine Verbindung zu {host}:{}: {e}", s.port)))?;
    let stream = match s.security.as_str() {
        "ssl" => tls_wrap(tcp, host).await?,
        "starttls" => {
            let mut client = async_imap::Client::new(ImapStream::Plain(tcp));
            read_greeting(&mut client).await?;
            timed(client.run_command_and_check_ok("STARTTLS", None)).await.map_err(|e| match e {
                Error::Network(_) => e,
                _ => Error::Network(format!("{host} unterstützt kein STARTTLS ({e})")),
            })?;
            let ImapStream::Plain(tcp) = client.into_inner() else {
                unreachable!("plain stream before STARTTLS")
            };
            tls_wrap(tcp, host).await?
        }
        _ => ImapStream::Plain(tcp),
    };
    let mut client = async_imap::Client::new(stream);
    if s.security != "starttls" {
        read_greeting(&mut client).await?;
    }
    let login = async {
        match credential {
            Credential::Password(password) => client.login(&s.username, password).await,
            Credential::OAuth2(token) => {
                let response = Some(crate::google::xoauth2(&s.username, token));
                client.authenticate("XOAUTH2", XOAuth2 { response }).await
            }
        }
    };
    let rejected = match credential {
        Credential::Password(_) => "IMAP-Anmeldung fehlgeschlagen: Benutzername oder Passwort falsch.",
        Credential::OAuth2(_) => "IMAP-Anmeldung mit Google fehlgeschlagen. Bitte das Konto erneut mit Google anmelden.",
    };
    let session = tokio::time::timeout(OP_TIMEOUT, login)
        .await
        .map_err(|_| Error::Network("Zeitüberschreitung bei der IMAP-Anmeldung".into()))?
        .map_err(|(e, _)| match e {
            async_imap::error::Error::No(_) | async_imap::error::Error::Bad(_) => Error::Auth(rejected.into()),
            other => imap_err(other),
        })?;
    let mut conn = Conn {
        session,
        selected: None,
        can_move: false,
        uidplus: false,
        nonsync_literals: false,
    };
    if let Ok(caps) = timed(conn.session.capabilities()).await {
        conn.can_move = caps.has_str("MOVE");
        conn.uidplus = caps.has_str("UIDPLUS");
        conn.nonsync_literals = caps.has_str("LITERAL+") || caps.has_str("LITERAL-");
    }
    Ok(conn)
}

async fn read_greeting(client: &mut async_imap::Client<ImapStream>) -> Result<()> {
    tokio::time::timeout(CONNECT_TIMEOUT, client.read_response())
        .await
        .map_err(|_| Error::Network("IMAP-Server antwortet nicht".into()))?
        .map_err(io_err)?
        .ok_or_else(|| Error::Network("IMAP-Server hat die Verbindung geschlossen".into()))?;
    Ok(())
}

/// One cached session per account.
#[derive(Default)]
pub struct Pool {
    conns: std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<Option<Conn>>>>>,
}

impl Pool {
    pub fn slot(&self, account_id: &str) -> Arc<tokio::sync::Mutex<Option<Conn>>> {
        self.conns
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(account_id.to_string())
            .or_default()
            .clone()
    }

    pub fn forget(&self, account_id: &str) {
        self.conns
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(account_id);
    }
}
