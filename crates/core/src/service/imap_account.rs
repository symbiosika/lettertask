//! IMAP/SMTP accounts: setup, credentials, connection handling.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use super::Service;
use crate::auth::TokenSet;
use crate::error::{Error, Result};
use crate::google::{self, GoogleConfig};
use crate::imap::conn::{self, Conn};
use crate::imap::discover::{self, Discovered};
use crate::imap::{self as imapmod, smtp, Credential, ImapAccountInput, ServerSettings};
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

/// Refresh token of an account signed in with Google.
fn google_key(account_id: &str) -> String {
    format!("google-refresh:{account_id}")
}

/// Server settings of a Gmail / Google Workspace mailbox.
fn google_input(email: &str, display_name: &str) -> ImapAccountInput {
    let server = |host: &str| ServerSettings {
        host: host.into(),
        port: if host == google::IMAP_HOST { 993 } else { 465 },
        security: "ssl".into(),
        username: email.into(),
    };
    ImapAccountInput {
        email: email.into(),
        display_name: display_name.into(),
        imap: server(google::IMAP_HOST),
        smtp: server(google::SMTP_HOST),
        password: String::new(),
        smtp_password: None,
        auth: imapmod::AUTH_GOOGLE.into(),
    }
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
    if !matches!(input.auth.as_str(), "" | "password" | imapmod::AUTH_GOOGLE) {
        return Err(Error::invalid(format!("Unbekannte Anmeldeart: {}", input.auth)));
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
            self.forget_google_tokens(account_id);
            self.imap.forget(account_id);
        }
    }

    fn forget_google_tokens(&self, account_id: &str) {
        self.opts.token_store.delete(&google_key(account_id));
        self.google_tokens.lock().unwrap().remove(account_id);
    }

    pub(crate) fn google_config(&self) -> GoogleConfig {
        let c = self.config();
        GoogleConfig::new(&c.google_client_id, &c.google_client_secret)
    }

    /// Valid access token of a Google account, refreshed when needed.
    pub(crate) async fn google_access_token(&self, account_id: &str) -> Result<String> {
        if let Some(t) = self.google_tokens.lock().unwrap().get(account_id) {
            if !t.needs_refresh() {
                return Ok(t.access_token.clone());
            }
        }
        let rt = self
            .opts
            .token_store
            .load(&google_key(account_id))
            .ok_or_else(|| Error::Auth("Nicht bei Google angemeldet. Bitte das Konto erneut mit Google anmelden.".into()))?;
        let t = google::refresh(&self.http, &self.google_config(), &rt).await?;
        if t.refresh_token != rt {
            self.opts.token_store.save(&google_key(account_id), &t.refresh_token)?;
        }
        let access = t.access_token.clone();
        self.google_tokens.lock().unwrap().insert(account_id.to_string(), t);
        Ok(access)
    }

    /// Login for the account's IMAP (`smtp` = false) or SMTP server.
    pub(crate) async fn imap_credential(&self, account_id: &str, smtp: bool) -> Result<Credential> {
        if self.imap_settings(account_id)?.is_google() {
            return Ok(Credential::OAuth2(self.google_access_token(account_id).await?));
        }
        Ok(Credential::Password(if smtp {
            self.smtp_password(account_id)?
        } else {
            self.imap_password(account_id)?
        }))
    }

    /// Whether the account's mailbox lives at Google (Gmail semantics:
    /// labels, "All Mail", sent copies saved by the server).
    pub(crate) fn is_gmail(&self, account_id: &str) -> bool {
        self.imap_settings(account_id)
            .is_ok_and(|s| s.is_google() || google::is_google_host(&s.imap.host))
    }

    /// Accounts for the UI, IMAP accounts with their sign-in kind.
    pub(crate) fn list_accounts_for_ui(&self) -> Result<Vec<Account>> {
        let mut accounts = self.store.list_accounts()?;
        for a in accounts.iter_mut().filter(|a| a.kind == "imap") {
            if let Ok(s) = self.imap_settings(&a.id) {
                a.auth = s.auth;
            }
        }
        Ok(accounts)
    }

    /// "Mit Google anmelden": sign-in in the browser, then the mailbox is
    /// set up via IMAP/SMTP. Signing in again renews an existing account.
    pub(crate) async fn add_google_account(self: &Arc<Self>, login_hint: Option<String>) -> Result<Account> {
        let host = self.host.clone();
        let tokens = google::interactive_login(
            &self.http,
            &self.google_config(),
            login_hint.as_deref(),
            move |url| super::open_login_url(host.as_ref(), &url),
            Duration::from_secs(300),
        )
        .await?;
        self.finish_google_login(tokens).await
    }

    /// Registers the Google account the tokens belong to (or renews its
    /// sign-in), verifies IMAP and SMTP and syncs the inbox.
    pub async fn finish_google_login(self: &Arc<Self>, tokens: TokenSet) -> Result<Account> {
        let info = google::user_info(&self.http, &self.google_config(), &tokens.access_token).await?;
        let email = info.email.trim().to_string();
        let id = imapmod::account_id_for(&email);
        let existing = self.imap_settings(&id).ok();
        let name = existing
            .as_ref()
            .map(|e| e.display_name.clone())
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| if info.name.trim().is_empty() { email.clone() } else { info.name.trim().to_string() });
        let previous = self.opts.token_store.load(&google_key(&id));
        self.opts.token_store.save(&google_key(&id), &tokens.refresh_token)?;
        self.google_tokens.lock().unwrap().insert(id.clone(), tokens);
        self.imap.forget(&id);
        let r = self.store_imap_account(&id, google_input(&email, &name)).await;
        if r.is_err() {
            // Leave the previous sign-in as it was.
            match previous {
                Some(p) => {
                    let _ = self.opts.token_store.save(&google_key(&id), &p);
                    self.google_tokens.lock().unwrap().remove(&id);
                }
                None => self.forget_google_tokens(&id),
            }
        }
        r
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
        let credential = self.imap_credential(account_id, false).await?;
        conn::connect_with(&s.imap, &credential).await
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
        let (imap_cred, smtp_cred) = if input.is_google() {
            match self.google_access_token(&imapmod::account_id_for(&input.email)).await {
                Ok(t) => (Credential::OAuth2(t.clone()), Credential::OAuth2(t)),
                Err(e) => {
                    let e = e.to_string();
                    return json!({"imapOk": false, "smtpOk": false, "imapError": e, "smtpError": e});
                }
            }
        } else {
            let (imap_pw, smtp_pw) = self.resolve_passwords(input);
            (Credential::Password(imap_pw), Credential::Password(smtp_pw))
        };
        let imap = match conn::connect_with(&input.imap, &imap_cred).await {
            Ok(mut c) => {
                let _ = conn::timed(c.s().logout()).await;
                Ok(())
            }
            Err(e) => Err(e),
        };
        let smtp = smtp::check_with(&input.smtp, &smtp_cred).await;
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
        if input.is_google() {
            return Err(Error::invalid("Google-Konten werden über „Mit Google anmelden“ eingerichtet."));
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
        if existing.is_google() {
            // Servers and login are fixed; only the name can change.
            let mut s = existing;
            s.display_name = input.display_name;
            self.imap.forget(account_id);
            return self.store_imap_account(account_id, s).await;
        }
        if input.is_google() {
            return Err(Error::invalid("Zur Google-Anmeldung bitte „Mit Google anmelden“ verwenden."));
        }
        let (imap_pw, smtp_pw) = self.resolve_passwords(&input);
        input.password = imap_pw;
        input.smtp_password = (smtp_pw != input.password).then_some(smtp_pw);
        self.imap.forget(account_id);
        self.store_imap_account(account_id, input).await
    }

    /// Verifies both logins, then stores settings and credentials. Google
    /// accounts must have their tokens stored already.
    async fn store_imap_account(self: &Arc<Self>, id: &str, input: ImapAccountInput) -> Result<Account> {
        let smtp_pw = input.smtp_password.clone().filter(|p| !p.is_empty()).unwrap_or_else(|| input.password.clone());
        let (imap_cred, smtp_cred) = if input.is_google() {
            let t = self.google_access_token(id).await?;
            (Credential::OAuth2(t.clone()), Credential::OAuth2(t))
        } else {
            (Credential::Password(input.password.clone()), Credential::Password(smtp_pw.clone()))
        };
        let mut c = conn::connect_with(&input.imap, &imap_cred).await?;
        smtp::check_with(&input.smtp, &smtp_cred).await?;

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
        if input.is_google() {
            self.opts.token_store.delete(&pass_key(id));
            self.opts.token_store.delete(&smtp_pass_key(id));
        } else {
            self.opts.token_store.save(&pass_key(id), &input.password)?;
            if smtp_pw != input.password {
                self.opts.token_store.save(&smtp_pass_key(id), &smtp_pw)?;
            } else {
                self.opts.token_store.delete(&smtp_pass_key(id));
            }
            self.forget_google_tokens(id);
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
