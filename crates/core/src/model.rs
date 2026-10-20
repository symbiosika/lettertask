//! Types exchanged with the UI. See `docs/API.md`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Address {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub address: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub email: String,
    pub display_name: String,
    /// "o365" | "imap" (derived from the id prefix).
    #[serde(default)]
    pub kind: String,
    pub mailboxes: Vec<Mailbox>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Mailbox {
    pub id: String,
    pub account_id: String,
    pub address: String,
    pub display_name: String,
    /// "primary" | "shared"
    pub kind: String,
}

impl Mailbox {
    pub fn is_primary(&self) -> bool {
        self.kind == "primary"
    }

    /// Graph path prefix for this mailbox.
    pub fn graph_prefix(&self) -> String {
        if self.is_primary() {
            "/me".to_string()
        } else {
            format!("/users/{}", urlencoding::encode(&self.address))
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    pub id: String,
    pub mailbox_id: String,
    pub parent_id: Option<String>,
    pub display_name: String,
    pub well_known: Option<String>,
    pub unread_count: i64,
    pub total_count: i64,
    pub child_folder_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MessageSummary {
    pub id: String,
    pub mailbox_id: String,
    pub folder_id: String,
    pub conversation_id: String,
    pub subject: String,
    pub from: Option<Address>,
    pub to: Vec<Address>,
    pub received_at: String,
    pub is_read: bool,
    pub is_draft: bool,
    pub importance: String,
    pub flag: String,
    pub has_attachments: bool,
    pub categories: Vec<String>,
    pub preview: String,
    /// Follow-up due time (UTC); for snoozed mails the "Wiedervorlage" time.
    #[serde(default)]
    pub due_at: Option<String>,
    /// Recurrence rule of a recurring task ("monthly", …), from the hidden "[#…]" category.
    #[serde(default)]
    pub recurrence: Option<String>,
}

/// Recurrence encoded in a hidden category "[#<rule>]".
pub fn recurrence_of(categories: &[String]) -> Option<String> {
    categories.iter().find_map(|c| {
        let r = c.strip_prefix("[#")?.strip_suffix(']')?;
        RECURRENCES.contains(&r).then(|| r.to_string())
    })
}

pub const RECURRENCES: &[&str] = &["daily", "weekdays", "weekly", "biweekly", "monthly", "quarterly", "yearly"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub id: String,
    pub name: String,
    pub content_type: String,
    pub size: i64,
    pub is_inline: bool,
    pub content_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MessageDetail {
    #[serde(flatten)]
    pub summary: MessageSummary,
    pub cc: Vec<Address>,
    pub bcc: Vec<Address>,
    pub reply_to: Vec<Address>,
    pub sent_at: Option<String>,
    pub body_html: String,
    pub attachments: Vec<Attachment>,
    pub internet_message_id: Option<String>,
    pub web_link: Option<String>,
    pub is_meeting_request: bool,
    pub event_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MessagePage {
    pub items: Vec<MessageSummary>,
    pub total: i64,
    pub unread: i64,
    pub can_load_older: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct SyncResult {
    pub added: i64,
    pub updated: i64,
    pub removed: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Category {
    pub id: String,
    pub display_name: String,
    pub color: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Calendar {
    pub id: String,
    pub account_id: String,
    pub name: String,
    pub color: Option<String>,
    pub can_edit: bool,
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Attendee {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub address: String,
    #[serde(default = "default_attendee_type", rename = "type")]
    pub kind: String,
    #[serde(default = "default_none")]
    pub response: String,
}

fn default_attendee_type() -> String {
    "required".into()
}
fn default_none() -> String {
    "none".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Recurrence {
    pub pattern: String,
    #[serde(default = "one")]
    pub interval: i64,
    #[serde(default)]
    pub days_of_week: Vec<String>,
    #[serde(default)]
    pub day_of_month: i64,
    #[serde(default)]
    pub month: i64,
    /// Week of the month for relative patterns: first … fourth, last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<String>,
    pub start_date: String,
    #[serde(default)]
    pub end_date: Option<String>,
    #[serde(default)]
    pub occurrences: Option<i64>,
}

fn one() -> i64 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub id: String,
    pub account_id: String,
    pub calendar_id: String,
    pub subject: String,
    pub start: String,
    pub end: String,
    pub is_all_day: bool,
    pub location: String,
    pub organizer: Option<Address>,
    pub attendees: Vec<Attendee>,
    pub show_as: String,
    pub categories: Vec<String>,
    pub is_organizer: bool,
    pub response_status: String,
    pub body_preview: String,
    pub is_online_meeting: bool,
    pub online_meeting_url: Option<String>,
    pub recurrence: Option<Recurrence>,
    pub series_master_id: Option<String>,
    #[serde(rename = "type")]
    pub kind: String,
    pub is_reminder_on: bool,
    pub reminder_minutes: i64,
    pub importance: String,
    pub sensitivity: String,
    pub is_cancelled: bool,
    pub web_link: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EventDetail {
    #[serde(flatten)]
    pub event: Event,
    pub body_html: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AttendeeInput {
    #[serde(default)]
    pub name: String,
    pub address: String,
    #[serde(default = "default_attendee_type", rename = "type")]
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EventInput {
    #[serde(default)]
    pub subject: String,
    pub start: String,
    pub end: String,
    #[serde(default = "utc")]
    pub time_zone: String,
    #[serde(default)]
    pub is_all_day: bool,
    #[serde(default)]
    pub location: String,
    #[serde(default)]
    pub body_html: String,
    #[serde(default)]
    pub attendees: Vec<AttendeeInput>,
    #[serde(default = "busy")]
    pub show_as: String,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub is_reminder_on: bool,
    #[serde(default)]
    pub reminder_minutes: i64,
    #[serde(default)]
    pub is_online_meeting: bool,
    #[serde(default)]
    pub recurrence: Option<Recurrence>,
    #[serde(default = "normal")]
    pub sensitivity: String,
    #[serde(default = "normal")]
    pub importance: String,
}

fn utc() -> String {
    "UTC".into()
}
fn busy() -> String {
    "busy".into()
}
fn normal() -> String {
    "normal".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Draft {
    pub id: Option<String>,
    pub mailbox_id: String,
    pub mode: String,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub bcc: Vec<Address>,
    pub subject: String,
    pub body_html: String,
    pub importance: String,
    pub request_read_receipt: bool,
    pub attachments: Vec<Attachment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NewAttachment {
    pub name: String,
    #[serde(default = "octet")]
    pub content_type: String,
    pub content_base64: String,
}

fn octet() -> String {
    "application/octet-stream".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DraftInput {
    #[serde(default)]
    pub id: Option<String>,
    pub mailbox_id: String,
    #[serde(default = "new_mode")]
    pub mode: String,
    #[serde(default)]
    pub to: Vec<Address>,
    #[serde(default)]
    pub cc: Vec<Address>,
    #[serde(default)]
    pub bcc: Vec<Address>,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub body_html: String,
    #[serde(default = "normal")]
    pub importance: String,
    #[serde(default)]
    pub request_read_receipt: bool,
    #[serde(default)]
    pub new_attachments: Vec<NewAttachment>,
    #[serde(default)]
    pub removed_attachment_ids: Vec<String>,
    /// "Erinnerung an mich" after sending (ISO UTC).
    #[serde(default)]
    pub remind_at: Option<String>,
}

fn new_mode() -> String {
    "new".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub client_id: String,
    pub tenant: String,
    pub sync_interval_secs: u64,
    pub load_remote_images: bool,
    pub reading_pane: String,
    pub mark_read_delay_secs: i64,
    pub list_mode: String,
    pub show_date_separators: bool,
    pub snooze_options: Vec<SnoozeOption>,
    pub kanban_reading_pane: String,
    /// Read-only for the UI: set when running against the demo server.
    pub demo: bool,
    /// Read-only for the UI: a client ID was compiled into this build.
    pub builtin_client_id: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SnoozeOption {
    pub label: String,
    /// `<base>@HH:MM`, base = tomorrow | days:N | weekend | nextWeek (evaluated by the UI).
    pub rule: String,
}

/// App registration of Symbiosika in Entra ID. Desktop apps are public
/// clients (no secret), so the client id is not confidential. The
/// registration is multi-tenant, so sign-in goes through `organizations`
/// and accepts work accounts from any tenant.
/// Other builds can override both via `LETTERTASK_CLIENT_ID` /
/// `LETTERTASK_TENANT` at compile time.
const SYMBIOSIKA_CLIENT_ID: &str = "c0bb40f0-6019-427c-bbfb-01c552e44257";
const DEFAULT_TENANT: &str = "organizations";

/// Tenant id that earlier builds wrote into the config as default; it only
/// admits accounts of the Symbiosika tenant.
pub const LEGACY_SYMBIOSIKA_TENANT_ID: &str = "be11db5c-df94-439f-b934-4c3240c9d1ad";

pub fn builtin_client_id() -> &'static str {
    option_env!("LETTERTASK_CLIENT_ID").map(str::trim).unwrap_or(SYMBIOSIKA_CLIENT_ID)
}

pub fn builtin_tenant() -> &'static str {
    option_env!("LETTERTASK_TENANT").map(str::trim).unwrap_or(DEFAULT_TENANT)
}

pub fn default_snooze_options() -> Vec<SnoozeOption> {
    [
        ("Morgen 8 Uhr", "tomorrow@08:00"),
        ("Übermorgen 8 Uhr", "days:2@08:00"),
        ("Am Wochenende 8 Uhr", "weekend@08:00"),
        ("Nächste Woche", "nextWeek@08:00"),
        ("In 14 Tagen", "days:14@08:00"),
        ("In 30 Tagen", "days:30@08:00"),
    ]
    .into_iter()
    .map(|(label, rule)| SnoozeOption {
        label: label.into(),
        rule: rule.into(),
    })
    .collect()
}

impl Default for Config {
    fn default() -> Self {
        Config {
            client_id: String::new(),
            tenant: builtin_tenant().into(),
            sync_interval_secs: 60,
            load_remote_images: false,
            reading_pane: "right".into(),
            mark_read_delay_secs: 2,
            list_mode: "comfortable".into(),
            show_date_separators: true,
            snooze_options: default_snooze_options(),
            kanban_reading_pane: "bottom".into(),
            demo: false,
            builtin_client_id: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AutoReply {
    pub status: String,
    #[serde(default)]
    pub internal_reply_message: String,
    #[serde(default)]
    pub external_reply_message: String,
    #[serde(default)]
    pub scheduled_start: Option<String>,
    #[serde(default)]
    pub scheduled_end: Option<String>,
    #[serde(default = "all")]
    pub external_audience: String,
}

fn all() -> String {
    "all".into()
}

/// Query for `list_messages` (served from the local cache).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ListQuery {
    pub mailbox_id: String,
    pub folder_id: String,
    pub sort_by: String,
    pub sort_desc: bool,
    pub unread_only: bool,
    pub flagged_only: bool,
    pub category: Option<String>,
    pub text: Option<String>,
    pub offset: i64,
    pub limit: i64,
}

impl Default for ListQuery {
    fn default() -> Self {
        ListQuery {
            mailbox_id: String::new(),
            folder_id: String::new(),
            sort_by: "received".into(),
            sort_desc: true,
            unread_only: false,
            flagged_only: false,
            category: None,
            text: None,
            offset: 0,
            limit: 200,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Signature {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub html: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct SignatureSettings {
    pub signatures: Vec<Signature>,
    pub default_id: Option<String>,
    pub use_on_new: bool,
    pub use_on_reply: bool,
    pub use_on_forward: bool,
}

impl Default for SignatureSettings {
    fn default() -> Self {
        SignatureSettings {
            signatures: vec![],
            default_id: None,
            use_on_new: true,
            use_on_reply: true,
            use_on_forward: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Phone {
    /// "business" | "mobile" | "home"
    #[serde(default = "business")]
    pub kind: String,
    #[serde(default)]
    pub number: String,
}

fn business() -> String {
    "business".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Contact {
    pub id: String,
    pub account_id: String,
    /// "o365" | "local" | "collected"
    pub source: String,
    pub display_name: String,
    pub given_name: String,
    pub surname: String,
    pub emails: Vec<String>,
    pub company: String,
    pub job_title: String,
    pub phones: Vec<Phone>,
    pub notes: String,
    pub read_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ContactInput {
    pub display_name: String,
    pub given_name: String,
    pub surname: String,
    pub emails: Vec<String>,
    pub company: String,
    pub job_title: String,
    pub phones: Vec<Phone>,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ContactPage {
    pub items: Vec<Contact>,
    pub total: i64,
}
