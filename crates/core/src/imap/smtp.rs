//! SMTP submission via lettre's low-level async connection, so login errors
//! can be reported separately from delivery errors.

use std::time::Duration;

use lettre::address::Envelope;
use lettre::transport::smtp::authentication::{Credentials, Mechanism};
use lettre::transport::smtp::client::{AsyncSmtpConnection, TlsParameters};
use lettre::transport::smtp::extension::ClientId;

use super::ServerSettings;
use crate::error::{Error, Result};

const TIMEOUT: Duration = Duration::from_secs(60);

fn err(e: impl std::fmt::Display) -> Error {
    Error::Network(format!("SMTP: {e}"))
}

async fn open(s: &ServerSettings, password: &str) -> Result<AsyncSmtpConnection> {
    let host = s.host.trim();
    if host.is_empty() {
        return Err(Error::invalid("SMTP-Server fehlt."));
    }
    let hello = ClientId::Domain("outlook-classic.local".into());
    let tls = || TlsParameters::new_rustls(host.to_string()).map_err(err);
    let wrapper = if s.security == "ssl" { Some(tls()?) } else { None };
    let mut conn = AsyncSmtpConnection::connect_tokio1((host, s.port), Some(TIMEOUT), &hello, wrapper, None)
        .await
        .map_err(|e| Error::Network(format!("Keine Verbindung zum SMTP-Server {host}:{}: {e}", s.port)))?;
    if s.security == "starttls" {
        if !conn.can_starttls() {
            return Err(Error::Network(format!("{host} unterstützt kein STARTTLS")));
        }
        conn.starttls(tls()?, &hello).await.map_err(err)?;
    }
    if !s.username.is_empty() {
        let creds = Credentials::new(s.username.clone(), password.to_string());
        conn.auth(&[Mechanism::Plain, Mechanism::Login], &creds)
            .await
            .map_err(|e| {
                if e.is_permanent() || e.is_transient() {
                    Error::Auth("SMTP-Anmeldung fehlgeschlagen: Benutzername oder Passwort falsch.".into())
                } else {
                    err(e)
                }
            })?;
    }
    Ok(conn)
}

/// Connects and logs in (used by "Verbindung testen").
pub async fn check(s: &ServerSettings, password: &str) -> Result<()> {
    let mut c = open(s, password).await?;
    let _ = c.quit().await;
    Ok(())
}

pub async fn send(s: &ServerSettings, password: &str, envelope: &Envelope, raw: &[u8]) -> Result<()> {
    let mut c = open(s, password).await?;
    c.send(envelope, raw)
        .await
        .map_err(|e| Error::Other(format!("Versand abgelehnt: {e}")))?;
    let _ = c.quit().await;
    Ok(())
}
