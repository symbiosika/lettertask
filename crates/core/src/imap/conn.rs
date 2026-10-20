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

use super::ServerSettings;
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
}

impl Conn {
    /// The session, after discarding queued unsolicited responses (async-imap
    /// buffers them in a bounded channel that would otherwise fill up).
    pub fn s(&mut self) -> &mut Session {
        while self.session.unsolicited_responses.try_recv().is_ok() {}
        &mut self.session
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

pub fn imap_err(e: async_imap::error::Error) -> Error {
    use async_imap::error::Error as E;
    match e {
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

/// Opens a connection and logs in.
pub async fn connect(s: &ServerSettings, password: &str) -> Result<Conn> {
    let host = s.host.trim();
    if host.is_empty() {
        return Err(Error::invalid("IMAP-Server fehlt."));
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
            timed(client.run_command_and_check_ok("STARTTLS", None)).await?;
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
    let session = tokio::time::timeout(OP_TIMEOUT, client.login(&s.username, password))
        .await
        .map_err(|_| Error::Network("Zeitüberschreitung bei der IMAP-Anmeldung".into()))?
        .map_err(|(e, _)| match e {
            async_imap::error::Error::No(_) | async_imap::error::Error::Bad(_) => Error::Auth(
                "IMAP-Anmeldung fehlgeschlagen: Benutzername oder Passwort falsch.".into(),
            ),
            other => imap_err(other),
        })?;
    let mut conn = Conn {
        session,
        selected: None,
        can_move: false,
        uidplus: false,
    };
    if let Ok(caps) = timed(conn.session.capabilities()).await {
        conn.can_move = caps.has_str("MOVE");
        conn.uidplus = caps.has_str("UIDPLUS");
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
