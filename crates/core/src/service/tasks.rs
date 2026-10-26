//! Inbox Zero helpers: tasks ("Aufgabe (Mail)"), "Erinnerung an mich" after
//! sending, and the inbox rule chain that runs before new mails are shown.
//!
//! Tasks and reminders are created directly on the server as normal (non-draft)
//! messages – in "Wiedervorlage" with a due date, or unread in the inbox for
//! "Sofort" – so they never show up early on any device.

use chrono::{DateTime, Datelike, Duration, Local, NaiveTime, TimeZone, Utc, Weekday};
use regex::Regex;
use serde_json::{json, Value};

use super::Service;
use crate::convert;
use crate::error::{Error, Result};
use crate::graph::seg;
use crate::model::*;

/// `PR_MESSAGE_FLAGS`: 1 = read; without the "unsent" bit (8) the message is
/// a normal mail, not a draft.
fn message_flags(read: bool) -> Value {
    json!([{"id": "Integer 0x0E07", "value": if read { "1" } else { "0" }}])
}

fn graph_dt(dt: DateTime<Utc>) -> Value {
    json!({"dateTime": dt.format("%Y-%m-%dT%H:%M:%S").to_string(), "timeZone": "UTC"})
}

pub(crate) fn parse_due(due: Option<&str>) -> Result<Option<DateTime<Utc>>> {
    let Some(d) = due.filter(|d| !d.trim().is_empty()) else {
        return Ok(None);
    };
    let dt = DateTime::parse_from_rfc3339(d)
        .map_err(|_| Error::invalid("Ungültiger Fälligkeitszeitpunkt"))?
        .with_timezone(&Utc);
    if dt <= Utc::now() {
        return Err(Error::invalid("Der Fälligkeitszeitpunkt liegt in der Vergangenheit."));
    }
    Ok(Some(dt))
}

/// A mail to create directly in a folder.
pub(crate) struct NewMail {
    pub subject: String,
    pub body_html: String,
    pub to: Vec<Address>,
    pub reply_to: Vec<Address>,
    pub due: Option<DateTime<Utc>>,
    /// Categories to set on creation (hidden ones like "[#monthly]", "[F_…]" included).
    pub categories: Vec<String>,
}

/// Hidden category linking a reminder to the thread of the sent mail.
pub(crate) fn followup_category(thread_id: &str) -> String {
    format!("[F_{}]", thread_hash(thread_id))
}

/// Hidden category linking a delegated task to the thread of the mail sent to
/// the assignee, so a reply can be recognised: "[DT_<hash>]".
pub(crate) fn delegation_thread_category(thread_id: &str) -> String {
    format!("[DT_{}]", thread_hash(thread_id))
}

fn thread_hash(thread_id: &str) -> String {
    use sha2::{Digest, Sha256};
    let h = Sha256::digest(thread_id.trim().as_bytes());
    h.iter().take(5).map(|b| format!("{b:02x}")).collect()
}

/// Hidden category marking a delegated task and its assignee: "[D_<address>]".
pub(crate) fn delegate_category(address: &str) -> String {
    format!("[D_{}]", address.trim().to_lowercase())
}

/// Subject of a delegated task: "ToDo: " unless switched off or already there.
pub(crate) fn delegated_subject(subject: &str, todo_prefix: bool) -> String {
    let s = subject.trim();
    if !todo_prefix || s.get(..5).is_some_and(|p| p.eq_ignore_ascii_case("todo:")) {
        s.to_string()
    } else {
        format!("ToDo: {s}")
    }
}

pub(crate) fn recurrence_category(rule: &str) -> String {
    format!("[#{rule}]")
}

fn is_recurrence_category(c: &str) -> bool {
    recurrence_of(&[c.to_string()]).is_some()
}

/// Hidden category keeping the intended day of month of a monthly/quarterly/
/// yearly task while an occurrence had to be clamped (31st → 28 Feb), so the
/// rhythm returns to the 31st afterwards: "[#d31]".
pub(crate) fn day_hint_category(day: u32) -> String {
    format!("[#d{day}]")
}

fn day_hint_of(categories: &[String]) -> Option<u32> {
    categories.iter().find_map(|c| {
        let d: u32 = c.strip_prefix("[#d")?.strip_suffix(']')?.parse().ok()?;
        (29..=31).contains(&d).then_some(d)
    })
}

fn is_day_hint_category(c: &str) -> bool {
    day_hint_of(&[c.to_string()]).is_some()
}

/// Next occurrence strictly after `now`, advancing from `anchor` in a fixed
/// rhythm (missed occurrences are skipped). Local wall time is kept.
#[cfg(test)]
pub(crate) fn next_occurrence(rule: &str, anchor: DateTime<Utc>, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    next_occurrence_in(rule, anchor, now, None, &Local)
}

/// [`next_occurrence`] in time zone `tz`; `day` is the intended day of month
/// for month based rules (defaults to the anchor's day).
pub(crate) fn next_occurrence_in<Z: TimeZone>(
    rule: &str,
    anchor: DateTime<Utc>,
    now: DateTime<Utc>,
    day: Option<u32>,
    tz: &Z,
) -> Option<DateTime<Utc>> {
    let a = anchor.with_timezone(tz).naive_local();
    let time = a.time();
    let add_months = |d: chrono::NaiveDate, n: i32| -> chrono::NaiveDate {
        let total = d.year() * 12 + d.month0() as i32 + n;
        let (y, m0) = (total.div_euclid(12), total.rem_euclid(12) as u32);
        // Clamp e.g. 31st to the last day of shorter months.
        let mut day = day.unwrap_or(d.day()).clamp(1, 31);
        loop {
            if let Some(x) = chrono::NaiveDate::from_ymd_opt(y, m0 + 1, day) {
                return x;
            }
            day -= 1;
        }
    };
    let anchor_day = a.date();
    let mut i: i32 = 0;
    loop {
        i += 1;
        if i > 100_000 {
            return None;
        }
        let date = match rule {
            "daily" => anchor_day + Duration::days(i as i64),
            "weekdays" => {
                let mut d = anchor_day + Duration::days(i as i64);
                while matches!(d.weekday(), Weekday::Sat | Weekday::Sun) {
                    d += Duration::days(1);
                }
                d
            }
            "weekly" => anchor_day + Duration::weeks(i as i64),
            "biweekly" => anchor_day + Duration::weeks(2 * i as i64),
            "monthly" => add_months(anchor_day, i),
            "quarterly" => add_months(anchor_day, 3 * i),
            "yearly" => add_months(anchor_day, 12 * i),
            _ => return None,
        };
        let dt = at_in(tz, date, time)?;
        if dt > now {
            return Some(dt);
        }
    }
}

impl Service {
    pub(crate) async fn create_task(
        &self,
        mailbox_id: &str,
        subject: &str,
        body_html: &str,
        due: Option<&str>,
        recurrence: Option<&str>,
        // Assignee of a delegated task and whether "ToDo: " goes in front of the subject.
        delegate: Option<(&Address, bool)>,
    ) -> Result<MessageSummary> {
        if let Some((d, todo_prefix)) = delegate {
            if recurrence.is_some_and(|r| !r.is_empty()) {
                return Err(Error::invalid("Delegierte Aufgaben können nicht wiederholt werden."));
            }
            return self.delegate_task(mailbox_id, subject, body_html, due, d, todo_prefix).await;
        }
        let subject = subject.trim();
        let mut categories = Vec::new();
        if let Some(r) = recurrence.filter(|r| !r.is_empty()) {
            if !RECURRENCES.contains(&r) {
                return Err(Error::invalid(format!("Unbekannte Wiederholung: {r}")));
            }
            categories.push(recurrence_category(r));
        }
        if subject.is_empty() {
            return Err(Error::invalid("Bitte einen Betreff für die Aufgabe angeben."));
        }
        let due = parse_due(due)?;
        let mb = self.store.get_mailbox(mailbox_id)?;
        let me = Address {
            name: mb.display_name.clone(),
            address: mb.address.clone(),
        };
        self.place_mail(
            mailbox_id,
            NewMail {
                subject: subject.to_string(),
                body_html: body_html.to_string(),
                to: vec![me],
                reply_to: vec![],
                due,
                categories,
            },
        )
        .await
    }

    /// Delegation: the task goes out as a mail to `to`; a copy for yourself
    /// marked "[D_<address>]" waits in "Wiedervorlage" (or the inbox without
    /// due time) like a normal task, so it can be followed up.
    async fn delegate_task(
        &self,
        mailbox_id: &str,
        subject: &str,
        body_html: &str,
        due: Option<&str>,
        to: &Address,
        todo_prefix: bool,
    ) -> Result<MessageSummary> {
        if subject.trim().is_empty() {
            return Err(Error::invalid("Bitte einen Betreff für die Aufgabe angeben."));
        }
        let to = Address { name: to.name.trim().to_string(), address: to.address.trim().to_string() };
        if !super::compose::valid_address(&to.address) {
            return Err(Error::invalid(format!("Ungültige Adresse: {}", to.address)));
        }
        let due = parse_due(due)?;
        let subject = delegated_subject(subject, todo_prefix);
        let thread = self.send_draft_thread(DraftInput {
            id: None,
            mailbox_id: mailbox_id.to_string(),
            mode: "new".into(),
            to: vec![to.clone()],
            cc: vec![],
            bcc: vec![],
            subject: subject.clone(),
            body_html: body_html.to_string(),
            importance: "normal".into(),
            request_read_receipt: false,
            new_attachments: vec![],
            source_message_id: None,
            removed_attachment_ids: vec![],
            remind_at: None,
        }, true)
        .await?;
        self.place_delegation(mailbox_id, subject, body_html, &to, due, thread).await
    }

    /// Delegating an existing mail: it is forwarded (`d`, mode "forward",
    /// exactly one "An" recipient: the assignee) and followed up like a
    /// delegated task.
    pub(crate) async fn delegate_draft(&self, mut d: DraftInput, due: Option<&str>, todo_prefix: bool) -> Result<MessageSummary> {
        let to = match d.to.as_slice() {
            [one] => Address { name: one.name.trim().to_string(), address: one.address.trim().to_string() },
            [] => return Err(Error::invalid("Bitte die Person angeben, an die delegiert wird.")),
            _ => return Err(Error::invalid("Eine Aufgabe kann nur an genau eine Person delegiert werden (weitere bitte in Cc).")),
        };
        if !super::compose::valid_address(&to.address) {
            return Err(Error::invalid(format!("Ungültige Adresse: {}", to.address)));
        }
        let due = parse_due(due)?;
        d.subject = delegated_subject(&d.subject, todo_prefix);
        // The delegation copy is the reminder; no second one.
        d.remind_at = None;
        let (mailbox_id, subject, body_html) = (d.mailbox_id.clone(), d.subject.clone(), d.body_html.clone());
        let thread = self.send_draft_thread(d, true).await?;
        self.place_delegation(&mailbox_id, subject, &body_html, &to, due, thread).await
    }

    /// The copy of a delegation for yourself: "[D_<address>]" plus the
    /// thread marker, in "Wiedervorlage" (or the inbox without due time).
    async fn place_delegation(
        &self,
        mailbox_id: &str,
        subject: String,
        body_html: &str,
        to: &Address,
        due: Option<DateTime<Utc>>,
        thread: Option<String>,
    ) -> Result<MessageSummary> {
        let name = if to.name.is_empty() { to.address.clone() } else { format!("{} &lt;{}&gt;", html_escape::encode_text(&to.name), to.address) };
        let header = format!(
            "<div style=\"padding:8px 12px;margin-bottom:12px;background:#f3f6fc;border-left:3px solid #2f6fe4;font-family:sans-serif;font-size:13px\">\
             <b>Delegiert an:</b> {name}<br>Gesendet: {}</div>",
            Local::now().format("%d.%m.%Y %H:%M")
        );
        // The mail is out; a failing copy must say so instead of inviting a resend.
        self.place_mail(
            mailbox_id,
            NewMail {
                subject,
                body_html: format!("{header}{body_html}"),
                to: vec![to.clone()],
                reply_to: vec![to.clone()],
                due,
                categories: std::iter::once(delegate_category(&to.address))
                    .chain(thread.as_deref().filter(|t| !t.is_empty()).map(delegation_thread_category))
                    .collect(),
            },
        )
        .await
        .map_err(|e| Error::Other(format!("Die Aufgabe wurde an {} gesendet, die Wiedervorlage konnte aber nicht angelegt werden: {e}", to.address)))
    }

    /// The open delegated task a received mail answers (same thread), if any.
    pub(crate) fn find_delegation(&self, mailbox_id: &str, message_id: &str) -> Result<Option<MessageSummary>> {
        let Some(m) = self.store.get_summary(mailbox_id, message_id)? else { return Ok(None) };
        let me = self.store.get_mailbox(mailbox_id)?.address;
        if m.conversation_id.is_empty() || m.from.as_ref().is_some_and(|f| f.address.eq_ignore_ascii_case(&me)) {
            return Ok(None);
        }
        let closed: Vec<String> = self
            .store
            .list_folders(mailbox_id)?
            .into_iter()
            .filter(|f| matches!(f.well_known.as_deref(), Some("deleteditems" | "archive" | "sentitems" | "drafts" | "junkemail")))
            .map(|f| f.id)
            .collect();
        Ok(self
            .store
            .messages_with_category(mailbox_id, &delegation_thread_category(&m.conversation_id))?
            .into_iter()
            .find(|d| d.id != m.id && !closed.contains(&d.folder_id) && delegate_of(&d.categories).is_some()))
    }

    /// Open delegated tasks of all accounts (not archived or deleted), by due time.
    pub(crate) fn list_delegations(&self) -> Result<Vec<MessageSummary>> {
        let mut out = Vec::new();
        for acc in self.store.list_accounts()? {
            for mb in &acc.mailboxes {
                let excluded: Vec<String> = self
                    .store
                    .list_folders(&mb.id)?
                    .into_iter()
                    .filter(|f| matches!(f.well_known.as_deref(), Some("deleteditems" | "archive" | "sentitems" | "drafts" | "junkemail")))
                    .map(|f| f.id)
                    .collect();
                out.extend(
                    self.store
                        .messages_with_category_prefix(&mb.id, "[D_")?
                        .into_iter()
                        .filter(|m| !excluded.contains(&m.folder_id) && delegate_of(&m.categories).is_some()),
                );
            }
        }
        // Due ones first (earliest on top), then the rest newest first.
        out.sort_by(|a, b| match (&a.due_at, &b.due_at) {
            (Some(x), Some(y)) => x.cmp(y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => b.received_at.cmp(&a.received_at),
        });
        Ok(out)
    }

    /// Creates a non-draft mail from yourself: due → "Wiedervorlage" (read),
    /// no due → inbox (unread).
    pub(crate) async fn place_mail(&self, mailbox_id: &str, m: NewMail) -> Result<MessageSummary> {
        if crate::imap::is_imap_id(mailbox_id) {
            return self.imap_place_mail(mailbox_id, m).await;
        }
        let folder = match m.due {
            Some(_) => self.ensure_folder(mailbox_id, "snoozed", super::snooze::SNOOZE_FOLDER_NAME).await?,
            None => self
                .store
                .folder_by_well_known(mailbox_id, "inbox")?
                .ok_or_else(|| Error::NotFound("Posteingang".into()))?,
        };
        let (mb, c) = self.mailbox(mailbox_id)?;
        let mut body = json!({
            "subject": m.subject,
            "body": {"contentType": "html", "content": m.body_html},
            "from": {"emailAddress": {"name": mb.display_name, "address": mb.address}},
            "toRecipients": convert::recipients_json(&m.to),
            "singleValueExtendedProperties": message_flags(m.due.is_some()),
        });
        if !m.reply_to.is_empty() {
            body["replyTo"] = convert::recipients_json(&m.reply_to);
        }
        if !m.categories.is_empty() {
            body["categories"] = json!(m.categories);
        }
        if let Some(due) = m.due {
            body["flag"] = json!({
                "flagStatus": "flagged",
                "startDateTime": graph_dt(Utc::now()),
                "dueDateTime": graph_dt(due),
            });
        }
        let v = c
            .post(
                &format!("{}/mailFolders/{}/messages", mb.graph_prefix(), seg(&folder.id)),
                &body,
            )
            .await?;
        let mut summary = convert::message_summary(&v, mailbox_id, &folder.id);
        summary.folder_id = folder.id.clone();
        self.store.upsert_messages(std::slice::from_ref(&summary))?;
        self.store
            .adjust_folder_counts(mailbox_id, &folder.id, if summary.is_read { 0 } else { 1 }, 1)?;
        self.emit_mail_changed(mailbox_id, Some(&folder.id));
        Ok(summary)
    }

    /// "Erinnerung an mich": a follow-up mail in "Wiedervorlage" after sending.
    pub(crate) async fn create_send_reminder(&self, d: &DraftInput, remind_at: &str, thread_id: Option<&str>) -> Result<()> {
        let due = parse_due(Some(remind_at))?;
        let recipients: Vec<Address> = d.to.iter().chain(&d.cc).cloned().collect();
        let names = recipients
            .iter()
            .map(|a| if a.name.trim().is_empty() { a.address.clone() } else { a.name.clone() })
            .collect::<Vec<_>>()
            .join(", ");
        let sent = Local::now().format("%d.%m.%Y %H:%M");
        let header = format!(
            "<div style=\"padding:8px 12px;margin-bottom:12px;background:#f3f6fc;border-left:3px solid #2f6fe4;font-family:sans-serif;font-size:13px\">\
             <b>Erinnerung:</b> Nachfassen, ob diese Mail bearbeitet wurde.<br>An: {}<br>Gesendet: {}</div>",
            html_escape::encode_text(&names),
            sent
        );
        self.place_mail(
            &d.mailbox_id,
            NewMail {
                subject: format!("Nachfassen: {}", d.subject.trim()),
                body_html: format!("{header}{}", d.body_html),
                to: recipients.clone(),
                reply_to: recipients,
                due,
                categories: thread_id.map(followup_category).into_iter().collect(),
            },
        )
        .await?;
        Ok(())
    }

    /// Rule chain for new inbox mails, run before they are shown. Returns the
    /// ids of mails that were moved away.
    pub(crate) async fn apply_inbox_rules(&self, mailbox_id: &str, new_mails: &[MessageSummary]) -> Vec<String> {
        let Ok(mb) = self.store.get_mailbox(mailbox_id) else {
            return vec![];
        };
        let options = self.config().snooze_options;
        let mut moved = Vec::new();
        let mut subjects = Vec::new();
        for m in new_mails {
            let from_me = m
                .from
                .as_ref()
                .is_some_and(|f| f.address.eq_ignore_ascii_case(&mb.address));
            if !from_me {
                // Rule: a reply in a thread we wait on resolves the reminder.
                if !m.conversation_id.is_empty() {
                    self.resolve_followups(mailbox_id, m).await;
                }
                continue;
            }
            let Some(due) = due_from_subject(&m.subject, Local::now(), &options) else {
                continue;
            };
            let until = due.to_rfc3339();
            match self.snooze_messages(mailbox_id, std::slice::from_ref(&m.id), &until).await {
                Ok(()) => {
                    moved.push(m.id.clone());
                    subjects.push(m.subject.clone());
                }
                Err(e) => tracing::warn!("Regel konnte nicht angewendet werden: {e}"),
            }
        }
        if !moved.is_empty() {
            self.emit(
                "rules-applied",
                json!({"mailboxId": mailbox_id, "count": moved.len(), "subjects": subjects}),
            );
        }
        moved
    }
}

impl Service {
    async fn resolve_followups(&self, mailbox_id: &str, reply: &MessageSummary) {
        let cat = followup_category(&reply.conversation_id);
        let Ok(reminders) = self.store.messages_with_category(mailbox_id, &cat) else {
            return;
        };
        let ids: Vec<String> = reminders.iter().filter(|r| r.id != reply.id).map(|r| r.id.clone()).collect();
        if ids.is_empty() {
            return;
        }
        match self.archive_messages(mailbox_id, &ids).await {
            Ok(()) => {
                let from = reply
                    .from
                    .as_ref()
                    .map(|a| if a.name.is_empty() { a.address.clone() } else { a.name.clone() })
                    .unwrap_or_default();
                for r in &reminders {
                    self.emit(
                        "followup-resolved",
                        json!({"mailboxId": mailbox_id, "subject": r.subject, "from": from}),
                    );
                }
            }
            Err(e) => tracing::warn!("Nachfassen konnte nicht erledigt werden: {e}"),
        }
    }

    /// Archiving a recurring task places its next occurrence in "Wiedervorlage".
    pub(crate) async fn schedule_recurrences(&self, mailbox_id: &str, ids: &[String]) -> Result<()> {
        for id in ids {
            let Some(m) = self.store.get_summary(mailbox_id, id)? else { continue };
            let Some(rule) = m.recurrence.clone() else { continue };
            let anchor = m
                .due_at
                .as_deref()
                .and_then(|d| DateTime::parse_from_rfc3339(d).ok())
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(Utc::now);
            let intended = day_hint_of(&m.categories).unwrap_or_else(|| anchor.with_timezone(&Local).day());
            let Some(next) = next_occurrence_in(&rule, anchor, Utc::now(), Some(intended), &Local) else { continue };
            let mut categories: Vec<String> = m.categories.iter().filter(|c| !is_day_hint_category(c)).cloned().collect();
            if matches!(rule.as_str(), "monthly" | "quarterly" | "yearly") && next.with_timezone(&Local).day() != intended {
                categories.push(day_hint_category(intended));
            }
            let detail = self.get_message(mailbox_id, id).await?;
            let mb = self.store.get_mailbox(mailbox_id)?;
            self.place_mail(
                mailbox_id,
                NewMail {
                    subject: m.subject.clone(),
                    body_html: detail.body_html,
                    to: vec![Address { name: mb.display_name.clone(), address: mb.address.clone() }],
                    reply_to: vec![],
                    due: Some(next),
                    categories,
                },
            )
            .await?;
            self.emit(
                "recurrence-scheduled",
                json!({"mailboxId": mailbox_id, "subject": m.subject, "due": next.to_rfc3339()}),
            );
        }
        Ok(())
    }

    pub(crate) async fn set_recurrence(&self, mailbox_id: &str, ids: &[String], rule: Option<&str>) -> Result<()> {
        if let Some(r) = rule {
            if !RECURRENCES.contains(&r) {
                return Err(Error::invalid(format!("Unbekannte Wiederholung: {r}")));
            }
        }
        for id in ids {
            let current = match self.store.get_summary(mailbox_id, id)? {
                Some(m) => m.categories,
                None => self.get_message(mailbox_id, id).await?.summary.categories,
            };
            let mut cats: Vec<String> = current
                .into_iter()
                .filter(|c| !is_recurrence_category(c) && !is_day_hint_category(c))
                .collect();
            if let Some(r) = rule {
                cats.push(recurrence_category(r));
            }
            self.set_categories(mailbox_id, std::slice::from_ref(id), &cats).await?;
        }
        Ok(())
    }

    /// "Heute": everything due before the end of today plus each account's first lane.
    pub(crate) fn list_today(&self) -> Result<Value> {
        let tomorrow = Local::now().date_naive() + Duration::days(1);
        let end = at(tomorrow, NaiveTime::MIN).unwrap_or_else(Utc::now).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let due = self.due_before(&end)?;
        let mut todo = Vec::new();
        for acc in self.store.list_accounts()? {
            let lanes = self.get_lanes(&acc.id)?;
            for mb in &acc.mailboxes {
                let excluded = self.closed_folders(&mb.id)?;
                if let Some(first) = lanes.first() {
                    let items: Vec<MessageSummary> = self
                        .store
                        .messages_with_category(&mb.id, &super::lanes::lane_category(first))?
                        .into_iter()
                        .filter(|m| !excluded.contains(&m.folder_id) && !self.pending.is_leaving(m))
                        .collect();
                    todo.push(json!({"accountId": acc.id, "lane": first, "items": items}));
                }
            }
        }
        Ok(json!({"due": due, "todo": todo}))
    }

    /// Folders whose mails are done or not tasks (trash, archive, sent, drafts, junk).
    fn closed_folders(&self, mailbox_id: &str) -> Result<Vec<String>> {
        Ok(self
            .store
            .list_folders(mailbox_id)?
            .into_iter()
            .filter(|f| matches!(f.well_known.as_deref(), Some("deleteditems" | "archive" | "sentitems" | "drafts" | "junkemail")))
            .map(|f| f.id)
            .collect())
    }

    /// Open mails of all accounts with a due time before `end` (RFC 3339), oldest first.
    pub(crate) fn due_before(&self, end: &str) -> Result<Vec<MessageSummary>> {
        let mut due = Vec::new();
        for acc in self.store.list_accounts()? {
            for mb in &acc.mailboxes {
                let excluded = self.closed_folders(&mb.id)?;
                due.extend(
                    self.store
                        .messages_due_before(&mb.id, end)?
                        .into_iter()
                        .filter(|m| !excluded.contains(&m.folder_id) && !self.pending.is_leaving(m)),
                );
            }
        }
        due.sort_by(|a, b| a.due_at.cmp(&b.due_at));
        Ok(due)
    }

    // ---- Fokuszeit ----

    pub(crate) fn focus_until(&self) -> Option<DateTime<Utc>> {
        self.store
            .kv_get("focus_until")
            .ok()
            .flatten()
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|d| d.with_timezone(&Utc))
            .filter(|d| *d > Utc::now())
    }

    pub(crate) fn set_focus(&self, minutes: i64) -> Result<Value> {
        let until = if minutes > 0 {
            let u = Utc::now() + Duration::minutes(minutes.min(24 * 60));
            self.store.kv_set("focus_until", &u.to_rfc3339())?;
            Some(u.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        } else {
            self.store.kv_set("focus_until", "")?;
            None
        };
        let v = json!({"until": until});
        self.emit("focus-changed", v.clone());
        Ok(v)
    }
}

/// Time of day from the snooze option with the same base, else 08:00.
fn option_time(options: &[SnoozeOption], base: &str) -> NaiveTime {
    options
        .iter()
        .find_map(|o| {
            let (b, t) = o.rule.split_once('@')?;
            (b == base).then(|| NaiveTime::parse_from_str(t, "%H:%M").ok())?
        })
        .unwrap_or_else(|| NaiveTime::from_hms_opt(8, 0, 0).expect("valid time"))
}

fn at(date: chrono::NaiveDate, time: NaiveTime) -> Option<DateTime<Utc>> {
    at_in(&Local, date, time)
}

/// Wall time in `tz`; inside a DST gap (02:30 on the last Sunday of March)
/// the first valid time after it, for ambiguous times the earlier instant.
fn at_in<Z: TimeZone>(tz: &Z, date: chrono::NaiveDate, time: NaiveTime) -> Option<DateTime<Utc>> {
    let local = date.and_time(time);
    (0..=180)
        .find_map(|m| tz.from_local_datetime(&(local + Duration::minutes(m))).earliest())
        .map(|d| d.with_timezone(&Utc))
}

fn next_weekday(from: chrono::NaiveDate, wd: Weekday) -> chrono::NaiveDate {
    let mut d = from + Duration::days(1);
    while d.weekday() != wd {
        d += Duration::days(1);
    }
    d
}

/// Due date from a token in the subject: #heute, #morgen, #übermorgen,
/// #wochenende, #nächstewoche, #<N>t / #<N>d (days), #<N>w (weeks).
pub(crate) fn due_from_subject<Z: TimeZone>(subject: &str, now: DateTime<Z>, options: &[SnoozeOption]) -> Option<DateTime<Utc>> {
    let tz = now.timezone();
    let at = |date: chrono::NaiveDate, time: NaiveTime| at_in(&tz, date, time);
    use std::sync::OnceLock;
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?i)(?:^|\s)#(heute|morgen|übermorgen|uebermorgen|wochenende|nächstewoche|naechstewoche|(\d{1,3})([tdw]))(?:\s|$|[.,;:!?)])")
            .expect("regex")
    });
    let caps = re.captures(subject)?;
    let word = caps[1].to_lowercase();
    let today = now.date_naive();
    let now = now.with_timezone(&Utc);
    let due = match word.as_str() {
        "heute" => {
            let five = at(today, NaiveTime::from_hms_opt(17, 0, 0)?)?;
            if five > now + Duration::minutes(30) {
                five
            } else {
                now + Duration::hours(1)
            }
        }
        "morgen" => at(today + Duration::days(1), option_time(options, "tomorrow"))?,
        "übermorgen" | "uebermorgen" => at(today + Duration::days(2), option_time(options, "days:2"))?,
        "wochenende" => at(next_weekday(today, Weekday::Sat), option_time(options, "weekend"))?,
        "nächstewoche" | "naechstewoche" => at(next_weekday(today, Weekday::Mon), option_time(options, "nextWeek"))?,
        _ => {
            let n: i64 = caps.get(2)?.as_str().parse().ok()?;
            if n == 0 {
                return None;
            }
            let days = if caps[3].eq_ignore_ascii_case("w") { n * 7 } else { n };
            at(today + Duration::days(days), option_time(options, &format!("days:{days}")))?
        }
    };
    (due > now).then_some(due)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(y: i32, m: u32, d: u32, h: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, m, d, h, 0, 0).unwrap()
    }

    #[test]
    fn subject_tokens() {
        let opts = default_snooze_options();
        let now = local(2026, 10, 21, 10); // Wednesday
        let due = |s: &str| due_from_subject(s, now, &opts).map(|d| d.with_timezone(&Local));
        assert_eq!(due("Angebot prüfen #morgen").unwrap(), local(2026, 10, 22, 8));
        assert_eq!(due("#Übermorgen Rückruf").unwrap(), local(2026, 10, 23, 8));
        assert_eq!(due("Steuer #wochenende").unwrap(), local(2026, 10, 24, 8));
        assert_eq!(due("Planung #nächstewoche").unwrap(), local(2026, 10, 26, 8));
        assert_eq!(due("Vertrag #14t").unwrap(), local(2026, 11, 4, 8));
        assert_eq!(due("Vertrag #2w.").unwrap(), local(2026, 11, 4, 8));
        assert_eq!(due("Heute noch #heute").unwrap(), local(2026, 10, 21, 17));
        assert!(due("Kein Kürzel").is_none());
        assert!(due("Ticket#morgen").is_none(), "token must stand alone");
        assert!(due("#0t").is_none());
        // Late in the day "#heute" means in one hour.
        let late = local(2026, 10, 21, 18);
        let d = due_from_subject("x #heute", late, &opts).unwrap();
        assert_eq!(d, (late + Duration::hours(1)).with_timezone(&Utc));
    }
}


#[cfg(test)]
mod recurrence_tests {
    use super::*;

    fn local(y: i32, m: u32, d: u32, h: u32) -> DateTime<Utc> {
        Local.with_ymd_and_hms(y, m, d, h, 0, 0).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn fixed_rhythm() {
        let anchor = local(2026, 1, 31, 8);
        // Month ends are clamped.
        assert_eq!(next_occurrence("monthly", anchor, anchor), Some(local(2026, 2, 28, 8)));
        // Missed occurrences are skipped, wall time kept across DST.
        assert_eq!(next_occurrence("monthly", anchor, local(2026, 4, 1, 0)), Some(local(2026, 4, 30, 8)));
        assert_eq!(next_occurrence("weekly", local(2026, 10, 19, 9), local(2026, 10, 30, 0)), Some(local(2026, 11, 2, 9)));
        assert_eq!(next_occurrence("biweekly", local(2026, 10, 19, 9), local(2026, 10, 19, 9)), Some(local(2026, 11, 2, 9)));
        // Friday → Monday for weekdays.
        assert_eq!(next_occurrence("weekdays", local(2026, 10, 23, 8), local(2026, 10, 23, 8)), Some(local(2026, 10, 26, 8)));
        assert_eq!(next_occurrence("quarterly", local(2026, 11, 30, 8), local(2026, 11, 30, 8)), Some(local(2027, 2, 28, 8)));
        assert_eq!(next_occurrence("yearly", local(2028, 2, 29, 8), local(2028, 2, 29, 8)), Some(local(2029, 2, 28, 8)));
        assert_eq!(next_occurrence("daily", local(2026, 10, 20, 7), local(2026, 10, 20, 7)), Some(local(2026, 10, 21, 7)));
        assert!(next_occurrence("hourly", anchor, anchor).is_none());
    }

    #[test]
    fn delegation_markers() {
        assert_eq!(delegate_category(" Anna@Example.com "), "[D_anna@example.com]");
        assert_eq!(delegate_of(&["X".into(), "[D_anna@example.com]".into()]).as_deref(), Some("anna@example.com"));
        assert_eq!(delegate_of(&["[D_kein-mail]".into()]), None);
        assert_eq!(delegated_subject(" Bericht ", true), "ToDo: Bericht");
        assert_eq!(delegated_subject("todo: Bericht", true), "todo: Bericht");
        assert_eq!(delegated_subject("Bericht", false), "Bericht");
    }

    #[test]
    fn followup_category_is_stable() {
        let a = followup_category("AAQkADconv1");
        assert_eq!(a, followup_category(" AAQkADconv1 "));
        assert!(a.starts_with("[F_") && a.len() == 14);
        assert_ne!(a, followup_category("AAQkADconv2"));
    }
}

/// Edge cases in a fixed zone (Europe/Berlin), independent of the machine's
/// local time zone.
#[cfg(test)]
mod berlin_tests {
    use super::*;
    use chrono_tz::Europe::Berlin;

    fn b(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Berlin.with_ymd_and_hms(y, m, d, h, min, 0).earliest().unwrap().with_timezone(&Utc)
    }

    fn next(rule: &str, anchor: DateTime<Utc>, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        next_occurrence_in(rule, anchor, now, None, &Berlin)
    }

    fn next_day(rule: &str, anchor: DateTime<Utc>, day: u32) -> Option<DateTime<Utc>> {
        next_occurrence_in(rule, anchor, anchor, Some(day), &Berlin)
    }

    #[test]
    fn wall_time_survives_dst() {
        // Spring forward (last Sunday of March): 08:00 stays 08:00.
        assert_eq!(next("daily", b(2027, 3, 27, 8, 0), b(2027, 3, 27, 8, 0)), Some(b(2027, 3, 28, 8, 0)));
        assert_eq!(b(2027, 3, 28, 8, 0).to_rfc3339(), "2027-03-28T06:00:00+00:00");
        // Fall back (last Sunday of October).
        assert_eq!(next("weekly", b(2026, 10, 19, 9, 0), b(2026, 10, 19, 9, 0)), Some(b(2026, 10, 26, 9, 0)));
        assert_eq!(b(2026, 10, 26, 9, 0).to_rfc3339(), "2026-10-26T08:00:00+00:00");
    }

    #[test]
    fn occurrence_inside_dst_gap_is_not_lost() {
        // 02:30 does not exist on 2027-03-28; the task must not silently stop.
        let n = next("daily", b(2027, 3, 27, 2, 30), b(2027, 3, 27, 2, 30)).expect("next occurrence");
        assert_eq!(n.to_rfc3339(), "2027-03-28T01:00:00+00:00", "03:00 CEST, right after the gap");
        // 02:30 exists twice on 2026-10-25: the first one.
        let n = next("daily", b(2026, 10, 24, 2, 30), b(2026, 10, 24, 2, 30)).unwrap();
        assert_eq!(n.to_rfc3339(), "2026-10-25T00:30:00+00:00");
    }

    #[test]
    fn weekdays_skip_weekends() {
        // 2026-10-23 is a Friday.
        assert_eq!(next("weekdays", b(2026, 10, 22, 8, 0), b(2026, 10, 22, 8, 0)), Some(b(2026, 10, 23, 8, 0)));
        assert_eq!(next("weekdays", b(2026, 10, 23, 8, 0), b(2026, 10, 23, 8, 0)), Some(b(2026, 10, 26, 8, 0)));
        assert_eq!(next("weekdays", b(2026, 10, 24, 8, 0), b(2026, 10, 24, 8, 0)), Some(b(2026, 10, 26, 8, 0)));
        assert_eq!(next("weekdays", b(2026, 10, 25, 8, 0), b(2026, 10, 25, 8, 0)), Some(b(2026, 10, 26, 8, 0)));
        // Missed days while away: next working day after "now".
        assert_eq!(next("weekdays", b(2026, 10, 19, 8, 0), b(2026, 10, 24, 12, 0)), Some(b(2026, 10, 26, 8, 0)));
    }

    #[test]
    fn month_ends_and_leap_years() {
        // 31st → end of February (2027 is no leap year), 2028 is.
        assert_eq!(next("monthly", b(2027, 1, 31, 8, 0), b(2027, 1, 31, 8, 0)), Some(b(2027, 2, 28, 8, 0)));
        assert_eq!(next("monthly", b(2028, 1, 31, 8, 0), b(2028, 1, 31, 8, 0)), Some(b(2028, 2, 29, 8, 0)));
        // Without a hint the clamped day becomes the new anchor …
        assert_eq!(next("monthly", b(2027, 2, 28, 8, 0), b(2027, 2, 28, 8, 0)), Some(b(2027, 3, 28, 8, 0)));
        // … with the intended day the rhythm returns to the month end.
        assert_eq!(next_day("monthly", b(2027, 2, 28, 8, 0), 31), Some(b(2027, 3, 31, 8, 0)));
        assert_eq!(next_day("monthly", b(2027, 3, 31, 8, 0), 31), Some(b(2027, 4, 30, 8, 0)));
        assert_eq!(next_day("monthly", b(2027, 4, 30, 8, 0), 31), Some(b(2027, 5, 31, 8, 0)));
        assert_eq!(next_day("quarterly", b(2026, 11, 30, 8, 0), 30), Some(b(2027, 2, 28, 8, 0)));
        assert_eq!(next_day("quarterly", b(2027, 2, 28, 8, 0), 30), Some(b(2027, 5, 30, 8, 0)));
        // 29 February, yearly: 28 Feb in common years, back to the 29th in leap years.
        assert_eq!(next("yearly", b(2028, 2, 29, 8, 0), b(2028, 2, 29, 8, 0)), Some(b(2029, 2, 28, 8, 0)));
        assert_eq!(next_day("yearly", b(2031, 2, 28, 8, 0), 29), Some(b(2032, 2, 29, 8, 0)));
        // Day hint never produces invalid dates.
        assert_eq!(next_day("monthly", b(2027, 1, 15, 8, 0), 31), Some(b(2027, 2, 28, 8, 0)));
    }

    #[test]
    fn year_boundaries_and_missed_occurrences() {
        assert_eq!(next("daily", b(2026, 12, 31, 18, 0), b(2026, 12, 31, 18, 0)), Some(b(2027, 1, 1, 18, 0)));
        assert_eq!(next("weekly", b(2026, 12, 28, 9, 0), b(2026, 12, 28, 9, 0)), Some(b(2027, 1, 4, 9, 0)));
        assert_eq!(next("biweekly", b(2026, 12, 21, 9, 0), b(2026, 12, 21, 9, 0)), Some(b(2027, 1, 4, 9, 0)));
        assert_eq!(next("monthly", b(2026, 12, 15, 9, 0), b(2026, 12, 15, 9, 0)), Some(b(2027, 1, 15, 9, 0)));
        assert_eq!(next("quarterly", b(2026, 11, 15, 9, 0), b(2026, 11, 15, 9, 0)), Some(b(2027, 2, 15, 9, 0)));
        assert_eq!(next("yearly", b(2026, 12, 31, 9, 0), b(2026, 12, 31, 9, 0)), Some(b(2027, 12, 31, 9, 0)));
        // Months offline: the next occurrence after now, in the old rhythm.
        assert_eq!(next("monthly", b(2026, 1, 15, 8, 0), b(2026, 10, 20, 12, 0)), Some(b(2026, 11, 15, 8, 0)));
        assert_eq!(next("weekly", b(2025, 1, 6, 8, 0), b(2026, 10, 20, 12, 0)), Some(b(2026, 10, 26, 8, 0)));
        // "Strictly after now".
        assert_eq!(next("daily", b(2026, 10, 19, 8, 0), b(2026, 10, 20, 8, 0)), Some(b(2026, 10, 21, 8, 0)));
        assert!(next("hourly", b(2026, 10, 19, 8, 0), b(2026, 10, 19, 8, 0)).is_none());
        assert!(next("", b(2026, 10, 19, 8, 0), b(2026, 10, 19, 8, 0)).is_none());
    }

    #[test]
    fn day_hint_categories() {
        assert_eq!(day_hint_category(31), "[#d31]");
        assert_eq!(day_hint_of(&["Info".into(), "[#monthly]".into(), "[#d31]".into()]), Some(31));
        assert_eq!(day_hint_of(&["[#d15]".into()]), None, "only 29–31 need a hint");
        assert_eq!(day_hint_of(&["[#dx]".into(), "[#d]".into()]), None);
        assert!(!is_recurrence_category("[#d31]"));
        // The hint is hidden but not a recurrence rule.
        assert_eq!(recurrence_of(&["[#d31]".into(), "[#monthly]".into()]).as_deref(), Some("monthly"));
    }

    fn at_berlin(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<chrono_tz::Tz> {
        Berlin.with_ymd_and_hms(y, m, d, h, min, 0).earliest().unwrap()
    }

    fn due(subject: &str, now: DateTime<chrono_tz::Tz>) -> Option<DateTime<chrono_tz::Tz>> {
        due_from_subject(subject, now, &default_snooze_options()).map(|d| d.with_timezone(&Berlin))
    }

    #[test]
    fn subject_tokens_at_boundary_times() {
        // Late evening: "#heute" means in one hour, even across midnight.
        let late = at_berlin(2026, 10, 21, 23, 30);
        assert_eq!(due("x #heute", late), Some(at_berlin(2026, 10, 22, 0, 30)));
        // 17:00 counts only with more than 30 minutes left.
        assert_eq!(due("x #heute", at_berlin(2026, 10, 21, 16, 29)), Some(at_berlin(2026, 10, 21, 17, 0)));
        assert_eq!(due("x #heute", at_berlin(2026, 10, 21, 16, 31)), Some(at_berlin(2026, 10, 21, 17, 31)));
        // Friday/Saturday/Sunday → the next Saturday strictly after today.
        assert_eq!(due("#wochenende", at_berlin(2026, 10, 23, 18, 0)), Some(at_berlin(2026, 10, 24, 8, 0)));
        assert_eq!(due("#wochenende", at_berlin(2026, 10, 24, 7, 0)), Some(at_berlin(2026, 10, 31, 8, 0)));
        assert_eq!(due("#wochenende", at_berlin(2026, 10, 25, 7, 0)), Some(at_berlin(2026, 10, 31, 8, 0)));
        // "Nächste Woche" on Sunday is tomorrow, on Monday a week later.
        assert_eq!(due("#nächstewoche", at_berlin(2026, 10, 25, 20, 0)), Some(at_berlin(2026, 10, 26, 8, 0)));
        assert_eq!(due("#naechstewoche", at_berlin(2026, 10, 26, 7, 0)), Some(at_berlin(2026, 11, 2, 8, 0)));
        // New Year.
        assert_eq!(due("#morgen", at_berlin(2026, 12, 31, 23, 59)), Some(at_berlin(2027, 1, 1, 8, 0)));
        assert_eq!(due("#übermorgen", at_berlin(2026, 12, 31, 10, 0)), Some(at_berlin(2027, 1, 2, 8, 0)));
        assert_eq!(due("#2w", at_berlin(2026, 12, 25, 10, 0)), Some(at_berlin(2027, 1, 8, 8, 0)));
        // Across DST: 08:00 local both times.
        let d = due("#14t", at_berlin(2026, 10, 20, 10, 0)).unwrap();
        assert_eq!(d, at_berlin(2026, 11, 3, 8, 0));
        assert_eq!(d.with_timezone(&Utc).to_rfc3339(), "2026-11-03T07:00:00+00:00");
        // Leap day.
        assert_eq!(due("#morgen", at_berlin(2028, 2, 28, 9, 0)), Some(at_berlin(2028, 2, 29, 8, 0)));
    }

    #[test]
    fn subject_token_spelling() {
        let now = at_berlin(2026, 10, 21, 10, 0);
        let tomorrow = Some(at_berlin(2026, 10, 22, 8, 0));
        assert_eq!(due("#MORGEN", now), tomorrow);
        assert_eq!(due("Bitte prüfen #Morgen!", now), tomorrow);
        assert_eq!(due("#NÄCHSTEWOCHE", now), Some(at_berlin(2026, 10, 26, 8, 0)));
        assert_eq!(due("#UEBERMORGEN", now), Some(at_berlin(2026, 10, 23, 8, 0)));
        assert_eq!(due("#3T", now), Some(at_berlin(2026, 10, 24, 8, 0)));
        assert_eq!(due("#3d", now), Some(at_berlin(2026, 10, 24, 8, 0)));
        // The first token wins.
        assert_eq!(due("#morgen #nächstewoche", now), tomorrow);
        // Not standalone / not a token.
        assert!(due("(#morgen)", now).is_none());
        assert!(due("mail#morgen", now).is_none());
        assert!(due("#morgens", now).is_none());
        assert!(due("#1000t", now).is_none());
        assert!(due("#0w", now).is_none());
        assert!(due("", now).is_none());
        assert!(due("#", now).is_none());
        // Large but valid.
        assert_eq!(due("#999w", now).map(|d| d.date_naive()), Some(now.date_naive() + Duration::weeks(999)));
    }

    #[test]
    fn subject_tokens_use_configured_times() {
        let now = at_berlin(2026, 10, 21, 10, 0);
        let opts = vec![
            SnoozeOption { label: "a".into(), rule: "tomorrow@07:30".into() },
            SnoozeOption { label: "b".into(), rule: "days:3@09:15".into() },
            SnoozeOption { label: "c".into(), rule: "weekend@25:00".into() },
            SnoozeOption { label: "d".into(), rule: "nextWeek".into() },
        ];
        let d = |s: &str| due_from_subject(s, now, &opts).map(|d| d.with_timezone(&Berlin));
        assert_eq!(d("#morgen"), Some(at_berlin(2026, 10, 22, 7, 30)));
        assert_eq!(d("#3t"), Some(at_berlin(2026, 10, 24, 9, 15)));
        // Invalid or missing times fall back to 08:00.
        assert_eq!(d("#wochenende"), Some(at_berlin(2026, 10, 24, 8, 0)));
        assert_eq!(d("#nächstewoche"), Some(at_berlin(2026, 10, 26, 8, 0)));
        assert_eq!(d("#übermorgen"), Some(at_berlin(2026, 10, 23, 8, 0)));
        // No options at all.
        assert_eq!(due_from_subject("#morgen", now, &[]).map(|d| d.with_timezone(&Berlin)), Some(at_berlin(2026, 10, 22, 8, 0)));
    }
}
