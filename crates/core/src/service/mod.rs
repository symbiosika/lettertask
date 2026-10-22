//! Service layer: everything the UI can do, exposed through [`Service::dispatch`].

mod calendar;
mod categories;
mod compose;
mod contacts;
mod imap_account;
mod imap_compose;
mod imap_mail;
mod lanes;
mod mail;
mod signatures;
mod snooze;
mod tasks;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use chrono::Utc;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use crate::auth::{self, OAuthConfig, TokenSet};
use crate::error::{Error, Result};
use crate::graph::GraphClient;
use crate::host::Host;
use crate::model::*;
use crate::store::Store;
use crate::tokens::TokenStore;

pub struct ServiceOptions {
    pub data_dir: PathBuf,
    pub graph_base: String,
    pub authority: String,
    /// Demo mode: login is completed automatically without a browser.
    pub demo: bool,
    pub token_store: Arc<dyn TokenStore>,
}

pub struct Service {
    opts: ServiceOptions,
    http: reqwest::Client,
    pub(crate) store: Store,
    config: RwLock<Config>,
    clients: RwLock<HashMap<String, Arc<GraphClient>>>,
    pub(crate) host: Arc<dyn Host>,
    active_folders: RwLock<HashMap<String, String>>,
    folder_locks: std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    sync_all_lock: tokio::sync::Mutex<()>,
    last_sync: RwLock<Option<String>>,
    /// Cached IMAP sessions of IMAP accounts.
    pub(crate) imap: crate::imap::conn::Pool,
    /// Access tokens of accounts signed in with Google.
    google_tokens: std::sync::Mutex<HashMap<String, TokenSet>>,
    /// Mails sent in the background, in order, to one worker (started on demand).
    send_queue: std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedSender<DraftInput>>>,
    /// Mails queued or being sent in the background.
    pending_sends: std::sync::atomic::AtomicUsize,
}

fn arg<T: DeserializeOwned>(args: &Value, name: &str) -> Result<T> {
    let v = args.get(name).cloned().unwrap_or(Value::Null);
    serde_json::from_value(v).map_err(|e| Error::invalid(format!("Parameter '{name}': {e}")))
}

/// Message of a caught panic (`panic!("…")` carries a &str or a String).
fn panic_text(p: &Box<dyn std::any::Any + Send>) -> String {
    p.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unbekannte Ursache".into())
}

/// Longest name of an account in the views.
const LABEL_MAX: usize = 60;
/// Account badge: at most this many lines of this many characters.
const BADGE_LINES: usize = 2;
const BADGE_LINE_MAX: usize = 8;

/// Name in the views: trimmed, empty = the address again.
fn clean_account_label(v: Option<String>) -> Result<Option<String>> {
    let v = v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    if v.as_ref().is_some_and(|s| s.chars().count() > LABEL_MAX || s.chars().any(char::is_control)) {
        return Err(Error::invalid(format!("Der Kontoname darf höchstens {LABEL_MAX} Zeichen lang sein.")));
    }
    Ok(v)
}

/// Badge colour "#rrggbb" and up to 2 lines of up to 8 characters (trimmed).
fn clean_account_badge(v: Option<AccountBadge>) -> Result<AccountBadge> {
    let v = v.unwrap_or_default();
    let color = v.color.map(|c| c.trim().to_ascii_lowercase()).filter(|c| !c.is_empty());
    if let Some(c) = &color {
        if c.len() != 7 || !c.starts_with('#') || !c[1..].chars().all(|ch| ch.is_ascii_hexdigit()) {
            return Err(Error::invalid("Die Farbe ist ungültig."));
        }
    }
    let lines: Vec<String> = v
        .text
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    if lines.len() > BADGE_LINES
        || lines.iter().any(|l| l.chars().count() > BADGE_LINE_MAX || l.chars().any(char::is_control))
    {
        return Err(Error::invalid(format!(
            "Höchstens {BADGE_LINES} Zeilen mit je {BADGE_LINE_MAX} Zeichen sind möglich."
        )));
    }
    let text = (!lines.is_empty()).then(|| lines.join("\n"));
    Ok(AccountBadge { color, text })
}

/// Largest accepted account picture (the UI scales it down to 128 px first).
const AVATAR_MAX: usize = 300_000;

/// Account pictures are shown as <img src>: only base64 PNG/JPEG/WebP data URLs.
fn check_avatar(v: &str) -> Result<()> {
    use base64::Engine;
    let Some((head, data)) = v.split_once(',') else {
        return Err(Error::invalid("Das Bild ist ungültig."));
    };
    if !matches!(head, "data:image/png;base64" | "data:image/jpeg;base64" | "data:image/webp;base64") {
        return Err(Error::invalid("Nur PNG-, JPG- oder WebP-Bilder sind möglich."));
    }
    if v.len() > AVATAR_MAX {
        return Err(Error::invalid("Das Bild ist zu groß."));
    }
    if base64::engine::general_purpose::STANDARD.decode(data).is_err() {
        return Err(Error::invalid("Das Bild ist ungültig."));
    }
    Ok(())
}

/// Ids become URL path segments ("…/mailFolders/{id}/messages"). "." and ".."
/// (also percent-encoded) would be normalised away and silently retarget the
/// request – e.g. emptying "folder .." would delete the whole mailbox.
fn check_ids(v: &Value) -> Result<()> {
    fn dot_segment(s: &str) -> bool {
        let t = s.trim().to_ascii_lowercase().replace("%2e", ".");
        t == "." || t == ".."
    }
    fn check_value(v: &Value) -> Result<()> {
        match v {
            Value::String(s) if dot_segment(s) => Err(Error::invalid(format!("Ungültige ID: {s}"))),
            Value::Array(a) => a.iter().try_for_each(check_value),
            _ => Ok(()),
        }
    }
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                if k == "id" || k == "ids" || k.ends_with("Id") || k.ends_with("Ids") {
                    check_value(x)?;
                }
                check_ids(x)?;
            }
            Ok(())
        }
        Value::Array(a) => a.iter().try_for_each(check_ids),
        _ => Ok(()),
    }
}

fn ok<T: serde::Serialize>(v: T) -> Result<Value> {
    Ok(serde_json::to_value(v)?)
}

impl Service {
    pub fn new(opts: ServiceOptions, host: Arc<dyn Host>) -> Result<Arc<Self>> {
        std::fs::create_dir_all(&opts.data_dir)?;
        // The cache holds whole mailboxes (and maybe the token file): owner only.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&opts.data_dir, std::fs::Permissions::from_mode(0o700))?;
        }
        let store = Store::open(&opts.data_dir.join("cache.db"))?;
        let mut config: Config = std::fs::read_to_string(opts.data_dir.join("config.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        config.demo = opts.demo;
        Self::apply_defaults(&mut config, opts.demo);
        let http = reqwest::Client::builder()
            .user_agent(concat!("lettertask/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(120))
            .connect_timeout(Duration::from_secs(15))
            .build()?;
        let svc = Arc::new(Service {
            opts,
            http,
            store,
            config: RwLock::new(config),
            clients: RwLock::new(HashMap::new()),
            host: Arc::new(crate::host::Guarded(host)),
            active_folders: RwLock::new(HashMap::new()),
            folder_locks: Default::default(),
            sync_all_lock: tokio::sync::Mutex::new(()),
            last_sync: RwLock::new(None),
            imap: Default::default(),
            google_tokens: Default::default(),
            send_queue: Default::default(),
            pending_sends: Default::default(),
        });
        svc.restore_clients()?;
        Ok(svc)
    }

    pub fn config(&self) -> Config {
        self.config.read().unwrap().clone()
    }

    fn oauth(&self) -> OAuthConfig {
        let c = self.config();
        OAuthConfig {
            authority: self.opts.authority.clone(),
            tenant: if c.tenant.trim().is_empty() {
                builtin_tenant().to_string()
            } else {
                c.tenant.trim().to_string()
            },
            client_id: c.client_id.trim().to_string(),
        }
    }

    fn make_client(&self, account_id: &str, tokens: TokenSet) -> Arc<GraphClient> {
        Arc::new(GraphClient::new(
            self.http.clone(),
            self.opts.graph_base.clone(),
            account_id,
            self.oauth(),
            tokens,
            self.opts.token_store.clone(),
        ))
    }

    /// Re-creates Graph clients for stored accounts from saved refresh tokens.
    fn restore_clients(&self) -> Result<()> {
        let mut map = HashMap::new();
        for a in self.store.list_accounts()? {
            if let Some(rt) = self.opts.token_store.load(&a.id) {
                let tokens = TokenSet {
                    access_token: String::new(),
                    refresh_token: rt,
                    // Expired: the first request fetches a fresh access token.
                    expires_at: chrono::DateTime::<Utc>::UNIX_EPOCH,
                };
                map.insert(a.id.clone(), self.make_client(&a.id, tokens));
            }
        }
        *self.clients.write().unwrap() = map;
        Ok(())
    }

    pub(crate) fn client(&self, account_id: &str) -> Result<Arc<GraphClient>> {
        self.clients
            .read()
            .unwrap()
            .get(account_id)
            .cloned()
            .ok_or_else(|| Error::Auth("Konto nicht angemeldet. Bitte erneut anmelden (Datei › Konto hinzufügen).".into()))
    }

    pub(crate) fn mailbox(&self, mailbox_id: &str) -> Result<(Mailbox, Arc<GraphClient>)> {
        let mb = self.store.get_mailbox(mailbox_id)?;
        let c = self.client(&mb.account_id)?;
        Ok((mb, c))
    }

    pub(crate) fn folder_lock(&self, mailbox_id: &str, folder_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.folder_locks
            .lock()
            .unwrap()
            .entry(format!("{mailbox_id}|{folder_id}"))
            .or_default()
            .clone()
    }

    pub(crate) fn emit(&self, event: &str, payload: Value) {
        self.host.emit(event, payload);
    }

    pub(crate) fn emit_mail_changed(&self, mailbox_id: &str, folder_id: Option<&str>) {
        self.emit("mail-changed", json!({"mailboxId": mailbox_id, "folderId": folder_id}));
    }

    fn emit_status(&self, state: &str, message: &str) {
        let last = self.last_sync.read().unwrap().clone();
        self.emit(
            "sync-status",
            json!({"state": state, "message": message, "lastSync": last}),
        );
    }

    /// Runs one UI command. A bug that panics becomes an error the UI shows,
    /// instead of a call that never answers (endless "Wird synchronisiert…").
    pub async fn dispatch(self: &Arc<Self>, cmd: &str, args: Value) -> Result<Value> {
        use futures::FutureExt;
        match std::panic::AssertUnwindSafe(self.dispatch_inner(cmd, args)).catch_unwind().await {
            Ok(r) => r,
            Err(p) => {
                let msg = format!("Interner Fehler bei „{cmd}“: {}", panic_text(&p));
                tracing::error!("{msg}");
                self.emit_status("error", &msg);
                Err(Error::Other(msg))
            }
        }
    }

    async fn dispatch_inner(self: &Arc<Self>, cmd: &str, args: Value) -> Result<Value> {
        check_ids(&args)?;
        let a = &args;
        match cmd {
            // ---- app / accounts ----
            "app_info" => ok(json!({ "version": env!("CARGO_PKG_VERSION") })),
            "get_config" => ok(self.config()),
            "save_config" => ok(self.save_config(arg(a, "config")?)?),
            "list_accounts" => ok(self.list_accounts_for_ui()?),
            "add_account" => ok(self.add_account().await?),
            "add_google_account" => ok(self
                .add_google_account(arg::<Option<String>>(a, "loginHint")?)
                .await?),
            "discover_imap" => ok(self.discover_imap(&arg::<String>(a, "email")?).await),
            "test_imap_account" => ok(self.test_imap_account(&arg(a, "input")?).await),
            "add_imap_account" => ok(self.add_imap_account(arg(a, "input")?).await?),
            "update_imap_account" => ok(self
                .update_imap_account(&arg::<String>(a, "accountId")?, arg(a, "input")?)
                .await?),
            "get_imap_account" => ok(self.get_imap_account(&arg::<String>(a, "accountId")?)?),
            "set_account_order" => {
                self.store.set_account_order(&arg::<Vec<String>>(a, "accountIds")?)?;
                self.emit("accounts-changed", json!({}));
                ok(Value::Null)
            }
            "set_account_avatar" => {
                let id: String = arg(a, "accountId")?;
                let avatar: Option<String> = arg(a, "avatar")?;
                if let Some(v) = &avatar {
                    check_avatar(v)?;
                }
                if !self.store.set_account_avatar(&id, avatar.as_deref())? {
                    return Err(Error::invalid("Unbekanntes Konto."));
                }
                self.emit("accounts-changed", json!({}));
                ok(Value::Null)
            }
            "set_account_look" => {
                let id: String = arg(a, "accountId")?;
                let label = clean_account_label(arg(a, "label")?)?;
                let badge = clean_account_badge(arg(a, "badge")?)?;
                if !self.store.set_account_look(&id, label.as_deref(), &badge)? {
                    return Err(Error::invalid("Unbekanntes Konto."));
                }
                self.emit("accounts-changed", json!({}));
                ok(Value::Null)
            }
            "remove_account" => {
                self.remove_account(&arg::<String>(a, "accountId")?)?;
                ok(Value::Null)
            }
            "set_active_folder" => {
                let mb: String = arg(a, "mailboxId")?;
                let f: String = arg(a, "folderId")?;
                self.active_folders.write().unwrap().insert(mb, f);
                ok(Value::Null)
            }
            "sync_all" => {
                self.sync_all(false).await;
                ok(Value::Null)
            }

            // ---- folders ----
            "list_folders" => ok(self.store.list_folders(&arg::<String>(a, "mailboxId")?)?),
            "sync_folders" => ok(self.sync_folders(&arg::<String>(a, "mailboxId")?).await?),
            "create_folder" => ok(self
                .create_folder(
                    &arg::<String>(a, "mailboxId")?,
                    arg::<Option<String>>(a, "parentId")?.as_deref(),
                    &arg::<String>(a, "name")?,
                )
                .await?),
            "rename_folder" => ok(self
                .rename_folder(
                    &arg::<String>(a, "mailboxId")?,
                    &arg::<String>(a, "folderId")?,
                    &arg::<String>(a, "name")?,
                )
                .await?),
            "delete_folder" => {
                self.delete_folder(&arg::<String>(a, "mailboxId")?, &arg::<String>(a, "folderId")?)
                    .await?;
                ok(Value::Null)
            }
            "mark_folder_read" => {
                self.mark_folder_read(&arg::<String>(a, "mailboxId")?, &arg::<String>(a, "folderId")?)
                    .await?;
                ok(Value::Null)
            }
            "empty_folder" => {
                self.empty_folder(&arg::<String>(a, "mailboxId")?, &arg::<String>(a, "folderId")?)
                    .await?;
                ok(Value::Null)
            }

            // ---- messages ----
            "list_messages" => {
                let q: ListQuery = serde_json::from_value(args.clone())
                    .map_err(|e| Error::invalid(e.to_string()))?;
                ok(self.list_messages(&q)?)
            }
            "sync_folder" => ok(self
                .sync_folder(&arg::<String>(a, "mailboxId")?, &arg::<String>(a, "folderId")?, false)
                .await?),
            "load_older" => ok(self
                .load_older(&arg::<String>(a, "mailboxId")?, &arg::<String>(a, "folderId")?)
                .await?),
            "search_messages" => ok(self
                .search_messages(
                    &arg::<String>(a, "mailboxId")?,
                    &arg::<String>(a, "text")?,
                    arg::<Option<String>>(a, "folderId")?.as_deref(),
                    &arg::<Option<String>>(a, "field")?.unwrap_or_else(|| "all".into()),
                    arg::<Option<bool>>(a, "hasAttachments")?.unwrap_or(false),
                )
                .await?),
            "get_message" => ok(self
                .get_message(&arg::<String>(a, "mailboxId")?, &arg::<String>(a, "messageId")?)
                .await?),
            "set_read" => {
                self.set_read(&arg::<String>(a, "mailboxId")?, &arg::<Vec<String>>(a, "ids")?, arg(a, "isRead")?)
                    .await?;
                ok(Value::Null)
            }
            "set_flag" => {
                self.set_flag(&arg::<String>(a, "mailboxId")?, &arg::<Vec<String>>(a, "ids")?, &arg::<String>(a, "flag")?)
                    .await?;
                ok(Value::Null)
            }
            "set_categories" => {
                self.set_categories(
                    &arg::<String>(a, "mailboxId")?,
                    &arg::<Vec<String>>(a, "ids")?,
                    &arg::<Vec<String>>(a, "categories")?,
                )
                .await?;
                ok(Value::Null)
            }
            "move_messages" => {
                self.move_messages(
                    &arg::<String>(a, "mailboxId")?,
                    &arg::<Vec<String>>(a, "ids")?,
                    &arg::<String>(a, "destFolderId")?,
                    false,
                )
                .await?;
                ok(Value::Null)
            }
            "copy_messages" => {
                self.move_messages(
                    &arg::<String>(a, "mailboxId")?,
                    &arg::<Vec<String>>(a, "ids")?,
                    &arg::<String>(a, "destFolderId")?,
                    true,
                )
                .await?;
                ok(Value::Null)
            }
            "archive_messages" => {
                self.archive_messages(&arg::<String>(a, "mailboxId")?, &arg::<Vec<String>>(a, "ids")?)
                    .await?;
                ok(Value::Null)
            }
            "snooze_messages" => {
                self.snooze_messages(
                    &arg::<String>(a, "mailboxId")?,
                    &arg::<Vec<String>>(a, "ids")?,
                    &arg::<String>(a, "until")?,
                )
                .await?;
                ok(Value::Null)
            }
            "unsnooze_messages" => {
                self.unsnooze_messages(&arg::<String>(a, "mailboxId")?, &arg::<Vec<String>>(a, "ids")?)
                    .await?;
                ok(Value::Null)
            }
            "delete_messages" => {
                self.delete_messages(
                    &arg::<String>(a, "mailboxId")?,
                    &arg::<Vec<String>>(a, "ids")?,
                    arg::<Option<bool>>(a, "permanent")?.unwrap_or(false),
                )
                .await?;
                ok(Value::Null)
            }
            "download_attachment" => ok(self
                .download_attachment(
                    &arg::<String>(a, "mailboxId")?,
                    &arg::<String>(a, "messageId")?,
                    &arg::<String>(a, "attachmentId")?,
                    &arg::<Option<String>>(a, "action")?.unwrap_or_else(|| "open".into()),
                )
                .await?),
            "get_message_source" => ok(self
                .get_message_source(&arg::<String>(a, "mailboxId")?, &arg::<String>(a, "messageId")?)
                .await?),

            // ---- compose ----
            "create_draft" => ok(self
                .create_draft(
                    &arg::<String>(a, "mailboxId")?,
                    &arg::<Option<String>>(a, "mode")?.unwrap_or_else(|| "new".into()),
                    arg::<Option<String>>(a, "sourceMessageId")?.as_deref(),
                )
                .await?),
            "get_draft" => ok(self
                .get_draft(&arg::<String>(a, "mailboxId")?, &arg::<String>(a, "messageId")?)
                .await?),
            "save_draft" => ok(self.save_draft(arg(a, "draft")?).await?),
            "send_draft" => {
                let d = arg(a, "draft")?;
                if arg::<Option<bool>>(a, "background")?.unwrap_or(false) {
                    self.queue_send(d)?;
                } else {
                    self.send_draft(d).await?;
                }
                ok(Value::Null)
            }
            "discard_draft" => {
                self.discard_draft(&arg::<String>(a, "mailboxId")?, arg::<Option<String>>(a, "draftId")?.as_deref())
                    .await?;
                ok(Value::Null)
            }
            "suggest_recipients" => ok(self
                .suggest_recipients(&arg::<String>(a, "mailboxId")?, &arg::<String>(a, "query")?)
                .await?),

            // ---- categories ----
            // ---- inbox zero ----
            "create_task" => ok(self
                .create_task(
                    &arg::<String>(a, "mailboxId")?,
                    &arg::<String>(a, "subject")?,
                    &arg::<Option<String>>(a, "bodyHtml")?.unwrap_or_default(),
                    arg::<Option<String>>(a, "due")?.as_deref(),
                    arg::<Option<String>>(a, "recurrence")?.as_deref(),
                    arg::<Option<crate::model::Address>>(a, "delegate")?
                        .as_ref()
                        .map(|d| (d, arg::<Option<bool>>(a, "todoPrefix").ok().flatten().unwrap_or(true))),
                )
                .await?),
            "list_delegations" => ok(self.list_delegations()?),
            "find_delegation" => ok(self.find_delegation(&arg::<String>(a, "mailboxId")?, &arg::<String>(a, "messageId")?)?),
            "set_recurrence" => {
                self.set_recurrence(
                    &arg::<String>(a, "mailboxId")?,
                    &arg::<Vec<String>>(a, "ids")?,
                    arg::<Option<String>>(a, "recurrence")?.as_deref(),
                )
                .await?;
                ok(Value::Null)
            }
            "list_today" => ok(self.list_today()?),
            "list_due" => {
                let end = arg::<String>(a, "end")?;
                let end = chrono::DateTime::parse_from_rfc3339(&end)
                    .map_err(|_| Error::invalid(format!("Ungültiges Datum: {end}")))?
                    .with_timezone(&chrono::Utc)
                    .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
                ok(self.due_before(&end)?)
            }
            "get_focus" => ok(json!({"until": self.focus_until().map(|u| u.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))})),
            "set_focus" => ok(self.set_focus(arg::<Option<i64>>(a, "minutes")?.unwrap_or(0))?),
            "get_lanes" => ok(self.get_lanes(&arg::<String>(a, "accountId")?)?),
            "save_lanes" => ok(self
                .save_lanes(&arg::<String>(a, "accountId")?, &arg::<Vec<String>>(a, "lanes")?)
                .await?),
            "set_lane" => {
                self.set_lane(
                    &arg::<String>(a, "mailboxId")?,
                    &arg::<Vec<String>>(a, "ids")?,
                    arg::<Option<String>>(a, "lane")?.as_deref(),
                )
                .await?;
                ok(Value::Null)
            }

            // ---- signatures ----
            "get_signatures" => ok(self.get_signatures(&arg::<String>(a, "accountId")?)?),
            "save_signatures" => ok(self.save_signatures(&arg::<String>(a, "accountId")?, arg(a, "settings")?)?),

            // ---- contacts ----
            "list_contacts" => ok(self.list_contacts(
                arg::<Option<String>>(a, "query")?.as_deref(),
                arg::<Option<String>>(a, "accountId")?.as_deref(),
                arg::<Option<String>>(a, "source")?.as_deref(),
                arg::<Option<i64>>(a, "offset")?.unwrap_or(0),
                arg::<Option<i64>>(a, "limit")?.unwrap_or(500),
            )?),
            "get_contact" => ok(self.get_contact(&arg::<String>(a, "id")?)?),
            "sync_contacts" => ok(self.sync_contacts(&arg::<String>(a, "accountId")?).await?),
            "create_contact" => ok(self
                .create_contact(&arg::<String>(a, "accountId")?, arg(a, "input")?)
                .await?),
            "update_contact" => ok(self.update_contact(&arg::<String>(a, "id")?, arg(a, "input")?).await?),
            "delete_contact" => {
                self.delete_contact(&arg::<String>(a, "id")?).await?;
                ok(Value::Null)
            }

            "list_categories" => {
                let account_id = arg::<String>(a, "accountId")?;
                // `cached`: windows render from the local copy instead of waiting for Graph.
                match arg::<Option<bool>>(a, "cached")?.unwrap_or(false) {
                    true => match self.cached_categories(&account_id)? {
                        Some(c) => ok(c),
                        None => ok(self.list_categories(&account_id).await?),
                    },
                    false => ok(self.list_categories(&account_id).await?),
                }
            }
            "create_category" => ok(self
                .create_category(
                    &arg::<String>(a, "accountId")?,
                    &arg::<String>(a, "displayName")?,
                    &arg::<String>(a, "color")?,
                )
                .await?),
            "update_category" => ok(self
                .update_category(&arg::<String>(a, "accountId")?, &arg::<String>(a, "id")?, &arg::<String>(a, "color")?)
                .await?),
            "delete_category" => {
                self.delete_category(&arg::<String>(a, "accountId")?, &arg::<String>(a, "id")?)
                    .await?;
                ok(Value::Null)
            }

            // ---- calendar ----
            "list_calendars" => {
                let account_id = arg::<String>(a, "accountId")?;
                // `cached`: the last known list (no Graph round trip), if there is one.
                let cached: Option<Vec<Calendar>> = match arg::<Option<bool>>(a, "cached")?.unwrap_or(false) {
                    true => self.store.get_account_json(&account_id, "calendars")?,
                    false => None,
                };
                match cached {
                    Some(c) if !c.is_empty() => ok(c),
                    _ => ok(self.list_calendars(&account_id).await?),
                }
            }
            "add_people_calendar" => ok(self
                .add_people_calendar(
                    &arg::<String>(a, "accountId")?,
                    &arg::<String>(a, "address")?,
                    &arg::<Option<String>>(a, "name")?.unwrap_or_default(),
                )
                .await?),
            "remove_people_calendar" => ok(self
                .remove_people_calendar(&arg::<String>(a, "accountId")?, &arg::<String>(a, "calendarId")?)
                .await?),
            "list_events" => ok(self
                .list_events(
                    &arg::<String>(a, "accountId")?,
                    &arg::<Vec<String>>(a, "calendarIds")?,
                    &arg::<String>(a, "start")?,
                    &arg::<String>(a, "end")?,
                )
                .await?),
            "get_event" => ok(self
                .get_event(&arg::<String>(a, "accountId")?, &arg::<String>(a, "eventId")?)
                .await?),
            "create_event" => ok(self
                .create_event(&arg::<String>(a, "accountId")?, &arg::<String>(a, "calendarId")?, arg(a, "input")?)
                .await?),
            "update_event" => ok(self
                .update_event(&arg::<String>(a, "accountId")?, &arg::<String>(a, "eventId")?, arg(a, "input")?)
                .await?),
            "delete_event" => {
                self.delete_event(&arg::<String>(a, "accountId")?, &arg::<String>(a, "eventId")?)
                    .await?;
                ok(Value::Null)
            }
            "respond_event" => {
                self.respond_event(
                    &arg::<String>(a, "accountId")?,
                    &arg::<String>(a, "eventId")?,
                    &arg::<String>(a, "response")?,
                    &arg::<Option<String>>(a, "comment")?.unwrap_or_default(),
                    arg::<Option<bool>>(a, "sendResponse")?.unwrap_or(true),
                )
                .await?;
                ok(Value::Null)
            }

            // ---- mailbox settings ----
            "get_auto_reply" => ok(self.get_auto_reply(&arg::<String>(a, "accountId")?).await?),
            "set_auto_reply" => ok(self
                .set_auto_reply(&arg::<String>(a, "accountId")?, arg(a, "autoReply")?)
                .await?),

            // ---- host ----
            "open_window" => {
                let kind: String = arg(a, "kind")?;
                if !matches!(kind.as_str(), "compose" | "message" | "event" | "task") {
                    return Err(Error::invalid("Unbekannter Fenstertyp"));
                }
                let params: HashMap<String, Value> = arg::<Option<_>>(a, "params")?.unwrap_or_default();
                let mut ser = url::form_urlencoded::Serializer::new(String::new());
                for (k, v) in &params {
                    let s = match v {
                        Value::String(s) => s.clone(),
                        Value::Null => continue,
                        other => other.to_string(),
                    };
                    ser.append_pair(k, &s);
                }
                self.host.open_window(&kind, &ser.finish())?;
                ok(Value::Null)
            }
            "open_url" => {
                let url: String = arg(a, "url")?;
                self.open_url(&url)?;
                ok(Value::Null)
            }
            _ => Err(Error::invalid(format!("Unbekannter Befehl: {cmd}"))),
        }
    }

    fn open_url(&self, url: &str) -> Result<()> {
        let lower = url.trim().to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("mailto:") {
            let _ = rest;
            let addr = url.trim()[7..].split('?').next().unwrap_or("");
            let addr = urlencoding::decode(addr).map(|c| c.into_owned()).unwrap_or_default();
            let mailbox = self
                .store
                .list_accounts()?
                .into_iter()
                .flat_map(|a| a.mailboxes)
                .next()
                .ok_or_else(|| Error::invalid("Kein Konto vorhanden"))?;
            let mut ser = url::form_urlencoded::Serializer::new(String::new());
            ser.append_pair("mailboxId", &mailbox.id);
            ser.append_pair("mode", "new");
            ser.append_pair("to", &addr);
            return self.host.open_window("compose", &ser.finish());
        }
        if lower.starts_with("http://") || lower.starts_with("https://") {
            return self.host.open_url(url.trim());
        }
        Err(Error::invalid("Nur http(s)- und mailto-Links können geöffnet werden."))
    }

    // ---- config & accounts ----

    /// Fills values the user left empty: built-in client ID, snooze options.
    fn apply_defaults(c: &mut Config, demo: bool) {
        let builtin = builtin_client_id();
        c.builtin_client_id = !demo && !builtin.is_empty();
        if demo && c.client_id.is_empty() {
            c.client_id = "demo-client".into();
        }
        if c.client_id.trim().is_empty() && !builtin.is_empty() {
            c.client_id = builtin.to_string();
        }
        if c.tenant.trim().is_empty() || c.tenant.trim() == LEGACY_SYMBIOSIKA_TENANT_ID {
            c.tenant = builtin_tenant().to_string();
        }
        let (google_id, google_secret) = builtin_google_client();
        c.builtin_google_client = !demo && !google_id.is_empty();
        if c.google_client_id.trim().is_empty() && !google_id.is_empty() {
            c.google_client_id = google_id.to_string();
            c.google_client_secret = google_secret.to_string();
        }
        if c.snooze_options.is_empty() {
            c.snooze_options = default_snooze_options();
        }
        if !matches!(c.kanban_reading_pane.as_str(), "bottom" | "right") {
            c.kanban_reading_pane = "bottom".into();
        }
        if !matches!(c.list_mode.as_str(), "comfortable" | "compact") {
            c.list_mode = "comfortable".into();
        }
    }

    fn save_config(&self, mut c: Config) -> Result<Config> {
        c.demo = self.opts.demo;
        if self.opts.demo {
            c.client_id = "demo-client".into();
        }
        Self::apply_defaults(&mut c, self.opts.demo);
        c.sync_interval_secs = c.sync_interval_secs.clamp(15, 3600);
        let changed_auth = {
            let cur = self.config.read().unwrap();
            cur.client_id != c.client_id || cur.tenant != c.tenant
        };
        let changed_google = {
            let cur = self.config.read().unwrap();
            cur.google_client_id != c.google_client_id || cur.google_client_secret != c.google_client_secret
        };
        std::fs::write(
            self.opts.data_dir.join("config.json"),
            serde_json::to_string_pretty(&c)?,
        )?;
        *self.config.write().unwrap() = c.clone();
        if changed_auth {
            self.restore_clients()?;
        }
        if changed_google {
            // Tokens are bound to the client that issued them.
            self.google_tokens.lock().unwrap().clear();
        }
        Ok(c)
    }

    pub async fn add_account(self: &Arc<Self>) -> Result<Account> {
        let oauth = self.oauth();
        let tokens = if self.opts.demo {
            let http = self.http.clone();
            auth::interactive_login(
                &self.http,
                &oauth,
                move |url| {
                    // Demo: the fake authorize endpoint redirects straight to our loopback.
                    tokio::spawn(async move {
                        let _ = http.get(url).send().await;
                    });
                    Ok(())
                },
                Duration::from_secs(30),
            )
            .await?
        } else {
            let host = self.host.clone();
            auth::interactive_login(&self.http, &oauth, move |url| host.open_url(&url), Duration::from_secs(300))
                .await?
        };
        self.finish_login(tokens).await
    }

    /// OAuth config as currently configured (client ID, tenant, authority).
    pub fn oauth_config(&self) -> OAuthConfig {
        self.oauth()
    }

    pub fn http_client(&self) -> &reqwest::Client {
        &self.http
    }

    /// Registers the signed-in user as account (or refreshes its token) and
    /// performs the initial folder/inbox sync.
    pub async fn finish_login(self: &Arc<Self>, tokens: TokenSet) -> Result<Account> {
        // Use a temporary client to learn who signed in.
        let tmp = self.make_client("pending", tokens.clone());
        let me = tmp.get("/me?$select=id,displayName,mail,userPrincipalName").await?;
        let id = me["id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| Error::Auth("Benutzer-ID fehlt".into()))?
            .to_string();
        let email = me["mail"]
            .as_str()
            .filter(|s| !s.is_empty())
            .or(me["userPrincipalName"].as_str())
            .unwrap_or("")
            .to_string();
        let name = me["displayName"].as_str().unwrap_or(&email).to_string();
        // The temporary client may have rotated the refresh token; take the latest.
        drop(tmp);
        self.opts.token_store.save(&id, &tokens.refresh_token)?;
        self.store.upsert_account(&id, &email, &name)?;
        let mailbox = Mailbox {
            id: id.clone(),
            account_id: id.clone(),
            address: email.clone(),
            display_name: name.clone(),
            kind: "primary".into(),
        };
        self.store.upsert_mailbox(&mailbox)?;
        self.clients
            .write()
            .unwrap()
            .insert(id.clone(), self.make_client(&id, tokens));
        self.sync_folders(&id).await?;
        if let Some(inbox) = self.store.folder_by_well_known(&id, "inbox")? {
            let _ = self.sync_folder(&id, &inbox.id, true).await;
        }
        self.emit("accounts-changed", json!({}));
        let acc = self
            .store
            .list_accounts()?
            .into_iter()
            .find(|a| a.id == id)
            .ok_or_else(|| Error::NotFound("Konto".into()))?;
        Ok(acc)
    }

    fn remove_account(&self, account_id: &str) -> Result<()> {
        self.store.delete_account(account_id)?;
        self.store.delete_account_contacts(account_id)?;
        self.opts.token_store.delete(account_id);
        self.forget_imap_account(account_id);
        self.clients.write().unwrap().remove(account_id);
        self.emit("accounts-changed", json!({}));
        Ok(())
    }

    // ---- background sync ----

    /// "Senden/Empfangen": folders, inbox and active folder of every mailbox.
    pub async fn sync_all(self: &Arc<Self>, background: bool) {
        let Ok(_guard) = self.sync_all_lock.try_lock() else {
            return;
        };
        let accounts = match self.store.list_accounts() {
            Ok(a) => a,
            Err(e) => {
                self.emit_status("error", &e.to_string());
                return;
            }
        };
        if accounts.is_empty() {
            return;
        }
        self.emit_status("syncing", "Senden/Empfangen…");
        let mut errors = Vec::new();
        let mut offline = false;
        for acc in &accounts {
            if let Err(e) = self.sync_contacts(&acc.id).await {
                offline |= e.is_network();
                errors.push(format!("{} (Kontakte): {e}", acc.email));
            }
        }
        for mb in accounts.iter().flat_map(|a| a.mailboxes.iter()) {
            if let Err(e) = self.sync_folders(&mb.id).await {
                offline |= e.is_network();
                errors.push(format!("{}: {e}", mb.address));
                continue;
            }
            if let Err(e) = self.process_due_snoozes(&mb.id).await {
                offline |= e.is_network();
                errors.push(format!("{}: {e}", mb.address));
            }
            let mut targets = Vec::new();
            if let Ok(Some(inbox)) = self.store.folder_by_well_known(&mb.id, "inbox") {
                targets.push(inbox.id);
            }
            if let Some(active) = self.active_folders.read().unwrap().get(&mb.id).cloned() {
                if !targets.contains(&active) {
                    targets.push(active);
                }
            }
            for f in targets {
                if let Err(e) = self.sync_folder(&mb.id, &f, !background).await {
                    offline |= e.is_network();
                    errors.push(format!("{}: {e}", mb.address));
                }
            }
        }
        if errors.is_empty() {
            *self.last_sync.write().unwrap() = Some(Utc::now().to_rfc3339());
            self.emit_status("idle", "Alle Ordner sind auf dem neuesten Stand.");
        } else if offline {
            self.emit_status("offline", "Offline – Verbindung zu Microsoft 365 nicht möglich.");
        } else {
            self.emit_status("error", &errors.join("; "));
        }
    }

    /// Spawns the periodic sync loop.
    pub fn start_background_sync(self: &Arc<Self>) {
        let svc = Arc::downgrade(self);
        use futures::FutureExt;
        tokio::spawn(async move {
            // Small delay so the UI is up before the first sync.
            tokio::time::sleep(Duration::from_secs(2)).await;
            let mut focus_active = false;
            loop {
                let Some(s) = svc.upgrade() else { return };
                // Fokuszeit pauses the automatic fetch; when it ends, sync once.
                match s.focus_until() {
                    Some(_) => focus_active = true,
                    None => {
                        if focus_active {
                            focus_active = false;
                            s.emit("focus-changed", json!({"until": null}));
                        }
                        // A panicking sync must not end the loop silently.
                        if let Err(p) = std::panic::AssertUnwindSafe(s.sync_all(true)).catch_unwind().await {
                            let msg = format!("Interner Fehler beim Abgleich: {}", panic_text(&p));
                            tracing::error!("{msg}");
                            s.emit_status("error", &msg);
                        }
                    }
                }
                let wait = s.config().sync_interval_secs.max(15);
                drop(s);
                tokio::time::sleep(Duration::from_secs(wait)).await;
            }
        });
    }

    /// Demo mode: sign in automatically when no account exists yet.
    pub async fn ensure_demo_account(self: &Arc<Self>) -> Result<()> {
        if self.opts.demo && self.store.list_accounts()?.is_empty() {
            self.add_account().await?;
        }
        Ok(())
    }

    // ---- mailbox settings ----

    async fn get_auto_reply(&self, account_id: &str) -> Result<AutoReply> {
        if crate::imap::is_imap_id(account_id) {
            return Err(Error::invalid("Abwesenheitsnotizen sind nur für Microsoft-365-Konten verfügbar."));
        }
        let c = self.client(account_id)?;
        let v = c
            .get_h(
                "/me/mailboxSettings/automaticRepliesSetting",
                &[("Prefer", "outlook.timezone=\"UTC\"".into())],
            )
            .await?;
        Ok(crate::convert::auto_reply_from_graph(&v))
    }

    async fn set_auto_reply(&self, account_id: &str, a: AutoReply) -> Result<AutoReply> {
        if crate::imap::is_imap_id(account_id) {
            return Err(Error::invalid("Abwesenheitsnotizen sind nur für Microsoft-365-Konten verfügbar."));
        }
        let c = self.client(account_id)?;
        c.patch(
            "/me/mailboxSettings",
            &json!({"automaticRepliesSetting": crate::convert::auto_reply_to_graph(&a)}),
        )
        .await?;
        self.get_auto_reply(account_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_tenant_id_is_migrated_to_multi_tenant_default() {
        let mut c = Config {
            tenant: LEGACY_SYMBIOSIKA_TENANT_ID.into(),
            ..Config::default()
        };
        Service::apply_defaults(&mut c, false);
        assert_eq!(c.tenant, builtin_tenant());

        let mut custom = Config {
            tenant: "contoso.onmicrosoft.com".into(),
            ..Config::default()
        };
        Service::apply_defaults(&mut custom, false);
        assert_eq!(custom.tenant, "contoso.onmicrosoft.com");
    }
}
