//! IMAP/SMTP accounts: setup, credentials, connection handling.

use std::sync::Arc;

use serde_json::{json, Value};

use super::Service;
use crate::error::{Error, Result};
use crate::imap::conn::{self, Conn};
use crate::imap::discover::{self, Discovered};
use crate::imap::{self as imapmod, smtp, ImapAccountInput};
use crate::model::{Account, Mailbox};

const SETTINGS_KEY: &str = "imap";

/// Exclusive use of an account's IMAP session.
pub(crate) struct ImapGuard(tokio::sync::OwnedMutexGuard<Option<Conn>>);

impl ImapGuard {
    pub(crate) fn conn(&mut self) -> &mut Conn {
        self.0.as_mut().expect("connected")
    }

    /// Passes `r` through; drops the session after connection errors.
    pub(crate) fn finish<T>(mut self, r: Result<T>) -> Result<T> {
        match r {
            Err(e) if e.is_network() => {
                *self.0 = None;
                Err(conn::unreadable_to_rejection(e))
            }
            r => r,
        }
    }
}

/// Runs a block with `$c: &mut Conn` on the account's IMAP session.
macro_rules! with_imap {
    ($svc:expr, $acc:expr, |$c:ident| $body:block) => {{
        let mut guard = $svc.imap_conn($acc).await?;
        let $c: &mut $crate::imap::conn::Conn = guard.conn();
        let r: $crate::error::Result<_> = async { $body }.await;
        guard.finish(r)
    }};
}
pub(crate) use with_imap;

fn pass_key(account_id: &str) -> String {
    format!("imap-pass:{account_id}")
}

fn smtp_pass_key(account_id: &str) -> String {
    format!("smtp-pass:{account_id}")
}

fn validate(input: &ImapAccountInput) -> Result<()> {
    if !input.email.contains('@') {
        return Err(Error::invalid("Bitte eine gültige E-Mail-Adresse angeben."));
    }
    if input.imap.host.trim().is_empty() || input.smtp.host.trim().is_empty() {
        return Err(Error::invalid("IMAP- und SMTP-Server müssen angegeben werden."));
    }
    for s in [&input.imap.security, &input.smtp.security] {
        if !matches!(s.as_str(), "ssl" | "starttls" | "none") {
            return Err(Error::invalid(format!("Unbekannte Verschlüsselung: {s}")));
        }
    }
    Ok(())
}

impl Service {
    pub(crate) fn imap_settings(&self, account_id: &str) -> Result<ImapAccountInput> {
        self.store
            .get_account_json::<ImapAccountInput>(account_id, SETTINGS_KEY)?
            .ok_or_else(|| Error::NotFound(format!("IMAP-Konto {account_id}")))
    }

    pub(crate) fn imap_password(&self, account_id: &str) -> Result<String> {
        self.opts
            .token_store
            .load(&pass_key(account_id))
            .ok_or_else(|| Error::Auth("Kein Passwort gespeichert. Bitte in den Kontoeinstellungen eingeben.".into()))
    }

    pub(crate) fn smtp_password(&self, account_id: &str) -> Result<String> {
        match self.opts.token_store.load(&smtp_pass_key(account_id)) {
            Some(p) => Ok(p),
            None => self.imap_password(account_id),
        }
    }

    pub(crate) fn forget_imap_account(&self, account_id: &str) {
        if imapmod::is_imap_id(account_id) {
            self.opts.token_store.delete(&pass_key(account_id));
            self.opts.token_store.delete(&smtp_pass_key(account_id));
            self.imap.forget(account_id);
        }
    }

    /// The account's cached IMAP session (connecting on demand). A reused
    /// session is checked with NOOP first, so sessions the server dropped
    /// while idle are replaced transparently.
    pub(crate) async fn imap_conn(&self, account_id: &str) -> Result<ImapGuard> {
        let slot = self.imap.slot(account_id);
        let mut guard = slot.lock_owned().await;
        if let Some(c) = guard.as_mut() {
            if !c.alive().await {
                *guard = None;
            }
        }
        if guard.is_none() {
            *guard = Some(self.imap_connect(account_id).await?);
        }
        Ok(ImapGuard(guard))
    }

    /// Replaces a session that became unusable inside a `with_imap!` block.
    pub(crate) async fn imap_reconnect(&self, account_id: &str, c: &mut Conn) -> Result<()> {
        *c = self.imap_connect(account_id).await?;
        Ok(())
    }

    async fn imap_connect(&self, account_id: &str) -> Result<Conn> {
        let s = self.imap_settings(account_id)?;
        let pw = self.imap_password(account_id)?;
        conn::connect(&s.imap, &pw).await
    }

    pub(crate) async fn discover_imap(&self, email: &str) -> Discovered {
        discover::discover(&self.http, email).await
    }

    /// Fills empty passwords of an existing account from the keychain.
    fn resolve_passwords(&self, input: &ImapAccountInput) -> (String, String) {
        let id = imapmod::account_id_for(&input.email);
        let imap = if input.password.is_empty() {
            self.imap_password(&id).unwrap_or_default()
        } else {
            input.password.clone()
        };
        let smtp = match &input.smtp_password {
            Some(p) if !p.is_empty() => p.clone(),
            _ if input.password.is_empty() => self.smtp_password(&id).unwrap_or_else(|_| imap.clone()),
            _ => imap.clone(),
        };
        (imap, smtp)
    }

    pub(crate) async fn test_imap_account(&self, input: &ImapAccountInput) -> Value {
        let (imap_pw, smtp_pw) = self.resolve_passwords(input);
        let imap = match conn::connect(&input.imap, &imap_pw).await {
            Ok(mut c) => {
                let _ = conn::timed(c.s().logout()).await;
                Ok(())
            }
            Err(e) => Err(e),
        };
        let smtp = smtp::check(&input.smtp, &smtp_pw).await;
        json!({
            "imapOk": imap.is_ok(),
            "smtpOk": smtp.is_ok(),
            "imapError": imap.err().map(|e| e.to_string()),
            "smtpError": smtp.err().map(|e| e.to_string()),
        })
    }

    pub(crate) async fn add_imap_account(self: &Arc<Self>, input: ImapAccountInput) -> Result<Account> {
        validate(&input)?;
        let id = imapmod::account_id_for(&input.email);
        if self.store.list_accounts()?.iter().any(|a| a.id == id) {
            return Err(Error::invalid(format!("Das Konto {} ist bereits eingerichtet.", input.email)));
        }
        if input.password.is_empty() {
            return Err(Error::invalid("Bitte das Passwort angeben."));
        }
        self.store_imap_account(&id, input).await
    }

    pub(crate) async fn update_imap_account(self: &Arc<Self>, account_id: &str, mut input: ImapAccountInput) -> Result<Account> {
        validate(&input)?;
        let existing = self.imap_settings(account_id)?;
        if !existing.email.eq_ignore_ascii_case(&input.email) {
            return Err(Error::invalid("Die E-Mail-Adresse eines Kontos kann nicht geändert werden."));
        }
        let (imap_pw, smtp_pw) = self.resolve_passwords(&input);
        input.password = imap_pw;
        input.smtp_password = (smtp_pw != input.password).then_some(smtp_pw);
        self.imap.forget(account_id);
        self.store_imap_account(account_id, input).await
    }

    /// Verifies both logins, then stores settings and credentials.
    async fn store_imap_account(self: &Arc<Self>, id: &str, input: ImapAccountInput) -> Result<Account> {
        let smtp_pw = input.smtp_password.clone().filter(|p| !p.is_empty()).unwrap_or_else(|| input.password.clone());
        let mut c = conn::connect(&input.imap, &input.password).await?;
        smtp::check(&input.smtp, &smtp_pw).await?;

        let email = input.email.trim().to_string();
        let name = if input.display_name.trim().is_empty() {
            email.clone()
        } else {
            input.display_name.trim().to_string()
        };
        self.store.upsert_account(id, &email, &name)?;
        self.store.upsert_mailbox(&Mailbox {
            id: id.to_string(),
            account_id: id.to_string(),
            address: email.clone(),
            display_name: name,
            kind: "primary".into(),
        })?;
        self.opts.token_store.save(&pass_key(id), &input.password)?;
        if smtp_pw != input.password {
            self.opts.token_store.save(&smtp_pass_key(id), &smtp_pw)?;
        } else {
            self.opts.token_store.delete(&smtp_pass_key(id));
        }
        let mut stored = input.clone();
        stored.password = String::new();
        stored.smtp_password = None;
        stored.email = email;
        self.store.put_account_json(id, SETTINGS_KEY, &stored)?;
        // Reuse the verified session.
        c.selected = None;
        *self.imap.slot(id).lock().await = Some(c);

        self.sync_folders(id).await?;
        if let Some(inbox) = self.store.folder_by_well_known(id, "inbox")? {
            let _ = self.sync_folder(id, &inbox.id, true).await;
        }
        self.emit("accounts-changed", json!({}));
        self.store
            .list_accounts()?
            .into_iter()
            .find(|a| a.id == id)
            .ok_or_else(|| Error::NotFound("Konto".into()))
    }

    pub(crate) fn get_imap_account(&self, account_id: &str) -> Result<ImapAccountInput> {
        let mut s = self.imap_settings(account_id)?;
        s.password = String::new();
        s.smtp_password = None;
        Ok(s)
    }
}
