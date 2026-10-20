//! Kanban lanes: per-account list of lane names, stored on mails as hidden
//! categories "[K_<lane>]" so other mail clients keep working.

use std::collections::HashMap;

use serde_json::json;

use super::Service;
use crate::error::{Error, Result};
use crate::model::ListQuery;

const KEY: &str = "lanes";
pub(crate) const LANE_PREFIX: &str = "[K_";

pub(crate) fn lane_category(lane: &str) -> String {
    format!("{LANE_PREFIX}{lane}]")
}

pub(crate) fn is_lane_category(cat: &str) -> bool {
    cat.starts_with(LANE_PREFIX) && cat.ends_with(']')
}

fn validate(lanes: &[String]) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for l in lanes {
        let l = l.trim();
        let ok = (1..=30).contains(&l.chars().count())
            && l.chars().all(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_'));
        if !ok {
            return Err(Error::invalid(format!(
                "Ungültiger Lane-Name \"{l}\": nur Buchstaben, Ziffern, Leerzeichen, - und _ (max. 30 Zeichen)."
            )));
        }
        if out.iter().any(|x| x.to_lowercase() == l.to_lowercase()) {
            return Err(Error::invalid(format!("Die Lane \"{l}\" gibt es doppelt.")));
        }
        out.push(l.to_string());
    }
    Ok(out)
}

impl Service {
    pub(crate) fn get_lanes(&self, account_id: &str) -> Result<Vec<String>> {
        Ok(self.store.get_account_json(account_id, KEY)?.unwrap_or_default())
    }

    pub(crate) async fn save_lanes(&self, account_id: &str, lanes: &[String]) -> Result<Vec<String>> {
        let lanes = validate(lanes)?;
        let old = self.get_lanes(account_id)?;
        // Outlook category names ignore case: "ToDo" → "todo" would delete the
        // shared category and strip the lane from all mails.
        if let Some(l) = lanes
            .iter()
            .find(|l| !old.contains(l) && old.iter().any(|o| o.to_lowercase() == l.to_lowercase()))
        {
            return Err(Error::invalid(format!(
                "Die Lane \"{l}\" unterscheidet sich nur in der Groß-/Kleinschreibung von einer bestehenden. Bitte einen anderen Namen wählen."
            )));
        }
        let existing = self.list_categories(account_id).await?;
        for l in lanes.iter().filter(|l| !old.contains(l)) {
            let name = lane_category(l);
            if !existing.iter().any(|c| c.display_name == name) {
                match self.create_category(account_id, &name, "none").await {
                    Ok(_) => {}
                    // Created meanwhile by another client: fine.
                    Err(Error::Invalid(m)) if m.contains("existiert bereits") => {}
                    Err(e) => return Err(e),
                }
            }
        }
        for l in old.iter().filter(|l| !lanes.contains(l)) {
            let name = lane_category(l);
            if let Some(c) = existing.iter().find(|c| c.display_name == name) {
                self.delete_category(account_id, &c.id).await?;
            }
            self.strip_category_from_cached(account_id, &name).await?;
        }
        self.store.put_account_json(account_id, KEY, &lanes)?;
        self.emit("lanes-changed", json!({"accountId": account_id}));
        Ok(lanes)
    }

    /// Removes a category from all cached messages of the account (server too).
    async fn strip_category_from_cached(&self, account_id: &str, category: &str) -> Result<()> {
        let mailboxes: Vec<_> = self
            .store
            .list_accounts()?
            .into_iter()
            .filter(|a| a.id == account_id)
            .flat_map(|a| a.mailboxes)
            .collect();
        for mb in mailboxes {
            for f in self.store.list_folders(&mb.id)? {
                let page = self.store.list_messages(&ListQuery {
                    mailbox_id: mb.id.clone(),
                    folder_id: f.id.clone(),
                    category: Some(category.to_string()),
                    limit: 100_000,
                    ..Default::default()
                })?;
                for m in page.items {
                    let cats: Vec<String> = m.categories.iter().filter(|c| *c != category).cloned().collect();
                    self.set_categories(&mb.id, std::slice::from_ref(&m.id), &cats).await?;
                }
            }
        }
        Ok(())
    }

    /// Puts messages into a lane (or none), keeping all other categories.
    pub(crate) async fn set_lane(&self, mailbox_id: &str, ids: &[String], lane: Option<&str>) -> Result<()> {
        let mb = self.store.get_mailbox(mailbox_id)?;
        if let Some(l) = lane {
            if !self.get_lanes(&mb.account_id)?.iter().any(|x| x == l) {
                return Err(Error::invalid(format!("Die Lane \"{l}\" existiert nicht.")));
            }
        }
        // Group messages by their resulting category list to save requests.
        let mut groups: HashMap<Vec<String>, Vec<String>> = HashMap::new();
        for id in ids {
            let current = match self.store.get_summary(mailbox_id, id)? {
                Some(m) => m.categories,
                None => self.get_message(mailbox_id, id).await?.summary.categories,
            };
            let mut cats: Vec<String> = current.into_iter().filter(|c| !is_lane_category(c)).collect();
            if let Some(l) = lane {
                cats.push(lane_category(l));
            }
            groups.entry(cats).or_default().push(id.clone());
        }
        for (cats, ids) in groups {
            self.set_categories(mailbox_id, &ids, &cats).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lane_names() {
        assert_eq!(validate(&["ToDo".into(), " Warten auf ".into()]).unwrap(), ["ToDo", "Warten auf"]);
        assert!(validate(&["To[Do]".into()]).is_err());
        assert!(validate(&["".into()]).is_err());
        assert!(validate(&["a".repeat(31)]).is_err());
        assert!(validate(&["ToDo".into(), "todo".into()]).is_err());
        assert!(validate(&["Über_Prüfung-2".into()]).is_ok());
        assert!(is_lane_category("[K_ToDo]"));
        assert!(!is_lane_category("Info"));
    }
}
