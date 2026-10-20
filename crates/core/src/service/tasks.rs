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

fn parse_due(due: Option<&str>) -> Result<Option<DateTime<Utc>>> {
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
    use sha2::{Digest, Sha256};
    let h = Sha256::digest(thread_id.trim().as_bytes());
    let hex: String = h.iter().take(5).map(|b| format!("{b:02x}")).collect();
    format!("[F_{hex}]")
}

pub(crate) fn recurrence_category(rule: &str) -> String {
    format!("[#{rule}]")
}

fn is_recurrence_category(c: &str) -> bool {
    recurrence_of(&[c.to_string()]).is_some()
}

/// Next occurrence strictly after `now`, advancing from `anchor` in a fixed
/// rhythm (missed occurrences are skipped). Local wall time is kept.
pub(crate) fn next_occurrence(rule: &str, anchor: DateTime<Utc>, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let a = anchor.with_timezone(&Local).naive_local();
    let time = a.time();
    let add_months = |d: chrono::NaiveDate, n: i32| -> chrono::NaiveDate {
        let total = d.year() * 12 + d.month0() as i32 + n;
        let (y, m0) = (total.div_euclid(12), total.rem_euclid(12) as u32);
        // Clamp e.g. 31st to the last day of shorter months.
        let mut day = d.day();
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
        let dt = at(date, time)?;
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
    ) -> Result<MessageSummary> {
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
            let Some(next) = next_occurrence(&rule, anchor, Utc::now()) else { continue };
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
                    categories: m.categories.clone(),
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
            let mut cats: Vec<String> = current.into_iter().filter(|c| !is_recurrence_category(c)).collect();
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
        let mut due = Vec::new();
        let mut todo = Vec::new();
        for acc in self.store.list_accounts()? {
            let lanes = self.get_lanes(&acc.id)?;
            for mb in &acc.mailboxes {
                let excluded: Vec<String> = self
                    .store
                    .list_folders(&mb.id)?
                    .into_iter()
                    .filter(|f| matches!(f.well_known.as_deref(), Some("deleteditems" | "archive" | "sentitems" | "drafts" | "junkemail")))
                    .map(|f| f.id)
                    .collect();
                due.extend(
                    self.store
                        .messages_due_before(&mb.id, &end)?
                        .into_iter()
                        .filter(|m| !excluded.contains(&m.folder_id)),
                );
                if let Some(first) = lanes.first() {
                    let items: Vec<MessageSummary> = self
                        .store
                        .messages_with_category(&mb.id, &super::lanes::lane_category(first))?
                        .into_iter()
                        .filter(|m| !excluded.contains(&m.folder_id))
                        .collect();
                    todo.push(json!({"accountId": acc.id, "lane": first, "items": items}));
                }
            }
        }
        due.sort_by(|a, b| a.due_at.cmp(&b.due_at));
        Ok(json!({"due": due, "todo": todo}))
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
    Local
        .from_local_datetime(&date.and_time(time))
        .earliest()
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
pub(crate) fn due_from_subject(subject: &str, now: DateTime<Local>, options: &[SnoozeOption]) -> Option<DateTime<Utc>> {
    use std::sync::OnceLock;
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?i)(?:^|\s)#(heute|morgen|übermorgen|uebermorgen|wochenende|nächstewoche|naechstewoche|(\d{1,3})([tdw]))(?:\s|$|[.,;:!?)])")
            .expect("regex")
    });
    let caps = re.captures(subject)?;
    let word = caps[1].to_lowercase();
    let today = now.date_naive();
    let due = match word.as_str() {
        "heute" => {
            let five = at(today, NaiveTime::from_hms_opt(17, 0, 0)?)?;
            if five > now.with_timezone(&Utc) + Duration::minutes(30) {
                five
            } else {
                now.with_timezone(&Utc) + Duration::hours(1)
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
    (due > now.with_timezone(&Utc)).then_some(due)
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
    fn followup_category_is_stable() {
        let a = followup_category("AAQkADconv1");
        assert_eq!(a, followup_category(" AAQkADconv1 "));
        assert!(a.starts_with("[F_") && a.len() == 14);
        assert_ne!(a, followup_category("AAQkADconv2"));
    }
}
