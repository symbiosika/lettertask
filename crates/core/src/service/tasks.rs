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
}

impl Service {
    pub(crate) async fn create_task(
        &self,
        mailbox_id: &str,
        subject: &str,
        body_html: &str,
        due: Option<&str>,
    ) -> Result<MessageSummary> {
        let subject = subject.trim();
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
    pub(crate) async fn create_send_reminder(&self, d: &DraftInput, remind_at: &str) -> Result<()> {
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

