//! In-memory data model of the fake tenant.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use chrono::{DateTime, Duration, NaiveDateTime, Utc};
use serde_json::{json, Value};

use crate::util::{email, email_addr, iso, merge, new_id, preview};

pub const WELL_KNOWN: &[(&str, &str)] = &[
    ("inbox", "Posteingang"),
    ("drafts", "Entwürfe"),
    ("sentitems", "Gesendete Elemente"),
    ("deleteditems", "Gelöschte Elemente"),
    ("archive", "Archiv"),
    ("junkemail", "Junk-E-Mail"),
    ("outbox", "Postausgang"),
];

#[derive(Clone, Debug)]
pub struct Folder {
    pub id: String,
    pub display_name: String,
    pub parent_id: String,
    pub well_known: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Att {
    pub id: String,
    pub name: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
    pub is_inline: bool,
    pub content_id: Option<String>,
    pub modified: DateTime<Utc>,
}

impl Att {
    pub fn new(name: &str, content_type: &str, bytes: Vec<u8>, is_inline: bool, content_id: Option<&str>) -> Self {
        Att {
            id: new_id("AAMkAT"),
            name: name.to_string(),
            content_type: content_type.to_string(),
            bytes,
            is_inline,
            content_id: content_id.map(str::to_string),
            modified: Utc::now(),
        }
    }

    pub fn render(&self, with_bytes: bool) -> Value {
        let mut v = json!({
            "@odata.type": "#microsoft.graph.fileAttachment",
            "id": self.id,
            "lastModifiedDateTime": iso(self.modified),
            "name": self.name,
            "contentType": self.content_type,
            "size": self.bytes.len(),
            "isInline": self.is_inline,
            "contentId": self.content_id,
            "contentLocation": null,
        });
        if with_bytes {
            v["contentBytes"] = Value::String(crate::util::b64(&self.bytes));
        }
        v
    }

    pub fn with_new_id(&self) -> Att {
        let mut a = self.clone();
        a.id = new_id("AAMkAT");
        a
    }
}

#[derive(Clone, Debug)]
pub struct Msg {
    pub id: String,
    pub folder_id: String,
    /// Graph properties, excluding id / parentFolderId / hasAttachments (computed on render).
    pub json: Value,
    pub received: DateTime<Utc>,
    pub attachments: Vec<Att>,
    /// For meeting requests: linked calendar event.
    pub event_id: Option<String>,
}

impl Msg {
    pub fn render(&self) -> Value {
        let mut v = self.json.clone();
        v["id"] = json!(self.id);
        v["parentFolderId"] = json!(self.folder_id);
        v["hasAttachments"] = json!(self.attachments.iter().any(|a| !a.is_inline));
        v["@odata.etag"] = json!(format!("W/\"{}\"", v["changeKey"].as_str().unwrap_or("")));
        v
    }

    pub fn is_read(&self) -> bool {
        self.json["isRead"].as_bool().unwrap_or(false)
    }

    /// Mark modified (new changeKey + lastModifiedDateTime).
    pub fn touch(&mut self) {
        self.json["changeKey"] = json!(new_id("CQAAAB"));
        self.json["lastModifiedDateTime"] = json!(iso(Utc::now()));
    }
}

/// Default Graph message JSON (all properties present, like a real GET).
pub fn message_json(now: DateTime<Utc>, overrides: &Value) -> Value {
    let mut v = json!({
        "createdDateTime": iso(now),
        "lastModifiedDateTime": iso(now),
        "changeKey": new_id("CQAAAB"),
        "categories": [],
        "receivedDateTime": iso(now),
        "sentDateTime": iso(now),
        "internetMessageId": format!("<{}@contoso.example>", uuid::Uuid::new_v4().simple()),
        "subject": "",
        "bodyPreview": "",
        "importance": "normal",
        "conversationId": new_id("AAQkAD"),
        "conversationIndex": crate::util::b64(uuid::Uuid::new_v4().as_bytes()),
        "isDeliveryReceiptRequested": false,
        "isReadReceiptRequested": false,
        "isRead": true,
        "isDraft": false,
        "webLink": format!("https://outlook.office365.com/owa/?ItemID={}&exvsurl=1&viewmodel=ReadMessageItem", uuid::Uuid::new_v4().simple()),
        "inferenceClassification": "focused",
        "body": { "contentType": "html", "content": "" },
        "sender": null,
        "from": null,
        "toRecipients": [],
        "ccRecipients": [],
        "bccRecipients": [],
        "replyTo": [],
        "flag": { "flagStatus": "notFlagged" },
    });
    merge(&mut v, overrides);
    if v["sender"].is_null() && !v["from"].is_null() {
        v["sender"] = v["from"].clone();
    }
    if overrides.get("bodyPreview").is_none() {
        v["bodyPreview"] = json!(preview(&v["body"]));
    }
    v
}

#[derive(Clone, Debug)]
pub struct Cal {
    pub id: String,
    pub name: String,
    pub color: String,
    pub hex_color: String,
    pub can_edit: bool,
    pub is_default: bool,
}

impl Cal {
    pub fn render(&self, owner_name: &str, owner_addr: &str) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "color": self.color,
            "hexColor": self.hex_color,
            "isDefaultCalendar": self.is_default,
            "changeKey": "fJKdEpJ5k0SzGy9Y8CdMgAAAAACNtQ==",
            "canShare": self.can_edit,
            "canViewPrivateItems": true,
            "canEdit": self.can_edit,
            "isRemovable": !self.is_default,
            "isTallyingResponses": true,
            "allowedOnlineMeetingProviders": ["teamsForBusiness"],
            "defaultOnlineMeetingProvider": "teamsForBusiness",
            "owner": { "name": owner_name, "address": owner_addr }
        })
    }
}

/// An override for one occurrence of a series.
#[derive(Clone, Debug)]
pub struct Occ {
    pub start: NaiveDateTime,
    pub end: NaiveDateTime,
    pub patch: Value,
}

#[derive(Clone, Debug)]
pub struct Ev {
    pub id: String,
    pub cal_id: String,
    /// Graph props except id, start, end, type, seriesMasterId, recurrence.
    pub json: Value,
    /// UTC for timed events; floating local midnight for all-day events.
    pub start: NaiveDateTime,
    pub end: NaiveDateTime,
    pub all_day: bool,
    /// Time zone the event was created in (IANA or "UTC").
    pub tz: String,
    pub recurrence: Option<Value>,
    /// Keyed by the occurrence's original local date `YYYYMMDD`.
    pub exceptions: HashMap<String, Occ>,
    pub deleted_occ: HashSet<String>,
}

#[derive(Clone, Debug)]
pub struct Upload {
    pub mailbox: String,
    pub message_id: String,
    pub name: String,
    pub content_type: String,
    pub size: usize,
    pub data: Vec<u8>,
    pub is_inline: bool,
    pub content_id: Option<String>,
    pub expires: DateTime<Utc>,
}

pub struct Mailbox {
    pub addr: String,
    pub display_name: String,
    pub user_id: String,
    pub is_primary: bool,
    pub root_id: String,
    pub folders: Vec<Folder>,
    pub messages: HashMap<String, Msg>,
    /// (seq, folder_id, message_id) – change log for delta queries.
    pub changes: Vec<(u64, String, String)>,
    pub seq: u64,
    pub categories: Vec<Value>,
    pub calendars: Vec<Cal>,
    pub events: HashMap<String, Ev>,
    pub auto_reply: Value,
    pub people: Vec<Value>,
    /// Outlook contacts: (last change seq, contact json).
    pub contacts: Vec<(u64, Value)>,
    /// Deleted contacts: (seq, id) – for delta queries.
    pub contact_tombstones: Vec<(u64, String)>,
    /// Delta tokens below this change number are expired (410 syncStateNotFound).
    pub delta_floor: u64,
}

impl Mailbox {
    pub fn new(addr: &str, display_name: &str, is_primary: bool) -> Self {
        let root_id = new_id("AAMkAR");
        let mut folders = vec![Folder {
            id: root_id.clone(),
            display_name: "Oberste Ebene des Informationsspeichers".into(),
            parent_id: new_id("AAMkAP"),
            well_known: Some("msgfolderroot".into()),
        }];
        for (wk, name) in WELL_KNOWN {
            folders.push(Folder {
                id: new_id("AAMkAF"),
                display_name: name.to_string(),
                parent_id: root_id.clone(),
                well_known: Some(wk.to_string()),
            });
        }
        let now = Utc::now();
        Mailbox {
            addr: addr.to_ascii_lowercase(),
            display_name: display_name.to_string(),
            user_id: uuid::Uuid::new_v4().to_string(),
            is_primary,
            root_id,
            folders,
            messages: HashMap::new(),
            changes: Vec::new(),
            seq: 0,
            categories: Vec::new(),
            calendars: vec![Cal {
                id: new_id("AAMkAC"),
                name: "Kalender".into(),
                color: "auto".into(),
                hex_color: String::new(),
                can_edit: true,
                is_default: true,
            }],
            events: HashMap::new(),
            auto_reply: json!({
                "status": "disabled",
                "externalAudience": "all",
                "internalReplyMessage": "",
                "externalReplyMessage": "",
                "scheduledStartDateTime": { "dateTime": crate::util::graph_dt(now.naive_utc()), "timeZone": "UTC" },
                "scheduledEndDateTime": { "dateTime": crate::util::graph_dt((now + Duration::days(1)).naive_utc()), "timeZone": "UTC" }
            }),
            people: Vec::new(),
            contacts: Vec::new(),
            contact_tombstones: Vec::new(),
            delta_floor: 0,
        }
    }

    pub fn me(&self) -> Value {
        email(&self.display_name, &self.addr)
    }

    pub fn user_json(&self) -> Value {
        let (given, sur) = self.display_name.split_once(' ').unwrap_or((&self.display_name, ""));
        json!({
            "@odata.context": "https://graph.microsoft.com/v1.0/$metadata#users/$entity",
            "id": self.user_id,
            "displayName": self.display_name,
            "givenName": given,
            "surname": sur,
            "mail": self.addr,
            "userPrincipalName": self.addr,
            "preferredLanguage": "de-DE",
            "jobTitle": null,
            "mobilePhone": null,
            "officeLocation": null,
            "businessPhones": []
        })
    }

    /// Resolves an id or well-known name to a folder.
    pub fn folder(&self, key: &str) -> Option<&Folder> {
        self.folders.iter().find(|f| f.id == key || f.well_known.as_deref().is_some_and(|w| w.eq_ignore_ascii_case(key)))
    }

    pub fn folder_id(&self, key: &str) -> Option<String> {
        self.folder(key).map(|f| f.id.clone())
    }

    pub fn default_calendar_id(&self) -> String {
        self.calendars.iter().find(|c| c.is_default).map(|c| c.id.clone()).unwrap_or_default()
    }

    pub fn render_folder(&self, f: &Folder) -> Value {
        let child = self.folders.iter().filter(|c| c.parent_id == f.id).count();
        let total = self.messages.values().filter(|m| m.folder_id == f.id).count();
        let unread = self.messages.values().filter(|m| m.folder_id == f.id && !m.is_read()).count();
        json!({
            "id": f.id,
            "displayName": f.display_name,
            "parentFolderId": f.parent_id,
            "childFolderCount": child,
            "unreadItemCount": unread,
            "totalItemCount": total,
            "sizeInBytes": total * 24_000,
            "isHidden": false
        })
    }

    fn log(&mut self, folder: &str, id: &str) {
        self.seq += 1;
        self.changes.push((self.seq, folder.to_string(), id.to_string()));
    }

    /// Messages of a folder sorted newest first.
    pub fn folder_messages(&self, folder_id: &str) -> Vec<&Msg> {
        let mut v: Vec<&Msg> = self.messages.values().filter(|m| m.folder_id == folder_id).collect();
        v.sort_by(|a, b| b.received.cmp(&a.received).then(a.id.cmp(&b.id)));
        v
    }

    pub fn insert(&mut self, msg: Msg) -> String {
        let id = msg.id.clone();
        let folder = msg.folder_id.clone();
        self.messages.insert(id.clone(), msg);
        self.log(&folder, &id);
        id
    }

    /// Re-logs a message after an in-place change.
    pub fn changed(&mut self, id: &str) {
        if let Some(m) = self.messages.get_mut(id) {
            m.touch();
            let f = m.folder_id.clone();
            self.log(&f, id);
        }
    }

    pub fn remove(&mut self, id: &str) -> Option<Msg> {
        let m = self.messages.remove(id)?;
        self.log(&m.folder_id, id);
        Some(m)
    }

    /// Moves a message; like Exchange, the moved item gets a new id.
    pub fn move_msg(&mut self, id: &str, dest: &str) -> Option<String> {
        let mut m = self.remove(id)?;
        m.id = new_id("AAMkAD");
        m.folder_id = dest.to_string();
        m.touch();
        Some(self.insert(m))
    }

    pub fn copy_msg(&mut self, id: &str, dest: &str) -> Option<String> {
        let mut m = self.messages.get(id)?.clone();
        m.id = new_id("AAMkAD");
        m.folder_id = dest.to_string();
        m.attachments = m.attachments.iter().map(Att::with_new_id).collect();
        m.touch();
        Some(self.insert(m))
    }

    /// Removes a folder, its subfolders and their messages.
    pub fn delete_folder(&mut self, id: &str) {
        let children: Vec<String> = self.folders.iter().filter(|f| f.parent_id == id).map(|f| f.id.clone()).collect();
        for c in children {
            self.delete_folder(&c);
        }
        let ids: Vec<String> = self.messages.values().filter(|m| m.folder_id == id).map(|m| m.id.clone()).collect();
        for m in ids {
            self.remove(&m);
        }
        self.folders.retain(|f| f.id != id);
    }
}

#[derive(Default)]
pub struct AuthState {
    /// code → (client_id, redirect_uri, challenge, scope)
    pub codes: HashMap<String, (String, String, Option<String>, String)>,
    /// access token → expiry
    pub access: HashMap<String, DateTime<Utc>>,
    /// refresh token → scope
    pub refresh: HashMap<String, String>,
    /// device code → (client_id, scope, polls so far)
    pub device: HashMap<String, (String, String, u32)>,
}

/// An injected failure for the next matching Graph request.
#[derive(Clone, Debug)]
pub struct Fault {
    pub status: u16,
    /// `Retry-After` header value.
    pub retry_after: Option<String>,
    /// Raw response body; default is a Graph JSON error.
    pub body: Option<String>,
    pub content_type: Option<String>,
    /// Only requests whose path (after `/v1.0`) contains this text.
    pub path_contains: Option<String>,
}

impl Fault {
    pub fn status(status: u16) -> Self {
        Fault { status, retry_after: None, body: None, content_type: None, path_contains: None }
    }
    pub fn retry_after(mut self, v: &str) -> Self {
        self.retry_after = Some(v.to_string());
        self
    }
    /// Non-JSON body, e.g. an HTML page of a proxy.
    pub fn body(mut self, content_type: &str, body: &str) -> Self {
        self.content_type = Some(content_type.to_string());
        self.body = Some(body.to_string());
        self
    }
    pub fn on_path(mut self, part: &str) -> Self {
        self.path_contains = Some(part.to_string());
        self
    }
}

pub struct State {
    pub base_url: String,
    pub mailboxes: Vec<Mailbox>,
    pub auth: AuthState,
    pub uploads: HashMap<String, Upload>,
    /// Injected failures: (status, remaining count).
    pub faults: VecDeque<Fault>,
    pub request_count: u64,
    /// Requests to the OAuth token endpoint.
    pub token_requests: u64,
    /// Refresh tokens are rejected (revoked sign-in, password change).
    pub refresh_revoked: bool,
}

impl State {
    pub fn mailbox_idx(&self, key: &str) -> Option<usize> {
        if key == "me" {
            return self.mailboxes.iter().position(|m| m.is_primary);
        }
        let k = key.to_ascii_lowercase();
        self.mailboxes.iter().position(|m| m.addr == k || m.user_id == k)
    }

    pub fn issue_access_token(&mut self) -> String {
        let t = format!("fake-at-{}", uuid::Uuid::new_v4().simple());
        self.auth.access.insert(t.clone(), Utc::now() + Duration::seconds(3600));
        t
    }

    /// Delivers a sent message to every recipient that is a mailbox of this tenant.
    pub fn deliver(&mut self, sent: &Msg) {
        let j = &sent.json;
        let mut rcpts: Vec<String> = Vec::new();
        for key in ["toRecipients", "ccRecipients", "bccRecipients"] {
            if let Some(arr) = j[key].as_array() {
                rcpts.extend(arr.iter().map(email_addr));
            }
        }
        rcpts.sort();
        rcpts.dedup();
        let now = Utc::now();
        for r in rcpts {
            let Some(idx) = self.mailbox_idx(&r) else { continue };
            let mb = &mut self.mailboxes[idx];
            let Some(inbox) = mb.folder_id("inbox") else { continue };
            let mut copy = j.clone();
            copy["bccRecipients"] = json!([]);
            copy["isRead"] = json!(false);
            copy["isDraft"] = json!(false);
            copy["receivedDateTime"] = json!(iso(now));
            copy["createdDateTime"] = json!(iso(now));
            copy["flag"] = json!({ "flagStatus": "notFlagged" });
            copy["categories"] = json!([]);
            copy["changeKey"] = json!(new_id("CQAAAB"));
            let msg = Msg {
                id: new_id("AAMkAD"),
                folder_id: inbox,
                json: copy,
                received: now,
                attachments: sent.attachments.iter().map(Att::with_new_id).collect(),
                event_id: None,
            };
            mb.insert(msg);
        }
    }
}

/// Cloneable handle to the fake's state, with helpers for tests.
#[derive(Clone)]
pub struct SharedState(pub(crate) Arc<Mutex<State>>);

impl SharedState {
    pub(crate) fn lock(&self) -> MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Issues a valid access token without going through the OAuth flow.
    pub fn issue_token(&self) -> String {
        self.lock().issue_access_token()
    }

    /// Makes every issued access token expire (next Graph call → 401).
    pub fn expire_tokens(&self) {
        let past = Utc::now() - Duration::seconds(1);
        for v in self.lock().auth.access.values_mut() {
            *v = past;
        }
    }

    /// The next `count` Graph requests fail with `status` (429/503 add `Retry-After: 1`).
    pub fn fail_next(&self, status: u16, count: usize) {
        let mut f = Fault::status(status);
        if status == 429 || status == 503 {
            f = f.retry_after("1");
        }
        let mut st = self.lock();
        for _ in 0..count {
            st.faults.push_back(f.clone());
        }
    }

    /// Queues an arbitrary failure for the next matching Graph request.
    pub fn inject_fault(&self, fault: Fault) {
        self.lock().faults.push_back(fault);
    }

    /// Requests to the OAuth token endpoint (code exchange and refreshes).
    pub fn token_requests(&self) -> u64 {
        self.lock().token_requests
    }

    /// Rejects all refresh tokens from now on (`invalid_grant`), like a
    /// revoked session.
    pub fn revoke_refresh_tokens(&self) {
        self.lock().refresh_revoked = true;
    }

    /// Expires all delta tokens (mail and contacts) issued so far: the next
    /// delta round with them answers 410 `syncStateNotFound`.
    pub fn expire_delta_tokens(&self) {
        for mb in self.lock().mailboxes.iter_mut() {
            mb.seq += 1;
            mb.delta_floor = mb.seq;
        }
    }

    /// Server-side deletion of a contact (by Graph id), e.g. in Outlook web.
    pub fn delete_contact(&self, mailbox: &str, id: &str) -> bool {
        let mut st = self.lock();
        let Some(idx) = st.mailbox_idx(mailbox) else { return false };
        let mb = &mut st.mailboxes[idx];
        let Some(pos) = mb.contacts.iter().position(|(_, c)| c["id"] == json!(id)) else { return false };
        mb.contacts.remove(pos);
        mb.seq += 1;
        let seq = mb.seq;
        mb.contact_tombstones.push((seq, id.to_string()));
        true
    }

    /// Server-side contacts (rendered).
    pub fn contacts(&self, mailbox: &str) -> Vec<Value> {
        let st = self.lock();
        st.mailbox_idx(mailbox).map(|i| st.mailboxes[i].contacts.iter().map(|(_, c)| c.clone()).collect()).unwrap_or_default()
    }

    /// Number of Graph requests served (excluding OAuth/upload endpoints).
    pub fn request_count(&self) -> u64 {
        self.lock().request_count
    }

    /// Mailbox addresses known to the fake.
    pub fn mailboxes(&self) -> Vec<String> {
        self.lock().mailboxes.iter().map(|m| m.addr.clone()).collect()
    }

    /// Folder id for an id or well-known name (`"inbox"`, ...). `mailbox` = address or `"me"`.
    pub fn folder_id(&self, mailbox: &str, folder: &str) -> Option<String> {
        let st = self.lock();
        let idx = st.mailbox_idx(mailbox)?;
        st.mailboxes[idx].folder_id(folder)
    }

    /// Folder id by display name (first match).
    pub fn folder_id_by_name(&self, mailbox: &str, name: &str) -> Option<String> {
        let st = self.lock();
        let idx = st.mailbox_idx(mailbox)?;
        st.mailboxes[idx].folders.iter().find(|f| f.display_name == name).map(|f| f.id.clone())
    }

    /// Simulates an incoming message. `overrides` are Graph message properties
    /// (e.g. `{"subject": "..", "from": {...}, "isRead": false}`); returns the new id.
    pub fn inject_incoming(&self, mailbox: &str, folder: &str, overrides: Value) -> String {
        let mut st = self.lock();
        let idx = st.mailbox_idx(mailbox).expect("unknown mailbox");
        let mb = &mut st.mailboxes[idx];
        let folder_id = mb.folder_id(folder).expect("unknown folder");
        let now = Utc::now();
        let mut base = json!({
            "isRead": false,
            "from": email("Erika Beispiel", "erika.beispiel@fabrikam.example"),
            "toRecipients": [mb.me()],
            "subject": "Neue Nachricht",
            "body": { "contentType": "html", "content": "<html><body><p>Hallo!</p></body></html>" },
        });
        merge(&mut base, &overrides);
        let json = message_json(now, &base);
        let received = json["receivedDateTime"].as_str().and_then(crate::util::parse_utc).unwrap_or(now);
        mb.insert(Msg { id: new_id("AAMkAD"), folder_id, json, received, attachments: vec![], event_id: None })
    }

    /// Rendered messages of a folder (id or well-known name), newest first.
    pub fn messages_in(&self, mailbox: &str, folder: &str) -> Vec<Value> {
        let st = self.lock();
        let Some(idx) = st.mailbox_idx(mailbox) else { return vec![] };
        let mb = &st.mailboxes[idx];
        let Some(fid) = mb.folder_id(folder) else { return vec![] };
        mb.folder_messages(&fid).into_iter().map(Msg::render).collect()
    }

    /// Rendered messages of the Sent Items folder.
    pub fn sent_messages(&self, mailbox: &str) -> Vec<Value> {
        self.messages_in(mailbox, "sentitems")
    }

    /// Rendered message by id (any folder).
    pub fn message(&self, mailbox: &str, id: &str) -> Option<Value> {
        let st = self.lock();
        let idx = st.mailbox_idx(mailbox)?;
        st.mailboxes[idx].messages.get(id).map(Msg::render)
    }

    /// Attachments (name, contentType, bytes) of a message.
    pub fn attachments(&self, mailbox: &str, id: &str) -> Vec<(String, String, Vec<u8>)> {
        let st = self.lock();
        let Some(idx) = st.mailbox_idx(mailbox) else { return vec![] };
        st.mailboxes[idx]
            .messages
            .get(id)
            .map(|m| m.attachments.iter().map(|a| (a.name.clone(), a.content_type.clone(), a.bytes.clone())).collect())
            .unwrap_or_default()
    }

    /// Server-side change of a message (e.g. read on another device). Returns false if unknown.
    pub fn update_message(&self, mailbox: &str, id: &str, patch: Value) -> bool {
        let mut st = self.lock();
        let Some(idx) = st.mailbox_idx(mailbox) else { return false };
        let mb = &mut st.mailboxes[idx];
        let Some(m) = mb.messages.get_mut(id) else { return false };
        merge(&mut m.json, &patch);
        mb.changed(id);
        true
    }

    /// Server-side move; returns the new id.
    pub fn move_message(&self, mailbox: &str, id: &str, dest_folder: &str) -> Option<String> {
        let mut st = self.lock();
        let idx = st.mailbox_idx(mailbox)?;
        let mb = &mut st.mailboxes[idx];
        let dest = mb.folder_id(dest_folder)?;
        mb.move_msg(id, &dest)
    }

    /// Server-side hard delete.
    pub fn delete_message(&self, mailbox: &str, id: &str) -> bool {
        let mut st = self.lock();
        let Some(idx) = st.mailbox_idx(mailbox) else { return false };
        st.mailboxes[idx].remove(id).is_some()
    }

    /// Stored events (singles, series masters, exceptions are not expanded), rendered in UTC.
    pub fn events(&self, mailbox: &str) -> Vec<Value> {
        let st = self.lock();
        let Some(idx) = st.mailbox_idx(mailbox) else { return vec![] };
        let mb = &st.mailboxes[idx];
        let mut evs: Vec<&Ev> = mb.events.values().collect();
        evs.sort_by_key(|e| e.start);
        evs.into_iter().map(|e| crate::calendar::render_event(e, None, "UTC")).collect()
    }

    /// Relabels the time zone of a stored event (test hook, e.g. "Customized
    /// Time Zone" as Exchange uses for foreign ICS invites). Times stay put.
    pub fn set_event_time_zone(&self, mailbox: &str, id: &str, label: &str) -> bool {
        let mut st = self.lock();
        let Some(idx) = st.mailbox_idx(mailbox) else { return false };
        let Some(ev) = st.mailboxes[idx].events.get_mut(id) else { return false };
        ev.tz = label.to_string();
        ev.json["originalStartTimeZone"] = json!(label);
        ev.json["originalEndTimeZone"] = json!(label);
        true
    }

    /// Master category list of a mailbox.
    pub fn categories(&self, mailbox: &str) -> Vec<Value> {
        let st = self.lock();
        st.mailbox_idx(mailbox).map(|i| st.mailboxes[i].categories.clone()).unwrap_or_default()
    }

    /// Current `automaticRepliesSetting` of a mailbox.
    pub fn auto_reply(&self, mailbox: &str) -> Value {
        let st = self.lock();
        st.mailbox_idx(mailbox).map(|i| st.mailboxes[i].auto_reply.clone()).unwrap_or(Value::Null)
    }
}
